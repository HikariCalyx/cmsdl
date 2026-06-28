//! Manual single-file download from a CMS CDN URL.
//!
//! Supports auto-signing of unsigned URLs (and re-signing of expired ones) for
//! the two known CDN hosts (`mxdver0.jijiagames.com` / `mxdcclient.jijiagames.com`).
//! Downloads use segmented byte-range requests (10 segments) when the server
//! provides a Content-Length header, with a sidecar `.cmsdl` resume file.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};

use crate::cms;

/// Maximum number of segments for a manual download.
const SEGMENTS: usize = 5;

/// Smallest segment size; files smaller than this are downloaded single-threaded.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// If no data arrives on a connection for this long, the read fails and the
/// download is treated as stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for establishing a connection (and resolving DNS).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of consecutive stalls tolerated before giving up.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before re-signing and resuming after a stall.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

/// Known CDN hosts that accept signed URLs.
const ALLOWED_HOSTS: &[&str] = &[
    "mxdver0.jijiagames.com",
    "mxdcclient.jijiagames.com",
];

/// Prefix used by the secauth redirector; the real CDN URL is the value of the
/// `url` query parameter.
const SECAUTH_PREFIX: &str = "https://secauth.gcdn.sdo.com/?url=";

/// Entry point for `cmsdl manual --download <url> <target_dir> [--output <name>]`.
///
/// 1. Validates the domain.
/// 2. Strips (and re-signs) any expired or missing signature.
/// 3. Optionally performs a HEAD request to learn the file size.
/// 4. Downloads the file (segmented when size is known, single-stream otherwise).
/// 5. When `--verbose` is set, response headers are printed.
/// 6. When `--dry-run` is set, the download itself is skipped.
pub fn manual_download(
    url: &str,
    target_dir: &Path,
    output: Option<&Path>,
    dry_run: bool,
    verbose: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    // 0. Unwrap secauth redirector wrapper: the real CDN URL is inside ?url=.
    let original_url = url.to_owned();
    let url = unwrap_secauth(url);

    // 1. Validate domain — extract host and ensure it's one of the two known CDN hosts.
    let (scheme_host, raw_path) = split_url(&url)?;
    let host = scheme_host
        .strip_prefix("https://")
        .or_else(|| scheme_host.strip_prefix("http://"))
        .ok_or_else(|| anyhow!("URL must use https:// scheme: {url}"))?;

    if !ALLOWED_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) {
        bail!(
            "unsupported host '{host}'; manual download is only available from {}",
            ALLOWED_HOSTS.join(" or ")
        );
    }

    // 2. Strip any existing (possibly expired) signature and re-sign.
    let unsigned_path = strip_signature(raw_path);

    // 3. Obtain the challenge code and build the signed URL.
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();
    let challenge = cms::get_challenge_key(&agent).context("failed to obtain challenge code")?;
    let utc8_time = cms::get_current_utc8_time();
    let signed_url = cms::build_signed_url_for_host(scheme_host, &challenge, utc8_time, unsigned_path);

    if verbose {
        println!("  original URL:  {original_url}");
        if original_url != url {
            println!("  unwrapped URL: {url}");
        }
        println!("  unsigned path: {unsigned_path}");
        println!("  signed URL:    {signed_url}");
    }

    // 4. Determine the output filename.
    let file_name = if let Some(p) = output {
        p.file_name()
    } else {
        None
    }
    .or_else(|| {
        // Derive from the last path segment, stripping any query params.
        unsigned_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .map(std::ffi::OsStr::new)
    })
        .ok_or_else(|| anyhow!("could not determine output filename from URL"))?;
    let dest = target_dir.join(file_name);

    std::fs::create_dir_all(target_dir)
        .with_context(|| format!("failed to create target directory {}", target_dir.display()))?;

    // 5. HEAD request to learn the file size (and optionally show headers).
    let size = head_size(&signed_url, &agent, verbose)?;

    if dry_run {
        if let Some(sz) = size {
            println!(
                "  content-length: {sz} ({}) — would download as {} segment(s)",
                format_bytes(sz),
                effective_segments(sz, SEGMENTS)
            );
        } else {
            println!("  content-length: unknown — would download as a single stream");
        }
        println!("  output: {}", dest.display());
        println!("dry-run complete; no data was downloaded.");
        return Ok(());
    }

    println!(
        "cmsdl {}: downloading '{}' to '{}'.",
        env!("CARGO_PKG_VERSION"),
        &signed_url,
        dest.display()
    );

    // 6. Download.
    match size {
        Some(sz) if sz > 0 => {
            download_segmented(&signed_url, &dest, sz, &agent, &challenge, scheme_host, unsigned_path)?;
        }
        _ => {
            download_single(&signed_url, &dest, &agent, &challenge, scheme_host, unsigned_path)?;
        }
    }

    println!(
        "done: saved {} ({})",
        dest.display(),
        format_bytes(dest.metadata().map(|m| m.len()).unwrap_or(0))
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// If `url` starts with the secauth redirector prefix
/// (`https://secauth.gcdn.sdo.com/?url=`), return the inner CDN URL (the value
/// of the `url` query parameter). Otherwise return `url` unchanged.
///
/// The inner URL is percent-decoded so `%2F` becomes `/` etc.
fn unwrap_secauth(url: &str) -> String {
    if let Some(inner) = url.strip_prefix(SECAUTH_PREFIX) {
        if !inner.is_empty() {
            return percent_decode(inner);
        }
    }
    url.to_owned()
}

/// Percent-decode a string like `%2F` → `/`, `%3A` → `:`, etc.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.bytes();
    loop {
        match chars.next() {
            None => break,
            Some(b'%') => {
                let hi = chars.next().and_then(hex_nibble);
                let lo = chars.next().and_then(hex_nibble);
                match (hi, lo) {
                    (Some(h), Some(l)) => out.push((h << 4 | l) as char),
                    _ => {
                        out.push('%');
                        if let Some(h) = hi {
                            out.push(hex_nibble_to_char(h));
                        }
                        if let Some(l) = lo {
                            out.push(hex_nibble_to_char(l));
                        }
                    }
                }
            }
            Some(b) => out.push(b as char),
        }
    }
    out
}

