//! CMS (China mainland region) specific control-file logic.
//!
//! The launcher control file (`v3ctrl.xml`) stores several "md5key" values
//! that are RSA-encrypted with the matching private key. They can be recovered
//! by applying the public RSA operation (`m = c^e mod n`) and stripping the
//! PKCS#1 v1.5 padding. The challenge key used by the launcher is built from
//! the first half of the decrypted `client` key and the second half of the
//! decrypted `server-let` key.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{FixedOffset, Utc};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use md5::{Digest, Md5};
use rsa::pkcs8::DecodePublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPublicKey};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Number of files downloaded concurrently.
const PARALLEL_FILES: usize = 10;

/// Maximum number of segments (threads) used per file.
const SEGMENTS_PER_FILE: usize = 5;

/// Smallest segment size; files smaller than this are downloaded single-threaded.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// URL of the CMS launcher control file.
const CTRL_XML_URL: &str = "https://downloader.dorado.sdo.com/v3launcher/5/v3ctrl.xml";

/// Host serving the signed client download files.
const DOWNLOAD_HOST: &str = "https://mxdver0.jijiagames.com";

/// Path of the client file list, relative to the download host.
const CLIENT_FILE_LIST_PATH: &str = "/v3client/build/5/8848/apppc/1020/client_all_files_list.dat";

/// Base64-encoded DER (SubjectPublicKeyInfo) of the RSA public key used to
/// decrypt the control-file `md5key` values.
const RSA_PUBLIC_KEY_DER_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCbHyTRH+DWw75sjRijIHobLf2rMNE3ob36WrpZePKU8V9ePQlLXvCVCQq4uFSF2KDtJwm9IBoSHzka36c38yMfYk/+FO/uIjcWOhgyzGbDajHQqtsKSTGqCWuoDdJiBDdb/fAVyvUToTaRFwpc8hYLn62iO8zhpevAa4tWgHDPFwIDAQAB";

/// Parse the embedded RSA public key.
fn public_key() -> Result<RsaPublicKey> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(RSA_PUBLIC_KEY_DER_B64)
        .context("failed to base64-decode RSA public key")?;
    RsaPublicKey::from_public_key_der(&der).context("failed to parse RSA public key")
}

/// Fetch the CMS control file and compute the launcher challenge key.
///
/// The challenge key is the concatenation of the first 16 characters of the
/// decrypted `client` md5key and the last 16 characters of the decrypted
/// `server-let` md5key.
pub fn get_challenge_key() -> Result<String> {
    let xml = fetch_ctrl_xml().context("failed to fetch v3ctrl.xml")?;
    let (server_let_hex, client_hex) =
        extract_md5keys(&xml).context("failed to parse md5keys from v3ctrl.xml")?;

    let public_key = public_key()?;

    let server_let = decrypt_md5key(&public_key, &server_let_hex)
        .context("failed to decrypt server-let md5key")?;
    let client =
        decrypt_md5key(&public_key, &client_hex).context("failed to decrypt client md5key")?;

    build_challenge_key(&client, &server_let)
}

/// Download the control file as text.
fn fetch_ctrl_xml() -> Result<String> {
    // The document declares `encoding="gbk"`, but every value we care about is
    // ASCII hex, so a lossy UTF-8 decode is sufficient.
    http_get_text(CTRL_XML_URL).context("HTTP request failed")
}

/// Perform a GET request and return the response body as (lossy) UTF-8 text.
fn http_get_text(url: &str) -> Result<String> {
    let mut reader = ureq::get(url).call().context("HTTP request failed")?.into_reader();

    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).context("failed to read response body")?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Extract the `server-let` and `client` md5key hex strings from the XML.
