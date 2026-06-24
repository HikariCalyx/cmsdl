//! TMS (Taiwan region) specific download logic.
//!
//! Unlike CMS, the Taiwan launcher publishes a plain `productInfo.json` manifest
//! that lists every client file with its size and SHA-256 checksum. Download
//! URLs are static (no signing), so a stalled connection is simply retried with
//! the same URL rather than a freshly re-signed one.

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Number of files downloaded concurrently.
const PARALLEL_FILES: usize = 10;

/// Maximum number of segments (threads) used per file.
const SEGMENTS_PER_FILE: usize = 5;

/// Smallest segment size; files smaller than this are downloaded single-threaded.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// If no data arrives on a connection for this long, the read fails and the
/// download is treated as stalled, triggering a resume from the same URL.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for establishing a connection (and resolving DNS).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of consecutive stalls (with no bytes received) tolerated for a
/// single segment before it is reported as failed. The counter resets whenever
/// any progress is made.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before resuming after a stall.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

/// URL of the TMS product manifest.
const PRODUCT_INFO_URL: &str =
    "https://maplestory-download.beanfun.com/maplestory/productInfo.json";

/// The product manifest published by the Taiwan launcher.
#[derive(Debug, Clone, Deserialize)]
pub struct ProductInfo {
    /// Human-readable product name (e.g. `新楓之谷`).
    #[serde(rename = "productName")]
    pub product_name: String,
    /// Product identifier used in download paths (e.g. `MS`).
    #[serde(rename = "productId")]
    pub product_id: String,
    /// Client version string (e.g. `V280`).
    pub version: String,
    /// Declared total install size, in bytes.
    #[serde(rename = "sizeInBytes")]
    pub size_in_bytes: u64,
    /// Base URL all download paths are appended to (ends with `/`).
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    /// Path to the main executable, relative to `base_url`
    /// (e.g. `P2P2dQtkA2f8rU3/MapleStory.exe`).
    #[serde(rename = "executionPath")]
    pub execution_path: String,
    /// Every client file, with its size and checksum.
    pub files: Vec<FileItem>,
}

/// A single file entry from the manifest's `files` array.
#[derive(Debug, Clone, Deserialize)]
pub struct FileItem {
    /// Path relative to the base path, using forward slashes
    /// (e.g. `Data/Base/Base.ini`).
    pub path: String,
    /// Expected size in bytes.
    #[serde(rename = "sizeInBytes")]
    pub size_in_bytes: u64,
    /// Expected lowercase hex SHA-256 checksum.
    pub sha256: String,
}

/// Summary information parsed from the product manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSummary {
    /// Human-readable product name.
    pub product_name: String,
    /// Client version string.
    pub version: String,
    /// Number of file entries.
    pub file_count: usize,
    /// Sum of the size field across all file entries, in bytes.
    pub total_size: u64,
}

/// Fetch and parse the product manifest.
pub fn get_product_info(agent: &ureq::Agent) -> Result<ProductInfo> {
    let bytes =
        http_get_bytes(agent, PRODUCT_INFO_URL).context("failed to fetch productInfo.json")?;
    serde_json::from_slice(&bytes).context("failed to parse productInfo.json")
}

/// Fetch the product manifest and summarize it.
pub fn get_product_info_summary(allow_insecure: bool, proxy: Option<&str>) -> Result<ProductSummary> {
    let agent = crate::net::agent(allow_insecure, proxy);
    let info = get_product_info(&agent)?;
    let total_size = info.files.iter().map(|f| f.size_in_bytes).sum();
    Ok(ProductSummary {
        product_name: info.product_name,
        version: info.version,
        file_count: info.files.len(),
        total_size,
    })
}

/// The default file name for the torrent (e.g. `MS_V280.torrent`).
fn torrent_file_name(product_id: &str, version: &str) -> String {
    format!("{product_id}_{version}.torrent")
}

/// Build the torrent URL: `<baseUrl>torrent/<productId>_<version>.torrent`.
fn torrent_url(base_url: &str, product_id: &str, version: &str) -> String {
    join_url(base_url, &format!("torrent/{}", torrent_file_name(product_id, version)))
}

