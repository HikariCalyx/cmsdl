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
use std::fs::File;
use std::io::{BufWriter, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::ZlibDecoder;
use indicatif::{ProgressBar, ProgressStyle};

use crate::plog;

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

/// Parsed WzPatch ready for application.
struct WzPatch {
    /// All patch parts in order.
    parts: Vec<PatchPart>,
    /// The decompressed patch stream, spilled to a scratch file so that a
    /// multi-gigabyte patch does not have to stay resident in memory.
    stream: PatchStream,
    /// Whether this patch uses KMST1125 format (file hash list at start,
    /// no old_checksum in Rebuild parts, FromOldFile carries source path).
    is_kmst1125: bool,
    /// Old-file CRC-32 map from the leading KMST1125 file-hash list
    /// (file name -> pre-patch checksum).  Empty for classic patches.
    old_file_hashes: HashMap<String, u32>,
}

/// Name of the scratch file (inside the patch's temp directory) that holds the
/// decompressed patch stream.
const PATCH_STREAM_FILE: &str = ".cmsdl_patch_stream";

/// Read/write chunk size used for all patch I/O.
const IO_CHUNK: usize = 1 << 20; // 1 MiB

/// A read-only window over a byte range of a file.
///
/// Reads are positional, so the file's own cursor is never disturbed.
struct FileSlice<'a> {
    file: &'a File,
    pos: u64,
    end: u64,
}

impl Read for FileSlice<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.end {
            return Ok(0);
        }
        let take = (self.end - self.pos).min(buf.len() as u64) as usize;
        read_exact_at(self.file, self.pos, &mut buf[..take])?;
        self.pos += take as u64;
        Ok(take)
    }
}

/// Read exactly `buf.len()` bytes at absolute `offset` without moving the
/// file's cursor.
fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> std::io::Result<()> {
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;

    while !buf.is_empty() {
        #[cfg(windows)]
        let read = file.seek_read(buf, offset);
        #[cfg(unix)]
        let read = file.read_at(buf, offset);
        #[cfg(not(any(windows, unix)))]
        let read = {
            use std::io::{Seek, SeekFrom};
            let mut handle = file.try_clone()?;
            handle.seek(SeekFrom::Start(offset))?;
            Read::read(&mut handle, buf)
        };
        let n = read?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "unexpected end of patch stream",
            ));
        }
        offset += n as u64;
        buf = &mut buf[n..];
    }
    Ok(())
}

/// Random-access view over the decompressed patch stream.
///
/// TMS patches decompress to several gigabytes, so the stream is spilled to a
/// scratch file and read back in bounded chunks.  Peak memory is therefore
/// independent of the patch size: only the parts table and the small I/O
/// buffers stay resident.  The scratch file is removed when the stream is
/// dropped.
struct PatchStream {
    file: File,
    path: PathBuf,
    len: u64,
}

impl PatchStream {
    /// Open an existing scratch file as a patch stream.
    fn open(path: PathBuf) -> Result<Self> {
        let file = File::open(&path)
            .with_context(|| format!("failed to open patch stream {}", path.display()))?;
        let len = file
            .metadata()
            .with_context(|| format!("failed to stat {}", path.display()))?
            .len();
        Ok(PatchStream { file, path, len })
    }

    /// Length of the decompressed stream, in bytes.
    fn len(&self) -> u64 {
        self.len
    }

    /// Fill `buf` with bytes starting at absolute `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset.saturating_add(buf.len() as u64) > self.len {
            bail!(
                "patch stream read out of bounds (offset {offset}, len {}, size {})",
                buf.len(),
                self.len
            );
        }
        read_exact_at(&self.file, offset, buf).context("failed to read the patch stream")
    }

    /// Read `len` bytes at `offset` as a lossy UTF-8 string.
    fn read_string_at(&self, offset: u64, len: usize) -> Result<String> {
        let mut buf = vec![0u8; len];
        self.read_at(offset, &mut buf)?;
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

impl Drop for PatchStream {
    fn drop(&mut self) {
        // Best effort: the caller removes the scratch directory as well.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Read-ahead window of [`StreamCursor`].
const CURSOR_BUF: usize = 8192;

/// A [`Read`] + [`Seek`] cursor over a [`PatchStream`].
///
/// This gives the patch parser the same API it had over an in-memory slice
/// without keeping the (multi-gigabyte) stream in memory.  Sequential reads are
/// served from a small read-ahead window so that parsing the instruction
/// stream is not a syscall per field.
struct StreamCursor<'a> {
    stream: &'a PatchStream,
    pos: u64,
    buf: [u8; CURSOR_BUF],
    buf_start: u64,
    buf_len: usize,
}

impl<'a> StreamCursor<'a> {
    fn new(stream: &'a PatchStream) -> Self {
        Self::at(stream, 0)
    }

    fn at(stream: &'a PatchStream, pos: u64) -> Self {
        StreamCursor { stream, pos, buf: [0u8; CURSOR_BUF], buf_start: 0, buf_len: 0 }
    }

    /// Current absolute position in the stream.
    fn position(&self) -> u64 {
        self.pos
    }

    /// Move to an absolute position and drop the read-ahead window.
    fn set_position(&mut self, pos: u64) {
        self.pos = pos;
        self.buf_len = 0;
    }

    /// Bytes left between the cursor and the end of the stream.
    fn remaining(&self) -> u64 {
        self.stream.len().saturating_sub(self.pos)
    }
}

impl Read for StreamCursor<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let remaining = self.stream.len().saturating_sub(self.pos);
        if remaining == 0 {
            return Ok(0);
        }
        // Refill the read-ahead window when the request is not already covered.
        let covered = self.pos >= self.buf_start
            && self.pos - self.buf_start < self.buf_len as u64;
        if !covered {
            let take = remaining.min(CURSOR_BUF as u64) as usize;
            read_exact_at(&self.stream.file, self.pos, &mut self.buf[..take])?;
            self.buf_start = self.pos;
            self.buf_len = take;
        }
        let offset = (self.pos - self.buf_start) as usize;
        let take = (self.buf_len - offset).min(out.len());
        out[..take].copy_from_slice(&self.buf[offset..offset + take]);
        self.pos += take as u64;
        Ok(take)
    }
}

impl Seek for StreamCursor<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(n) => self.stream.len() as i128 + n as i128,
            SeekFrom::Current(n) => self.pos as i128 + n as i128,
        };
        if target < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start of the patch stream",
            ));
        }
        self.set_position(target as u64);
        Ok(self.pos)
    }
}

/// The WzPatch block located inside a `.patch` file.
struct BlockRange {
    /// Absolute offset of the block (the WzPatch header) in the file.
    start: u64,
    /// Length of the block in bytes.
    len: u64,
}