/// Convert a hex byte (0–15) back to its ASCII character.
fn hex_nibble_to_char(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + n - 10) as char,
    }
}

/// Convert an ASCII byte to its hex value, or `None`.
fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Split `url` into `(scheme://host, /path)`.
fn split_url(url: &str) -> Result<(&str, &str)> {
    let scheme_end = url
        .find("://")
        .map(|i| i + 3)
        .ok_or_else(|| anyhow!("URL missing scheme: {url}"))?;
    let path_start = url[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .unwrap_or(url.len());
    Ok((&url[..path_start], &url[path_start..]))
}

/// If `path` starts with `/timestamp/md5/` (a signed signature prefix), remove
/// it and return the remaining path. Otherwise return the path unchanged.
///
/// A signed path looks like:
/// `/202606231838/a1b2c3d4.../v3client/build/5/8848/apppc/1020/somefile.dat`
fn strip_signature(path: &str) -> &str {
    // Fast check: does the second segment look like a timestamp (12 digits)?
    let bytes = path.as_bytes();
    if bytes.len() < 47 {
        // 1 (leading /) + 12 (ts) + 1 (/) + 32 (md5) + 1 (/) + 1 (min path) = 48
        return path;
    }
    if bytes[0] != b'/' {
        return path;
    }

    // Check that bytes 1..13 are all ASCII digits.
    let ts_end = 1 + 12;
    if bytes[1..ts_end].iter().any(|b| !b.is_ascii_digit()) {
        return path;
    }
    if bytes[ts_end] != b'/' {
        return path;
    }

    // Check that bytes 14..46 are all ASCII hex digits.
    let md5_start = ts_end + 1;
    let md5_end = md5_start + 32;
    if md5_end >= bytes.len() {
        return path;
    }
    if bytes[md5_start..md5_end]
        .iter()
        .any(|b| !b.is_ascii_hexdigit())
    {
        return path;
    }
    if bytes[md5_end] != b'/' {
        return path;
    }

    // Strip the signature prefix (keep the leading /).
    &path[md5_end..]
}

// ---------------------------------------------------------------------------
// HEAD request
// ---------------------------------------------------------------------------

/// Perform a HEAD request and return the `Content-Length`, if any.
///
/// When `verbose` is set the response status line and every header are printed.
fn head_size(url: &str, agent: &ureq::Agent, verbose: bool) -> Result<Option<u64>> {
    let resp = agent
        .head(url)
        .call()
        .context("HEAD request failed")?;

    if verbose {
        println!();
        println!("  response headers:");
        println!("    HTTP {} {}", resp.status(), resp.status_text());
        for name in resp.headers_names() {
            for value in resp.all(&name) {
                println!("    {name}: {value}");
            }
        }
        println!();
    }

    Ok(resp
        .header("Content-Length")
        .and_then(|v| v.trim().parse().ok()))
}

// ---------------------------------------------------------------------------
// Segmented download (known size)
// ---------------------------------------------------------------------------

/// Download `url` to `dest` using parallel byte-range segments, with a sidecar
/// resume file. `scheme_host` is the original CDN host (e.g.
/// `https://mxdcclient.jijiagames.com`) used when re-signing on retry.
fn download_segmented(
    url: &str,
    dest: &Path,
    size: u64,
    agent: &ureq::Agent,
    challenge: &str,
    scheme_host: &str,
    unsigned_path: &str,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let segments = effective_segments(size, SEGMENTS);
    let progress_path = crate::resume::progress_path(dest);

    // --- Determine ranges: resume from saved progress or start fresh. ---
    let saved_opt = crate::resume::read_progress(&progress_path)
        .filter(|_| dest.exists())
        .filter(|_| dest.metadata().map_or(false, |m| m.len() == size));

    let ranges: Vec<(u64, u64)>;
    let progress: crate::resume::FileProgress;

    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
             {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    if let Some(saved) = saved_opt
        .and_then(|s| crate::resume::build_resume_ranges(&s, size).map(|(r, pre)| (s, r, pre)))
    {
        let (saved_segs, resume_ranges, pre_completed) = saved;
        progress = crate::resume::FileProgress::from_saved(dest, &saved_segs, &resume_ranges)
            .with_context(|| format!("failed to write progress file {}", progress_path.display()))?;
        ranges = resume_ranges;
        // Fast-forward the progress bar past bytes already on disk.
        pb.inc(pre_completed);
    } else {
        // Fresh download: pre-allocate the destination file.
        {
            let file = std::fs::File::create(dest)
                .with_context(|| format!("failed to create file {}", dest.display()))?;
            file.set_len(size)
                .with_context(|| format!("failed to size file {}", dest.display()))?;
        }
        let fresh_ranges = compute_ranges(size, segments);
        progress = crate::resume::FileProgress::new(dest, &fresh_ranges)
            .with_context(|| format!("failed to create progress file {}", progress_path.display()))?;
        ranges = fresh_ranges;
    }

    let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        let progress_ref = &progress;
        let pb_ref = &pb;
        let handles: Vec<_> = ranges
            .iter()
            .enumerate()
            .map(|(slot, &(start, end))| {
                scope.spawn(move || {
                    download_range(
                        url, dest, start, end, pb_ref, agent, challenge,
                        scheme_host, unsigned_path, progress_ref, slot,
                    )
                })
            })
            .collect();

        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let mut slot = first_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(e);
                    }
                }
                Err(_) => {
                    let mut slot = first_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(anyhow!("a download segment thread panicked"));
                    }
                }
            }
        }
    });

    pb.finish_and_clear();

    match first_err.into_inner().unwrap() {
        Some(e) => Err(e),
        None => {
            progress.delete();
            Ok(())
        }
    }
}

