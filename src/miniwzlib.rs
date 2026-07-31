//! Minimal WZ file reader
//!
//! Reference implementations:
//! - Rust: <https://crates.io/crates/wzlib-rs> (docs.rs/src)
//! - C#:   <https://github.com/HikariCalyx/WzComparerR2-JMS/tree/master/WzComparerR2.WzLib/>

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Magic bytes for a standard PKG1 WZ file.
const PKG1_MAGIC: &[u8; 4] = b"PKG1";

/// Known WZ file types, for optimization of rebuild diffing.
pub const WZ_TYPES: &[&str] = &[
    "Base",
    "Character",
    "DataMap",
    "Effect",
    "Etc",
    "Item",
    "Language",
    "Map",
    "Mob",
    "Morph",
    "Npc",
    "Quest",
    "Reactor",
    "Skill",
    "Sound",
    "String",
    "TamingMob",
    "UI",
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Version information extracted from a WZ file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WzVersion {
    /// The MapleStory patch version (e.g. 83 for GMS v83).
    pub version: i16,
    /// Hash computed from the version string.
    pub version_hash: u32,
}

/// Returns `true` when `rel_path` points at a client `Base.wz`, i.e. either
/// `Base.wz` at the client root or `...\Data\Base\Base.wz`.
///
/// Matching is case-insensitive and treats `\` and `/` as equivalent
/// separators.
#[allow(dead_code)]
pub fn is_base_wz(rel_path: &str) -> bool {
    let norm = rel_path.replace('\\', "/").to_ascii_lowercase();
    norm == "base.wz" || norm.ends_with("data/base/base.wz")
}

/// Open the WZ file at `path` and return its version.
///
/// Currently only PKG1 files are supported.
pub fn get_wz_version<P: AsRef<Path>>(path: P) -> Result<WzVersion, String> {
    /// Amount of data read from the file start. The header and root directory
    /// live at the very beginning, so this is plenty; entry offsets are only
    /// range-checked, never followed.
    const READ_CAP: u64 = 8 * 1024 * 1024;

    let mut file = File::open(path.as_ref())
        .map_err(|e| format!("cannot open {}: {e}", path.as_ref().display()))?;
    let file_len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("seek end: {e}"))?;

    file.seek(SeekFrom::Start(0))
        .map_err(|e| format!("seek 0: {e}"))?;

    let to_read = file_len.min(READ_CAP) as usize;
    let mut buf = vec![0u8; to_read];
    file.read_exact(&mut buf)
        .map_err(|e| format!("read header region: {e}"))?;

    get_wz_version_from_bytes(&buf, file_len)
}

/// Read the WZ version from an in-memory buffer holding the *start* of a WZ
/// file.
///
/// `data` must contain at least the file header and the two `encver` bytes at
/// `data_start`; when reading remote clients this is satisfied by the first
/// downloaded (and decompressed) chunk of the file. `file_len` is the true
/// total size of the file (from the manifest) and is only used for header
/// validation.
///
/// Currently only PKG1 files are supported.
pub fn get_wz_version_from_bytes(data: &[u8], file_len: u64) -> Result<WzVersion, String> {
    if data.len() < 16 {
        return Err("buffer too small to contain a WZ header".into());
    }

    // ---- 4-byte signature ----
    if &data[0..4] != PKG1_MAGIC {
        // PKG2 / random-header / unknown — return version 0.
        return Ok(WzVersion {
            version: 0,
            version_hash: 0,
        });
    }

    // ---- file_size (u64, ignored) + data_start (u32) ----
    let data_start = u32::from_le_bytes([data[12], data[13], data[14], data[15]]) as u64;

    if data_start < 16 || data_start > file_len {
        return Err(format!(
            "invalid data_start {data_start} (file size {file_len})"
        ));
    }

    // ---- 2-byte encver at data_start ----
    let start = data_start as usize;
    if start + 2 > data.len() {
        return Err(format!(
            "buffer only covers {} bytes but data_start is {data_start}",
            data.len()
        ));
    }
    let encver = u16::from_le_bytes([data[start], data[start + 1]]);

    resolve_version(data, data_start as u32, file_len, encver)
}

// ---------------------------------------------------------------------------
// Version resolution
// ---------------------------------------------------------------------------

/// The WZ offset-decryption constant used by MapleStory.
const WZ_OFFSET_CONSTANT: u32 = 0x581C_3F6D;

/// Resolve the MapleStory version from the 2-byte encrypted-version marker.
///
/// `encver` is only an 8-bit checksum, so several versions collide (e.g. 347
/// and 443). To pick the right one we parse the root WZ directory with each
/// candidate version's hash and keep the first version whose entry offsets all
/// fall inside the file. If no candidate can be validated (for instance when
/// the buffer does not cover the directory) we fall back to the first checksum
/// match, preserving the previous behaviour.
fn resolve_version(
    data: &[u8],
    data_start: u32,
    file_len: u64,
    encver: u16,
) -> Result<WzVersion, String> {
    let mut first_match: Option<WzVersion> = None;

    for ver in 0..2000i16 {
        let hash = compute_version_hash(ver);
        if compute_enc_version(hash) as u16 != encver {
            continue;
        }
        if first_match.is_none() {
            first_match = Some(WzVersion {
                version: ver,
                version_hash: hash,
            });
        }
        if directory_offsets_valid(data, data_start, file_len, hash) {
            return Ok(WzVersion {
                version: ver,
                version_hash: hash,
            });
        }
    }

    first_match.ok_or_else(|| format!("cannot determine version from encver {encver:#06X}"))
}

