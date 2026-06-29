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
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Number of files downloaded concurrently.
const PARALLEL_FILES: usize = 10;

/// Maximum number of segments (threads) used per file.
const SEGMENTS_PER_FILE: usize = 5;

/// Smallest segment size; files smaller than this are downloaded single-threaded.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// If no data arrives on a connection for this long, the read fails and the
/// download is treated as stalled, triggering a re-signed resume.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for establishing a connection (and resolving DNS).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of consecutive stalls (with no bytes received) tolerated for a
/// single segment before it is reported as failed. The counter resets whenever
/// any progress is made, so flaky-but-advancing connections keep going.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before re-signing the URL and resuming after a stall.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

/// URL of the CMS launcher control file.
const CTRL_XML_URL: &str = "https://downloader.dorado.sdo.com/v3launcher/5/v3ctrl.xml";

/// URL of the CMS patch metadata file (`ver2.dat`), a JSON document listing
/// every published incremental patch. It is served without any signing.
const PATCH_DATA_URL: &str =
    "https://v3launcher.jijiagames.com/v3launcher/build/ver2data/5/8848/-1/ver2.dat";

/// Host serving the signed client download files.
const DOWNLOAD_HOST: &str = "https://mxdver0.jijiagames.com";

/// Portion of the client-file-list path that precedes the build number.
const CLIENT_FILE_LIST_PATH_PREFIX: &str = "/v3client/build/5/8848/apppc/";

/// Portion of the client-file-list path that follows the build number.
const CLIENT_FILE_LIST_PATH_SUFFIX: &str = "/client_all_files_list.dat";

/// Build number to start the exhaustive search from when neither a persisted
/// value nor the launcher's initial number is available.
const DEFAULT_CLIENT_NUMBER: u32 = 961;

/// Unsigned launcher file list whose header records the current build number.
/// Used to seed the exhaustive search the first time, before any value has been
/// persisted to [`LAST_CLIENT_VERSION_FILE`].
const INITIAL_CLIENT_LIST_URL: &str =
    "https://v3launcher.jijiagames.com/v3launcher/build/5/8848/client-all-files-list/client_all_files_list.dat";

/// Number of build numbers to probe past the last known version when searching
/// for the newest one. A fixed window (rather than stopping at the first gap)
/// tolerates missing intermediate builds.
const SEARCH_WINDOW: u32 = 160;

/// Number of concurrent threads used to probe the search window.
const PARALLEL_PROBES: usize = 16;

/// Name of the file used to remember the highest build number found, so the
/// next exhaustive search can resume from there instead of from the default.
const LAST_CLIENT_VERSION_FILE: &str = "last_client_version.ini";

/// Build the client-file-list path (relative to the download host) for a given
/// build number, e.g. `/v3client/build/5/8848/apppc/1020/client_all_files_list.dat`.
fn client_file_list_path(number: u32) -> String {
    format!("{CLIENT_FILE_LIST_PATH_PREFIX}{number}{CLIENT_FILE_LIST_PATH_SUFFIX}")
}

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
pub fn get_challenge_key(agent: &ureq::Agent) -> Result<String> {
    let xml = fetch_ctrl_xml(agent).context("failed to fetch v3ctrl.xml")?;
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
fn fetch_ctrl_xml(agent: &ureq::Agent) -> Result<String> {
    // The document declares `encoding="gbk"`, but every value we care about is
    // ASCII hex, so a lossy UTF-8 decode is sufficient.
    http_get_text(agent, CTRL_XML_URL).context("HTTP request failed")
}

/// Perform a GET request and return the response body as (lossy) UTF-8 text.
///
/// Retries up to [`HTTP_GET_TEXT_RETRIES`] times on transient failures (timeouts,
/// connection errors, read errors). The last error is returned if all attempts fail.
fn http_get_text(agent: &ureq::Agent, url: &str) -> Result<String> {
    const HTTP_GET_TEXT_RETRIES: usize = 10;

    let mut last_err = anyhow!("no attempts made");

    for _ in 0..=HTTP_GET_TEXT_RETRIES {
        let resp = match agent.get(url).call().context("HTTP request failed") {
            Ok(r) => r,
            Err(e) => {
                last_err = e;
                continue;
            }
        };

        let mut reader = resp.into_reader();
        let mut buf = Vec::new();
        match std::io::Read::read_to_end(&mut reader, &mut buf)
            .context("failed to read response body")
        {
            Ok(_) => return Ok(String::from_utf8_lossy(&buf).into_owned()),
            Err(e) => {
                last_err = e;
                continue;
            }
        }
    }

    Err(last_err)
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

/// A single incremental patch entry parsed from the patch metadata
/// (`ver2.dat`). Only the fields relevant for listing and applying are captured.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PatchPackage {
    /// Source version the patch upgrades from (e.g. `0.0.0.1`).
    pub from: String,
    /// Target version the patch upgrades to (e.g. `0.0.0.3`).
    pub to: String,
    /// Human-facing display version (e.g. `v222`).
    #[serde(rename = "versionView")]
    pub version_view: String,
    /// Path (relative to the patch `baseUrl`) of this patch's `FileList.dat`,
    /// e.g. `/0.0.0.14-0.0.0.15-<hash>/5_0.0.0.14-0.0.0.15_FileList.dat`.
    #[serde(rename = "fileListUrl")]
    pub file_list_url: String,
}

/// The patch metadata document (`ver2.dat`): a base URL plus the list of
/// published incremental patches.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PatchData {
    /// Base URL the patch `fileListUrl`s are relative to, e.g.
    /// `https://mxdver0.jijiagames.com/v3client/build/5/8848/diff`.
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    /// Every published patch, in chained order.
    pub packages: Vec<PatchPackage>,
}