/// Locate the WzPatch block inside `file`.
///
/// TMS patches are usually the whole file, optionally followed by a 32- or
/// 64-bit footer (block length + notice length + end marker).  Only the head
/// and tail of the file are inspected, so a multi-gigabyte patch is never read
/// into memory just to find the block.
fn locate_patch_block(file: &File, file_len: u64) -> Result<BlockRange> {
    // 32-bit footer.
    if file_len >= 12 {
        let mut tail = [0u8; 4];
        read_exact_at(file, file_len - 4, &mut tail)?;
        if u32::from_le_bytes(tail) == END_MARKER {
            let mut lens = [0u8; 8];
            read_exact_at(file, file_len - 12, &mut lens)?;
            let patch_len = u32::from_le_bytes(lens[..4].try_into().unwrap()) as u64;
            let block_end = file_len - 12;
            if patch_len <= block_end {
                return Ok(BlockRange { start: block_end - patch_len, len: patch_len });
            }
        }
    }

    // 64-bit footer: the end marker is followed by four zero bytes.
    if file_len >= 24 {
        let mut tail = [0u8; 8];
        read_exact_at(file, file_len - 8, &mut tail)?;
        let lo = u32::from_le_bytes(tail[..4].try_into().unwrap());
        let hi = u32::from_le_bytes(tail[4..].try_into().unwrap());
        if lo == END_MARKER && hi == 0 {
            let mut lens = [0u8; 8];
            read_exact_at(file, file_len - 24, &mut lens)?;
            let patch_len = u64::from_le_bytes(lens);
            let block_end = file_len - 24;
            if patch_len <= block_end {
                return Ok(BlockRange { start: block_end - patch_len, len: patch_len });
            }
        }
    }

    // No footer: find the WzPatch magic (a patch may carry leading data).
    let mut buf = vec![0u8; IO_CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut pos = 0u64;
    while pos < file_len {
        let take = (file_len - pos).min(buf.len() as u64) as usize;
        read_exact_at(file, pos, &mut buf[..take])?;
        // Extend the window with the previous chunk's tail so that a magic
        // straddling a chunk boundary is still found.
        let mut window = Vec::with_capacity(carry.len() + take);
        window.extend_from_slice(&carry);
        window.extend_from_slice(&buf[..take]);
        if let Some(i) = window
            .windows(WZPATCH_MAGIC.len())
            .position(|w| w == WZPATCH_MAGIC)
        {
            let start = pos - carry.len() as u64 + i as u64;
            return Ok(BlockRange { start, len: file_len - start });
        }
        carry.clear();
        let keep = window.len().min(WZPATCH_MAGIC.len() - 1);
        carry.extend_from_slice(&window[window.len() - keep..]);
        pos += take as u64;
    }

    // Fallback: treat the whole file as the patch block.
    Ok(BlockRange { start: 0, len: file_len })
}

/// Decompress the body of a WzPatch block into `scratch`.
fn decompress_body_to_file(
    file: &File,
    body_start: u64,
    body_len: u64,
    scratch: &Path,
) -> Result<()> {
    // The compressed stream always starts at byte 16 of the patch block.  Check
    // whether the first two bytes form a zlib header (CMF=0x78, FLG where
    // (CMF*256+FLG)%31==0).  If so, use a ZlibDecoder; otherwise use raw
    // DeflateDecoder.
    let mut probe = [0u8; 2];
    let is_zlib = if body_len >= 2 {
        read_exact_at(file, body_start, &mut probe)?;
        probe[0] == 0x78 && (probe[0] as u16 * 256 + probe[1] as u16) % 31 == 0
    } else {
        false
    };

    let body = FileSlice { file, pos: body_start, end: body_start + body_len };
    let mut decoder: Box<dyn Read> = if is_zlib {
        Box::new(ZlibDecoder::new(body))
    } else {
        Box::new(flate2::read::DeflateDecoder::new(body))
    };

    let out = File::create(scratch)
        .with_context(|| format!("failed to create {}", scratch.display()))?;
    let mut writer = BufWriter::with_capacity(IO_CHUNK, out);
    let mut buf = vec![0u8; IO_CHUNK];
    loop {
        let n = decoder
            .read(&mut buf)
            .context("failed to decompress patch data")?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .with_context(|| format!("failed to write {}", scratch.display()))?;
    }
    writer
        .flush()
        .with_context(|| format!("failed to write {}", scratch.display()))?;
    Ok(())
}

/// Read a `.patch` file, spilling its decompressed stream to `scratch_dir`.
///
/// Neither the compressed file nor the decompressed stream is held in memory:
/// the compressed body is decompressed straight from disk to a scratch file,
/// which is removed once the returned [`WzPatch`] is dropped.
fn load_wzpatch(path: &Path, scratch_dir: &Path) -> Result<WzPatch> {
    let file = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let file_len = file
        .metadata()
        .with_context(|| format!("failed to read {}", path.display()))?
        .len();
    if file_len < 16 {
        bail!("patch file too small ({file_len})");
    }

    let block = locate_patch_block(&file, file_len)?;
    if block.len < 16 {
        bail!("patch block too small ({})", block.len);
    }
    let mut header = [0u8; 16];
    read_exact_at(&file, block.start, &mut header)?;
    if &header[..8] != WZPATCH_MAGIC {
        bail!("invalid WzPatch magic");
    }
    let _version = i32::from_le_bytes(header[8..12].try_into().unwrap());
    let _checksum0 = u32::from_le_bytes(header[12..16].try_into().unwrap());

    let scratch = scratch_dir.join(PATCH_STREAM_FILE);
    decompress_body_to_file(&file, block.start + 16, block.len - 16, &scratch)?;
    drop(file);

    let stream = PatchStream::open(scratch)?;
    // Parse patch parts (handles the KMST1125 hash list if present).
    let (parts, is_kmst1125, old_file_hashes) = parse_patch_parts(&stream)?;

    Ok(WzPatch { parts, stream, is_kmst1125, old_file_hashes })
}

/// Parse the sequence of patch parts from the decompressed stream.
///
/// Returns `(parts, is_kmst1125, old_file_hashes)`.
///
/// If the data begins with a KMST1125 file-hash list (a positive i32 count
/// followed by that many `{i32 len, ASCII name, u32 checksum}` entries), the
/// list is consumed and `is_kmst1125` is set.  Otherwise the cursor resets
/// and parsing proceeds in classic mode.
fn parse_patch_parts(stream: &PatchStream) -> Result<(Vec<PatchPart>, bool, HashMap<String, u32>)> {
    let mut cursor = StreamCursor::new(stream);

    // ── Try to read a KMST1125 file-hash list ──────────────────────────
    let (is_kmst1125, old_file_hashes) = try_read_kmst1125_hash_list(&mut cursor);

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
                    // Directory marker - skip.
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
fn try_read_kmst1125_hash_list(cursor: &mut StreamCursor<'_>) -> (bool, HashMap<String, u32>) {
    let start = cursor.position();
    if cursor.remaining() < 4 {
        return (false, HashMap::new());
    }

    let count = match read_i32(cursor) {
        Ok(c) => c,
        Err(_) => return (false, HashMap::new()),
    };
    // Reasonable bounds: 1 .. 500_000
    if count <= 0 || count > 500_000 {
        cursor.set_position(start);
        return (false, HashMap::new());
    }

    let mut hashes = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let name_len = match read_i32(cursor) {
            Ok(n) if n > 0 && n <= 260 => n as usize,
            _ => {
                cursor.set_position(start);
                return (false, HashMap::new());
            }
        };
        if cursor.remaining() < name_len as u64 + 4 {
            cursor.set_position(start);
            return (false, HashMap::new());
        }
        let mut name_bytes = vec![0u8; name_len];
        if cursor.read_exact(&mut name_bytes).is_err() {
            cursor.set_position(start);
            return (false, HashMap::new());
        }
        let name = String::from_utf8_lossy(&name_bytes).into_owned();
        let checksum = match read_u32(cursor) {
            Ok(c) => c,
            Err(_) => {
                cursor.set_position(start);
                return (false, HashMap::new());
            }
        };
        hashes.insert(name, checksum);
    }

    (true, hashes)
}

/// Read a file name followed by a type byte from the patch stream.
///
/// File name bytes are read until a byte - 2 is encountered; that byte is the
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
    /// No version patches were needed, but the standalone executable hotfix
    /// (ExePatch.dat → MapleStory.exe) was downloaded and installed.
    MinorPatchApplied,
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
        // Already at the target version — still keep MapleStory.exe current by
        // fetching the standalone executable hotfix (ExePatch.dat), if any is
        // published for this version.
        if ensure_latest_minor_patch(target_dir, target_version, allow_insecure, proxy) {
            return Ok(PatchOutcome::MinorPatchApplied);
        }
        plog!("client is already at version {}; nothing to do.", target_version);
        return Ok(PatchOutcome::AlreadyUpToDate);
    }

    if current_version > target_version {
        plog!(
            "client is at version {}, which is newer than the requested target {}; nothing to patch.",
            current_version, target_version
        );
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
            // The TMS patch CDN only serves plain HTTP (TLS is not supported on
            // this host), so try http:// first and only fall back to https:// in
            // case the CDN is upgraded in the future.
            let patch_url = build_patch_url(current, target);
            let zip_name = format!("{:05}to{:05}.patch", current, target);
            let dest = patchdata.join(&zip_name);

            plog!("trying patch: {} -> {} ({})", current, target, patch_url);

            // Probe the URL to get the file size.  Try the primary (http://)
            // URL first; if that fails try the https:// equivalent.
            let fallback_url: String;
            let (size, download_url) = match probe_file_size(&agent, &patch_url) {
                Ok(s) if s > 0 => (s, patch_url.as_str()),
                Ok(_) => {
                    plog!("  server returned empty file; skipping.");
                    continue;
                }
                Err(e) => {
                    // Try fallback to HTTPS.
                    fallback_url = patch_url.replacen("http://", "https://", 1);
                    plog!("  primary URL failed: {:#}; trying {}", e, fallback_url);
                    match probe_file_size(&agent, &fallback_url) {
                        Ok(s) if s > 0 => (s, fallback_url.as_str()),
                        Ok(_) => {
                            plog!("  server returned empty file (fallback); skipping.");
                            continue;
                        }
                        Err(_) => {
                            continue;
                        }
                    }
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

            if let Err(e) = download_patch_to_file(&agent, download_url, &dest, size) {
                plog!("  failed to download {}: {:#}", zip_name, e);
                let _ = std::fs::remove_file(&dest);
                let _ = std::fs::remove_file(crate::resume::progress_path(&dest));
                continue;
            }
            patch_index.fetch_add(1, Ordering::Relaxed);

            plog!("  applying {}...", zip_name);
            // Reading + decompressing + parsing the downloaded patch is the
            // (short) pause seen before the pre-patch checksum phase, surfaced
            // on the GUI.  The decompressed stream goes to a scratch file inside
            // the client, so a multi-gigabyte patch never has to be resident.
            crate::progress::loading_patch();
            let temp_dir = create_temp_dir(target_dir)?;
            let loaded = load_wzpatch(&dest, &temp_dir);
            // The download has been consumed as a whole; release it before the
            // apply phase starts.
            let _ = std::fs::remove_file(&dest);
            let result = match loaded {
                Ok(patch) => apply_patch_data(&patch, target_dir, &temp_dir),
                Err(e) => Err(e.context(format!(
                    "failed to read downloaded patch {}",
                    dest.display()
                ))),
            };
            let _ = std::fs::remove_dir_all(&temp_dir);
            let corrupted = result
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

    if current == target_version {
        // All patch files applied — fetch the standalone executable hotfix for
        // the version the client is now on.
        ensure_latest_minor_patch(target_dir, target_version, allow_insecure, proxy);
    }
    plog!("patching successful: now at version {}.", current);
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

    let patch_size = std::fs::metadata(patch_path)
        .with_context(|| format!("failed to read {}", patch_path.display()))?
        .len();

    plog!("applying local patch '{}' ({:.2} MiB)...",
        patch_path.display(),
        patch_size as f64 / (1024.0 * 1024.0));

    // Decompress the patch into a scratch file and parse it, then apply it; the
    // compressed file and the decompressed stream are never held in memory.
    crate::progress::loading_patch();
    let temp_dir = create_temp_dir(target_dir)?;
    let result = (|| -> Result<Vec<String>> {
        let patch = load_wzpatch(patch_path, &temp_dir)?;
        apply_patch_data(&patch, target_dir, &temp_dir)
    })();
    let _ = std::fs::remove_dir_all(&temp_dir);
    let corrupted = result.context("failed to apply patch")?;

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
pub(crate) fn get_latest_version(agent: &ureq::Agent) -> Result<i16> {
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
///
/// The TMS patch CDN only serves plain HTTP (the official patcher uses `http://`;
/// TLS is not supported on this host), so the URL is built as `http://`.
pub(crate) fn build_patch_url(old_ver: i16, new_ver: i16) -> String {
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

/// Build the standalone executable hotfix ("minor patch") URL for `version`.
pub(crate) fn build_exe_patch_url(version: i16) -> String {
    format!(
        "http://tw.cdnpatch.maplestory.beanfun.com/maplestory/patch/patchdir/{:05}/ExePatch.dat",
        version
    )
}

/// Stream a bounded range request from `*pos` to `end` (inclusive) into
/// `dest` at `*pos`.  Errors (a dropped/stalled connection) are returned to
/// the caller so it can retry — bytes already written are preserved, and the
/// next attempt resumes from where the stream stopped.
fn stream_exe_bounded(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    pos: &mut u64,
    end: u64,
    size: u64,
    pb: &ProgressBar,
    dl_progress: &AtomicUsize,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-{}", *pos, end))
        .call()
        .map_err(|e| anyhow!("minor patch range request failed: {e}"))?;
    let mut reader = resp.into_reader();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;
    file.seek(SeekFrom::Start(*pos))?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let remaining = (end + 1).saturating_sub(*pos) as usize;
        if remaining == 0 {
            break;
        }
        let take = n.min(remaining);
        file.write_all(&buf[..take])?;
        *pos += take as u64;
        pb.inc(take as u64);
        if crate::progress::active() {
            // GUI mode: the bar is hidden, so drive the on-screen
            // progress/speed through the reporter phase instead.
            let total = dl_progress.fetch_add(take, Ordering::Relaxed) + take;
            crate::progress::minor_patch_progress(total as u64, size);
        }
        if take < n {
            break;
        }
    }
    Ok(())
}

/// Download one byte range `[start, end]` (inclusive) of `url` into `dest`,
/// which must already be pre-allocated.  A plain range write with **no resume
/// sidecar**; transient failures (a dropped/stalled connection) are retried
/// from where the stream stopped, up to `MAX_STALL_RETRIES` consecutive
/// no-progress attempts.  Progress is reported through `pb` (console bar) and
/// the `minor_patch_progress` reporter phase (GUI), using the shared
/// cumulative `dl_progress` counter.
fn download_exe_range(
    agent: &ureq::Agent,
    url: &str,
    dest: &Path,
    start: u64,
    end: u64,
    size: u64,
    pb: &ProgressBar,
    dl_progress: &AtomicUsize,
) -> Result<()> {
    let mut pos = start;
    let mut stalls = 0usize;
    while pos <= end {
        let before = pos;
        // Best-effort stream of the remainder of this range.  Errors are
        // handled here rather than propagated: if any progress was made the
        // stall counter resets, otherwise we retry after a short backoff.
        // Only `MAX_STALL_RETRIES` consecutive stalled attempts give up.
        let _ = stream_exe_bounded(agent, url, dest, &mut pos, end, size, pb, dl_progress);
        if pos > end {
            return Ok(());
        }
        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                bail!(
                    "minor patch download stalled after {MAX_STALL_RETRIES} retries (range {start}-{end})"
                );
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    Ok(())
}

/// Download `url` (of `size` bytes) into `dest` using up to `SEGMENTS_PER_FILE`
/// (5) parallel byte-range segments and **no** resume sidecar.  Progress is
/// shown via an indicatif bar (console) or the `minor_patch_progress` reporter
/// phase (GUI) with live transfer speed.
fn download_exe_segments(agent: &ureq::Agent, url: &str, dest: &Path, size: u64) -> Result<()> {
    if size == 0 {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    {
        let file = std::fs::File::create(dest)?;
        file.set_len(size)?;
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
    let segments = effective_segments(size, SEGMENTS_PER_FILE).max(1);
    let ranges = compute_ranges(size, segments);

    let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let err_slot = &first_err;
    std::thread::scope(|scope| {
        for &(start, end) in &ranges {
            let pb = &pb;
            let dl_progress = &dl_progress;
            scope.spawn(move || {
                if let Err(e) =
                    download_exe_range(agent, url, dest, start, end, size, pb, dl_progress)
                {
                    let mut g = err_slot.lock().unwrap();
                    if g.is_none() {
                        *g = Some(e);
                    }
                }
            });
        }
    });

    pb.finish_and_clear();
    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }
    // Final render so the GUI drops the live speed indicator.
    crate::progress::minor_patch_progress(size, size);
    Ok(())
}

/// Download the standalone executable hotfix for `version` into `target_dir`
/// and install it as `MapleStory.exe`.
///
/// Order of operations (mirrors the official patcher):
///   1. `ExePatch.dat` is downloaded to the client root (`target_dir`),
///      directly next to `MapleStory.exe` so the rename stays on one volume.
///   2. Once the download finishes, the existing `MapleStory.exe` is deleted
///      (the read-only attribute is cleared first, if any).
///   3. `ExePatch.dat` is renamed to `MapleStory.exe`.
///
/// Returns `true` when a minor patch was installed, `false` when none is
/// published for this version (HTTP 404 / no reported size).
fn download_minor_patch(
    target_dir: &Path,
    version: i16,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<bool> {
    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();
    let url = build_exe_patch_url(version);

    let size = match probe_file_size(&agent, &url) {
        Ok(s) if s > 0 => s,
        Ok(_) => return Ok(false), // no Content-Length → treat as absent
        Err(e) if format!("{e:#}").contains("404") => {
            plog!("  ExePatch.dat for version {version} not found (404); no minor patch.");
            return Ok(false);
        }
        Err(e) => return Err(e),
    };

    let exe_patch = target_dir.join("ExePatch.dat");

    // 1. Download ExePatch.dat into the client root. If a complete copy from a
    // previous interrupted install is already there, reuse it and skip the
    // download; otherwise clear the stale/partial file and fetch it fresh.
    let already_have = exe_patch.exists()
        && exe_patch.metadata().map_or(false, |m| m.len() == size);
    if !already_have {
        remove_readonly_file(&exe_patch);
        let _ = std::fs::remove_file(&exe_patch);
        // A real download is about to start: switch the UI to the minor-patch
        // phase (fresh speed/bar).  When no file is published we never reach
        // here, so the previous progress state is left untouched.
        crate::progress::minor_patch(&format!("V{version}"), size);
        if let Err(e) = download_exe_segments(&agent, &url, &exe_patch, size) {
            let _ = std::fs::remove_file(&exe_patch);
            return Err(e);
        }
    } else {
        plog!("  ExePatch.dat already downloaded; installing...");
    }

    // 2. Delete the existing MapleStory.exe if present (clear read-only first).
    //    A missing file is fine: treat "not found" as success, so a stale
    //    existence check or an already-deleted executable never blocks the
    //    install. Only a genuine failure (e.g. the file is locked because the
    //    game is running) is reported.
    let exe_path = target_dir.join("MapleStory.exe");
    remove_readonly_file(&exe_path);
    match std::fs::remove_file(&exe_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "failed to remove existing {} (is the game running, or does cmsdl need administrator rights here?)",
                    exe_path.display()
                )
            });
        }
    }

    // 3. Rename ExePatch.dat → MapleStory.exe (same directory → atomic). If the
    //    destination is somehow still occupied (e.g. an existence/delete
    //    mismatch), clear it and retry once before giving up.
    if std::fs::rename(&exe_patch, &exe_path).is_err() {
        remove_readonly_file(&exe_path);
        let _ = std::fs::remove_file(&exe_path);
        std::fs::rename(&exe_patch, &exe_path).with_context(|| {
            format!(
                "failed to rename {} to {} (is the game running, or does cmsdl need administrator rights here?)",
                exe_patch.display(),
                exe_path.display()
            )
        })?;
    }
    Ok(true)
}

/// Best-effort wrapper: downloads and installs the latest minor executable
/// patch (`ExePatch.dat` → `MapleStory.exe`) for `version` at `target_dir`.
/// Failures are logged, never fatal to the patch procedure itself.
///
/// Returns `true` when `MapleStory.exe` was actually replaced.
fn ensure_latest_minor_patch(
    target_dir: &Path,
    version: i16,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> bool {
    plog!("Update Minor Patch (V{version}): checking for ExePatch.dat...");
    match download_minor_patch(target_dir, version, allow_insecure, proxy) {
        Ok(true) => {
            plog!("Update Minor Patch (V{version}): MapleStory.exe updated.");
            true
        }
        Ok(false) => {
            plog!("Update Minor Patch (V{version}): no minor patch available.");
            false
        }
        Err(e) => {
            plog!("Update Minor Patch (V{version}): warning: {e:#}");
            false
        }
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

/// CRC-32 of an open file, computed in bounded chunks so that verifying a
/// several-hundred-megabyte WZ archive never loads it into memory.
fn crc32_file_handle(file: &File, len: u64) -> Result<u32> {
    let mut buf = vec![0u8; IO_CHUNK];
    let mut crc = 0u32;
    let mut pos = 0u64;
    while pos < len {
        let take = (len - pos).min(buf.len() as u64) as usize;
        read_exact_at(file, pos, &mut buf[..take])?;
        crc = crate::patch_builder::crc32_update(crc, &buf[..take]);
        pos += take as u64;
    }
    Ok(crc)
}

/// Validate that an old file exists with the expected CRC-32.
///
/// Returns:
/// - `Ok(PreValidate::Ok)` if the file exists and matches `old_checksum`.
/// - `Ok(PreValidate::AlreadyUpToDate)` if the file already matches
///   `new_checksum` (only relevant when the file is itself rebuilt by this
///   patch; source-only KMST1125 files pass `None`).
/// - `Err(...)` if the file is missing or has an unexpected checksum.
fn validate_old_file(
    target_dir: &Path,
    file_name: &str,
    old_checksum: u32,
    new_checksum: Option<u32>,
) -> Result<PreValidate> {
    let old_path = target_dir.join(sanitize_path(file_name));
    if !old_path.exists() {
        bail!("old file not found");
    }
    let file = File::open(&old_path)
        .with_context(|| format!("failed to read {}", old_path.display()))?;
    let len = file
        .metadata()
        .with_context(|| format!("failed to read {}", old_path.display()))?
        .len();
    let actual_crc = crc32_file_handle(&file, len)
        .with_context(|| format!("failed to read {}", old_path.display()))?;

    if new_checksum == Some(actual_crc) {
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
    stream: &PatchStream,
    inst_offset: u64,
    is_kmst1125: bool,
) -> Result<HashSet<String>> {
    let mut deps = HashSet::new();
    if !is_kmst1125 {
        return Ok(deps);
    }
    let mut cursor = StreamCursor::at(stream, inst_offset);
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
                    let pos = cursor.position();
                    if pos + name_len as u64 <= stream.len() {
                        let name = stream
                            .read_string_at(pos, name_len as usize)?
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

/// Emit one line of the DeadPatch execution plan.  The line is written to the
/// patch log (the GUI reporter's `cmsdl_patcher.log`, or stdout in console
/// mode) and, when a GUI reporter is active and a real terminal is attached
/// (i.e. the GUI was launched from one), it is also echoed to stdout so it
/// shows up on the debug console.
fn plan_line(line: &str) {
    crate::plog!("{}", line);
    // stdout() can be an invalid handle after the GUI frees its own console
    // (Explorer/shortcut launch), where println! would panic — only echo when
    // a genuine terminal is attached.
    if crate::progress::active() && std::io::stdout().is_terminal() {
        println!("{}", line);
    }
}

/// Apply a parsed patch to `target_dir`.  Returns the list of corrupted file
/// paths.
///
/// DeadPatch is enabled by default: a pre-patch validation phase checks every
/// old file's CRC-32 and reports the execution plan (files, sizes, disk space)
/// before writing a single byte to the target directory.
///
/// `temp_dir` must be a scratch directory inside the client (see
/// [`create_temp_dir`]); rebuilt files that still have to wait for their last
/// consumer are staged there before being committed.
fn apply_patch_data(patch: &WzPatch, target_dir: &Path, temp_dir: &Path) -> Result<Vec<String>> {
    // ── DeadPatch: pre-patch validation & execution plan ──────────────────
    let (create_count, rebuild_count, delete_count, total_bytes) = pre_patch_report(&patch);
    plog!("  patch plan: {} create, {} rebuild, {} delete ({} total)",
        create_count, rebuild_count, delete_count,
        crate::progress::format_size(total_bytes));

    // Validate old files before touching anything.  Classic patches embed an
    // old_checksum in each Rebuild part.  KMST1125 patches instead carry the
    // authoritative pre-patch checksum list up front (the first section of the
    // decompressed stream), which also covers source files referenced by
    // rebuilds but not rebuilt themselves — so verify every entry of that list.
    let verify_items: Vec<(String, u32, Option<u32>)> = if patch.is_kmst1125 {
        // Which files are also rebuilt by this patch (so an already-updated
        // file can be skipped instead of treated as corrupt).
        let new_by_name: HashMap<&str, u32> = patch
            .parts
            .iter()
            .filter_map(|p| match p {
                PatchPart::Rebuild { file_name, new_checksum, .. } => {
                    Some((file_name.as_str(), *new_checksum))
                }
                _ => None,
            })
            .collect();
        let mut items: Vec<(String, u32, Option<u32>)> = patch
            .old_file_hashes
            .iter()
            .map(|(name, old_crc)| (name.clone(), *old_crc, new_by_name.get(name.as_str()).copied()))
            .collect();
        // Stable order for progress/log output (a HashMap is unordered).
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items
    } else {
        patch
            .parts
            .iter()
            .filter_map(|p| match p {
                PatchPart::Rebuild { file_name, old_checksum, new_checksum, .. } => {
                    Some((file_name.clone(), *old_checksum, Some(*new_checksum)))
                }
                _ => None,
            })
            .collect()
    };

    let total_verify = verify_items.len();
    if total_verify > 0 {
        crate::progress::begin_verify(total_verify);
    }

    // Verify old files in parallel: each worker reads a file and computes its
    // CRC-32, which is both I/O- and CPU-bound.  On a mechanical hard drive a
    // single worker avoids seek thrashing; on an SSD use up to a handful of
    // threads.
    let workers = if total_verify == 0 {
        0
    } else if crate::is_hdd::is_hdd(target_dir) {
        plog!("  HDD detected — verifying files one at a time.");
        1
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(VERIFY_PARALLEL_SSD)
            .min(total_verify)
            .max(1)
    };

    if total_verify > 0 && workers > 1 {
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let pre_failures: Mutex<Vec<String>> = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= total_verify {
                        break;
                    }
                    let (file_name, old_checksum, new_checksum) = &verify_items[idx];
                    let result = validate_old_file(
                        target_dir,
                        file_name,
                        *old_checksum,
                        *new_checksum,
                    );
                    let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
                    crate::progress::verify_progress(completed, total_verify, file_name);
                    match result {
                        Ok(PreValidate::Ok) => {
                            plog!("    ok {}", file_name);
                        }
                        Ok(PreValidate::AlreadyUpToDate) => {
                            plog!("    skip {} (already up to date)", file_name);
                        }
                        Err(e) => {
                            plog!("    pre-validate fail: {} - {}", file_name, e);
                            pre_failures.lock().unwrap().push(file_name.clone());
                        }
                    }
                });
            }
        });
        let failed = pre_failures.lock().unwrap().len();
        if failed > 0 {
            plog!("  {} file(s) failed pre-patch validation", failed);
        }
    } else {
        // Small file set (or HDD): sequential path keeps the log order stable.
        let mut pre_failures: Vec<String> = Vec::new();
        for (i, (file_name, old_checksum, new_checksum)) in verify_items.iter().enumerate() {
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
            crate::progress::verify_progress(i + 1, total_verify, file_name);
        }
        if !pre_failures.is_empty() {
            plog!("  {} file(s) failed pre-patch validation", pre_failures.len());
        }
    }
    // ─────────────────────────────────────────────────────────────────────

    let mut corrupted: Vec<String> = Vec::new();

    if patch.is_kmst1125 {
        // Signal the GUI that the patch execution plan is being computed.
        crate::progress::planning();
    }

    // ── Collect source dependencies for each Rebuild part ───────────────
    // For KMST1125 patches, FromOldFile instructions can reference different
    // source files.  We scan instructions upfront (without building) so we
    // know which parts depend on which source files.
    let deps: Vec<HashSet<String>> = patch.parts.iter().map(|part| {
        match part {
            PatchPart::Rebuild { inst_offset, .. } => {
                collect_source_deps(&patch.stream, *inst_offset, patch.is_kmst1125)
                    .unwrap_or_default()
            }
            _ => HashSet::new(),
        }
    }).collect();

    // ── DeadPatch execution plan (ported from WzComparerR2.CLI) ─────────
    // A Rebuild part reads OLD copies of its source files, so a produced file
    // must not be committed into the client while any later part may still
    // read the OLD copy as a source.  Mirror the reference
    // DeadPatchExecutionPlan: walk the parts in stream order and assign each
    // file to the LAST part that reads it (its "owner").  A file that no
    // later part reads is owned by itself and may be committed as soon as it
    // is built.  Walking in order and overwriting naturally makes the mapping
    // the last consumer, exactly like the reference.
    let mut file_owner: HashMap<String, String> = HashMap::new();
    let mut name_to_index: HashMap<String, usize> = HashMap::new();
    for (i, part) in patch.parts.iter().enumerate() {
        let file_name = match part {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => continue,
        };
        // Normalise to forward slashes for matching.
        let key = file_name.replace('\\', "/");
        name_to_index.insert(key.clone(), i);
        // A target is its own owner unless a later part reads it as a source.
        file_owner.insert(key.clone(), key.clone());
        for dep in &deps[i] {
            file_owner.insert(dep.clone(), key.clone());
        }
    }

    // owner_index[i] = index of the part that must finish building before
    // part i's temp file may be committed (i itself when nothing reads it).
    let owner_index: Vec<usize> = patch
        .parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let file_name = match part {
                PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
                PatchPart::Delete { .. } => return i,
            };
            let key = file_name.replace('\\', "/");
            file_owner
                .get(&key)
                .and_then(|owner| name_to_index.get(owner))
                .copied()
                .unwrap_or(i)
        })
        .collect();

    // Emit the DeadPatch execution plan.  KMST1125 patches merge old files, so
    // this shows, per patch part, which produced files are committed once that
    // part (their last consumer) has been built.  Mirrors the reference
    // patcher's plan output; every line goes to the log and (in GUI mode) the
    // debug console.
    if patch.is_kmst1125 {
        // Invert owner_index: files owned by part `o` are committed when `o`
        // executes.
        let mut plan_groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (j, part) in patch.parts.iter().enumerate() {
            if matches!(part, PatchPart::Create { .. } | PatchPart::Rebuild { .. }) {
                plan_groups.entry(owner_index[j]).or_default().push(j);
            }
        }
        plan_line("  dead-patch execution plan:");
        for (i, part) in patch.parts.iter().enumerate() {
            let file_name = match part {
                PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
                PatchPart::Delete { .. } => continue,
            };
            if let Some(group) = plan_groups.get(&i) {
                plan_line(&format!("    + execute {}", file_name));
                for &j in group {
                    if let PatchPart::Create { file_name, .. }
                    | PatchPart::Rebuild { file_name, .. } = &patch.parts[j]
                    {
                        plan_line(&format!("        - apply {}", file_name));
                    }
                }
            } else {
                plan_line(&format!("    - execute {} (apply deferred)", file_name));
            }
        }
    }

    // Track which part indices are still "in-flight" (built to temp but not
    // yet applied because a later part may still read their old copy).
    let mut pending: Vec<usize> = Vec::new();

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

    // Phase 1: Build each part to temp, committing a part's temp file as soon
    // as its owner (last consumer) part has been built, so the OLD copy stays
    // on disk for every part that still reads it as a source.
    let total_apply = patch.parts.iter().filter(|p| !matches!(p, PatchPart::Delete { .. })).count();
    crate::progress::begin_apply(total_apply, total_bytes);
    // Console-only byte bar (in GUI mode the reporter drives the window).
    let apply_bar = if total_bytes > 0 && !crate::progress::active() {
        let pb = ProgressBar::new(total_bytes);
        pb.set_style(
            ProgressStyle::with_template(
                "    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta}) {msg}",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(120));
        Some(pb)
    } else {
        None
    };
    let apply_done = AtomicUsize::new(0);
    let mut bytes_done: u64 = 0;
    for (i, part) in patch.parts.iter().enumerate() {
        let file_name = match part {
            PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
            PatchPart::Delete { .. } => continue,
        };

        let result = match part {
            PatchPart::Create { file_name, file_length, checksum, data_offset } => {
                apply_create(&patch.stream, file_name, *data_offset, *file_length, *checksum, &temp_dir, target_dir)
            }
            PatchPart::Rebuild { file_name, old_checksum, new_checksum, inst_offset, .. } => {
                apply_rebuild(&patch.stream, file_name, *inst_offset, *old_checksum, *new_checksum, &temp_dir, target_dir, patch.is_kmst1125)
            }
            PatchPart::Delete { .. } => unreachable!(),
        };

        let done = apply_done.fetch_add(1, Ordering::Relaxed) + 1;
        // Progress is reported in bytes: patch parts differ by two orders of
        // magnitude in size, so a per-file count would leave most of the work
        // hidden in the last few percent of the bar.
        bytes_done = bytes_done.saturating_add(part.byte_delta().max(0) as u64);
        crate::progress::apply_bytes(bytes_done, total_bytes);
        crate::progress::apply_progress(done, total_apply, file_name);
        if let Some(pb) = &apply_bar {
            pb.set_message(file_basename(file_name).to_string());
            pb.set_position(bytes_done);
        }

        if let Err(e) = result {
            plog!("    apply fail: {} - {}", file_name, e);
            corrupted.push(file_name.clone());
            continue;
        }

        // This part is now built (its temp file exists).  Add it to pending,
        // then commit every pending part whose owner is this part: no later
        // part reads those files' OLD copies anymore.  Base files are always
        // deferred to the end (Phase 3).
        pending.push(i);
        let mut new_pending = Vec::new();
        for &idx in &pending {
            let pname = match &patch.parts[idx] {
                PatchPart::Create { file_name, .. } | PatchPart::Rebuild { file_name, .. } => file_name,
                PatchPart::Delete { .. } => continue,
            };
            if owner_index[idx] == i && !is_base_file(pname) {
                apply_pending(idx, &patch.parts, &mut corrupted, &temp_dir, target_dir);
            } else {
                new_pending.push(idx);
            }
        }
        pending = new_pending;
    }
    if let Some(pb) = &apply_bar {
        pb.finish_and_clear();
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
    // These were built during Phase 1; committing them moves the staged file
    // into the client, so no further byte progress is reported here.
    if !pending.is_empty() {
        plog!("  committing {} deferred file(s)...", pending.len());
    }
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

/// Sanitize a file path from the patch manifest (backslash - forward slash,
/// remove leading separators).
fn sanitize_path(path: &str) -> PathBuf {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_start_matches('/').trim_start_matches('\\');
    PathBuf::from(trimmed)
}

/// The final path component of a patch file name (used for progress messages).
fn file_basename(name: &str) -> &str {
    name.rsplit(|c| c == '\\' || c == '/')
        .find(|s| !s.is_empty())
        .unwrap_or(name)
}

/// Apply a "create" patch part: extract raw file data and verify checksum.
fn apply_create(
    stream: &PatchStream,
    file_name: &str,
    data_offset: u64,
    file_length: u32,
    expected_crc: u32,
    temp_dir: &Path,
    _target_dir: &Path,
) -> Result<()> {
    let length = file_length as u64;
    if data_offset.saturating_add(length) > stream.len() {
        bail!("create data for '{}' extends past decompressed data", file_name);
    }

    let temp_path = temp_dir.join(sanitize_path(file_name));
    if let Some(parent) = temp_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
    }

    // Copy the payload straight from the patch stream into the temp file while
    // checksumming it, so the file's data is never held in memory.
    let out = File::create(&temp_path)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    let mut writer = BufWriter::with_capacity(IO_CHUNK, out);
    let mut buf = vec![0u8; IO_CHUNK];
    let mut crc = 0u32;
    let mut pos = data_offset;
    let mut remaining = length;
    while remaining > 0 {
        let take = remaining.min(buf.len() as u64) as usize;
        stream.read_at(pos, &mut buf[..take])?;
        crc = crate::patch_builder::crc32_update(crc, &buf[..take]);
        writer
            .write_all(&buf[..take])
            .with_context(|| format!("failed to write {}", temp_path.display()))?;
        pos += take as u64;
        remaining -= take as u64;
    }
    writer
        .flush()
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    drop(writer);

    if crc != expected_crc {
        let _ = std::fs::remove_file(&temp_path);
        bail!(
            "CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
            file_name, expected_crc, crc
        );
    }

    Ok(())
}

/// An open source file for rebuild copy instructions.  Only the handle and the
/// file's length are kept, never its contents.
struct SourceFile {
    file: File,
    len: u64,
}

/// Apply a "rebuild" patch part: follow rebuild instructions to construct the
/// new file from the old file and patch data.
///
/// For KMST1125 patches the target's own old file is *optional*: a piece that
/// is newly split in this patch (no old copy on the client) is assembled purely
/// from other old source files, so it is rebuilt without its own old copy —
/// exactly like the reference patcher, which only acts on the old file when it
/// exists.  Classic rebuilds always read from their own old file, so it is
/// required there.
fn apply_rebuild(
    stream: &PatchStream,
    file_name: &str,
    inst_offset: u64,
    old_checksum: u32,
    new_checksum: u32,
    temp_dir: &Path,
    target_dir: &Path,
    is_kmst1125: bool,
) -> Result<()> {
    let old_path = target_dir.join(sanitize_path(file_name));

    // Open the target's own old copy when present.  If it already matches the
    // new checksum the file is up to date and is copied through unchanged.  A
    // checksum mismatch is only fatal for classic patches: in KMST1125 it was
    // already reported during pre-patch validation and the rebuild may still
    // succeed (the reference patcher's dead-patch mode does the same).
    //
    // Only file handles are cached, never file contents: a client WZ archive can
    // be several hundred megabytes and holding old + new copies in memory is
    // what made large patches thrash on machines with little RAM.
    let mut sources: HashMap<String, SourceFile> = HashMap::new();
    if old_path.exists() {
        let file = File::open(&old_path)
            .with_context(|| format!("failed to read old file {}", old_path.display()))?;
        let old_len = file
            .metadata()
            .with_context(|| format!("failed to read old file {}", old_path.display()))?
            .len();
        let actual_old_crc = crc32_file_handle(&file, old_len)
            .with_context(|| format!("failed to read old file {}", old_path.display()))?;
        if actual_old_crc == new_checksum {
            let temp_path = temp_dir.join(sanitize_path(file_name));
            if let Some(parent) = temp_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
            }
            std::fs::copy(&old_path, &temp_path)
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
            // A file copy also copies the source's attributes; make sure the
            // staged file stays writable so committing it later succeeds.
            if let Ok(meta) = std::fs::metadata(&temp_path) {
                let mut perms = meta.permissions();
                if perms.readonly() {
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(&temp_path, perms);
                }
            }
            return Ok(());
        }
        if !is_kmst1125 && actual_old_crc != old_checksum {
            bail!(
                "old CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
                file_name, old_checksum, actual_old_crc
            );
        }
        sources.insert(file_name.to_string(), SourceFile { file, len: old_len });
    } else if !is_kmst1125 {
        bail!("old file '{}' not found", file_name);
    }

    // Build the new file straight into the temp file, checksumming as we go, so
    // neither the old copy nor the new copy of a large WZ archive is ever
    // resident in memory.
    let temp_path = temp_dir.join(sanitize_path(file_name));
    if let Some(parent) = temp_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir for {}", temp_path.display()))?;
    }
    let out = File::create(&temp_path)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    let mut writer = BufWriter::with_capacity(IO_CHUNK, out);

    let mut copy_buf = vec![0u8; IO_CHUNK];
    let mut fill_buf = vec![0u8; IO_CHUNK];
    let mut new_crc = 0u32;
    let mut cursor = StreamCursor::at(stream, inst_offset);

    loop {
        let cmd = read_u32(&mut cursor)?;
        if cmd == 0 {
            break;
        }
        match cmd >> 28 {
            0x08 => {
                let len = (cmd & 0x0FFF_FFFF) as u64;
                let mut pos = cursor.position();
                if pos.saturating_add(len) > stream.len() {
                    bail!("patch data extends past end of decompressed stream for '{}'", file_name);
                }
                let mut remaining = len;
                while remaining > 0 {
                    let take = remaining.min(copy_buf.len() as u64) as usize;
                    stream.read_at(pos, &mut copy_buf[..take])?;
                    new_crc = crate::patch_builder::crc32_update(new_crc, &copy_buf[..take]);
                    writer
                        .write_all(&copy_buf[..take])
                        .with_context(|| format!("failed to write {}", temp_path.display()))?;
                    pos += take as u64;
                    remaining -= take as u64;
                }
                cursor.seek(SeekFrom::Current(len as i64))?;
            }
            0x0C => {
                let len = ((cmd & 0x0FFF_FF00) >> 8) as usize;
                let fill_byte = (cmd & 0xFF) as u8;
                fill_buf.fill(fill_byte);
                let mut remaining = len;
                while remaining > 0 {
                    let take = remaining.min(fill_buf.len());
                    writer
                        .write_all(&fill_buf[..take])
                        .with_context(|| format!("failed to write {}", temp_path.display()))?;
                    remaining -= take;
                }
                new_crc = crc32_fill_bytes(new_crc, fill_byte, len);
            }
            _ => {
                let len = cmd as u64;
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
                    let pos = cursor.position();
                    if pos.saturating_add(name_len as u64) > stream.len() {
                        bail!("source file name extends past stream for '{}'", file_name);
                    }
                    let name = stream.read_string_at(pos, name_len as usize)?;
                    cursor.seek(SeekFrom::Current(name_len as i64))?;
                    name
                } else {
                    file_name.to_string()
                };

                // Get or open the source file.
                if !sources.contains_key(&source_file) {
                    let src_path = target_dir.join(sanitize_path(&source_file));
                    let file = File::open(&src_path)
                        .with_context(|| format!("failed to read source file {}", src_path.display()))?;
                    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
                    sources.insert(source_file.clone(), SourceFile { file, len });
                }
                let source = sources.get(&source_file).unwrap();

                let old_start = old_offset as u64;
                if old_start.saturating_add(len) > source.len {
                    bail!(
                        "source file reference out of bounds for '{}' (source='{}'): offset {}, len {}, size {}",
                        file_name, source_file, old_offset, len, source.len
                    );
                }
                let mut pos = old_start;
                let mut remaining = len;
                while remaining > 0 {
                    let take = remaining.min(copy_buf.len() as u64) as usize;
                    read_exact_at(&source.file, pos, &mut copy_buf[..take])
                        .with_context(|| format!("failed to read source file {}", source_file))?;
                    new_crc = crate::patch_builder::crc32_update(new_crc, &copy_buf[..take]);
                    writer
                        .write_all(&copy_buf[..take])
                        .with_context(|| format!("failed to write {}", temp_path.display()))?;
                    pos += take as u64;
                    remaining -= take as u64;
                }
            }
        }
    }

    writer
        .flush()
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    drop(writer);

    // Verify new CRC-32.
    if new_crc != new_checksum {
        let _ = std::fs::remove_file(&temp_path);
        bail!(
            "new CRC-32 mismatch for '{}': expected {:08X}, got {:08X}",
            file_name, new_checksum, new_crc
        );
    }

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
/// - 8.3 format: `XXXXXXXX.XXX` (name - 8 chars, extension - 3 chars)
fn is_junk_dir_name(name: &str) -> bool {
    if name.to_ascii_lowercase().ends_with(".$$$") {
        return true;
    }
    // 8.3 format: check if name looks like `name.ext` with name - 8 and ext - 3.
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
/// Maximum number of files checksum-verified concurrently on SSD during the
/// pre-patch validation phase.
const VERIFY_PARALLEL_SSD: usize = 8;

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

    // Report total file count and byte size for the GUI progress bar.
    let total_bytes: u64 = items.iter().map(|i| i.size).sum();
    crate::progress::begin_repair(items.len(), total_bytes);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a KMST1125 `FromOldFile` instruction sequence that copies `len`
    /// bytes from `offset` of another source file `from_file` (no self read).
    fn inst_from_other(from_file: &str, offset: u32, len: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&len.to_le_bytes()); // cmd: top nibble 0 → FromOldFile
        v.extend_from_slice(&(offset as i32).to_le_bytes());
        let name = from_file.as_bytes();
        v.extend_from_slice(&(name.len() as i32).to_le_bytes());
        v.extend_from_slice(name);
        v.extend_from_slice(&0u32.to_le_bytes()); // end marker
        v
    }

    /// Build a classic `FromOldFile` instruction sequence that copies `len`
    /// bytes from `offset` of the part's own old file (no source name).
    fn inst_from_self(offset: u32, len: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&len.to_le_bytes());
        v.extend_from_slice(&(offset as i32).to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("cmsdl_apply_rebuild_{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Write an instruction blob to a scratch file and open it as a patch
    /// stream (the instructions then start at offset 0).
    fn stream_of(root: &Path, bytes: &[u8]) -> PatchStream {
        let path = root.join("stream.bin");
        std::fs::write(&path, bytes).unwrap();
        PatchStream::open(path).unwrap()
    }

    /// A KMST1125 Rebuild may produce a piece that has no old copy of its own
    /// (e.g. a WZ file newly split in this patch): it is assembled purely from
    /// other old source files.  Such a part must NOT be reported "old file not
    /// found" (regression for the 281→282 `_Canvas_059`/`Etc_004`/… cases).
    #[test]
    fn kmst1125_rebuild_without_own_old_file_builds_from_source() -> Result<()> {
        let root = temp_root("kmst_missing_own");
        let data = root.join("Data");
        std::fs::create_dir_all(&data)?;
        let a = b"0123456789ABCDEF".to_vec();
        std::fs::write(data.join("A.wz"), &a)?;
        let temp = root.join("patch_tmp");
        std::fs::create_dir_all(&temp)?;

        // New file B = A[0..5]; B itself does not exist in the old client.
        let expect = &a[0..5];
        let new_crc = crate::patch_builder::crc32_update(0, expect);
        let inst = inst_from_other("Data/A.wz", 0, 5);
        let stream = stream_of(&root, &inst);

        apply_rebuild(&stream, "Data/B.wz", 0, 0, new_crc, &temp, &root, true)?;

        let out = std::fs::read(temp.join("Data/B.wz"))?;
        assert_eq!(&out, expect, "rebuilt file must match the new checksum");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// Classic rebuilds always read from their own old file, so a missing old
    /// file must still fail there (unchanged behaviour).
    #[test]
    fn classic_rebuild_still_requires_own_old_file() -> Result<()> {
        let root = temp_root("classic_missing_own");
        let temp = root.join("patch_tmp");
        std::fs::create_dir_all(&temp)?;

        let inst = inst_from_self(0, 5);
        let stream = stream_of(&root, &inst);
        let res = apply_rebuild(&stream, "Data/B.wz", 0, 0, 0x1234_5678, &temp, &root, false);
        let err = format!("{:#}", res.expect_err("classic rebuild without an old file must fail"));
        assert!(err.contains("old file"), "unexpected error: {err}");
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A normal classic rebuild (own old file present, correct CRC) still
    /// builds from its own old data.
    #[test]
    fn classic_rebuild_with_own_old_file_builds_from_self() -> Result<()> {
        let root = temp_root("classic_self");
        let data = root.join("Data");
        std::fs::create_dir_all(&data)?;
        let old = b"HELLOworld".to_vec();
        std::fs::write(data.join("B.wz"), &old)?;
        let temp = root.join("patch_tmp");
        std::fs::create_dir_all(&temp)?;

        let old_crc = crate::patch_builder::crc32_update(0, &old);
        let new = &old[0..5];
        let new_crc = crate::patch_builder::crc32_update(0, new);
        let inst = inst_from_self(0, 5);
        let stream = stream_of(&root, &inst);

        apply_rebuild(&stream, "Data/B.wz", 0, old_crc, new_crc, &temp, &root, false)?;

        let out = std::fs::read(temp.join("Data/B.wz"))?;
        assert_eq!(&out, new);
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }

    /// A rebuild that fails its new-checksum check must not leave a partial
    /// file behind: the streaming writer creates the temp file up front.
    #[test]
    fn rebuild_discards_temp_file_on_crc_mismatch() -> Result<()> {
        let root = temp_root("crc_mismatch");
        let data = root.join("Data");
        std::fs::create_dir_all(&data)?;
        let old = b"HELLOworld".to_vec();
        std::fs::write(data.join("B.wz"), &old)?;
        let temp = root.join("patch_tmp");
        std::fs::create_dir_all(&temp)?;

        let old_crc = crate::patch_builder::crc32_update(0, &old);
        let inst = inst_from_self(0, 5);
        let stream = stream_of(&root, &inst);

        // Deliberately wrong new checksum.
        let res = apply_rebuild(&stream, "Data/B.wz", 0, old_crc, 0xDEAD_BEEF, &temp, &root, false);
        assert!(res.is_err(), "a bad new checksum must fail the rebuild");
        assert!(
            !temp.join("Data/B.wz").exists(),
            "the partially written temp file must be removed"
        );

        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
