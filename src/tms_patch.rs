//! TMS (Taiwan region) incremental patch application.
//!
//! Unlike CMS which uses signed zip-based incremental patches, TMS publishes
//! a single `.patch` file in the WzPatch binary format (also used by KMS,
//! KMST, MSEA). Each patch upgrades the client from one version to another.
//!
//! ## Patch file format
//!
//! The `.patch` file uses the WzPatch container:
//!
//! 1. A header: magic `WzPatch\x1A` (8 bytes), version (i32), checksum (u32)
//! 2. Zlib-compressed patch data
//! 3. An optional footer at the end of the file:
//!    - Last 4 bytes: `0xF2F7FBF3` end marker
//!    - Preceding 8 bytes: patch block length (u32) + notice length (u32)
//!
//! After decompression, the stream contains a sequence of patch parts:
//!
//! - **Create** (type 0): file name, then i32 length + u32 CRC32, then raw data.
//! - **Rebuild** (type 1): file name, then u32 old CRC32 + u32 new CRC32,
//!   then a sequence of rebuild instructions (u32 commands).
//! - **Delete** (type 2): file name only.
//!
//! Each rebuild instruction is a u32 whose top 4 bits encode the operation:
//!
//! | Bits  | Operation      | Meaning                              |
//! |-------|----------------|--------------------------------------|
//! | 0x08  | FromPatcher    | Copy N bytes from the patch stream   |
//! | 0x0C  | FillBytes      | Fill N bytes with a constant byte    |
//! | other | FromOldFile    | Copy N bytes from the old file       |
//!
//! The CRC-32 polynomial is `0x04C11DB7` (same as Ethernet/gzip).

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::ZlibDecoder;
use indicatif::{ProgressBar, ProgressStyle};

use crate::plog;
use crate::locale::tr;

// ── Constants ───────────────────────────────────────────────────────────────

/// Magic bytes at the start of every WzPatch block.
const WZPATCH_MAGIC: &[u8; 8] = b"WzPatch\x1A";

/// End-of-patch-block sentinel (little-endian u32).
const END_MARKER: u32 = 0xF2F7FBF3;

/// Timeout for establishing a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for reading data.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum number of HTTP retries.
const HTTP_RETRIES: usize = 3;

/// Number of parallel byte-range segments per download.
const SEGMENTS_PER_FILE: usize = 5;

/// Files smaller than this are downloaded with a single stream.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// Maximum consecutive stalls tolerated before failing a download.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before retrying a stalled download.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

// ── CRC-32 (delegates to patch_builder) ────────────────────────────────────

/// Compute CRC-32 for a run of identical bytes without allocating.
fn crc32_fill_bytes(mut crc: u32, fill_byte: u8, mut len: usize) -> u32 {
    let mut buf = [0u8; 8192];
    buf.fill(fill_byte);
    while len > 0 {
        let take = len.min(buf.len());
        crc = crate::patch_builder::crc32_update(crc, &buf[..take]);
        len -= take;
    }
    crc
}

// ── Patch container parsing ─────────────────────────────────────────────────

/// A single entry in the patch manifest.
#[derive(Debug, Clone)]
enum PatchPart {
    /// A newly created file (or directory if no extension).
    Create {
        file_name: String,
        file_length: u32,
        checksum: u32,
        /// Offset in the decompressed stream where file data begins.
        data_offset: u64,
    },
    /// A file rebuilt from an old version.
    Rebuild {
        file_name: String,
        old_checksum: u32,
        new_checksum: u32,
        /// Computed size of the rebuilt file in bytes (filled during parsing).
        new_file_length: u32,
        /// Offset in the decompressed stream where instructions begin.
        inst_offset: u64,
    },
    /// A file (or directory) to delete.
    Delete { file_name: String },
}

impl PatchPart {
    /// Net byte change this part contributes (positive for create/rebuild).
    fn byte_delta(&self) -> i64 {
        match self {
            PatchPart::Create { file_length, .. } => *file_length as i64,
            PatchPart::Rebuild { new_file_length, .. } => *new_file_length as i64,
            PatchPart::Delete { .. } => 0,
        }
    }
}

/// Parsed WzPatch file ready for application.
struct WzPatch {
    /// All patch parts in order.
    parts: Vec<PatchPart>,
    /// The decompressed data (kept in memory for random access).
    decompressed: Vec<u8>,
    /// Whether this patch uses KMST1125 format (file hash list at start,
    /// no old_checksum in Rebuild parts, FromOldFile carries source path).
    is_kmst1125: bool,
}

/// Try to locate and extract the WzPatch block from a `.patch` file.
///
/// For TMS, the entire file is typically the patch block with an optional
/// 64-bit footer.
fn read_wzpatch(data: &[u8]) -> Result<WzPatch> {
    let patch_block = extract_patch_block(data)?;

    // Verify header.
    if patch_block.len() < 16 {
        bail!("patch block too small ({})", patch_block.len());
    }
    if &patch_block[..8] != WZPATCH_MAGIC {
        bail!("invalid WzPatch magic");
    }

    let _version = i32::from_le_bytes(patch_block[8..12].try_into().unwrap());
    let _checksum0 = u32::from_le_bytes(patch_block[12..16].try_into().unwrap());

    // The compressed stream always starts at byte 16. Check whether the first
    // two bytes form a zlib header (CMF=0x78, FLG where (CMF*256+FLG)%31==0).
    // If so, use a ZlibDecoder; otherwise use raw DeflateDecoder.
    let body = &patch_block[16..];
    let decompressed = if body.len() >= 2
        && body[0] == 0x78
        && (body[0] as u16 * 256 + body[1] as u16) % 31 == 0
    {
        let mut decoder = ZlibDecoder::new(body);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out)
            .context("failed to decompress (zlib) patch data")?;
        out
    } else {
        let mut decoder = flate2::read::DeflateDecoder::new(body);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out)
            .context("failed to decompress (deflate) patch data")?;
        out
    };

    // Parse patch parts (handles KMST1125 hash list if present).
    let (parts, is_kmst1125, _old_file_hashes) = parse_patch_parts(&decompressed)?;

    Ok(WzPatch { parts, decompressed, is_kmst1125 })
}

/// Extract the WzPatch data block from a raw `.patch` file.
///
/// For non-MZ files (TMS patches), the entire file may be the block, or it
/// may have a 64-bit footer.
fn extract_patch_block(data: &[u8]) -> Result<&[u8]> {
    if data.len() < 16 {
        bail!("patch file too small ({})", data.len());
    }

    // Check for 64-bit footer: last 8 bytes as u32, not u64.
    // Try 32-bit footer first.
    if data.len() >= 12 {
        let end_marker = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap());
        if end_marker == END_MARKER {
            // 32-bit footer.
            let patch_len = u32::from_le_bytes(data[data.len() - 12..data.len() - 8].try_into().unwrap()) as usize;
            let _notice_len = u32::from_le_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap()) as usize;
            let block_end = data.len() - 12;
            if patch_len <= block_end {
                return Ok(&data[block_end - patch_len..block_end]);
            }
        }
    }

    // Try 64-bit footer (first check the last 4 bytes; if they are 0xF2F7FBF3
    // and the following 4 bytes are 0x00000000).
    if data.len() >= 24 {
        let marker_lo = u32::from_le_bytes(data[data.len() - 8..data.len() - 4].try_into().unwrap());
        let marker_hi = u32::from_le_bytes(data[data.len() - 4..].try_into().unwrap());
        if marker_lo == END_MARKER && marker_hi == 0 {
            let patch_len = u64::from_le_bytes(data[data.len() - 24..data.len() - 16].try_into().unwrap()) as usize;
            let _notice_len = u64::from_le_bytes(data[data.len() - 16..data.len() - 8].try_into().unwrap()) as usize;
            let block_end = data.len() - 24;
            if patch_len <= block_end {
                return Ok(&data[block_end - patch_len..block_end]);
            }
        }
    }

    // For TMS patches, if no footer found, treat the entire file as the
    // patch block (after skipping any leading non-WzPatch data).
    // But first, try to find the WzPatch magic.
    if let Some(pos) = data.windows(8).position(|w| w == WZPATCH_MAGIC) {
        return Ok(&data[pos..]);
    }

    // Fallback: entire file.
    Ok(data)
}