/// Fetch and parse the CMS patch metadata (`ver2.dat`).
///
/// The metadata is a JSON document served without any signing/challenge.
pub fn get_patch_data(allow_insecure: bool, proxy: Option<&str>) -> Result<PatchData> {
    let agent = crate::net::agent(allow_insecure, proxy);
    let body = http_get_text(&agent, PATCH_DATA_URL).context("failed to fetch patch metadata")?;
    serde_json::from_str(&body).context("failed to parse patch metadata JSON")
}

/// Strip the leading download host from a full URL, returning the path portion
/// (e.g. `https://mxdver0.jijiagames.com/v3client/...` -> `/v3client/...`).
///
/// If `url` does not start with the known [`DOWNLOAD_HOST`], it is returned
/// unchanged (it may already be a host-relative path).
pub(crate) fn strip_download_host(url: &str) -> &str {
    url.strip_prefix(DOWNLOAD_HOST).unwrap_or(url)
}

/// Summary information parsed from a client file list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientFileList {
    /// Build number discovered by exhaustive search (e.g. `1021`).
    pub build_number: u32,
    /// Client version, taken from the last field of the header line (e.g. `0.0.0.15`).
    pub version: String,
    /// Display version (e.g. `"V226.1"`) parsed from the server's
    /// `LocalVersion3.xml`, when that file is included in the build manifest.
    pub local_version_view: Option<String>,
    /// Sum of the size field across all file entries, in bytes.
    pub total_size: u64,
    /// Number of file entries (non-empty lines, excluding the header line).
    pub file_count: usize,
}

/// Download and parse the client file list summary.
///
/// When `build` is `Some(n)`, the manifest for build `n` is fetched directly
/// and an error is returned if that build does not exist on the server.
/// When `build` is `None`, the latest build is discovered by exhaustive search.
pub fn get_client_file_list_info(allow_insecure: bool, proxy: Option<&str>, build: Option<u32>) -> Result<ClientFileList> {
    let (info, _) = get_client_file_list_full(allow_insecure, proxy, build)?;
    Ok(info)
}

/// Download and parse the client file list, returning both the summary and the
/// full list of `(forward-slash path, size)` pairs for every file entry.
///
/// When `build` is `Some(n)`, the manifest for build `n` is fetched directly
/// and an error is returned if that build does not exist on the server.
/// When `build` is `None`, the latest build is discovered by exhaustive search.
pub fn get_client_file_list_full(allow_insecure: bool, proxy: Option<&str>, build: Option<u32>) -> Result<(ClientFileList, Vec<(String, u64)>)> {
    let agent = crate::net::agent(allow_insecure, proxy);
    let challenge_code = get_challenge_key(&agent).context("failed to obtain challenge code")?;

    let (number, contents) = fetch_build_and_contents(&agent, &challenge_code, build)?;

    let (mut info, entries) = parse_client_file_list_with_paths(&contents)?;
    info.build_number = number;

    // If LocalVersion3.xml is part of this build's manifest, try fetching and
    // parsing it to obtain the display version view (e.g. "V226.1").
    info.local_version_view =
        try_fetch_local_version_xml_view(&agent, &challenge_code, &info.version, &contents, &entries);

    Ok((info, entries))
}

/// Resolve a build number and fetch the corresponding file-list contents.
///
/// When `build` is `Some(n)`, the manifest for `n` is fetched directly; an
/// error is returned when the build does not exist on the server.
/// When `build` is `None`, the latest build is discovered by exhaustive search
/// (see [`discover_latest_client_number_with`]) and its manifest is fetched.
fn fetch_build_and_contents(
    agent: &ureq::Agent,
    challenge: &str,
    build: Option<u32>,
) -> Result<(u32, String)> {
    match build {
        Some(n) => match fetch_client_file_list_for(agent, challenge, n)? {
            Some(contents) => Ok((n, contents)),
            None => bail!("build {n} does not exist or is not available on the server"),
        },
        None => {
            let number = discover_latest_client_number_with(agent, challenge)?;
            let utc8_time = get_current_utc8_time();
            let url = build_signed_url(challenge, utc8_time, &client_file_list_path(number));
            let contents =
                http_get_text(agent, &url).context("failed to download client file list")?;
            Ok((number, contents))
        }
    }
}

/// Parse a client file list, returning both the summary and a
/// `(forward-slash path, size)` pair for every file entry.
fn parse_client_file_list_with_paths(contents: &str) -> Result<(ClientFileList, Vec<(String, u64)>)> {
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
    let mut entries: Vec<(String, u64)> = Vec::new();
    for line in lines {
        let mut parts = line.split('|');
        let raw_path = parts
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("missing path field in entry: {line}"))?;
        let size_str = parts
            .next()
            .map(str::trim)
            .ok_or_else(|| anyhow!("missing size field in entry: {line}"))?;
        let size = size_str
            .parse::<u64>()
            .with_context(|| format!("invalid size field in entry: {line}"))?;
        total_size += size;
        entries.push((raw_path.replace('\\', "/"), size));
    }

    let file_count = entries.len();
    Ok((
        ClientFileList {
            build_number: 0,
            version,
            local_version_view: None,
            total_size,
            file_count,
        },
        entries,
    ))
}

/// Build a signed download URL for an arbitrary `path` (relative to the host).
///
/// The path is signed with an MD5 of `<challengeCode><utc8Time><path>`, and the
/// resulting URL is `<host>/<utc8Time>/<md5><path>`.
pub(crate) fn build_signed_url(challenge_code: &str, utc8_time: u64, path: &str) -> String {
    build_signed_url_for_host(DOWNLOAD_HOST, challenge_code, utc8_time, path)
}