fn extract_md5keys(xml: &str) -> Result<(String, String)> {
    // roxmltree only supports UTF-8; rewrite the declared encoding so the
    // (ASCII) document parses cleanly.
    let sanitized = xml.replacen("encoding=\"gbk\"", "encoding=\"utf-8\"", 1);
    let doc = roxmltree::Document::parse(&sanitized).context("invalid XML")?;

    let server_let = find_md5key(&doc, "server-let")
        .ok_or_else(|| anyhow!("missing <server-let><md5key> element"))?;
    let client =
        find_md5key(&doc, "client").ok_or_else(|| anyhow!("missing <client><md5key> element"))?;

    Ok((server_let, client))
}

/// Find the text of `<parent><md5key>...</md5key></parent>` for the given parent tag.
fn find_md5key(doc: &roxmltree::Document, parent_tag: &str) -> Option<String> {
    doc.descendants()
        .find(|n| n.has_tag_name(parent_tag))?
        .children()
        .find(|c| c.has_tag_name("md5key"))?
        .text()
        .map(|t| t.trim().to_owned())
}

/// Apply the public RSA operation to a hex-encoded ciphertext and strip the
/// PKCS#1 v1.5 padding, returning the decrypted UTF-8 payload.
fn decrypt_md5key(public_key: &RsaPublicKey, hex_ciphertext: &str) -> Result<String> {
    let cipher_bytes = hex::decode(hex_ciphertext).context("md5key is not valid hex")?;

    // m = c^e mod n
    let c = BigUint::from_bytes_be(&cipher_bytes);
    let m = c.modpow(public_key.e(), public_key.n());

    // Left-pad to the modulus size so the PKCS#1 block structure is intact.
    let mut block = m.to_bytes_be();
    let key_size = public_key.size();
    if block.len() < key_size {
        let mut padded = vec![0u8; key_size - block.len()];
        padded.extend_from_slice(&block);
        block = padded;
    }

    let payload = pkcs1_unpad(&block)?;
    String::from_utf8(payload).context("decrypted md5key is not valid UTF-8")
}

/// Strip PKCS#1 v1.5 padding (block type 01 or 02) and return the payload.
///
/// The block layout is: `00 || BT || PS || 00 || payload`, where `BT` is the
/// block type and `PS` is the padding string.
fn pkcs1_unpad(block: &[u8]) -> Result<Vec<u8>> {
    if block.len() < 11 || block[0] != 0x00 || (block[1] != 0x01 && block[1] != 0x02) {
        bail!("invalid PKCS#1 padding");
    }

    // Find the 0x00 separator that terminates the padding string.
    let sep = block[2..]
        .iter()
        .position(|&b| b == 0x00)
        .map(|p| p + 2)
        .ok_or_else(|| anyhow!("PKCS#1 padding separator not found"))?;

    Ok(block[sep + 1..].to_vec())
}

/// Build the challenge key from the decrypted `client` and `server-let` keys.
fn build_challenge_key(client: &str, server_let: &str) -> Result<String> {
    if client.len() < 16 || server_let.len() < 16 {
        bail!("decrypted keys are too short to build a challenge key");
    }

    let client_head = &client[..16];
    let server_let_tail = &server_let[server_let.len() - 16..];

    Ok(format!("{client_head}{server_let_tail}"))
}

/// Return the current UTC+8 time as a number in `yyyyMMddHHmm` format.
///
/// For example, `2026-06-23 18:38` (UTC+8) is returned as `202606231838`.
pub fn get_current_utc8_time() -> u64 {
    // UTC+8 is a fixed offset of 8 hours east, so it is unaffected by DST.
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8 is a valid offset");
    let now = Utc::now().with_timezone(&offset);
    now.format("%Y%m%d%H%M")
        .to_string()
        .parse()
        .expect("yyyyMMddHHmm is always a valid number")
}

/// Build the signed URL for the client file list and download its contents.
///
/// The path is signed with an MD5 of `<challengeCode><utc8Time><path>`, and the
/// resulting URL is `<host>/<utc8Time>/<md5>/<path>`.
pub fn get_client_file_list() -> Result<String> {
    let utc8_time = get_current_utc8_time();
    let challenge_code = get_challenge_key().context("failed to obtain challenge code")?;

    let url = build_client_file_list_url(&challenge_code, utc8_time);
    http_get_text(&url).context("failed to download client file list")
}