/// Parse the sequence of patch parts from the decompressed data.
///
/// Returns `(parts, is_kmst1125, old_file_hashes)`.
///
/// If the data begins with a KMST1125 file-hash list (a positive i32 count
/// followed by that many `{i32 len, ASCII name, u32 checksum}` entries), the
/// list is consumed and `is_kmst1125` is set.  Otherwise the cursor resets
/// and parsing proceeds in classic mode.
fn parse_patch_parts(data: &[u8]) -> Result<(Vec<PatchPart>, bool, HashMap<String, u32>)> {
    let mut cursor = Cursor::new(data);

    // ── Try to read a KMST1125 file-hash list ──────────────────────────
    let (is_kmst1125, old_file_hashes) = try_read_kmst1125_hash_list(&mut cursor, data.len());

    if !is_kmst1125 {
        cursor.set_position(0);
    }

    // ── Parse patch parts ──────────────────────────────────────────────
    let mut parts = Vec::new();

    loop {
        let (name, type_byte) = match read_patch_file_name(&mut cursor) {
            Ok(t) => t,
            Err(_) => break,
        };

        if type_byte < 0 || type_byte > 2 {
            break;
        }

        let part = match type_byte {
            0 => {
                // Create.
                if Path::new(&name).extension().is_none() {
                    // Directory marker �?skip.
                    continue;
                }
                let file_length = read_i32(&mut cursor)? as u32;
                let checksum = read_u32(&mut cursor)?;
                let data_offset = cursor.position();
                cursor.seek(SeekFrom::Current(file_length as i64))
                    .context("failed to skip create file data")?;
                PatchPart::Create { file_name: name, file_length, checksum, data_offset }
            }
            1 => {
                // Rebuild.  In KMST1125 the old_checksum comes from the
                // hash list, not the stream.
                let old_checksum = if is_kmst1125 {
                    old_file_hashes.get(&name).copied().unwrap_or(0)
                } else {
                    read_u32(&mut cursor)?
                };
                let new_checksum = read_u32(&mut cursor)?;
                let inst_offset = cursor.position();
                let new_file_length = skip_rebuild_instructions(&mut cursor, is_kmst1125)?;
                PatchPart::Rebuild { file_name: name, old_checksum, new_checksum, new_file_length, inst_offset }
            }
            2 => {
                PatchPart::Delete { file_name: name }
            }
            _ => break,
        };
        parts.push(part);
    }

    Ok((parts, is_kmst1125, old_file_hashes))
}

/// Try to read a KMST1125 file-hash list from the current cursor position.
///
/// Returns `(true, hashes)` on success, or `(false, empty)` if the data
/// doesn't look like a hash list.
fn try_read_kmst1125_hash_list(
    cursor: &mut Cursor<&[u8]>,
    data_len: usize,
) -> (bool, HashMap<String, u32>) {
    let start = cursor.position() as usize;
    let remaining = data_len.saturating_sub(start);
    if remaining < 4 {
        return (false, HashMap::new());
    }

    let buf = cursor.get_ref();
    let count = i32::from_le_bytes([buf[start], buf[start+1], buf[start+2], buf[start+3]]);
    // Reasonable bounds: 1 .. 500_000
    if count <= 0 || count > 500_000 {
        return (false, HashMap::new());
    }
    cursor.set_position((start + 4) as u64);

    let mut hashes = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let name_len = match read_i32(cursor) {
            Ok(n) if n > 0 && n <= 260 => n as usize,
            _ => {
                cursor.set_position(start as u64);
                return (false, HashMap::new());
            }
        };
        let pos = cursor.position() as usize;
        if pos + name_len + 4 > data_len {
            cursor.set_position(start as u64);
            return (false, HashMap::new());
        }
        let name_bytes = &cursor.get_ref()[pos..pos + name_len];
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        cursor.set_position((pos + name_len) as u64);
        let checksum = match read_u32(cursor) {
            Ok(c) => c,
            Err(_) => {
                cursor.set_position(start as u64);
                return (false, HashMap::new());
            }
        };
        hashes.insert(name, checksum);
    }

    (true, hashes)
}