/// Try fetching `LocalVersion3.xml` for the current build and parse its
/// `view` field (e.g. `"V226.1"`).  Returns `None` when the file is not
/// listed in `entries`, the download fails, or the XML is malformed.
fn try_fetch_local_version_xml_view(
    agent: &ureq::Agent,
    challenge_code: &str,
    version: &str,
    contents: &str,
    entries: &[(String, u64)],
) -> Option<String> {
    // Only attempt the fetch when mxd/LocalVersion3.xml is present.
    if !entries
        .iter()
        .any(|(p, _)| p.eq_ignore_ascii_case("mxd/LocalVersion3.xml"))
    {
        return None;
    }

    // Re-parse the header to obtain the domain and base path.
    let header = contents.lines().find(|l| !l.trim().is_empty())?;
    let (domain, base_path) = parse_header_location(header).ok()?;

    let raw_path = "mxd\\LocalVersion3.xml";
    let obf_name = obfuscated_file_name(version, raw_path);
    let path = format!("{base_path}/mxd/{obf_name}");
    let utc8_time = get_current_utc8_time();
    let url = build_signed_url_for_host(&domain, challenge_code, utc8_time, &path);

    let body = http_get_text(agent, &url).ok()?;
    parse_local_version_xml_view(&body)
}

/// Extract the `view` field from a `LocalVersion3.xml` body.
fn parse_local_version_xml_view(xml: &str) -> Option<String> {
    let tag = "<zone5_8848_v3>";
    let start = xml.find(tag)? + tag.len();
    let end = xml.find("</zone5_8848_v3>")?;
    let json: serde_json::Value = serde_json::from_str(&xml[start..end]).ok()?;
    let view = json.get("version")?.get("view")?.as_str()?.to_owned();
    if view.is_empty() { None } else { Some(view) }
}

/// Like [`build_signed_url`] but uses a custom `host`
/// (e.g. `https://mxdcclient.jijiagames.com`).
pub(crate) fn build_signed_url_for_host(
    host: &str,
    challenge_code: &str,
    utc8_time: u64,
    path: &str,
) -> String {
    let signature_input = format!("{challenge_code}{utc8_time}{path}");
    let signature = md5_hex(&signature_input);
    format!("{host}/{utc8_time}/{signature}{path}")
}

/// Fetch the client file list for a specific build `number`.
///
/// Returns `Ok(Some(contents))` when the list exists (HTTP 200), `Ok(None)` when
/// the server reports it as unavailable (HTTP 403 or 404), and `Ok(None)` when
/// all retries are exhausted due to transient transport failures (treated as an
/// invalid/unavailable version). Up to [`FETCH_FILE_LIST_RETRIES`] retries are
/// attempted; the URL is re-signed on each attempt since it embeds a timestamp.
fn fetch_client_file_list_for(
    agent: &ureq::Agent,
    challenge_code: &str,
    number: u32,
) -> Result<Option<String>> {
    const FETCH_FILE_LIST_RETRIES: usize = 10;

    for attempt in 0..=FETCH_FILE_LIST_RETRIES {
        let utc8_time = get_current_utc8_time();
        let url = build_signed_url(challenge_code, utc8_time, &client_file_list_path(number));

        let resp = match agent.get(&url).call() {
            Ok(r) => r,
            // The build is simply not published (yet): not an error, just a stop signal.
            Err(ureq::Error::Status(403, _)) | Err(ureq::Error::Status(404, _)) => {
                return Ok(None);
            }
            Err(_) if attempt < FETCH_FILE_LIST_RETRIES => continue,
            // All retries exhausted: treat as an invalid version rather than a hard error.
            Err(_) => return Ok(None),
        };

        let mut reader = resp.into_reader();
        let mut buf = Vec::new();
        match reader.read_to_end(&mut buf) {
            Ok(_) => return Ok(Some(String::from_utf8_lossy(&buf).into_owned())),
            Err(_) if attempt < FETCH_FILE_LIST_RETRIES => continue,
            // All retries exhausted: treat as an invalid version rather than a hard error.
            Err(_) => return Ok(None),
        }
    }

    Ok(None)
}

/// Discover the highest available build number by exhaustive search, reusing an
/// already-computed challenge code.
///
/// Starting from the persisted (or seeded) number, it probes the next
/// [`SEARCH_WINDOW`] numbers and takes the highest one that still returns a
/// list. The result is written back to [`LAST_CLIENT_VERSION_FILE`] for the
/// next run.
fn discover_latest_client_number_with(agent: &ureq::Agent, challenge: &str) -> Result<u32> {
    // Prefer the persisted value; otherwise seed from the launcher's published
    // number; fall back to the hardcoded default only if both are unavailable.
    let start = load_last_client_version()
        .or_else(|| fetch_initial_client_number(agent))
        .unwrap_or(DEFAULT_CLIENT_NUMBER);

    // Confirm the starting point is valid. If a stale starting value is no
    // longer available, fall back to the known-good default and search again.
    let mut current = if fetch_client_file_list_for(agent, challenge, start)?.is_some() {
        start
    } else if start != DEFAULT_CLIENT_NUMBER
        && fetch_client_file_list_for(agent, challenge, DEFAULT_CLIENT_NUMBER)?.is_some()
    {
        DEFAULT_CLIENT_NUMBER
    } else {
        bail!("no client file list found to start the exhaustive search from");
    };

    // Probe the next SEARCH_WINDOW numbers across PARALLEL_PROBES threads and
    // keep the highest that still returns a list, so gaps (missing intermediate
    // builds) don't end the search prematurely.
    let base = current;
    let max_found = AtomicU32::new(current);
    let next_offset = AtomicU32::new(1);
    let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        let max_found = &max_found;
        let next_offset = &next_offset;
        let first_err = &first_err;
        let agent = &agent;

        for _ in 0..PARALLEL_PROBES {
            scope.spawn(move || loop {
                let offset = next_offset.fetch_add(1, Ordering::Relaxed);
                if offset > SEARCH_WINDOW {
                    break;
                }
                let candidate = base + offset;
                match fetch_client_file_list_for(agent, challenge, candidate) {
                    Ok(Some(_)) => {
                        max_found.fetch_max(candidate, Ordering::Relaxed);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let mut slot = first_err.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(e);
                        }
                        // Stop the remaining probes once any request hard-fails.
                        next_offset.store(SEARCH_WINDOW + 1, Ordering::Relaxed);
                        break;
                    }
                }
            });
        }
    });

    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }
    current = max_found.load(Ordering::Relaxed);

    save_last_client_version(current)?;
    Ok(current)
}