/// Download byte range `[start, end]` of `url` into `dest`, re-signing the URL
/// and resuming from the current byte offset whenever the connection stalls.
fn download_range(
    url: &str,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    agent: &ureq::Agent,
    challenge: &str,
    scheme_host: &str,
    unsigned_path: &str,
    progress: &crate::resume::FileProgress,
    slot: usize,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;

    let mut pos = start;
    let mut stalls = 0usize;
    let mut current_url = url.to_owned();

    while pos <= end {
        let before = pos;
        let _ = stream_segment(
            agent,
            &current_url,
            &mut file,
            &mut pos,
            end,
            pb,
            progress,
            slot,
        );

        // Record progress at every reconnect boundary.
        progress.update(slot, pos);

        if pos > end {
            return Ok(());
        }

        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                bail!(
                    "download stalled with no progress after {MAX_STALL_RETRIES} retries \
                     (no data for {}s)",
                    STALL_TIMEOUT.as_secs()
                );
            }
        }
        std::thread::sleep(RESUME_BACKOFF);

        // Re-sign the URL on every retry so the timestamp stays fresh.
        let utc8_time = cms::get_current_utc8_time();
        current_url = cms::build_signed_url_for_host(scheme_host, challenge, utc8_time, unsigned_path);
    }

    Ok(())
}