/// Resolve the destination path for the torrent file.
///
/// When `output` is an existing directory (or `None`), the torrent's own name is
/// used inside it (or the current directory). Otherwise `output` is taken as the
/// full destination file path.
fn resolve_torrent_output(output: Option<&Path>, default_name: &str) -> PathBuf {
    match output {
        Some(p) if p.is_dir() => p.join(default_name),
        Some(p) => p.to_path_buf(),
        None => PathBuf::from(default_name),
    }
}

/// Download the BitTorrent (`.torrent`) file for the latest version.
///
/// The torrent is served at `<baseUrl>torrent/<productId>_<version>.torrent`.
/// When `output` is omitted the file is written under its own name in the
/// current directory.
pub fn download_torrent(output: Option<&Path>, allow_insecure: bool, proxy: Option<&str>) -> Result<()> {
    let agent = crate::net::agent(allow_insecure, proxy);
    let info = get_product_info(&agent)?;
    let url = torrent_url(&info.base_url, &info.product_id, &info.version);
    let default_name = torrent_file_name(&info.product_id, &info.version);
    let dest = resolve_torrent_output(output, &default_name);

    let bytes = http_get_bytes(&agent, &url).context("failed to download torrent file")?;

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }
    }
    std::fs::write(&dest, &bytes)
        .with_context(|| format!("failed to write torrent to {}", dest.display()))?;

    println!(
        "saved {} ({} bytes) to {}",
        default_name,
        bytes.len(),
        dest.display()
    );
    Ok(())
}

/// Perform a GET request and return the raw response body.
fn http_get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let mut reader = agent
        .get(url)
        .call()
        .context("HTTP request failed")?
        .into_reader();
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .context("failed to read response body")?;
    Ok(buf)
}

/// A single resolved download: its remote URL, local destination (relative,
/// forward slashes) and, when known, its expected size and checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadItem {
    url: String,
    local_path: String,
    size: Option<u64>,
    sha256: Option<String>,
}

/// The directory portion of the execution path (e.g.
/// `P2P2dQtkA2f8rU3/MapleStory.exe` -> `P2P2dQtkA2f8rU3`).
fn base_path_of(execution_path: &str) -> &str {
    match execution_path.rfind('/') {
        Some(i) => &execution_path[..i],
        None => "",
    }
}

/// The final path segment (e.g. `P2P2dQtkA2f8rU3/MapleStory.exe` -> `MapleStory.exe`).
fn file_name_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Join a base URL and a relative path with exactly one `/` between them.
fn join_url(base: &str, rest: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        rest.trim_start_matches('/')
    )
}

/// Build the list of files to download from the manifest.
///
/// Every file entry yields `<baseUrl><basePath>/<path>`, downloaded to
/// `<target>/<path>`. The execution path yields `<baseUrl><executionPath>`,
/// downloaded to `<target>/<file name>`; it is skipped when a file entry already
/// covers the same local destination (the file entry carries a checksum and is
/// therefore preferred). When `wz_only` is set, only the data files (paths under
/// `Data/`) are kept and the executable is excluded.
fn build_download_items(info: &ProductInfo, wz_only: bool) -> Vec<DownloadItem> {
    let base_path = base_path_of(&info.execution_path);

    let mut items: Vec<DownloadItem> = info
        .files
        .iter()
        .filter(|f| !wz_only || is_data_path(&f.path))
        .map(|f| {
            let rest = if base_path.is_empty() {
                f.path.clone()
            } else {
                format!("{base_path}/{}", f.path)
            };
            DownloadItem {
                url: join_url(&info.base_url, &rest),
                local_path: f.path.clone(),
                size: Some(f.size_in_bytes),
                sha256: Some(f.sha256.clone()),
            }
        })
        .collect();

    // The execution path is the launcher itself. Keep it only if no file entry
    // already downloads to the same local destination, and never in wz-only mode
    // (the executable is not a data file).
    if !wz_only {
        let exec_local = file_name_of(&info.execution_path).to_owned();
        if !items.iter().any(|i| i.local_path == exec_local) {
            items.push(DownloadItem {
                url: join_url(&info.base_url, &info.execution_path),
                local_path: exec_local,
                size: None,
                sha256: None,
            });
        }
    }

    items
}