/// Read the highest build number recorded in [`LAST_CLIENT_VERSION_FILE`].
///
/// Returns `None` when the file is absent or contains no parseable value.
fn load_last_client_version() -> Option<u32> {
    let contents = std::fs::read_to_string(LAST_CLIENT_VERSION_FILE).ok()?;
    parse_last_client_version(&contents)
}

/// Read the initial build number from the public launcher file list.
///
/// The list's header location ends in the build number (e.g. `.../apppc/961`),
/// which is returned. Any failure yields `None` so callers can fall back to
/// [`DEFAULT_CLIENT_NUMBER`].
fn fetch_initial_client_number(agent: &ureq::Agent) -> Option<u32> {
    let contents = http_get_text(agent, INITIAL_CLIENT_LIST_URL).ok()?;
    parse_client_number_from_header(&contents)
}

/// Extract the build number from the first (header) line of a file list.
///
/// The header's first field is a URL whose final path segment is the build
/// number, e.g. `https://.../apppc/961|5|0.0.0.9` -> `961`.
fn parse_client_number_from_header(contents: &str) -> Option<u32> {
    let header = contents.lines().find(|l| !l.trim().is_empty())?;
    let (_, base_path) = parse_header_location(header).ok()?;
    base_path.rsplit('/').next()?.trim().parse().ok()
}