/// Stream a range request from `*pos` to `end` into `file`, advancing `*pos`
/// and the progress bar as bytes arrive.
///
/// Progress is flushed to `progress` every
/// [`crate::resume::PROGRESS_FLUSH_INTERVAL`] bytes so an interruption
/// loses at most one flush interval of work.
fn stream_segment(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    end: u64,
    pb: &ProgressBar,
    progress: &crate::resume::FileProgress,
    slot: usize,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-{end}", *pos))
        .call()
        .context("HTTP range request failed")?;
    let mut reader = resp.into_reader();

    file.seek(SeekFrom::Start(*pos))
        .with_context(|| "failed to seek before resuming")?;

    let mut buf = [0u8; 64 * 1024];
    let mut since_flush: u64 = 0;
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("failed to write to disk")?;
        *pos += n as u64;
        since_flush += n as u64;
        pb.inc(n as u64);
        if since_flush >= crate::resume::PROGRESS_FLUSH_INTERVAL {
            progress.update(slot, *pos);
            since_flush = 0;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Single-stream download (unknown size)
// ---------------------------------------------------------------------------

/// Download the whole of `url` into `dest` in a single stream, re-signing and
/// restarting from the beginning whenever the connection stalls.
fn download_single(
    url: &str,
    dest: &Path,
    agent: &ureq::Agent,
    challenge: &str,
    scheme_host: &str,
    unsigned_path: &str,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {bytes} ({binary_bytes_per_sec}) {wide_msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("downloading (unknown size)");

    let mut stalls = 0usize;
    let mut current_url = url.to_owned();

    loop {
        let mut written = 0u64;
        let result = (|| -> Result<()> {
            let resp = agent.get(&current_url).call().context("HTTP request failed")?;
            let mut reader = resp.into_reader();
            let mut file = std::fs::File::create(dest)
                .with_context(|| format!("failed to create file {}", dest.display()))?;

            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = reader.read(&mut buf).context("failed to read response body")?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n]).context("failed to write to disk")?;
                written += n as u64;
                pb.inc(n as u64);
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                pb.finish_and_clear();
                return Ok(());
            }
            Err(e) => {
                stalls += 1;
                if stalls > MAX_STALL_RETRIES {
                    pb.finish_and_clear();
                    return Err(e.context(format!(
                        "download stalled, giving up after {MAX_STALL_RETRIES} retries"
                    )));
                }
                pb.set_position(pb.position().saturating_sub(written));
                std::thread::sleep(RESUME_BACKOFF);

                // Re-sign the URL.
                let utc8_time = cms::get_current_utc8_time();
                current_url = cms::build_signed_url_for_host(scheme_host, challenge, utc8_time, unsigned_path);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Range / segment arithmetic
// ---------------------------------------------------------------------------

/// Decide how many segments to use for a file of the given size.
fn effective_segments(size: u64, max_segments: usize) -> usize {
    if max_segments <= 1 || size == 0 {
        return 1;
    }
    let by_size = (size / MIN_SEGMENT_SIZE).max(1) as usize;
    by_size.min(max_segments).max(1)
}

/// Split `size` bytes into `segments` contiguous inclusive `[start, end]` ranges.
fn compute_ranges(size: u64, segments: usize) -> Vec<(u64, u64)> {
    let segments = segments.max(1) as u64;
    let chunk = size / segments;
    let mut ranges = Vec::with_capacity(segments as usize);
    let mut start = 0u64;
    for i in 0..segments {
        let end = if i == segments - 1 {
            size - 1
        } else {
            start + chunk - 1
        };
        ranges.push((start, end));
        start = end + 1;
    }
    ranges
}

/// Format a byte count for human consumption.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MiB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_expired_signature() {
        let path = "/202606231838/a1b2c3d4e5f6789012345678abcdef01/v3client/build/5/8848/apppc/1020/client_all_files_list.dat";
        assert_eq!(
            strip_signature(path),
            "/v3client/build/5/8848/apppc/1020/client_all_files_list.dat"
        );
    }

    #[test]
    fn leaves_unsigned_path_alone() {
        let path = "/v3client/build/5/8848/apppc/1020/somefile.dat";
        assert_eq!(strip_signature(path), path);
    }

    #[test]
    fn leaves_short_path_alone() {
        assert_eq!(strip_signature("/foo/bar"), "/foo/bar");
        assert_eq!(strip_signature("/"), "/");
        assert_eq!(strip_signature(""), "");
    }

    #[test]
    fn not_a_timestamp_if_not_digits() {
        let path = "/abc123456789/abcdef0123456789abcdef0123456789/v3client/somefile.dat";
        assert_eq!(strip_signature(path), path);
    }

    #[test]
    fn splits_url_correctly() {
        let (host, path) = split_url("https://mxdver0.jijiagames.com/v3client/foo.dat").unwrap();
        assert_eq!(host, "https://mxdver0.jijiagames.com");
        assert_eq!(path, "/v3client/foo.dat");
    }

    #[test]
    fn ranges_cover_full_file() {
        let ranges = compute_ranges(1003, 5);
        assert_eq!(ranges.len(), 5);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 1002);
        let covered: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(covered, 1003);
    }

    #[test]
    fn segment_count_respects_max() {
        assert_eq!(effective_segments(15, 10), 1);
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 3, 10), 3);
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 100, 10), 10);
    }

    #[test]
    fn unwraps_secauth_url() {
        let url = "https://secauth.gcdn.sdo.com/?url=https://mxdver0.jijiagames.com/225/MaplePatch224to225.patch";
        assert_eq!(unwrap_secauth(url), "https://mxdver0.jijiagames.com/225/MaplePatch224to225.patch");
    }

    #[test]
    fn unwraps_secauth_with_percent_encoding() {
        let url = "https://secauth.gcdn.sdo.com/?url=https%3A%2F%2Fmxdver0.jijiagames.com%2F225%2FMaple.patch";
        assert_eq!(unwrap_secauth(url), "https://mxdver0.jijiagames.com/225/Maple.patch");
    }

    #[test]
    fn passes_non_secauth_url_through() {
        let url = "https://mxdcclient.jijiagames.com/V000/file.exe";
        assert_eq!(unwrap_secauth(url), url);
    }

    #[test]
    fn percent_decode_handles_basic_cases() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%2F%2F"), "//");
        assert_eq!(percent_decode("no_percents"), "no_percents");
        assert_eq!(percent_decode(""), "");
    }
}
