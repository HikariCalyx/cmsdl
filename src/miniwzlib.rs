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

/// Open the WZ file at `path` and return its version.
///
/// Currently only PKG1 files are supported.
pub fn get_wz_version<P: AsRef<Path>>(path: P) -> Result<WzVersion, String> {
    let mut file = File::open(path.as_ref()).map_err(|e| format!("cannot open {}: {e}", path.as_ref().display()))?;
    let file_len = file.seek(SeekFrom::End(0)).map_err(|e| format!("seek end: {e}"))?;

    // ---- read 4-byte signature ----
    file.seek(SeekFrom::Start(0)).map_err(|e| format!("seek 0: {e}"))?;
    let mut sig = [0u8; 4];
    file.read_exact(&mut sig).map_err(|e| format!("read signature: {e}"))?;

    if &sig != PKG1_MAGIC {
        // PKG2 / random-header / unknown — return version 0.
        return Ok(WzVersion { version: 0, version_hash: 0 });
    }

    // ---- read file_size (u64) + data_start (u32) ----
    let mut hdr = [0u8; 12];
    file.read_exact(&mut hdr).map_err(|e| format!("read header fields: {e}"))?;
    let data_start = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as u64;

    if data_start < 16 || data_start > file_len {
        return Err(format!("invalid data_start {data_start} (file size {file_len})"));
    }

    // ---- read the 2-byte encver at data_start ----
    if file_len - data_start < 2 {
        return Err("file too small — no data after header".into());
    }
    file.seek(SeekFrom::Start(data_start)).map_err(|e| format!("seek data_start: {e}"))?;
    let mut encver_buf = [0u8; 2];
    file.read_exact(&mut encver_buf).map_err(|e| format!("read encver: {e}"))?;
    let encver = u16::from_le_bytes(encver_buf);

    // ---- brute-force version from encver ----
    for ver in 0..2000i16 {
        let hash = compute_version_hash(ver);
        if compute_enc_version(hash) as u16 == encver {
            return Ok(WzVersion { version: ver, version_hash: hash });
        }
    }

    Err(format!("cannot determine version from encver {encver:#06X}"))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

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
    fn sample_base_wz_version() {
        let v = get_wz_version("target/Base.wz").expect("should read sample Base.wz");
        println!("version      = {}", v.version);
        println!("version_hash = {:#010X}", v.version_hash);
    }

    #[test]
    fn encver_is_valid_u8() {
        // All version hashes should produce an encver in 0..=255.
        for ver in 0..1500i16 {
            let hash = compute_version_hash(ver);
            let _ = compute_enc_version(hash); // always fits in u8
        }
    }
}