/// Parse the `last_client_version` value out of the INI contents.
fn parse_last_client_version(contents: &str) -> Option<u32> {
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("last_client_version") {
                if let Ok(n) = value.trim().parse::<u32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Persist the highest build number to [`LAST_CLIENT_VERSION_FILE`] as INI.
fn save_last_client_version(number: u32) -> Result<()> {
    let contents = format!("[CMS]\nlast_client_version = {number}\n");
    std::fs::write(LAST_CLIENT_VERSION_FILE, contents)
        .with_context(|| format!("failed to write {LAST_CLIENT_VERSION_FILE}"))
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
///
/// On Windows, once the download succeeds, launcher shortcuts are created (see
/// [`create_shortcuts`]). When `skip_create_shortcut` is set, the desktop and
/// Start Menu shortcuts are skipped, but the one next to the cmsdl binary is
/// still created.
pub fn download_client(
    target_dir: &Path,
    wz_only: bool,
    filter: Option<&crate::filter::FileFilter>,
    allow_insecure: bool,
    proxy: Option<&str>,
    build: Option<u32>,
    purge_wz_files: bool,
) -> Result<()> {
    // A shared HTTP agent with a read timeout, so a connection that stops
    // delivering data surfaces as an error (instead of hanging forever) and can
    // be resumed from its current byte offset with a freshly-signed URL. The
    // same agent is reused for the control/list metadata requests.
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();

    // Step 1: obtain the challenge code and keep it for the whole session.
    let challenge = get_challenge_key(&agent).context("failed to obtain challenge code")?;

    // Step 2: resolve the build number (discover the latest, or use the one
    // provided via --build), then fetch its file list.
    if build.is_none() {
        println!("scanning for the latest build version...");
    }
    let (number, contents) = fetch_build_and_contents(&agent, &challenge, build)?;

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

    println!("latest version: {version} (build {number}); starting download.");

    // Step 3: parse every remaining entry.
    // TODO: temporary cap for testing; uncomment to limit the number of files.
    // const MAX_FILES_FOR_TESTING: usize = 100;
    let entries: Vec<FileEntry> = lines
        // .take(MAX_FILES_FOR_TESTING)
        .map(parse_entry)
        .collect::<Result<_>>()?;

    // Purge stray files in mxd/Data/ before downloading (full manifest, pre-filter).
    if purge_wz_files {
        purge_junk_dirs(&target_dir.join("mxd"))?;
        purge_data_files(target_dir, &entries)?;
    }

    // When `--download-wz-only` is set, keep only the data files (paths under
    // `mxd/Data`). The published paths use backslashes, but `file_location`
    // is already normalized to forward slashes.
    let entries: Vec<FileEntry> = if wz_only {
        let kept: Vec<FileEntry> = entries
            .into_iter()
            .filter(|e| {
                e.file_location
                    .to_ascii_lowercase()
                    .starts_with("mxd/data/")
            })
            .collect();
        println!(
            "Limiting download to {} WZ file(s) under mxd/Data.",
            kept.len()
        );
        kept
    } else {
        entries
    };

    // Apply the user-supplied path filter, if any.
    let entries: Vec<FileEntry> = if let Some(f) = filter {
        let kept: Vec<FileEntry> = entries
            .into_iter()
            .filter(|e| {
                let path = format!("{}{}", e.file_location, e.file_name);
                f.matches(&path)
            })
            .collect();
        println!("Filter applied: {} file(s) match.", kept.len());
        kept
    } else {
        entries
    };

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
        let agent = &agent;
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

                    // Step 5: obfuscated server-side name, and steps 7 & 8: a
                    // re-signable URL (each `build()` uses the current time).
                    let url = SignedUrl::new(challenge, version, domain, base_path, entry);

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
                        agent,
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

    // Look up the version view (e.g. "V226.1") from the patch metadata so
    // LocalVersion3.xml can be written with the correct display version.
    let version_view = fetch_version_view_for(&agent, &version).unwrap_or_default();

    // If LocalVersion3.xml is already part of the downloaded file list, skip
    // writing our own copy to avoid overwriting the server-provided one.
    let has_local_version_xml = entries.iter().any(|e| {
        e.file_location == "mxd/" && e.file_name.eq_ignore_ascii_case("LocalVersion3.xml")
    });

    // Record the downloaded version at <target_dir>/mxd/cmsdl.ver and
    // write a matching LocalVersion3.xml for the launcher (if not in the
    // server's file list).
    if let Err(e) = write_version_file(target_dir, &version, &version_view, !has_local_version_xml) {
        eprintln!("warning: failed to write version file: {e:#}");
    }

    Ok(())
}

/// Name of the file recording the downloaded client version.
const VERSION_FILE_NAME: &str = "cmsdl.ver";

/// Fetch the version view string (e.g. `"V226.1"`) for a given client
/// `version` (e.g. `"0.0.0.15"`) from the patch metadata.
///
/// Returns `None` when the patch data cannot be fetched or the version is not
/// listed.
fn fetch_version_view_for(agent: &ureq::Agent, version: &str) -> Option<String> {
    let body = http_get_text(agent, PATCH_DATA_URL).ok()?;
    let data: PatchData = serde_json::from_str(&body).ok()?;
    data.packages
        .iter()
        .find(|p| p.to == version)
        .map(|p| p.version_view.clone())
}

/// Write the client `version` (e.g. `0.0.0.15`) to
/// `<target_dir>/mxd/cmsdl.ver`. When `write_xml` is true, also write a
/// matching `LocalVersion3.xml` for the launcher. Creates the `mxd`
/// directory if needed.
fn write_version_file(target_dir: &Path, version: &str, version_view: &str, write_xml: bool) -> Result<()> {
    let mxd_dir = target_dir.join("mxd");
    std::fs::create_dir_all(&mxd_dir)
        .with_context(|| format!("failed to create directory {}", mxd_dir.display()))?;

    let ver_path = mxd_dir.join(VERSION_FILE_NAME);
    std::fs::write(&ver_path, version)
        .with_context(|| format!("failed to write {}", ver_path.display()))?;
    println!("wrote version {version} to {}", ver_path.display());

    if write_xml {
        let xml_path = mxd_dir.join("LocalVersion3.xml");
        let xml = format!(
            r#"<?xmlversion="1.0"encoding="utf-8"?><Root><zone5_8848_v3>{{"product_name":"zone5_8848_v3","version":{{"v":"{version}","view":"{version_view}"}}}}</zone5_8848_v3></Root>"#
        );
        std::fs::write(&xml_path, &xml)
            .with_context(|| format!("failed to write {}", xml_path.display()))?;
        println!("wrote LocalVersion3.xml to {}", xml_path.display());
    }

    Ok(())
}

/// Create a launcher shortcut for the CMS client at `target_dir`.
///
/// Steps:
///   1. Verify `<target_dir>/mxd/MapleStory.exe` exists.
///   2. Copy cmsdl to `<target_dir>/cmsdl.exe` unless it is already there.
///   3. Choose the shortcut name by OS UI language: Simplified Chinese →
///      `"冒险岛"`; any other language → `"MapleStory CN"`.
///   4. Create a shortcut pointing at
///      `<target_dir>\cmsdl.exe cms --patch latest <target_dir> --launch-after-patching`
///      with the icon taken from `<target_dir>\mxd\MapleStory.exe`.
///   5. Place shortcuts on the desktop, Start Menu > Programs, and in `target_dir`.
#[cfg(windows)]
pub fn create_shortcut(target_dir: &Path, lrhook: bool) -> Result<()> {
    // Canonicalize early so the icon path in the shortcut is absolute,
    // which works regardless of where the .lnk is placed.
    let target_dir = target_dir
        .canonicalize()
        .with_context(|| format!("failed to resolve '{}'", target_dir.display()))?;

    // Step 1: verify that the client executable exists.
    let maple_exe = target_dir.join("mxd").join("MapleStory.exe");
    if !maple_exe.exists() {
        bail!(
            "MapleStory.exe not found at '{}'; \
             ensure the client is downloaded before creating a shortcut",
            maple_exe.display()
        );
    }

    // Step 1.5: if --lrhook is requested, verify LocaleRemulator files exist.
    let use_lrhook = lrhook && locale_remulator_available(&target_dir);
    if lrhook && !use_lrhook {
        println!(
            "warning: --lrhook was specified but LocaleRemulator files are missing; \
             shortcut will launch without Locale Remulator."
        );
    }

    // Step 2: copy cmsdl to target_dir if it is not already there.
    let current_exe = std::env::current_exe().context("failed to determine cmsdl binary path")?;
    let cmsdl_in_target = target_dir.join("cmsdl.exe");

    let same_dir = current_exe
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p == target_dir)
        .unwrap_or(false);

    if !same_dir {
        println!("copying cmsdl.exe to '{}'...", target_dir.display());
        std::fs::copy(&current_exe, &cmsdl_in_target)
            .with_context(|| format!("failed to copy cmsdl to '{}'", cmsdl_in_target.display()))?;
    }

    // Step 3: choose shortcut name by OS UI language.
    let shortcut_name = if os_locale_is_simplified_chinese() {
        "冒险岛"
    } else {
        "MapleStory CN"
    };
    let lnk_name = format!("{shortcut_name}.lnk");

    // Steps 4 & 5: create shortcuts via PowerShell (supported on x86_64 and ARM64 only).
    let arch = std::env::consts::ARCH;
    if arch != "x86_64" && arch != "aarch64" {
        bail!("--create-shortcut is only supported on Windows x64 and ARM64; current architecture is {arch}");
    }
    run_create_shortcut_script(&target_dir, &cmsdl_in_target, &maple_exe, &lnk_name, use_lrhook)
}

/// Return `true` if all required LocaleRemulator files exist under
/// `<target_dir>/LocaleRemulator/`.
pub fn locale_remulator_available(target_dir: &Path) -> bool {
    let lr = target_dir.join("LocaleRemulator");
    lr.join("LRConfig.xml").is_file()
        && lr.join("LRHookx32.dll").is_file()
        && lr.join("LRHookx64.dll").is_file()
        && lr.join("LRProc.exe").is_file()
        && lr.join("LRSubMenus.dll").is_file()
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
pub fn create_shortcut(_target_dir: &Path, _lrhook: bool) -> Result<()> {
    bail!("--create-shortcut is only supported on Windows")
}

/// Return `true` if the OS UI language is Simplified Chinese (zh-CN / zh-SG).
#[cfg(windows)]
fn os_locale_is_simplified_chinese() -> bool {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            "[System.Globalization.CultureInfo]::CurrentUICulture.LCID",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let lcid: u32 = String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse()
                .unwrap_or(0);
            // 2052 = zh-CN (Simplified Chinese, China)
            // 4100 = zh-SG (Simplified Chinese, Singapore)
            lcid == 2052 || lcid == 4100
        }
        _ => false,
    }
}

/// Strip the `\\?\` extended-length path prefix that `Path::canonicalize()`
/// produces on Windows. WScript.Shell's COM API rejects paths with this prefix.
#[cfg(windows)]
fn strip_extended_prefix(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
}

/// Drive WScript.Shell via PowerShell to write `.lnk` files at the desktop,
/// Start Menu > Programs, and `target_dir`.
#[cfg(windows)]
fn run_create_shortcut_script(
    target_dir: &Path,
    cmsdl_exe: &Path,
    icon_exe: &Path,
    lnk_name: &str,
    include_lrhook: bool,
) -> Result<()> {
    use std::process::Command;

    // Encode a string as a PowerShell single-quoted literal.
    fn ps_single_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    // Strip the \\?\ extended-length prefix so WScript.Shell accepts the paths.
    let cmsdl_s = strip_extended_prefix(cmsdl_exe);
    let target_dir_s = strip_extended_prefix(target_dir);
    let icon_s = strip_extended_prefix(icon_exe);
    let explicit_lnk_s = strip_extended_prefix(&target_dir.join(lnk_name));

    let cmsdl_q = ps_single_quote(&cmsdl_s);
    let wd_q = ps_single_quote(&target_dir_s);

    // Windows paths cannot contain `"`, so the inner path needs no further escaping.
    let lrhook_flag = if include_lrhook { " --lrhook" } else { "" };
    let args = format!("cms --patch latest \"{target_dir_s}\" --launch-after-patching{lrhook_flag}");
    let args_q = ps_single_quote(&args);

    // IconLocation is "<exe path>,<icon index>".
    let icon_location = format!("{icon_s},0");
    let icon_q = ps_single_quote(&icon_location);

    let explicit_q = ps_single_quote(&explicit_lnk_s);
    let name_q = ps_single_quote(lnk_name);

    let script = format!(
        "$ErrorActionPreference = 'Stop'; \
         $ws = New-Object -ComObject WScript.Shell; \
         $paths = @( \
           {explicit_q}, \
           (Join-Path ([Environment]::GetFolderPath('Desktop')) {name_q}), \
           (Join-Path ([Environment]::GetFolderPath('Programs')) {name_q}) \
         ); \
         foreach ($p in $paths) {{ \
           $dir = Split-Path -Parent $p; \
           if ($dir -and -not (Test-Path $dir)) {{ New-Item -ItemType Directory -Path $dir -Force | Out-Null }}; \
           $s = $ws.CreateShortcut($p); \
           $s.TargetPath = {cmsdl_q}; \
           $s.Arguments = {args_q}; \
           $s.WorkingDirectory = {wd_q}; \
           $s.IconLocation = {icon_q}; \
           $s.Save(); \
         }}"
    );

    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .context("failed to launch PowerShell to create shortcuts")?;

    if !status.success() {
        bail!("PowerShell exited with status {status}");
    }

    println!(
        "created shortcut '{lnk_name}' at the desktop, Start Menu, and '{}'.",
        target_dir.display()
    );
    Ok(())
}

/// Delete junk directories at the root of `target_dir` that are left behind by
/// launchers: directories whose name ends with `.$$$`, and directories with
/// short random-looking 8.3 names (e.g. `vkoa9asd.qwv`). 
///
/// Directories ends with `.$$$` were generated by the legacy MS patcher, and
/// directories with random 8.3 names were generated by the WzComparerR2 patcher.
/// Sometimes, these directories are left behind after patching, 
/// and they can be safely deleted.
pub(crate) fn purge_junk_dirs(target_dir: &Path) -> Result<()> {
    let protected: &[&str] = &["mxd", "Data", "patchdata"];

    let entries: Vec<_> = match std::fs::read_dir(target_dir) {
        Ok(iter) => iter.filter_map(|e| e.ok()).collect(),
        Err(_) => return Ok(()),
    };

    let mut removed = 0usize;
    for entry in &entries {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if protected.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".$$$") || is_junk_83_dir(&lower) {
            std::fs::remove_dir_all(&path).with_context(|| {
                format!("failed to remove junk directory {}", path.display())
            })?;
            removed += 1;
        }
    }

    if removed > 0 {
        println!(
            "removed {removed} director(ies) created by legacy patcher at '{}'.",
            target_dir.display()
        );
    }

    Ok(())
}