/// Summary information parsed from a client file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFileList {
    /// Client version, taken from the last field of the header line (e.g. `0.0.0.15`).
    pub version: String,
    /// Sum of the size field across all file entries, in bytes.
    pub total_size: u64,
    /// Number of file entries (non-empty lines, excluding the header line).
    pub file_count: usize,
}

/// Download and parse the client file list summary.
pub fn get_client_file_list_info() -> Result<ClientFileList> {
    let contents = get_client_file_list()?;
    parse_client_file_list(&contents)
}

/// Parse a client file list into its version, total size and file count.
///
/// The first non-empty line is a header whose last `|`-separated field is the
/// version. Every following non-empty line is `path|size|md5`; the sizes are
/// summed and the entries counted.
fn parse_client_file_list(contents: &str) -> Result<ClientFileList> {
    let mut lines = contents.lines().filter(|l| !l.trim().is_empty());

    let header = lines.next().context("client file list is empty")?;
    let version = header
        .rsplit('|')
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("missing version in client file list header"))?
        .trim()
        .to_owned();

    let mut total_size: u64 = 0;
    let mut file_count: usize = 0;
    for line in lines {
        let size = line
            .split('|')
            .nth(1)
            .ok_or_else(|| anyhow!("missing size field in entry: {line}"))?;
        total_size += size
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid size field in entry: {line}"))?;
        file_count += 1;
    }

    Ok(ClientFileList {
        version,
        total_size,
        file_count,
    })
}

/// Build the signed client-file-list URL from the challenge code and time value.
fn build_client_file_list_url(challenge_code: &str, utc8_time: u64) -> String {
    let signature_input = format!("{challenge_code}{utc8_time}{CLIENT_FILE_LIST_PATH}");
    let signature = md5_hex(&signature_input);

    format!("{DOWNLOAD_HOST}/{utc8_time}/{signature}{CLIENT_FILE_LIST_PATH}")
}

/// Compute the lowercase hex MD5 digest of `input`.
fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// A single file entry parsed from the client file list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileEntry {
    /// Path field exactly as published, using backslashes (e.g. `mxd\Data\...\Android_000.wz`).
    /// Used as-is when computing the obfuscated server-side name.
    raw_path: String,
    /// Directory portion with forward slashes and a trailing slash (e.g. `mxd/Data/Character/Android/`).
    file_location: String,
    /// File name without its directory (e.g. `Android_000.wz`).
    file_name: String,
    /// Expected size in bytes.
    file_size: u64,
    /// Expected MD5 checksum (hex, as published).
    md5_checksum: String,
}

/// Parse a single `path|size|md5` entry line into a [`FileEntry`].
fn parse_entry(line: &str) -> Result<FileEntry> {
    let mut parts = line.split('|');
    let raw_path = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("missing path in entry: {line}"))?;
    let size = parts
        .next()
        .map(str::trim)
        .ok_or_else(|| anyhow!("missing size in entry: {line}"))?;
    let md5 = parts
        .next()
        .map(str::trim)
        .ok_or_else(|| anyhow!("missing md5 in entry: {line}"))?;

    let forward = raw_path.replace('\\', "/");
    let (file_location, file_name) = match forward.rfind('/') {
        Some(i) => (forward[..=i].to_owned(), forward[i + 1..].to_owned()),
        None => (String::new(), forward.clone()),
    };

    Ok(FileEntry {
        raw_path: raw_path.to_owned(),
        file_location,
        file_name,
        file_size: size
            .parse()
            .with_context(|| format!("invalid size in entry: {line}"))?,
        md5_checksum: md5.to_owned(),
    })
}