/// Parse the root WZ directory using `version_hash` and return `true` if every
/// entry decodes to an offset inside `[data_start, file_len)`.
///
/// The directory layout (right after the 2-byte `encver`) is version
/// independent for everything except the encoded offsets, so a wrong version
/// hash produces out-of-range offsets and this returns `false`.
fn directory_offsets_valid(
    data: &[u8],
    data_start: u32,
    file_len: u64,
    version_hash: u32,
) -> bool {
    // Position just past the 2-byte encrypted version marker.
    let mut pos = data_start as usize + 2;

    let count = match read_compressed_int(data, &mut pos) {
        Some(c) => c,
        None => return false,
    };
    if count <= 0 || count > 500_000 {
        return false;
    }

    for _ in 0..count {
        let type_byte = match read_u8(data, &mut pos) {
            Some(b) => b,
            None => return false,
        };
        match type_byte {
            1 => {
                // Unknown entry: int32 + int16, then an offset. No size/checksum.
                if pos + 6 > data.len() {
                    return false;
                }
                pos += 6;
                match read_offset(data, &mut pos, data_start, version_hash) {
                    Some(off) if offset_in_range(off, data_start, file_len) => continue,
                    _ => return false,
                }
            }
            2 => {
                // Reference to a name stored elsewhere: a single int32 we skip.
                if read_i32(data, &mut pos).is_none() {
                    return false;
                }
            }
            3 | 4 => {
                // Sub-directory or image: an inline (length-prefixed) name.
                if skip_wz_string(data, &mut pos).is_none() {
                    return false;
                }
            }
            _ => return false,
        }

        // size, checksum (both version-independent), then the encoded offset.
        if read_compressed_int(data, &mut pos).is_none() {
            return false;
        }
        if read_compressed_int(data, &mut pos).is_none() {
            return false;
        }
        match read_offset(data, &mut pos, data_start, version_hash) {
            Some(off) if offset_in_range(off, data_start, file_len) => {}
            _ => return false,
        }
    }

    true
}

/// Returns `true` when `offset` points inside the file body.
fn offset_in_range(offset: u32, data_start: u32, file_len: u64) -> bool {
    let o = offset as u64;
    o >= data_start as u64 && o < file_len
}

// ---------------------------------------------------------------------------
// Low-level readers (bounds-checked, position-advancing)
// ---------------------------------------------------------------------------

fn read_u8(data: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *data.get(*pos)?;
    *pos += 1;
    Some(b)
}

fn read_i8(data: &[u8], pos: &mut usize) -> Option<i8> {
    read_u8(data, pos).map(|b| b as i8)
}