/// Return `true` if `name` looks like a random 8.3 directory name (e.g.
/// `vkoa9asd.qwv`): exactly 8 alphanumeric chars, a dot, then 3 alphanumeric
/// chars.
fn is_junk_83_dir(name: &str) -> bool {
    if let Some((base, ext)) = name.split_once('.') {
        base.len() == 8
            && ext.len() == 3
            && base.chars().all(|c| c.is_ascii_alphanumeric())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}

/// Delete files under `<target_dir>/mxd/Data/` that are not listed in `entries`.
///
/// `entries` should be the full (unfiltered) list parsed from the client file
/// list. Only the directory `<target_dir>/mxd/Data/` is examined; files outside
/// it are left untouched.
fn purge_data_files(target_dir: &Path, entries: &[FileEntry]) -> Result<()> {
    let data_dir = target_dir.join("mxd").join("Data");
    if !data_dir.is_dir() {
        return Ok(());
    }

    // Build the set of expected paths (forward slashes, relative to target_dir).
    let expected: std::collections::HashSet<String> = entries
        .iter()
        .map(|e| format!("{}{}", e.file_location, e.file_name))
        .filter(|p| p.to_ascii_lowercase().starts_with("mxd/data/"))
        .collect();

    let mut deleted = 0usize;
    purge_dir_recursive(&data_dir, &expected, &data_dir, &mut deleted)?;

    if deleted > 0 {
        println!(
            "purged {deleted} stray file(s) from '{}'.",
            data_dir.display()
        );
    } else {
        println!("no stray files in '{}'.", data_dir.display());
    }

    Ok(())
}

/// Recursively walk `dir`, deleting any file whose path (relative to
/// `data_dir`, forward-slash, prepended with `mxd/Data/`) is absent from
/// `expected`. Empty subdirectories are removed after their children have been
/// processed.
fn purge_dir_recursive(
    dir: &Path,
    expected: &std::collections::HashSet<String>,
    data_dir: &Path,
    deleted: &mut usize,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            purge_dir_recursive(&path, expected, data_dir, deleted)?;
        } else {
            let rel = path
                .strip_prefix(data_dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            // Never purge resume sidecar files.
            if rel.to_ascii_lowercase().ends_with(".cmsdl") {
                continue;
            }
            let manifest_key = format!("mxd/Data/{rel}");
            if !expected.contains(&manifest_key) {
                std::fs::remove_file(&path).with_context(|| {
                    format!("failed to delete stray file {}", path.display())
                })?;
                *deleted += 1;
            }
        }
    }

    // Remove the directory itself if it is now empty (but never the root data_dir).
    if dir != data_dir {
        let _ = std::fs::remove_dir(dir);
    }

    Ok(())
}