/// Return `true` if `path` is a data file (lives under `Data/`).
fn is_data_path(path: &str) -> bool {
    path.replace('\\', "/")
        .to_ascii_lowercase()
        .starts_with("data/")
}

/// Resolve a relative, forward-slash path into a local path under `root`.
fn local_path(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for component in rel.split('/').filter(|c| !c.is_empty()) {
        path.push(component);
    }
    path
}

/// Download (or update) all client files for the TMS region into `target_dir`.
///
/// Files are downloaded with up to [`PARALLEL_FILES`] running concurrently, and
/// each large file is fetched in up to [`SEGMENTS_PER_FILE`] parallel byte-range
/// segments. Files already present with the expected size and checksum are
/// skipped. When `wz_only` is set, only data files (paths under `Data/`) are
/// downloaded.
pub fn download_client(target_dir: &Path, wz_only: bool, allow_insecure: bool, proxy: Option<&str>) -> Result<()> {
    // A shared HTTP agent with a read timeout, so a connection that stops
    // delivering data surfaces as an error (instead of hanging forever) and can
    // be resumed from its current byte offset. The same agent is reused for the
    // manifest request.
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();

    let info = get_product_info(&agent)?;
    println!(
        "latest version: {} ({}); declared install size: {:.2} GB. starting download.",
        info.version,
        info.product_name,
        info.size_in_bytes as f64 / 1_073_741_824.0,
    );

    let mut items = build_download_items(&info, wz_only);
    if wz_only {
        println!(
            "Limiting download to {} WZ file(s) under Data/.",
            items.len()
        );
    }

    // Resolve any unknown sizes (e.g. the execution path) so the total progress
    // bar is accurate and segmented downloads can be planned.
    for item in &mut items {
        if item.size.is_none() {
            item.size = remote_size(&item.url, &agent);
        }
    }

    let total_bytes: u64 = items.iter().filter_map(|i| i.size).sum();

    // Progress bars: one overall bar plus one reusable bar per worker.
    let mp = MultiProgress::new();
    let total_pb = mp.add(ProgressBar::new(total_bytes));
    total_pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] \
             {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    total_pb.enable_steady_tick(Duration::from_millis(120));

    let worker_bars: Vec<ProgressBar> = (0..PARALLEL_FILES)
        .map(|_| {
            let pb = mp.add(ProgressBar::new(0));
            pb.set_style(
                ProgressStyle::with_template(
                    "  [{bar:25.green/white}] {bytes:>10}/{total_bytes:>10} \
                     ({binary_bytes_per_sec:>11}) {wide_msg}",
                )
                .unwrap()
                .progress_chars("=>-"),
            );
            pb.enable_steady_tick(Duration::from_millis(120));
            pb
        })
        .collect();

    let counter = AtomicUsize::new(0);
    let downloaded = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failures: Mutex<Vec<String>> = Mutex::new(Vec::new());

    std::thread::scope(|scope| {
        let items = &items;
        let counter = &counter;
        let downloaded = &downloaded;
        let skipped = &skipped;
        let failures = &failures;
        let total_pb = &total_pb;
        let agent = &agent;

        for bar in worker_bars.iter().cloned() {
            scope.spawn(move || {
                loop {
                    let idx = counter.fetch_add(1, Ordering::Relaxed);
                    if idx >= items.len() {
                        break;
                    }
                    let item = &items[idx];
                    let dest = local_path(target_dir, &item.local_path);

                    // Skip files already present and intact.
                    if is_up_to_date(&dest, item).unwrap_or(false) {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        if let Some(size) = item.size {
                            total_pb.inc(size);
                        }
                        continue;
                    }

                    let size = item.size.unwrap_or(0);
                    bar.set_length(size);
                    bar.set_position(0);
                    bar.set_message(item.local_path.clone());

                    match download_file(&item.url, &dest, item.size, SEGMENTS_PER_FILE, &bar, total_pb, agent) {
                        Ok(()) => {
                            downloaded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("{}: {e:#}", item.local_path));
                        }
                    }
                }
                bar.finish_and_clear();
            });
        }
    });

    total_pb.finish_and_clear();

    let downloaded = downloaded.load(Ordering::Relaxed);
    let skipped = skipped.load(Ordering::Relaxed);
    let failures = failures.into_inner().unwrap();

    println!(
        "done: {downloaded} downloaded, {skipped} already up to date, {} failed.",
        failures.len()
    );
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("  failed: {f}");
        }
        bail!("{} file(s) failed to download", failures.len());
    }

    Ok(())
}