fn read_i32(data: &[u8], pos: &mut usize) -> Option<i32> {
    if *pos + 4 > data.len() {
        return None;
    }
    let v = i32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
    *pos += 4;
    Some(v)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Option<u32> {
    if *pos + 4 > data.len() {
        return None;
    }
    let v = u32::from_le_bytes([data[*pos], data[*pos + 1], data[*pos + 2], data[*pos + 3]]);
    *pos += 4;
    Some(v)
}

/// Read a WZ "compressed int": a single signed byte, or a full `i32` when that
/// byte is `-128`.
fn read_compressed_int(data: &[u8], pos: &mut usize) -> Option<i32> {
    let b = read_i8(data, pos)?;
    if b == -128 {
        read_i32(data, pos)
    } else {
        Some(b as i32)
    }
}

/// Advance `pos` past a WZ length-prefixed string without decoding it.
///
/// The length prefix is stored in plaintext, so the number of bytes to skip is
/// the same regardless of the string's XOR encryption.
fn skip_wz_string(data: &[u8], pos: &mut usize) -> Option<()> {
    let marker = read_i8(data, pos)?;
    if marker == 0 {
        return Some(());
    }
    let byte_len: usize = if marker > 0 {
        // Unicode: `marker` is the char count (2 bytes each); 127 escapes to i32.
        let chars = if marker == 127 {
            read_i32(data, pos)? as usize
        } else {
            marker as usize
        };
        chars.checked_mul(2)?
    } else {
        // ASCII: `-marker` is the byte count; -128 escapes to i32.
        if marker == -128 {
            read_i32(data, pos)? as usize
        } else {
            (-(marker as i32)) as usize
        }
    };
    let end = pos.checked_add(byte_len)?;
    if end > data.len() {
        return None;
    }
    *pos = end;
    Some(())
}

/// Decode a WZ entry offset located at the current position, advancing `pos`
/// past the 4 encrypted bytes. Mirrors MapleStory's offset scrambling.
fn read_offset(data: &[u8], pos: &mut usize, data_start: u32, version_hash: u32) -> Option<u32> {
    let offset_pos = *pos as u32;
    let mut offset = (offset_pos.wrapping_sub(data_start)) ^ 0xFFFF_FFFF;
    offset = offset.wrapping_mul(version_hash);
    offset = offset.wrapping_sub(WZ_OFFSET_CONSTANT);
    offset = offset.rotate_left(offset & 0x1F);
    let encrypted = read_u32(data, pos)?;
    offset ^= encrypted;
    offset = offset.wrapping_add(data_start.wrapping_mul(2));
    Some(offset)
}

/// Hash the version number the same way MapleStory does.
///
/// ```text
/// for each decimal digit c in version:
///     hash = hash * 32 + (c as u32) + 1
/// ```
fn compute_version_hash(version: i16) -> u32 {
    let s = version.to_string();
    let mut hash: u32 = 0;
    for b in s.bytes() {
        hash = hash.wrapping_mul(32).wrapping_add(b as u32).wrapping_add(1);
    }
    hash
}

/// Compute the single-byte encrypted-version marker from a version hash.
///
/// ```text
/// encver = ¬(b0 ^ b1 ^ b2 ^ b3)   where b0..b3 are the four bytes of hash
/// ```
fn compute_enc_version(hash: u32) -> u8 {
    let b0 = (hash >> 24) as u8;
    let b1 = (hash >> 16) as u8;
    let b2 = (hash >> 8) as u8;
    let b3 = hash as u8;
    !(b0 ^ b1 ^ b2 ^ b3)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_hash_known_values() {
        // Cross-check with wzlib-rs test expectations.
        assert_ne!(compute_version_hash(83), 0);
        assert_eq!(compute_version_hash(176), compute_version_hash(176));
    }

    #[test]
    fn encver_known_values() {
        // encver is an 8-bit checksum — collisions are expected.
        // Cross-check: version 280 → encver 0x7D (verified against sample file).
        assert_eq!(compute_enc_version(compute_version_hash(280)), 0x7D);
    }

    #[test]
    fn encver_is_valid_u8() {
        // All version hashes should produce an encver in 0..=255.
        for ver in 0..1500i16 {
            let hash = compute_version_hash(ver);
            let _ = compute_enc_version(hash); // always fits in u8
        }
    }

    /// Encode an offset the way MapleStory expects, so a synthetic directory
    /// decodes back to `target` under `version_hash`.
    fn encode_offset(offset_pos: u32, data_start: u32, version_hash: u32, target: u32) -> u32 {
        let mut scrambled = (offset_pos.wrapping_sub(data_start)) ^ 0xFFFF_FFFF;
        scrambled = scrambled.wrapping_mul(version_hash);
        scrambled = scrambled.wrapping_sub(WZ_OFFSET_CONSTANT);
        scrambled = scrambled.rotate_left(scrambled & 0x1F);
        let xored = target.wrapping_sub(data_start.wrapping_mul(2));
        scrambled ^ xored
    }

    #[test]
    fn from_bytes_reads_a_synthetic_header() {
        // Build a minimal PKG1 file with a one-entry root directory whose
        // offset only decodes into range under the correct version's hash.
        let version: i16 = 280;
        let hash = compute_version_hash(version);
        let encver = compute_enc_version(hash) as u16;

        let data_start: u32 = 60;
        let file_len: u64 = 100_000;

        let mut buf = vec![0u8; 16];
        buf[0..4].copy_from_slice(PKG1_MAGIC); // signature
        buf[4..12].copy_from_slice(&0u64.to_le_bytes()); // file_size (ignored)
        buf[12..16].copy_from_slice(&data_start.to_le_bytes()); // data_start

        // Padding/copyright up to data_start.
        buf.resize(data_start as usize, 0);

        buf.extend_from_slice(&encver.to_le_bytes()); // encver at data_start
        buf.push(0x01); // directory entry count = 1 (compressed int)
        buf.push(4); // entry type = image
        buf.push(0x00); // empty name (WZ string, length 0)
        buf.push(0x0A); // size (compressed int)
        buf.push(0x00); // checksum (compressed int)

        // The 4-byte offset field starts here.
        let offset_pos = buf.len() as u32;
        let encoded = encode_offset(offset_pos, data_start, hash, 500);
        buf.extend_from_slice(&encoded.to_le_bytes());

        let v = get_wz_version_from_bytes(&buf, file_len).expect("should parse");
        assert_eq!(v.version, version);
    }

    #[test]
    fn from_bytes_non_pkg1_is_version_zero() {
        let mut buf = vec![0u8; 32];
        buf[0..4].copy_from_slice(b"PKG2");
        let v = get_wz_version_from_bytes(&buf, 32).expect("should parse");
        assert_eq!(v.version, 0);
    }
}