/// Fetch the latest client manifest and purge stray files from
/// `<target_dir>/mxd/Data/`.
///
/// Used after patching to clean up files that are no longer referenced by the
/// latest full client index.
pub fn purge_wz_files_after_patch(
    target_dir: &Path,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    let agent = crate::net::agent(allow_insecure, proxy);
    let challenge = get_challenge_key(&agent).context("failed to obtain challenge code")?;
    println!("purging stray WZ files not in the latest manifest...");
    let number =
        discover_latest_client_number_with(&agent, &challenge).context("failed to discover latest build")?;
    let utc8_time = get_current_utc8_time();
    let url = build_signed_url(&challenge, utc8_time, &client_file_list_path(number));
    let contents =
        http_get_text(&agent, &url).context("failed to download client file list for purge")?;

    let mut lines = contents.lines().filter(|l| !l.trim().is_empty());
    let _header = lines.next().context("client file list is empty")?;
    let entries: Vec<FileEntry> = lines.map(parse_entry).collect::<Result<_>>()?;

    purge_junk_dirs(&target_dir.join("mxd"))?;
    purge_data_files(target_dir, &entries)
}

/// Builds a freshly-signed download URL for a single client file on demand.
///
/// The signature embeds a `yyyyMMddHHmm` timestamp, so a URL can expire or stop
/// serving data. Calling [`SignedUrl::build`] again produces a URL signed for
/// the current time, which is used to resume a stalled download from its
/// current byte offset.
struct SignedUrl<'a> {
    challenge: &'a str,
    domain: &'a str,
    base_path: &'a str,
    file_location: &'a str,
    obf_name: String,
}

impl<'a> SignedUrl<'a> {
    fn new(
        challenge: &'a str,
        version: &str,
        domain: &'a str,
        base_path: &'a str,
        entry: &'a FileEntry,
    ) -> Self {
        SignedUrl {
            challenge,
            domain,
            base_path,
            file_location: &entry.file_location,
            obf_name: obfuscated_file_name(version, &entry.raw_path),
        }
    }

    /// Produce a URL signed for the current UTC+8 time.
    fn build(&self) -> String {
        let file_time = get_current_utc8_time();
        let signature_input = format!(
            "{}{file_time}{}/{}{}",
            self.challenge, self.base_path, self.file_location, self.obf_name
        );
        let signature = md5_hex(&signature_input);
        format!(
            "{}/{file_time}/{signature}{}/{}{}",
            self.domain, self.base_path, self.file_location, self.obf_name
        )
    }
}