/// Read a file name followed by a type byte from the patch stream.
///
/// File name bytes are read until a byte �?2 is encountered; that byte is the
/// patch type. Returns `(file_name, type_byte)` or `-1` if EOF.
fn read_patch_file_name<R: Read>(reader: &mut R) -> Result<(String, i32)> {
    let mut name_bytes = Vec::new();
    loop {
        let mut buf = [0u8; 1];
        match reader.read_exact(&mut buf) {
            Ok(()) => {
                if buf[0] <= 2 {
                    let name = String::from_utf8_lossy(&name_bytes).into_owned();
                    return Ok((name, buf[0] as i32));
                }
                name_bytes.push(buf[0]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok((String::from_utf8_lossy(&name_bytes).into_owned(), -1));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn read_i32<R: Read>(reader: &mut R) -> Result<i32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(i32::from_le_bytes(buf))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

/// Read and discard rebuild instructions until the ending marker (0).
/// Returns the total length of the rebuilt file in bytes.
///
/// When `is_kmst1125` is true, FromOldFile instructions carry an extra
/// length-prefixed source file name after the old file position.
fn skip_rebuild_instructions<R: Read + Seek>(reader: &mut R, is_kmst1125: bool) -> Result<u32> {
    let mut total_len = 0u32;
    loop {
        let cmd = read_u32(reader)?;
        if cmd == 0 {
            return Ok(total_len);
        }
        match cmd >> 28 {
            0x08 => {
                let len = cmd & 0x0FFF_FFFF;
                total_len += len;
                reader.seek(SeekFrom::Current(len as i64))?;
            }
            0x0C => {
                let len = (cmd & 0x0FFF_FF00) >> 8;
                total_len += len;
            }
            _ => {
                let len = cmd;
                total_len += len;
                let _old_pos = read_i32(reader)?;
                if is_kmst1125 {
                    // Skip the length-prefixed source file name.
                    let name_len = read_i32(reader)?;
                    if name_len > 0 && name_len <= 260 {
                        reader.seek(SeekFrom::Current(name_len as i64))?;
                    }
                }
            }
        }
    }
}

// ── Patch application ───────────────────────────────────────────────────────

/// Result of [`apply_tms_patches`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOutcome {
    /// One or more patches were applied.
    Updated,
    /// The client was already at the requested version; nothing to do.
    AlreadyUpToDate,
}

/// Apply TMS incremental patches to bring the client under `target_dir` up to
/// `max_version` (a version number like `"281"`, or `"latest"` for the newest
/// published version).
///
/// Downloads each needed `.patch` file, applies it, and repairs any corrupted
/// files from the full client manifest.
pub fn apply_patches(
    target_dir: &Path,
    max_version: &str,
    allow_insecure: bool,
    proxy: Option<&str>,
    purge_wz_files: bool,
) -> Result<PatchOutcome> {
    // Prevent the system from sleeping.
    let _awake = crate::keep_awake::KeepAwake::new();

    // 1. The client directory must exist.
    if !target_dir.is_dir() {
        bail!(
            "target directory '{}' does not exist",
            target_dir.display()
        );
    }

    // 2. Resolve the target version.
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();

    let target_version: i16 = if max_version.eq_ignore_ascii_case("latest") {
        let v = get_latest_version(&agent)?;
        plog!("latest TMS version: {}", v);
        v
    } else {
        max_version.parse::<i16>()
            .map_err(|_| anyhow!("invalid version number '{}'; expected an integer like 281 or 'latest'", max_version))?
    };

    // 3. Get the current client version from Data/Base/Base.wz.
    let current_version = get_current_version(target_dir)?;
    plog!("current client version: {}", current_version);

    // Signal the GUI that we are scanning for an update.
    crate::progress::scanning();

    if current_version == target_version {
        plog!("client is already at version {}; nothing to do.", target_version);
        crate::progress::finish(&tr("gui-patcher-nopatch-successful", &[]), true);
        return Ok(PatchOutcome::AlreadyUpToDate);
    }

    if current_version > target_version {
        plog!(
            "client is at version {}, which is newer than the requested target {}; nothing to patch.",
            current_version, target_version
        );
        crate::progress::finish(&tr("gui-patcher-nopatch-successful", &[]), true);
        return Ok(PatchOutcome::AlreadyUpToDate);
    }

    // 4. Purge junk directories if requested.
    if purge_wz_files {
        crate::progress::dl_purging();
        purge_junk_dirs(target_dir)?;
    }

    // 5. Download and apply patches iteratively.
    let patchdata = target_dir.join("patchdata");
    std::fs::create_dir_all(&patchdata)
        .with_context(|| format!("failed to create {}", patchdata.display()))?;

    let mut current = current_version;
    let mut all_corrupted: Vec<String> = Vec::new();
    // Track how many patches have been downloaded so far (for the global
    // "part X of ?" counter across multiple patch files).
    let patch_index = AtomicUsize::new(0);

    loop {
        // Try to find a patch from `current` to `target_version`, falling back
        // to progressively closer versions.
        let mut patch_found = false;
        for target in (current + 1..=target_version).rev() {
            let patch_url = build_patch_url(current, target);
            let zip_name = format!("{:05}to{:05}.patch", current, target);
            let dest = patchdata.join(&zip_name);

            plog!("trying patch: {} -> {} ({})", current, target, patch_url);

            // Probe the URL to get the file size.
            let size = match probe_file_size(&agent, &patch_url) {
                Ok(s) if s > 0 => s,
                Ok(_) => {
                    plog!("  server returned empty file; skipping.");
                    continue;
                }
                Err(_) => {
                    continue;
                }
            };

            // Report to the GUI that we are installing this update.
            let cur_str = current.to_string();
            let tgt_str = target.to_string();
            crate::progress::installing(&cur_str, &tgt_str);
            crate::progress::begin_download(1, 1, size);

            plog!("  [{}/?] downloading {} ({:.2} MiB)...",
                patch_index.load(Ordering::Relaxed) + 1,
                zip_name,
                size as f64 / (1024.0 * 1024.0));

            if let Err(e) = download_patch_to_file(&agent, &patch_url, &dest, size) {
                plog!("  failed to download {}: {:#}", zip_name, e);
                let _ = std::fs::remove_file(&dest);
                let _ = std::fs::remove_file(crate::resume::progress_path(&dest));
                continue;
            }
            patch_index.fetch_add(1, Ordering::Relaxed);

            plog!("  applying {}...", zip_name);
            let patch_data = std::fs::read(&dest)
                .with_context(|| format!("failed to read downloaded patch {}", dest.display()))?;
            let _ = std::fs::remove_file(&dest);

            // Apply the patch with progress reporting.
            crate::progress::begin_apply(0); // total parts determined inside apply_patch_data
            let corrupted = apply_patch_data(&patch_data, target_dir)
                .with_context(|| format!("failed to apply patch {} -> {}", current, target))?;

            if !corrupted.is_empty() {
                plog!("{} file(s) corrupted in patch {} -> {}:",
                    corrupted.len(), current, target);
                for f in &corrupted {
                    plog!("  {}", f);
                }
                all_corrupted = corrupted;
            } else {
                all_corrupted.clear();
            }

            current = target;
            patch_found = true;
            break;
        }

        if !patch_found {
            plog!("no patch found from version {}", current);
            break;
        }

        if current >= target_version {
            break;
        }
    }

    // 6. Repair corrupted files from the full client manifest.
    if !all_corrupted.is_empty() {
        plog!("\nrepairing {} corrupted file(s) from the full client...",
            all_corrupted.len());
        crate::progress::begin_repair(all_corrupted.len(), 0);
        let still_failed = repair_corrupted_files(
            target_dir, &all_corrupted, allow_insecure, proxy,
        )?;
        if !still_failed.is_empty() {
            plog!("{} file(s) still could not be repaired:", still_failed.len());
            for f in &still_failed {
                plog!("  {}", f);
            }
            bail!(
                "patching completed with {} unrepaired file(s)",
                still_failed.len()
            );
        }
        plog!("all corrupted files were repaired.");
    }

    plog!("patching successful: now at version {}.", current);
    crate::progress::finish(&tr("gui-patcher-patch-successful", &[]), true);
    Ok(PatchOutcome::Updated)
}

/// Apply a pre-downloaded `.patch` file directly to `target_dir`.
///
/// No version detection or download is performed — the file is read from disk
/// and applied immediately. Corrupted files are reported but not repaired
/// (repair requires network access; re-run with `--patch latest` instead).
pub fn apply_patch_file(
    target_dir: &Path,
    patch_path: &Path,
    purge_wz_files: bool,
) -> Result<PatchOutcome> {
    let _awake = crate::keep_awake::KeepAwake::new();

    if !target_dir.is_dir() {
        bail!("target directory '{}' does not exist", target_dir.display());
    }
    if !patch_path.exists() {
        bail!("patch file '{}' not found", patch_path.display());
    }

    if purge_wz_files {
        crate::progress::dl_purging();
        purge_junk_dirs(target_dir)?;
    }

    let patch_data = std::fs::read(patch_path)
        .with_context(|| format!("failed to read {}", patch_path.display()))?;

    plog!("applying local patch '{}' ({:.2} MiB)...",
        patch_path.display(),
        patch_data.len() as f64 / (1024.0 * 1024.0));

    let corrupted = apply_patch_data(&patch_data, target_dir)
        .context("failed to apply patch")?;

    if corrupted.is_empty() {
        plog!("patching successful.");
        Ok(PatchOutcome::Updated)
    } else {
        plog!("\n{} file(s) could not be patched:", corrupted.len());
        for f in &corrupted {
            plog!("  {}", f);
        }
        plog!("\nre-run with `cmsdl tms --patch latest <dir>` to download and repair corrupted files.");
        bail!(
            "patching completed with {} corrupted file(s)",
            corrupted.len()
        );
    }
}

/// Get the latest version number by downloading Base.wz from the TMS product
/// manifest and reading its version with miniwzlib.
fn get_latest_version(agent: &ureq::Agent) -> Result<i16> {
    let info = crate::tms::get_product_info(agent)
        .context("failed to fetch TMS product manifest")?;

    // Find Base.wz in the file list.
    let base_wz = info.files.iter().find(|f| {
        let p = f.path.replace('\\', "/").to_ascii_lowercase();
        p == "data/base/base.wz"
    }).ok_or_else(|| anyhow!("Base.wz not found in TMS product manifest"))?;

    // Build the download URL.
    let base_path = info.execution_path.rfind('/')
        .map(|i| &info.execution_path[..i])
        .unwrap_or("");
    let url = if base_path.is_empty() {
        format!("{}/{}", info.base_url.trim_end_matches('/'), base_wz.path)
    } else {
        format!("{}/{}/{}", info.base_url.trim_end_matches('/'), base_path, base_wz.path)
    };

    plog!("downloading Base.wz from {}...", url);

    let resp = agent.get(&url).call()
        .context("failed to download Base.wz")?;
    let mut reader = resp.into_reader();
    let mut data = Vec::new();
    reader.read_to_end(&mut data)
        .context("failed to read Base.wz")?;

    // Read version from the in-memory data.
    let wz = miniwzlib_from_bytes(&data)
        .context("failed to read version from Base.wz")?;
    Ok(wz.version)
}

/// Read WZ version from in-memory bytes using the miniwzlib from-bytes API.
fn miniwzlib_from_bytes(data: &[u8]) -> Result<crate::miniwzlib::WzVersion> {
    crate::miniwzlib::get_wz_version_from_bytes(data, data.len() as u64)
        .map_err(|e| anyhow!("{}", e))
}

/// Get the current client version from `target_dir/Data/Base/Base.wz`.
fn get_current_version(target_dir: &Path) -> Result<i16> {
    let wz_path = target_dir.join("Data").join("Base").join("Base.wz");
    if !wz_path.exists() {
        bail!(
            "Base.wz not found at '{}'; not a valid TMS client directory",
            wz_path.display()
        );
    }
    let wz = crate::miniwzlib::get_wz_version(&wz_path)
        .map_err(|e| anyhow!("failed to read version from {}: {}", wz_path.display(), e))?;
    if wz.version == 0 {
        bail!(
            "could not determine version from '{}' (unsupported WZ format?)",
            wz_path.display()
        );
    }
    Ok(wz.version)
}

/// Build a patch download URL from old and new version numbers.
fn build_patch_url(old_ver: i16, new_ver: i16) -> String {
    format!(
        "http://tw.cdnpatch.maplestory.beanfun.com/maplestory/patch/patchdir/{:05}/{:05}to{:05}.patch",
        new_ver, old_ver, new_ver
    )
}

// ── Multi-segment patch download with progress & resume ─────────────────────

/// Probe a URL with a HEAD request to get Content-Length.
/// Returns 0 if the server doesn't report a size or the file is absent.
fn probe_file_size(agent: &ureq::Agent, url: &str) -> Result<u64> {
    let resp = agent.head(url).call()?;
    if resp.status() == 404 {
        return Err(anyhow!("patch not found (404)"));
    }
    // Some servers return Content-Length on HEAD; fall back to GET Range probe.
    if let Some(len) = resp.header("Content-Length") {
        if let Ok(n) = len.parse::<u64>() {
            if n > 0 {
                return Ok(n);
            }
        }
    }
    // Fallback: GET with Range 0-0 to read Content-Range or Content-Length.
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(r) => {
            if r.status() == 404 {
                return Err(anyhow!("patch not found (404)"));
            }
            if let Some(cr) = r.header("Content-Range") {
                // "bytes 0-0/12345"
                if let Some(total) = cr.split('/').nth(1) {
                    if let Ok(n) = total.parse::<u64>() {
                        return Ok(n);
                    }
                }
            }
            if let Some(cl) = r.header("Content-Length") {
                if let Ok(n) = cl.parse::<u64>() {
                    return Ok(n);
                }
            }
            Ok(0)
        }
        Err(ureq::Error::Status(404, _)) => Err(anyhow!("patch not found (404)")),
        Err(e) => Err(e.into()),
    }
}

/// Download a patch file to `dest` with a progress bar, up to 5 parallel
/// byte-range segments, and resume support via a `<dest>.cmsdl` sidecar.
fn download_patch_to_file(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    size: u64,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Early skip: file already fully downloaded? Only trust this when no
    // .cmsdl sidecar exists (a sidecar means the download was interrupted).
    if dest.exists() && !crate::resume::progress_path(dest).exists() {
        if let Ok(meta) = dest.metadata() {
            if meta.len() == size {
                plog!("    {} already present (skipping download).",
                    dest.file_name().unwrap_or_default().to_string_lossy());
                crate::progress::download_progress(size);
                return Ok(());
            }
        }
    }

    let pb = if crate::progress::active() {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(size)
    };
    pb.set_style(
        ProgressStyle::with_template(
            "    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    let dl_progress = Arc::new(AtomicUsize::new(0));

    let segments = effective_segments(size, SEGMENTS_PER_FILE);

    if segments <= 1 || size == 0 || !supports_ranges(agent, url) {
        // Single resumable stream.
        let _ = std::fs::remove_file(crate::resume::progress_path(dest));
        download_single_stream(agent, url, dest, size, &pb, Some(&dl_progress))?;
    } else {
        // Multi-segment: resume from saved progress or start fresh.
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
            pb.inc(pre_completed);
            dl_progress.store(pre_completed as usize, Ordering::Relaxed);
            crate::progress::download_progress(pre_completed);
            progress = crate::resume::FileProgress::from_saved(dest, &saved_segs, &resume_ranges)
                .with_context(|| {
                    format!("failed to write progress file {}", progress_path.display())
                })?;
            ranges = resume_ranges;
        } else {
            // Fresh download: pre-allocate the file.
            {
                let file = std::fs::File::create(dest)
                    .with_context(|| format!("failed to create {}", dest.display()))?;
                file.set_len(size)
                    .with_context(|| format!("failed to size {}", dest.display()))?;
            }
            let fresh_ranges = compute_ranges(size, segments);
            progress = crate::resume::FileProgress::new(dest, &fresh_ranges).with_context(
                || format!("failed to create progress file {}", progress_path.display()),
            )?;
            ranges = fresh_ranges;
        }

        let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

        std::thread::scope(|scope| {
            let progress = &progress;
            let handles: Vec<_> = ranges
                .iter()
                .enumerate()
                .map(|(slot, &(start, end))| {
                    let pb = &pb;
                    let first_err = &first_err;
                    let dl_progress = &dl_progress;
                    scope.spawn(move || {
                        if let Err(e) = download_segment(agent, url, dest, start, end, pb, progress, slot, Some(dl_progress)) {
                            let mut s = first_err.lock().unwrap();
                            if s.is_none() {
                                *s = Some(e);
                            }
                        }
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        });

        if let Some(e) = first_err.into_inner().unwrap() {
            pb.finish_and_clear();
            return Err(e);
        }

        progress.delete();
    }

    pb.finish_and_clear();
    crate::progress::download_progress(size);
    Ok(())
}

/// Download as a single resumable stream (for small files or servers without
/// range support).
fn download_single_stream(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    size: u64,
    pb: &ProgressBar,
    dl_progress: Option<&AtomicUsize>,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    let mut pos = 0u64;
    let mut stalls = 0usize;

    while size == 0 || pos < size {
        let before = pos;
        let _ = stream_range(agent, url, &mut file, &mut pos, pb, dl_progress);

        if size != 0 && pos >= size {
            break;
        }
        if size == 0 && pos > 0 {
            break;
        }

        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                bail!("download stalled with no progress after {MAX_STALL_RETRIES} retries");
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    file.flush().ok();
    Ok(())
}

/// Download a single byte range `[start, end]` into `dest`, resuming from the
/// current offset on each stall.
fn download_segment(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    progress: &crate::resume::FileProgress,
    slot: usize,
    dl_progress: Option<&AtomicUsize>,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;

    let mut pos = start;
    let mut stalls = 0usize;

    while pos <= end {
        let before = pos;
        let _ = stream_bounded(agent, url, &mut file, &mut pos, end, pb, progress, slot, dl_progress);

        progress.update(slot, pos);

        if pos > end {
            return Ok(());
        }
        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                bail!("download segment stalled after {MAX_STALL_RETRIES} retries");
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    Ok(())
}

/// Probe whether the server honours HTTP range requests.
fn supports_ranges(agent: &ureq::Agent, url: &str) -> bool {
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => resp.status() == 206,
        Err(_) => false,
    }
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

/// Decide how many segments to use for a file of the given size.
fn effective_segments(size: u64, max_segments: usize) -> usize {
    if max_segments <= 1 || size == 0 {
        return 1;
    }
    let by_size = (size / MIN_SEGMENT_SIZE).max(1) as usize;
    by_size.min(max_segments).max(1)
}

/// Stream a range request starting at `*pos` (open-ended) into `file`.
fn stream_range(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    pb: &ProgressBar,
    dl_progress: Option<&AtomicUsize>,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-", *pos))
        .call()
        .context("HTTP range request failed")?;
    let status = resp.status();
    let mut reader = resp.into_reader();

    if status == 200 && *pos != 0 {
        *pos = 0;
        pb.set_position(0);
    }
    file.seek(SeekFrom::Start(*pos))
        .context("failed to seek before resuming")?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("failed to write to disk")?;
        *pos += n as u64;
        pb.set_position(*pos);
        if let Some(dp) = dl_progress {
            let total = dp.fetch_add(n, Ordering::Relaxed) + n;
            crate::progress::download_progress(total as u64);
        }
    }
    Ok(())
}

/// Stream a bounded range request from `*pos` to `end` (inclusive) into `file`.
fn stream_bounded(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    end: u64,
    pb: &ProgressBar,
    progress: &crate::resume::FileProgress,
    slot: usize,
    dl_progress: Option<&AtomicUsize>,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-{}", *pos, end))
        .call()
        .context("HTTP range request failed")?;
    let mut reader = resp.into_reader();

    file.seek(SeekFrom::Start(*pos))
        .context("failed to seek before resuming")?;

    let mut buf = [0u8; 64 * 1024];
    let mut since_flush: u64 = 0;
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        let remaining = (end + 1).saturating_sub(*pos) as usize;
        if remaining == 0 {
            break;
        }
        let take = n.min(remaining);
        file.write_all(&buf[..take]).context("failed to write to disk")?;
        *pos += take as u64;
        since_flush += take as u64;
        pb.inc(take as u64);
        if let Some(dp) = dl_progress {
            let total = dp.fetch_add(take, Ordering::Relaxed) + take;
            crate::progress::download_progress(total as u64);
        }
        if since_flush >= crate::resume::PROGRESS_FLUSH_INTERVAL {
            progress.update(slot, *pos);
            since_flush = 0;
        }
        if take < n {
            break;
        }
    }
    Ok(())
}

// ── DeadPatch: pre-patch validation & execution plan ────────────────────────

/// Result of validating an old file before patching.
enum PreValidate {
    /// Old file exists with correct CRC; ready to patch.
    Ok,
    /// File already matches the new checksum; no patching needed.
    AlreadyUpToDate,
}

/// Produce a summary of the patch execution plan.
///
/// Returns `(create_count, rebuild_count, delete_count, total_bytes_needed)`.
fn pre_patch_report(patch: &WzPatch) -> (usize, usize, usize, u64) {
    let mut create = 0usize;
    let mut rebuild = 0usize;
    let mut delete = 0usize;
    let mut bytes: i64 = 0;

    for part in &patch.parts {
        match part {
            PatchPart::Create { .. } => {
                create += 1;
                bytes += part.byte_delta();
            }
            PatchPart::Rebuild { .. } => {
                rebuild += 1;
                bytes += part.byte_delta();
            }
            PatchPart::Delete { .. } => {
                delete += 1;
            }
        }
    }

    (create, rebuild, delete, bytes.max(0) as u64)
}

/// Validate that an old file exists with the expected CRC-32.
///
/// Returns:
/// - `Ok(PreValidate::Ok)` if the file exists and matches `old_checksum`.
/// - `Ok(PreValidate::AlreadyUpToDate)` if the file already matches `new_checksum`.
/// - `Err(...)` if the file is missing or has an unexpected checksum.
fn validate_old_file(
    target_dir: &Path,
    file_name: &str,
    old_checksum: u32,
    new_checksum: u32,
) -> Result<PreValidate> {
    let old_path = target_dir.join(sanitize_path(file_name));
    if !old_path.exists() {
        bail!("old file not found");
    }
    let old_data = std::fs::read(&old_path)
        .with_context(|| format!("failed to read {}", old_path.display()))?;
    let actual_crc = crate::patch_builder::crc32_update(0, &old_data);

    if actual_crc == new_checksum {
        return Ok(PreValidate::AlreadyUpToDate);
    }
    if actual_crc != old_checksum {
        bail!(
            "CRC-32 mismatch: expected {:08X}, got {:08X}",
            old_checksum, actual_crc
        );
    }
    Ok(PreValidate::Ok)
}

/// Scan rebuild instructions to find which source files this part depends on
/// (only meaningful for KMST1125 format).  Returns an empty set on error or
/// for non-KMST1125 parts.
fn collect_source_deps(
    decompressed: &[u8],
    inst_offset: u64,
    is_kmst1125: bool,
) -> Result<HashSet<String>> {
    let mut deps = HashSet::new();
    if !is_kmst1125 {
        return Ok(deps);
    }
    let mut cursor = Cursor::new(&decompressed[inst_offset as usize..]);
    loop {
        let cmd = read_u32(&mut cursor)?;
        if cmd == 0 {
            break;
        }
        match cmd >> 28 {
            0x08 => {
                let len = (cmd & 0x0FFF_FFFF) as i64;
                cursor.seek(SeekFrom::Current(len))?;
            }
            0x0C => { /* FillBytes – no source dependency */ }
            _ => {
                // FromOldFile – read old_offset, then source file name.
                let _old_offset = read_i32(&mut cursor)?;
                let name_len = read_i32(&mut cursor)?;
                if name_len > 0 && name_len <= 260 {
                    let pos = inst_offset as usize + cursor.position() as usize;
                    if pos + name_len as usize <= decompressed.len() {
                        let name_bytes = &decompressed[pos..pos + name_len as usize];
                        let name = String::from_utf8_lossy(name_bytes)
                            .replace('\\', "/");
                        deps.insert(name);
                    }
                    cursor.seek(SeekFrom::Current(name_len as i64))?;
                }
            }
        }
    }
    Ok(deps)
}

/// Apply patch data (the raw bytes of a `.patch` file) to `target_dir`.
/// Returns the list of corrupted file paths.
///
/// DeadPatch is enabled by default: a pre-patch validation phase checks every
/// old file's CRC-32 and reports the execution plan (files, sizes, disk space)
/// before writing a single byte to the target directory.
fn apply_patch_data(patch_data: &[u8], target_dir: &Path) -> Result<Vec<String>> {
    let patch = read_wzpatch(patch_data)?;

    // ── DeadPatch: pre-patch validation & execution plan ─────────────────
    let (create_count, rebuild_count, delete_count, total_bytes) = pre_patch_report(&patch);
    plog!("  patch plan: {} create, {} rebuild, {} delete ({} total)",
        create_count, rebuild_count, delete_count,
        crate::progress::format_size(total_bytes));

    // Validate old files for every Rebuild part before touching anything.
    let mut pre_failures: Vec<String> = Vec::new();
    for part in &patch.parts {
        if let PatchPart::Rebuild { file_name, old_checksum, new_checksum, .. } = part {
            match validate_old_file(target_dir, file_name, *old_checksum, *new_checksum) {
                Ok(PreValidate::Ok) => {
                    plog!("    ok {}", file_name);
                }
                Ok(PreValidate::AlreadyUpToDate) => {
                    plog!("    skip {} (already up to date)", file_name);
                }
                Err(e) => {
                    plog!("    pre-validate fail: {} - {}", file_name, e);
                    pre_failures.push(file_name.clone());
                }
            }
        }
    }
    if !pre_failures.is_empty() {
        plog!("  {} file(s) failed pre-patch validation", pre_failures.len());
    }
    // ─────────────────────────────────────────────────────────────────────

    let mut corrupted: Vec<String> = Vec::new();
    let temp_dir = create_temp_dir(target_dir)?;

    // ── Collect source dependencies for each Rebuild part ───────────────
    // For KMST1125 patches, FromOldFile instructions can reference different
    // source files.  We scan instructions upfront (without building) so we
    // know which parts depend on which source files.
    let deps: Vec<HashSet<String>> = patch.parts.iter().map(|part| {
        match part {
            PatchPart::Rebuild { inst_offset, .. } => {
                collect_source_deps(&patch.decompressed, *inst_offset, patch.is_kmst1125)
                    .unwrap_or_default()
            }
            _ => HashSet::new(),
        }
    }).collect();

    // Build reverse index: source file → indices of parts that need it.
    let mut needed_by: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, part) in patch.parts.iter().enumerate() {
        let file_name = match part {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => continue,
        };
        // Normalise to forward slashes for matching.
        let key = file_name.replace('\\', "/");
        for dep in &deps[i] {
            needed_by.entry(dep.clone()).or_default().push(i);
        }
        // Also register the part itself so we can track remaining dependents.
        needed_by.entry(key).or_default();
    }

    // Track which part indices are still "in-flight" (built to temp but not
    // yet applied because later parts may reference their file as a source).
    let mut pending: Vec<usize> = Vec::new();
    // Remaining dependent count for each part (how many later parts still
    // reference this part's file as a source).
    let remaining_deps: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());

    // Helper: check if a file is a Base file (should be applied last).
    let is_base_file = |name: &str| -> bool {
        let n = name.replace('\\', "/").to_ascii_lowercase();
        n.starts_with("data/base/")
    };

    // Helper: apply a pending part's temp file to the target if the temp
    // file exists and the part is not corrupted.  Returns true on success.
    let apply_pending = |idx: usize,
                         parts: &[PatchPart],
                         corrupted: &mut Vec<String>,
                         temp_dir: &Path,
                         target_dir: &Path| {
        let part = &parts[idx];
        let file_name = match part {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => return,
        };
        if corrupted.contains(file_name) {
            return;
        }
        let temp_path = temp_dir.join(sanitize_path(file_name));
        let target_path = target_dir.join(sanitize_path(file_name));
        if temp_path.exists() {
            if let Err(e) = replace_file(&temp_path, &target_path) {
                plog!("  warning: failed to apply {}: {}", file_name, e);
                corrupted.push(file_name.clone());
            }
        }
    };

    // Phase 1: Build each part to temp, applying completed parts immediately
    // when no remaining dependents need their source file.
    let total_apply = patch.parts.iter().filter(|p| !matches!(p, PatchPart::Delete { .. })).count();
    let apply_done = AtomicUsize::new(0);
    for (i, part) in patch.parts.iter().enumerate() {
        let file_name = match part {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => continue,
        };

        let result = match part {
            PatchPart::Create { file_name, file_length, checksum, data_offset } => {
                apply_create(&patch.decompressed, file_name, *data_offset, *file_length, *checksum, &temp_dir, target_dir)
            }
            PatchPart::Rebuild { file_name, old_checksum, new_checksum, inst_offset, .. } => {
                apply_rebuild(&patch.decompressed, file_name, *inst_offset, *old_checksum, *new_checksum, &temp_dir, target_dir, patch.is_kmst1125)
            }
            PatchPart::Delete { .. } => unreachable!(),
        };

        let done = apply_done.fetch_add(1, Ordering::Relaxed) + 1;
        crate::progress::apply_progress(done, total_apply, file_name);

        if let Err(e) = result {
            plog!("    apply fail: {} - {}", file_name, e);
            corrupted.push(file_name.clone());
            continue;
        }

        // This part is now built (temp file exists).  Decrement the
        // dependent count for each source file it references.
        let key = file_name.replace('\\', "/");
        {
            let mut rd = remaining_deps.lock().unwrap();
            for dep in &deps[i] {
                *rd.entry(dep.clone()).or_insert(0) = rd.get(dep).copied().unwrap_or(0).saturating_sub(1);
            }
            // Initialise this part's own dependent count.
            if let Some(needers) = needed_by.get(&key) {
                rd.entry(key.clone()).or_insert(needers.len());
            }
        }

        // Add to pending.
        pending.push(i);

        // Now flush: apply any pending parts whose source files are no
        // longer needed by any remaining parts.  Skip Base files — they
        // are always deferred to the end.
        let mut flushed = true;
        while flushed {
            flushed = false;
            let mut new_pending = Vec::new();
            for &idx in &pending {
                let pname = match &patch.parts[idx] {
                    PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
                    PatchPart::Delete { .. } => continue,
                };
                let pkey = pname.replace('\\', "/");
                let remaining = {
                    remaining_deps.lock().unwrap().get(&pkey).copied().unwrap_or(0)
                };
                if remaining == 0 && !is_base_file(pname) {
                    apply_pending(idx, &patch.parts, &mut corrupted, &temp_dir, target_dir);
                    flushed = true;
                } else {
                    new_pending.push(idx);
                }
            }
            pending = new_pending;
        }
    }

    // Phase 2: Apply deletions.
    for part in &patch.parts {
        if let PatchPart::Delete { file_name } = part {
            let target_path = target_dir.join(sanitize_path(file_name));
            if target_path.exists() {
                if target_path.is_dir() {
                    let _ = std::fs::remove_dir_all(&target_path);
                } else {
                    let _ = remove_readonly_file(&target_path);
                }
            }
        }
    }

    // Phase 3: Apply remaining pending files (Base files last, then others).
    pending.sort_by(|a, b| {
        let a_name = match &patch.parts[*a] {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => "",
        };
        let b_name = match &patch.parts[*b] {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => "",
        };
        let a_base = is_base_file(a_name);
        let b_base = is_base_file(b_name);
        a_base.cmp(&b_base) // Base files sort last
    });
    for &idx in &pending {
        apply_pending(idx, &patch.parts, &mut corrupted, &temp_dir, target_dir);
    }

    // Clean up temp directory.
    let _ = std::fs::remove_dir_all(&temp_dir);

    corrupted.sort();
    corrupted.dedup();
    Ok(corrupted)
}

/// Create a temporary directory for patch work files.
fn create_temp_dir(target_dir: &Path) -> Result<PathBuf> {
    let patchdata = target_dir.join("patchdata");
    std::fs::create_dir_all(&patchdata)
        .with_context(|| format!("failed to create {}", patchdata.display()))?;
    // Use a random name to avoid collisions.
    let dir = patchdata.join(format!("tmp_{:x}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Sanitize a file path from the patch manifest (backslash �?forward slash,
/// remove leading separators).
fn sanitize_path(path: &str) -> PathBuf {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/').trim_start_matches('\\');
    PathBuf::from(trimmed)
}

/// Apply a "create" patch part: extract raw file data and verify checksum.
fn apply_create(
    decompressed: &[u8],
    file_name: &str,
    data_offset: u64,
    file_length: u32,
    expected_crc: u32,
    temp_dir: &Path,
    _target_dir: &Path,
) -> Result<()> {
    let offset = data_offset as usize;
    let length = file_length as usize;
    if offset + length > decompressed.len() {
        bail!("create data for '{}' extends past decompressed data", file_name);
    }

    let data = &decompressed[offset..offset + length];

    // Verify CRC-32.
    let actual_crc = crate::patch_builder::crc32_update(0, data);
    if actual_crc != expected_crc {
        bail!(
            "CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
            file_name, expected_crc, actual_crc
        );
    }

    let temp_path = temp_dir.join(sanitize_path(file_name));
    if let Some(parent) = temp_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
    }
    std::fs::write(&temp_path, data)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;

    Ok(())
}

/// Apply a "rebuild" patch part: follow rebuild instructions to construct the
/// new file from the old file and patch data.
fn apply_rebuild(
    decompressed: &[u8],
    file_name: &str,
    inst_offset: u64,
    old_checksum: u32,
    new_checksum: u32,
    temp_dir: &Path,
    target_dir: &Path,
    is_kmst1125: bool,
) -> Result<()> {
    let old_path = target_dir.join(sanitize_path(file_name));

    // Check if the old file exists and verify its checksum.
    if !old_path.exists() {
        bail!("old file '{}' not found", file_name);
    }

    let old_data = std::fs::read(&old_path)
        .with_context(|| format!("failed to read old file {}", old_path.display()))?;
    let actual_old_crc = crate::patch_builder::crc32_update(0, &old_data);
    if actual_old_crc != old_checksum {
        // Check if the file already matches the new checksum.
        if actual_old_crc == new_checksum {
            let temp_path = temp_dir.join(sanitize_path(file_name));
            if let Some(parent) = temp_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
            }
            std::fs::write(&temp_path, &old_data)
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
            return Ok(());
        }
        bail!(
            "old CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
            file_name, old_checksum, actual_old_crc
        );
    }

    // Cache of opened source files (KMST1125 can reference multiple).
    let mut file_cache: HashMap<String, Vec<u8>> = HashMap::new();
    file_cache.insert(file_name.to_string(), old_data);

    // Parse instructions and compute the new file.
    let mut cursor = Cursor::new(&decompressed[inst_offset as usize..]);
    let mut new_data = Vec::new();
    let mut new_crc = 0u32;

    loop {
        let cmd = read_u32(&mut cursor)?;
        if cmd == 0 {
            break;
        }
        match cmd >> 28 {
            0x08 => {
                let len = (cmd & 0x0FFF_FFFF) as usize;
                let pos = inst_offset as usize + cursor.position() as usize;
                if pos + len > decompressed.len() {
                    bail!("patch data extends past end of decompressed stream for '{}'", file_name);
                }
                let chunk = &decompressed[pos..pos + len];
                new_crc = crate::patch_builder::crc32_update(new_crc, chunk);
                new_data.extend_from_slice(chunk);
                cursor.seek(SeekFrom::Current(len as i64))?;
            }
            0x0C => {
                let len = ((cmd & 0x0FFF_FF00) >> 8) as usize;
                let fill_byte = (cmd & 0xFF) as u8;
                new_data.resize(new_data.len() + len, fill_byte);
                new_crc = crc32_fill_bytes(new_crc, fill_byte, len);
            }
            _ => {
                let len = cmd as usize;
                let old_offset = read_i32(&mut cursor)?;
                if old_offset < 0 {
                    bail!("negative old file offset for '{}'", file_name);
                }

                // KMST1125: read the source file name for this chunk.
                let source_file = if is_kmst1125 {
                    let name_len = read_i32(&mut cursor)?;
                    if name_len <= 0 || name_len > 260 {
                        bail!("invalid source file name length {} for '{}'", name_len, file_name);
                    }
                    let pos = inst_offset as usize + cursor.position() as usize;
                    if pos + name_len as usize > decompressed.len() {
                        bail!("source file name extends past stream for '{}'", file_name);
                    }
                    let name_bytes = &decompressed[pos..pos + name_len as usize];
                    let name = String::from_utf8_lossy(name_bytes).into_owned();
                    cursor.seek(SeekFrom::Current(name_len as i64))?;
                    name
                } else {
                    file_name.to_string()
                };

                // Get or open the source file data.
                let source_data = if let Some(data) = file_cache.get(&source_file) {
                    data
                } else {
                    let src_path = target_dir.join(sanitize_path(&source_file));
                    let data = std::fs::read(&src_path)
                        .with_context(|| format!("failed to read source file {}", src_path.display()))?;
                    file_cache.insert(source_file.clone(), data);
                    file_cache.get(&source_file).unwrap()
                };

                let old_start = old_offset as usize;
                if old_start + len > source_data.len() {
                    bail!(
                        "source file reference out of bounds for '{}' (source='{}'): offset {}, len {}, size {}",
                        file_name, source_file, old_offset, len, source_data.len()
                    );
                }
                let chunk = &source_data[old_start..old_start + len];
                new_crc = crate::patch_builder::crc32_update(new_crc, chunk);
                new_data.extend_from_slice(chunk);
            }
        }
    }

    // Verify new CRC-32.
    if new_crc != new_checksum {
        bail!(
            "new CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
            file_name, new_checksum, new_crc
        );
    }

    // Write the new file to temp.
    let temp_path = temp_dir.join(sanitize_path(file_name));
    if let Some(parent) = temp_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
    }
    std::fs::write(&temp_path, &new_data)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;

    Ok(())
}

/// Move `from` to `to`, falling back to copy+delete across filesystems.
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    // Remove destination if it exists (may be read-only).
    if to.exists() {
        remove_readonly_file(to);
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .with_context(|| format!("failed to copy into {}", to.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// Remove a file, clearing the read-only attribute first if needed.
fn remove_readonly_file(path: &Path) {
    // On Windows, clear the read-only attribute before removal.
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            use std::os::windows::fs::MetadataExt;
            let attrs = meta.file_attributes();
            const FILE_ATTRIBUTE_READONLY: u32 = 0x00000001;
            if attrs & FILE_ATTRIBUTE_READONLY != 0 {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    let _ = std::fs::remove_file(path);
}

// ── Junk directory purging ──────────────────────────────────────────────────

/// Delete directories whose names are 8.3 format (at most 8 characters + dot +
/// 3-character extension) or end with `.$$$`.
///
/// This is the TMS equivalent of `--purge-wz-files`. Aborts with an error if
/// any directory cannot be deleted (which typically means the process is not
/// running with administrator privileges).
pub fn purge_junk_dirs(target_dir: &Path) -> Result<()> {
    if !target_dir.is_dir() {
        return Ok(());
    }

    let mut deleted = 0usize;
    let mut failed = Vec::new();

    for entry in std::fs::read_dir(target_dir)
        .with_context(|| format!("failed to read {}", target_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if is_junk_dir_name(&name_str) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {
                    plog!("  deleted junk directory: {}", path.display());
                    deleted += 1;
                }
                Err(e) => {
                    plog!("  failed to delete {}: {}", path.display(), e);
                    failed.push(path);
                }
            }
        }
    }

    if !failed.is_empty() {
        bail!(
            "failed to delete {} junk director(ies); \
             try running as administrator",
            failed.len()
        );
    }

    if deleted > 0 {
        plog!("purged {} junk director(ies) from '{}'.", deleted, target_dir.display());
    }
    Ok(())
}

/// Check whether a directory name matches the junk patterns:
/// - Ends with `.$$$`
/// - 8.3 format: `XXXXXXXX.XXX` (name �?8 chars, extension �?3 chars)
fn is_junk_dir_name(name: &str) -> bool {
    if name.to_ascii_lowercase().ends_with(".$$$") {
        return true;
    }
    // 8.3 format: check if name looks like `name.ext` with name �?8 and ext �?3.
    if let Some(dot_pos) = name.rfind('.') {
        let base = &name[..dot_pos];
        let ext = &name[dot_pos + 1..];
        !base.is_empty() && !ext.is_empty()
            && base.len() <= 8 && ext.len() <= 3
            && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
    } else {
        false
    }
}

// ── Corrupted file repair ───────────────────────────────────────────────────

/// Sentinel file created in `Data/` before repair begins. Its presence means
/// the repair was interrupted; on the next run a full client download is
/// performed instead of patching.
const TMS_REPAIR_SENTINEL: &str = "Data/.incomplete";

/// Maximum number of files repaired concurrently on SSD.
const REPAIR_PARALLEL_SSD: usize = 10;
/// Maximum number of files repaired concurrently on HDD.
const REPAIR_PARALLEL_HDD: usize = 1;

/// Download specific files from the TMS full client manifest to repair
/// corrupted files.  Each file is downloaded with up to 5 parallel segments;
/// files are processed concurrently (10 on SSD, 1 on HDD).
///
/// Returns the list of files that still could not be repaired.
fn repair_corrupted_files(
    target_dir: &Path,
    corrupted: &[String],
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<Vec<String>> {
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();

    let info = crate::tms::get_product_info(&agent)
        .context("failed to fetch TMS product manifest for repair")?;

    let base_path = info.execution_path.rfind('/')
        .map(|i| &info.execution_path[..i])
        .unwrap_or("");

    // Normalise both sides to forward slashes for matching.
    let corrupted_set: HashSet<String> = corrupted
        .iter()
        .map(|s| s.replace('\\', "/"))
        .collect();

    // Build the repair list.
    struct RepairItem {
        url: String,
        dest: PathBuf,
        size: u64,
        sha256: String,
        path: String,
    }
    let mut items: Vec<RepairItem> = Vec::new();
    for file in &info.files {
        let normalized = file.path.replace('\\', "/");
        if !corrupted_set.contains(&normalized) {
            continue;
        }
        let url = if base_path.is_empty() {
            format!("{}/{}", info.base_url.trim_end_matches('/'), file.path)
        } else {
            format!("{}/{}/{}", info.base_url.trim_end_matches('/'), base_path, file.path)
        };
        items.push(RepairItem {
            url,
            dest: target_dir.join(sanitize_path(&file.path)),
            size: file.size_in_bytes,
            sha256: file.sha256.clone(),
            path: file.path.clone(),
        });
    }

    if items.is_empty() {
        return Ok(Vec::new());
    }

    // Create a sentinel so an interrupted repair triggers a full re-download
    // on the next run instead of a partial patch.
    let sentinel_path = target_dir.join(TMS_REPAIR_SENTINEL);
    if let Some(parent) = sentinel_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&sentinel_path, "").ok();

    let max_parallel = if crate::is_hdd::is_hdd(target_dir) {
        plog!("  HDD detected — repairing files one at a time.");
        REPAIR_PARALLEL_HDD
    } else {
        REPAIR_PARALLEL_SSD
    };

    let workers = max_parallel.min(items.len()).max(1);
    let counter = AtomicUsize::new(0);
    let done_counter = AtomicUsize::new(0);
    let bytes_downloaded = AtomicUsize::new(0);
    let still_failed: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let idx = counter.fetch_add(1, Ordering::Relaxed);
                    if idx >= items.len() {
                        break;
                    }
                    let item = &items[idx];
                    plog!("  repairing: {} ({:.2} MiB)...",
                        item.path,
                        item.size as f64 / (1024.0 * 1024.0));

                    match download_and_verify_segmented(&agent, &item.url, &item.dest, item.size, &item.sha256) {
                        Ok(()) => {}
                        Err(e) => {
                            plog!("  failed to repair {}: {}", item.path, e);
                            still_failed.lock().unwrap().push(item.path.clone());
                        }
                    }

                    let done = done_counter.fetch_add(1, Ordering::Relaxed) + 1;
                    let dl = bytes_downloaded.fetch_add(item.size as usize, Ordering::Relaxed) + item.size as usize;
                    crate::progress::repair_progress(done, items.len(), &item.path, dl as u64);
                }
            });
        }
    });

    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }

    let failed = still_failed.into_inner().unwrap();

    // Remove the sentinel only when repair completed successfully.
    if failed.is_empty() {
        let _ = std::fs::remove_file(&sentinel_path);
    }

    Ok(failed)
}

/// Download a file to `dest` with up to 5 parallel byte-range segments and
/// resume support, then verify SHA-256.
fn download_and_verify_segmented(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    // Already present and correct?
    if dest.exists() && !crate::resume::progress_path(dest).exists() {
        if let Ok(meta) = dest.metadata() {
            if meta.len() == expected_size {
                let data = std::fs::read(dest)
                    .with_context(|| format!("failed to read {}", dest.display()))?;
                use sha2::Digest;
                let mut hasher = sha2::Sha256::new();
                hasher.update(&data);
                let hash = hex::encode(hasher.finalize());
                if hash.eq_ignore_ascii_case(expected_sha256) {
                    return Ok(());
                }
            }
        }
    }

    // Use a hidden progress bar — per-file bars in a parallel repair loop
    // would interleave and garble the console output.  The plog! messages
    // above already identify each file being repaired.
    let pb = ProgressBar::hidden();

    let segments = effective_segments(expected_size, SEGMENTS_PER_FILE);

    if segments <= 1 || expected_size == 0 || !supports_ranges(agent, url) {
        let _ = std::fs::remove_file(crate::resume::progress_path(dest));
        // Simple stream: read into memory, verify, write.
        for attempt in 0..HTTP_RETRIES {
            match agent.get(url).call() {
                Ok(resp) => {
                    let mut reader = resp.into_reader();
                    let mut data = Vec::with_capacity(expected_size as usize);
                    if reader.read_to_end(&mut data).is_ok() {
                        if data.len() as u64 == expected_size {
                            use sha2::Digest;
                            let mut hasher = sha2::Sha256::new();
                            hasher.update(&data);
                            let hash = hex::encode(hasher.finalize());
                            if hash.eq_ignore_ascii_case(expected_sha256) {
                                std::fs::write(dest, &data)?;
                                pb.finish_and_clear();
                                return Ok(());
                            }
                        }
                    }
                }
                Err(_) if attempt + 1 < HTTP_RETRIES => {
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                Err(e) => {
                    pb.finish_and_clear();
                    return Err(e.into());
                }
            }
            if attempt + 1 >= HTTP_RETRIES {
                pb.finish_and_clear();
                bail!("failed to download after {HTTP_RETRIES} attempts");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    } else {
        // Multi-segment.
        let progress_path = crate::resume::progress_path(dest);
        let saved_opt = crate::resume::read_progress(&progress_path)
            .filter(|_| dest.exists())
            .filter(|_| dest.metadata().map_or(false, |m| m.len() == expected_size));

        let ranges: Vec<(u64, u64)>;
        let progress: crate::resume::FileProgress;

        if let Some(saved) = saved_opt
            .and_then(|s| crate::resume::build_resume_ranges(&s, expected_size).map(|(r, pre)| (s, r, pre)))
        {
            let (saved_segs, resume_ranges, pre_completed) = saved;
            pb.inc(pre_completed);
            progress = crate::resume::FileProgress::from_saved(dest, &saved_segs, &resume_ranges)
                .with_context(|| format!("failed to write progress file {}", progress_path.display()))?;
            ranges = resume_ranges;
        } else {
            {
                let file = std::fs::File::create(dest)?;
                file.set_len(expected_size)?;
            }
            let fresh_ranges = compute_ranges(expected_size, segments);
            progress = crate::resume::FileProgress::new(dest, &fresh_ranges)?;
            ranges = fresh_ranges;
        }

        let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);
        std::thread::scope(|scope| {
            let progress = &progress;
            for (slot, &(start, end)) in ranges.iter().enumerate() {
                let pb = &pb;
                let first_err = &first_err;
                scope.spawn(move || {
                    if let Err(e) = download_segment(agent, url, dest, start, end, pb, progress, slot, None) {
                        let mut s = first_err.lock().unwrap();
                        if s.is_none() { *s = Some(e); }
                    }
                });
            }
        });

        if let Some(e) = first_err.into_inner().unwrap() {
            pb.finish_and_clear();
            return Err(e);
        }
        progress.delete();

        // Verify SHA-256.
        let data = std::fs::read(dest)?;
        if data.len() as u64 != expected_size {
            pb.finish_and_clear();
            bail!("size mismatch: expected {expected_size}, got {}", data.len());
        }
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(&data);
        let hash = hex::encode(hasher.finalize());
        if !hash.eq_ignore_ascii_case(expected_sha256) {
            pb.finish_and_clear();
            bail!("SHA-256 mismatch: expected {expected_sha256}, got {hash}");
        }
    }

    pb.finish_and_clear();
    Ok(())
}