/// Split the file-list header's first field into `(domain, base_path)`.
///
/// `https://host/v3client/.../1020` -> (`https://host`, `/v3client/.../1020`).
fn parse_header_location(header: &str) -> Result<(String, String)> {
    let url = header.split('|').next().unwrap_or("").trim();
    let scheme_end = url
        .find("://")
        .map(|i| i + 3)
        .ok_or_else(|| anyhow!("header URL missing scheme: {url}"))?;
    let path_start = url[scheme_end..]
        .find('/')
        .map(|i| scheme_end + i)
        .ok_or_else(|| anyhow!("header URL missing path: {url}"))?;

    Ok((url[..path_start].to_owned(), url[path_start..].to_owned()))
}

/// Compute the obfuscated, server-side file name for an entry.
///
/// It is the uppercase MD5 of `5_<version>_<raw_path>` encoded as UTF-16LE.
fn obfuscated_file_name(version: &str, raw_path: &str) -> String {
    md5_utf16le_upper(&format!("5_{version}_{raw_path}"))
}

/// Compute the uppercase hex MD5 of `input` encoded as UTF-16LE (no BOM).
fn md5_utf16le_upper(input: &str) -> String {
    let mut hasher = Md5::new();
    for unit in input.encode_utf16() {
        hasher.update(unit.to_le_bytes());
    }
    hex::encode_upper(hasher.finalize())
}

/// Compute the lowercase hex MD5 of a file's contents, streaming from disk.
fn md5_file(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Md5::new();
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

/// Return `true` if `path` already exists with the expected size and checksum.
fn is_up_to_date(path: &Path, entry: &FileEntry) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let metadata = std::fs::metadata(path)?;
    if metadata.len() != entry.file_size {
        return Ok(false);
    }
    let actual = md5_file(path)?;
    Ok(actual.eq_ignore_ascii_case(&entry.md5_checksum))
}