/// Download `url` to `dest`, using parallel byte-range segments for large files.
///
/// When a `.cmsdl` sidecar file exists next to `dest` and `dest` itself is
/// present with the expected size, the download is resumed from the saved
/// per-segment byte offsets (shifted back by 16 bytes for safety).  On
/// success the sidecar file is deleted.
fn download_file(
    url: &SignedUrl,
    dest: &Path,
    size: u64,
    max_segments: usize,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let segments = effective_segments(size, max_segments);
    if segments <= 1 || !supports_ranges(&url.build(), agent) {
        // Single-threaded path: remove any stale progress file then download.
        let _ = std::fs::remove_file(crate::resume::progress_path(dest));
        return download_single(url, dest, pb, total, agent);
    }

    // --- Determine ranges: resume from saved progress or start fresh. ---
    let progress_path = crate::resume::progress_path(dest);
    let saved_opt = crate::resume::read_progress(&progress_path)
        .filter(|_| dest.exists())
        .filter(|_| dest.metadata().map_or(false, |m| m.len() == size));

    let ranges: Vec<(u64, u64)>;
    let progress: crate::resume::FileProgress;

    if let Some(saved) = saved_opt
        .and_then(|s| crate::resume::build_resume_ranges(&s, size).map(|(r, pre)| (s, r, pre)))
    {
        let (saved_segs, resume_ranges, pre_completed) = saved;
        // Fast-forward progress bars for the bytes already on disk.
        pb.inc(pre_completed);
        total.inc(pre_completed);
        // Reuse the existing (pre-allocated) destination file.
        progress = crate::resume::FileProgress::from_saved(dest, &saved_segs, &resume_ranges)
            .with_context(|| format!("failed to write progress file {}", progress_path.display()))?;
        ranges = resume_ranges;
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

    let mut first_err: Option<anyhow::Error> = None;

    std::thread::scope(|scope| {
        let progress = &progress;
        let handles: Vec<_> = ranges
            .iter()
            .enumerate()
            .map(|(slot, &(start, end))| {
                scope.spawn(move || {
                    download_range(url, dest, start, end, pb, total, agent, progress, slot)
                })
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
        None => {
            progress.delete();
            Ok(())
        }
    }
}

/// Download a byte range `[start, end]` of `url` into `dest`, resuming from the
/// current offset (with a freshly-signed URL) whenever the connection stalls.
fn download_range(
    url: &SignedUrl,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
    progress: &crate::resume::FileProgress,
    slot: usize,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;

    let mut pos = start;
    let mut stalls = 0usize;

    while pos <= end {
        let before = pos;
        // Re-sign the URL for the current time and resume from `pos`.
        let signed = url.build();
        let _ = stream_segment(agent, &signed, &mut file, &mut pos, end, pb, total, progress, slot);

        // Record progress at every reconnect boundary (coarse-grained safety net).
        progress.update(slot, pos);

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
/// both progress bars as bytes arrive. Returns `Ok(())` on a clean end of
/// stream and `Err` on any I/O error (including a stall read timeout); in both
/// cases `*pos` reflects exactly how many bytes were written.
///
/// Progress is flushed to `progress` every [`crate::resume::PROGRESS_FLUSH_INTERVAL`]
/// bytes so that an interruption loses at most one flush interval of work.
fn stream_segment(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    end: u64,
    pb: &ProgressBar,
    total: &ProgressBar,
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
        total.inc(n as u64);
        if since_flush >= crate::resume::PROGRESS_FLUSH_INTERVAL {
            progress.update(slot, *pos);
            since_flush = 0;
        }
    }
    Ok(())
}

/// Download the whole of `url` into `dest` in a single stream, retrying from the
/// start (with a freshly-signed URL) whenever the connection stalls. Used for
/// small files and servers that do not honour range requests.
fn download_single(
    url: &SignedUrl,
    dest: &Path,
    pb: &ProgressBar,
    total: &ProgressBar,
    agent: &ureq::Agent,
) -> Result<()> {
    let mut stalls = 0usize;

    loop {
        let mut written = 0u64;
        let result = (|| -> Result<()> {
            let resp = agent.get(&url.build()).call().context("HTTP request failed")?;
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

/// Download specific files (identified by their published backslash paths, e.g.
/// `mxd\Data\Base\Base.wz`) from the latest full client index, overwriting any
/// existing copies under `target_dir`.
///
/// Used to repair files that could not be patched: the newest build's
/// `client_all_files_list.dat` is fetched, and each requested path is matched
/// by its `raw_path` and downloaded via its obfuscated, signed URL.
///
/// Returns the list of requested paths that could not be found or downloaded.
pub(crate) fn replace_files_from_latest(
    target_dir: &Path,
    rel_paths: &[String],
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<Vec<String>> {
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();

    let challenge = get_challenge_key(&agent).context("failed to obtain challenge code")?;
    println!("scanning for the latest build version...");
    let number = discover_latest_client_number_with(&agent, &challenge)?;

    let list_time = get_current_utc8_time();
    let list_url = build_signed_url(&challenge, list_time, &client_file_list_path(number));
    let contents =
        http_get_text(&agent, &list_url).context("failed to download client file list")?;

    let mut lines = contents.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().context("client file list is empty")?;
    let version = header
        .rsplit('|')
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("missing version in client file list header"))?
        .trim()
        .to_owned();
    let (domain, base_path) = parse_header_location(header)?;

    // Index entries by their published path, normalized to backslashes so it
    // matches the patch manifest's keys.
    let mut by_path: std::collections::HashMap<String, FileEntry> = std::collections::HashMap::new();
    for line in lines {
        let entry = parse_entry(line)?;
        by_path.insert(entry.raw_path.replace('/', "\\"), entry);
    }

    let pb = ProgressBar::hidden();
    let total = ProgressBar::hidden();
    let mut failed = Vec::new();

    for rel in rel_paths {
        let key = rel.replace('/', "\\");
        let Some(entry) = by_path.get(&key) else {
            failed.push(rel.clone());
            continue;
        };
        let local_path = target_dir.join(&entry.file_location).join(&entry.file_name);
        let url = SignedUrl::new(&challenge, &version, &domain, &base_path, entry);
        println!("  repairing {rel} from build {number} ({version})...");
        match download_file(&url, &local_path, entry.file_size, SEGMENTS_PER_FILE, &pb, &total, &agent)
        {
            Ok(()) => {}
            Err(e) => {
                eprintln!("    failed: {e:#}");
                failed.push(rel.clone());
            }
        }
    }

    Ok(failed)
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
        let (info, _) = parse_client_file_list_with_paths(contents).unwrap();
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

    #[test]
    fn builds_client_file_list_path_for_number() {
        assert_eq!(
            client_file_list_path(1020),
            "/v3client/build/5/8848/apppc/1020/client_all_files_list.dat"
        );
    }

    #[test]
    fn parses_client_number_from_header() {
        let contents = "https://mxdver0.jijiagames.com/v3client/build/5/8848/apppc/961|5|0.0.0.9\n\
                        mxd\\bdvid64.dll|8432048|02D3A68F0F7EE2DEFEE6C315DC2F873E\n";
        assert_eq!(parse_client_number_from_header(contents), Some(961));
        assert_eq!(parse_client_number_from_header(""), None);
    }

    #[test]
    fn parses_last_client_version_ini() {
        assert_eq!(
            parse_last_client_version("[CMS]\nlast_client_version = 1023\n"),
            Some(1023)
        );
        // Case-insensitive key, no surrounding spaces.
        assert_eq!(
            parse_last_client_version("LAST_CLIENT_VERSION=1042"),
            Some(1042)
        );
        // Comments and unrelated keys are ignored.
        assert_eq!(
            parse_last_client_version("; a comment\nother = 7\n"),
            None
        );
        assert_eq!(parse_last_client_version(""), None);
    }
}