/// Return `true` if `path` already exists with the expected size and checksum.
///
/// When the expected size or checksum is unknown (e.g. the execution path), the
/// file cannot be verified and is always re-downloaded.
fn is_up_to_date(path: &Path, item: &DownloadItem) -> Result<bool> {
    let (Some(size), Some(sha)) = (item.size, item.sha256.as_deref()) else {
        return Ok(false);
    };
    if !path.exists() {
        return Ok(false);
    }
    let metadata = std::fs::metadata(path)?;
    if metadata.len() != size {
        return Ok(false);
    }
    let actual = sha256_file(path)?;
    Ok(actual.eq_ignore_ascii_case(sha))
}

/// Compute the lowercase hex SHA-256 of a file's contents, streaming from disk.
fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Query the server for the size of `url` via a HEAD request.
fn remote_size(url: &str, agent: &ureq::Agent) -> Option<u64> {
    let resp = agent.request("HEAD", url).call().ok()?;
    resp.header("Content-Length")?.trim().parse().ok()
}

/// Download `url` to `dest`, using parallel byte-range segments for large files.
fn download_file(
    url: &str,
    dest: &Path,
    size: Option<u64>,
    max_segments: usize,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    // A multi-segment download needs a known size and server range support.
    let size = match size {
        Some(s) if s > 0 => s,
        _ => return download_single(url, dest, pb, total, agent),
    };

    let segments = effective_segments(size, max_segments);
    if segments <= 1 || !supports_ranges(url, agent) {
        return download_single(url, dest, pb, total, agent);
    }

    // Pre-allocate the destination file so segments can be written at offsets.
    {
        let file = std::fs::File::create(dest)
            .with_context(|| format!("failed to create file {}", dest.display()))?;
        file.set_len(size)
            .with_context(|| format!("failed to size file {}", dest.display()))?;
    }

    let ranges = compute_ranges(size, segments);
    let mut first_err: Option<anyhow::Error> = None;

    std::thread::scope(|scope| {
        let handles: Vec<_> = ranges
            .into_iter()
            .map(|(start, end)| {
                scope.spawn(move || download_range(url, dest, start, end, pb, total, agent))
            })
            .collect();

        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(_) => {
                    if first_err.is_none() {
                        first_err = Some(anyhow!("a download segment thread panicked"));
                    }
                }
            }
        }
    });

    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Download a byte range `[start, end]` of `url` into `dest`, resuming from the
/// current offset (with the same URL) whenever the connection stalls.
fn download_range(
    url: &str,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;

    let mut pos = start;
    let mut stalls = 0usize;

    while pos <= end {
        let before = pos;
        let _ = stream_segment(agent, url, &mut file, &mut pos, end, pb, total);

        if pos > end {
            return Ok(());
        }

        // The range did not complete: either a stall/timeout or an early EOF.
        if pos > before {
            stalls = 0; // progress was made, so reset the stall counter
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
    }

    Ok(())
}

/// Stream a range request from `*pos` to `end` into `file`, advancing `*pos` and
/// both progress bars as bytes arrive.
fn stream_segment(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    end: u64,
    pb: &ProgressBar,
    total: &ProgressBar,
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
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("failed to write to disk")?;
        *pos += n as u64;
        pb.inc(n as u64);
        total.inc(n as u64);
    }
    Ok(())
}