/// Download (or update) all client files for the CMS region into `target_dir`.
///
/// Files are downloaded with up to [`PARALLEL_FILES`] running concurrently, and
/// each large file is fetched in up to [`SEGMENTS_PER_FILE`] parallel byte-range
/// segments. Files already present with the expected size and checksum are
/// skipped. Progress and download speed are shown with live progress bars.
pub fn download_client(target_dir: &Path) -> Result<()> {
    // Step 1: obtain the challenge code and keep it for the whole session.
    let challenge = get_challenge_key().context("failed to obtain challenge code")?;

    // Step 2: fetch the file list (signed with the stored challenge).
    let list_time = get_current_utc8_time();
    let list_url = build_client_file_list_url(&challenge, list_time);
    let contents = http_get_text(&list_url).context("failed to download client file list")?;

    let mut lines = contents.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().context("client file list is empty")?;

    // Step 4: version number, and step 6: domain + base path.
    let version = header
        .rsplit('|')
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("missing version in client file list header"))?
        .trim()
        .to_owned();
    let (domain, base_path) = parse_header_location(header)?;

    // Step 3: parse every remaining entry.
    // TODO: temporary cap for testing; uncomment to limit the number of files.
    // const MAX_FILES_FOR_TESTING: usize = 100;
    let entries: Vec<FileEntry> = lines
        // .take(MAX_FILES_FOR_TESTING)
        .map(parse_entry)
        .collect::<Result<_>>()?;

    let total_bytes: u64 = entries.iter().map(|e| e.file_size).sum();

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
        // Shared borrows for all workers (references are Copy).
        let entries = &entries;
        let counter = &counter;
        let downloaded = &downloaded;
        let skipped = &skipped;
        let failures = &failures;
        let total_pb = &total_pb;
        let challenge = challenge.as_str();
        let version = version.as_str();
        let domain = domain.as_str();
        let base_path = base_path.as_str();

        for bar in worker_bars.iter().cloned() {
            scope.spawn(move || {
                loop {
                    let idx = counter.fetch_add(1, Ordering::Relaxed);
                    if idx >= entries.len() {
                        break;
                    }
                    let entry = &entries[idx];
                    let rel_name = format!("{}{}", entry.file_location, entry.file_name);
                    let local_path = target_dir.join(&entry.file_location).join(&entry.file_name);

                    // Skip files already present and intact.
                    if is_up_to_date(&local_path, entry).unwrap_or(false) {
                        skipped.fetch_add(1, Ordering::Relaxed);
                        total_pb.inc(entry.file_size);
                        continue;
                    }

                    // Step 5: obfuscated server-side name.
                    let obf_name = obfuscated_file_name(version, &entry.raw_path);

                    // Steps 7 & 8: per-file signed URL (fresh time per request).
                    let file_time = get_current_utc8_time();
                    let signature_input = format!(
                        "{challenge}{file_time}{base_path}/{}{obf_name}",
                        entry.file_location
                    );
                    let signature = md5_hex(&signature_input);
                    let url = format!(
                        "{domain}/{file_time}/{signature}{base_path}/{}{obf_name}",
                        entry.file_location
                    );

                    bar.set_length(entry.file_size);
                    bar.set_position(0);
                    bar.set_message(rel_name.clone());

                    // Step 9: download to <target_dir>/<file_location><file_name>.
                    match download_file(
                        &url,
                        &local_path,
                        entry.file_size,
                        SEGMENTS_PER_FILE,
                        &bar,
                        total_pb,
                    ) {
                        Ok(()) => {
                            downloaded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            failures.lock().unwrap().push(format!("{rel_name}: {e:#}"));
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

/// Download `url` to `dest`, using parallel byte-range segments for large files.
fn download_file(
    url: &str,
    dest: &Path,
    size: u64,
    max_segments: usize,
    pb: &ProgressBar,
    total: &ProgressBar,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let segments = effective_segments(size, max_segments);
    if segments <= 1 || !supports_ranges(url) {
        return download_single(url, dest, pb, total);
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
            .map(|(start, end)| scope.spawn(move || download_range(url, dest, start, end, pb, total)))
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

/// Download a single byte range `[start, end]` of `url` into `dest` at `start`.
fn download_range(
    url: &str,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    total: &ProgressBar,
) -> Result<()> {
    let resp = ureq::get(url)
        .set("Range", &format!("bytes={start}-{end}"))
        .call()
        .context("HTTP range request failed")?;
    let mut reader = resp.into_reader();

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;
    file.seek(SeekFrom::Start(start))
        .with_context(|| format!("failed to seek in {}", dest.display()))?;

    copy_with_progress(&mut reader, &mut file, pb, total)
}

/// Download the whole of `url` into `dest` in a single stream.
fn download_single(url: &str, dest: &Path, pb: &ProgressBar, total: &ProgressBar) -> Result<()> {
    let resp = ureq::get(url).call().context("HTTP request failed")?;
    let mut reader = resp.into_reader();

    let mut file = std::fs::File::create(dest)
        .with_context(|| format!("failed to create file {}", dest.display()))?;

    copy_with_progress(&mut reader, &mut file, pb, total)
}

/// Copy from `reader` to `writer`, advancing both the file and total progress bars.
fn copy_with_progress<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    pb: &ProgressBar,
    total: &ProgressBar,
) -> Result<()> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).context("failed to write to disk")?;
        pb.inc(n as u64);
        total.inc(n as u64);
    }
    Ok(())
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
fn supports_ranges(url: &str) -> bool {
    match ureq::get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => resp.status() == 206,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hex values taken from a real v3ctrl.xml response.
    const SERVER_LET_HEX: &str = "451a58c3de16c4d133d4d3fa8fee0e4e0de76b77a4156224ca5ea186f22db1f4af56427d5f0ee7bf6a7f96401d1890a158f26d7542d170815b5e81514a869bef8bfb131109281b0125d7904597671aa62637c5fe9bcee704d8893ac5f0f2358eb82749b08ab493d526af2fc30e0aa8d7bd1677e945483db7570957910bd5ea48";
    const CLIENT_HEX: &str = "95338a729569daf8fdce6e0734be13296679204adf31007897a615b3b43c8597ac61f6979a97254cde9e8f45355221814cc69d1d1ab6d754a16982f078baf43d74ade36c8c494992318a97a62954587e2c12fb6f1d2e6553fbb1e46b3b53af6de95b8dda496f50b85652f7cde6612af53e770959b13254c4cf1031e45d590f10";

    fn key() -> RsaPublicKey {
        public_key().unwrap()
    }

    #[test]
    fn decrypts_server_let_md5key() {
        let decrypted = decrypt_md5key(&key(), SERVER_LET_HEX).unwrap();
        assert_eq!(decrypted, "89T532jrQxUen6375E983L7758vajQSz");
    }

    #[test]
    fn decrypts_client_md5key() {
        let decrypted = decrypt_md5key(&key(), CLIENT_HEX).unwrap();
        assert_eq!(decrypted, "A9D8rTV72Fh7O8w7XPLp672657844VeS");
    }

    #[test]
    fn builds_expected_challenge_key() {
        let server_let = decrypt_md5key(&key(), SERVER_LET_HEX).unwrap();
        let client = decrypt_md5key(&key(), CLIENT_HEX).unwrap();
        let challenge = build_challenge_key(&client, &server_let).unwrap();
        assert_eq!(challenge, "A9D8rTV72Fh7O8w75E983L7758vajQSz");
    }

    #[test]
    fn parses_client_file_list_summary() {
        let contents = "https://host/path|5|0.0.0.15\n\
                        mxd\\a.dll|100|ABC\n\
                        mxd\\b.dll|250|DEF\n\
                        \n\
                        mxd\\c.dll|650|GHI\n";
        let info = parse_client_file_list(contents).unwrap();
        assert_eq!(info.version, "0.0.0.15");
        assert_eq!(info.file_count, 3);
        assert_eq!(info.total_size, 1000);
    }

    #[test]
    fn parses_file_entry() {
        let entry =
            parse_entry("mxd\\Data\\Character\\Android\\Android_000.wz|236513|1CF163EDA833A9E5515494DA52057B63")
                .unwrap();
        assert_eq!(entry.raw_path, "mxd\\Data\\Character\\Android\\Android_000.wz");
        assert_eq!(entry.file_location, "mxd/Data/Character/Android/");
        assert_eq!(entry.file_name, "Android_000.wz");
        assert_eq!(entry.file_size, 236513);
        assert_eq!(entry.md5_checksum, "1CF163EDA833A9E5515494DA52057B63");
    }

    #[test]
    fn parses_header_location() {
        let (domain, base_path) = parse_header_location(
            "https://mxdver0.jijiagames.com/v3client/build/5/8848/apppc/1020|5|0.0.0.15",
        )
        .unwrap();
        assert_eq!(domain, "https://mxdver0.jijiagames.com");
        assert_eq!(base_path, "/v3client/build/5/8848/apppc/1020");
    }

    #[test]
    fn computes_obfuscated_file_name() {
        // UTF-16LE MD5 of "5_0.0.0.9_mxd\bdvid64.dll", uppercased.
        let name = obfuscated_file_name("0.0.0.9", "mxd\\bdvid64.dll");
        assert_eq!(name, "3A3BFEC833C1EA8EA541F20593ABFB0A");
    }

    #[test]
    fn splits_ranges_to_cover_whole_file() {
        let ranges = compute_ranges(1003, 5);
        assert_eq!(ranges.len(), 5);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 1002);
        // Contiguous, non-overlapping, and fully covering.
        for pair in ranges.windows(2) {
            assert_eq!(pair[1].0, pair[0].1 + 1);
        }
        let covered: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(covered, 1003);
    }

    #[test]
    fn picks_segment_count_by_size() {
        assert_eq!(effective_segments(15, 5), 1); // tiny file -> single thread
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 3, 5), 3);
        assert_eq!(effective_segments(MIN_SEGMENT_SIZE * 100, 5), 5); // capped
        assert_eq!(effective_segments(0, 5), 1);
    }
}