/// Download the whole of `url` into `dest` in a single stream, retrying from the
/// start whenever the connection stalls. Used for small files, servers that do
/// not honour range requests, and files of unknown size.
fn download_single(
    url: &str,
    dest: &Path,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
) -> Result<()> {
    let mut stalls = 0usize;

    loop {
        let mut written = 0u64;
        let result = (|| -> Result<()> {
            let resp = agent.get(url).call().context("HTTP request failed")?;
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
                total.inc(n as u64);
            }
            Ok(())
        })();

        match result {
            Ok(()) => return Ok(()),
            Err(e) => {
                stalls += 1;
                if stalls > MAX_STALL_RETRIES {
                    return Err(e.context(format!(
                        "download stalled, giving up after {MAX_STALL_RETRIES} retries"
                    )));
                }
                // Roll back the progress counted by this failed attempt, since
                // the next attempt restarts the file from the beginning.
                total.set_position(total.position().saturating_sub(written));
                pb.set_position(pb.position().saturating_sub(written));
                std::thread::sleep(RESUME_BACKOFF);
            }
        }
    }
}

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

/// Probe whether the server honours HTTP range requests for `url`.
fn supports_ranges(url: &str, agent: &ureq::Agent) -> bool {
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => resp.status() == 206,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ProductInfo {
        ProductInfo {
            product_name: "MapleStory".to_owned(),
            product_id: "MS".to_owned(),
            version: "V280".to_owned(),
            size_in_bytes: 1000,
            base_url: "https://host/maplestory/download/".to_owned(),
            execution_path: "P2P2dQtkA2f8rU3/MapleStory.exe".to_owned(),
            files: vec![
                FileItem {
                    path: "BlackCipher/BlackCall64.aes".to_owned(),
                    size_in_bytes: 100,
                    sha256: "aa".to_owned(),
                },
                FileItem {
                    path: "Data/Base/Base.ini".to_owned(),
                    size_in_bytes: 15,
                    sha256: "bb".to_owned(),
                },
                FileItem {
                    path: "MapleStory.exe".to_owned(),
                    size_in_bytes: 200,
                    sha256: "cc".to_owned(),
                },
            ],
        }
    }

    #[test]
    fn extracts_base_path_from_execution_path() {
        assert_eq!(base_path_of("P2P2dQtkA2f8rU3/MapleStory.exe"), "P2P2dQtkA2f8rU3");
        assert_eq!(base_path_of("MapleStory.exe"), "");
    }

    #[test]
    fn extracts_file_name() {
        assert_eq!(file_name_of("P2P2dQtkA2f8rU3/MapleStory.exe"), "MapleStory.exe");
        assert_eq!(file_name_of("MapleStory.exe"), "MapleStory.exe");
    }

    #[test]
    fn joins_url_with_single_slash() {
        assert_eq!(
            join_url("https://host/download/", "P2P2dQtkA2f8rU3/Data/Base.ini"),
            "https://host/download/P2P2dQtkA2f8rU3/Data/Base.ini"
        );
        assert_eq!(join_url("https://host/download", "/a/b"), "https://host/download/a/b");
    }

    #[test]
    fn builds_file_urls_with_base_path() {
        let items = build_download_items(&sample(), false);
        let black = items
            .iter()
            .find(|i| i.local_path == "BlackCipher/BlackCall64.aes")
            .unwrap();
        assert_eq!(
            black.url,
            "https://host/maplestory/download/P2P2dQtkA2f8rU3/BlackCipher/BlackCall64.aes"
        );
        assert_eq!(black.size, Some(100));
        assert_eq!(black.sha256.as_deref(), Some("aa"));
    }

    #[test]
    fn execution_path_deduped_against_file_entry() {
        // MapleStory.exe is already in the file list, so the execution path must
        // not add a second download for the same local destination.
        let items = build_download_items(&sample(), false);
        let exe_count = items.iter().filter(|i| i.local_path == "MapleStory.exe").count();
        assert_eq!(exe_count, 1);
        // The kept entry is the file entry (it has a checksum).
        let exe = items.iter().find(|i| i.local_path == "MapleStory.exe").unwrap();
        assert_eq!(exe.sha256.as_deref(), Some("cc"));
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn execution_path_added_when_not_in_files() {
        let mut info = sample();
        info.files.retain(|f| f.path != "MapleStory.exe");
        let items = build_download_items(&info, false);
        let exe = items.iter().find(|i| i.local_path == "MapleStory.exe").unwrap();
        assert_eq!(
            exe.url,
            "https://host/maplestory/download/P2P2dQtkA2f8rU3/MapleStory.exe"
        );
        assert_eq!(exe.size, None);
        assert_eq!(exe.sha256, None);
    }

    #[test]
    fn wz_only_keeps_data_paths_and_drops_executable() {
        let items = build_download_items(&sample(), true);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].local_path, "Data/Base/Base.ini");
    }

    #[test]
    fn recognizes_data_paths() {
        assert!(is_data_path("Data/Base/Base.ini"));
        assert!(is_data_path("data/base/base.ini"));
        assert!(!is_data_path("BlackCipher/BlackCall64.aes"));
        assert!(!is_data_path("MapleStory.exe"));
    }

    #[test]
    fn resolves_local_path_under_root() {
        let root = Path::new("C:/games/tms");
        let p = local_path(root, "Data/Base/Base.ini");
        assert!(p.ends_with("Base.ini"));
        assert!(p.starts_with(root));
    }

    #[test]
    fn splits_ranges_to_cover_whole_file() {
        let ranges = compute_ranges(1003, 5);
        assert_eq!(ranges.len(), 5);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 1002);
        for pair in ranges.windows(2) {
            assert_eq!(pair[1].0, pair[0].1 + 1);
        }
        let covered: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(covered, 1003);
    }

    #[test]
    fn picks_segment_count_by_size() {
        assert_eq!(effective_segments(15, 5), 1);
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 3, 5), 3);
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 100, 5), 5);
        assert_eq!(effective_segments(0, 5), 1);
    }

    #[test]
    fn parses_product_info_json() {
        let json = r#"{
            "productName": "新楓之谷",
            "productId": "MS",
            "sizeInBytes": 70725113850,
            "version": "V280",
            "baseUrl": "https://host/maplestory/download/",
            "executionPath": "P2P2dQtkA2f8rU3/MapleStory.exe",
            "files": [
                { "path": "Data/Base/Base.ini", "sizeInBytes": 15, "mtimeMs": 1, "sha256": "abc" }
            ]
        }"#;
        let info: ProductInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.product_name, "新楓之谷");
        assert_eq!(info.product_id, "MS");
        assert_eq!(info.version, "V280");
        assert_eq!(info.base_url, "https://host/maplestory/download/");
        assert_eq!(info.execution_path, "P2P2dQtkA2f8rU3/MapleStory.exe");
        assert_eq!(info.files.len(), 1);
        assert_eq!(info.files[0].path, "Data/Base/Base.ini");
        assert_eq!(info.files[0].size_in_bytes, 15);
        assert_eq!(info.files[0].sha256, "abc");
    }

    #[test]
    fn builds_torrent_url_and_name() {
        assert_eq!(torrent_file_name("MS", "V280"), "MS_V280.torrent");
        assert_eq!(
            torrent_url("https://host/maplestory/download/", "MS", "V280"),
            "https://host/maplestory/download/torrent/MS_V280.torrent"
        );
    }

    #[test]
    fn resolves_torrent_output_path() {
        // No output -> default name in current directory.
        assert_eq!(
            resolve_torrent_output(None, "MS_V280.torrent"),
            PathBuf::from("MS_V280.torrent")
        );
        // Explicit file path is used verbatim.
        assert_eq!(
            resolve_torrent_output(Some(Path::new("out/custom.torrent")), "MS_V280.torrent"),
            PathBuf::from("out/custom.torrent")
        );
    }
}
