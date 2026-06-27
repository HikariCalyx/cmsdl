//! Progress-file utilities for resumable multi-segment downloads.
//!
//! When a multi-segment file download is interrupted (e.g. by Ctrl+C or a
//! window close), a sidecar file named `<dest>.cmsdl` is maintained next to
//! the partial download.  The next run detects the sidecar and resumes each
//! segment from its last recorded byte offset minus a small safety margin.
//!
//! # Binary layout of `<dest>.cmsdl`
//!
//! ```text
//! 0x00-0x04  ASCII magic "CMSDL"
//! 0x05-0x0F  reserved (zeroed)
//! 0x10-0x17  end of segment 0  (u64 LE, inclusive byte offset)
//! 0x18-0x1F  progress of segment 0 (u64 LE, next byte to write)
//! 0x20-0x27  end of segment 1
//! 0x28-0x2F  progress of segment 1
//!   ...
//! 0x50-0x57  end of segment 4
//! 0x58-0x5F  progress of segment 4
//! ```
//!
//! Unused slots are zeroed.  The file is always exactly 96 bytes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Maximum number of download segments tracked by a single progress file.
pub const MAX_SEGMENTS: usize = 5;

const MAGIC: [u8; 5] = *b"CMSDL";
const HEADER_SIZE: usize = 0x10; // 16-byte header: 5-byte magic + 11 reserved
const SLOT_SIZE: usize = 16;     // 8-byte end + 8-byte progress per slot
const FILE_SIZE: usize = HEADER_SIZE + MAX_SEGMENTS * SLOT_SIZE; // 96 bytes

/// Safety roll-back applied to each segment's progress when resuming.
const RESUME_SHIFT: u64 = 16;

/// Minimum bytes received between progress flushes to disk.
pub const PROGRESS_FLUSH_INTERVAL: u64 = 1 << 20; // 1 MiB

/// Returns the path of the `.cmsdl` sidecar progress file for `dest`.
pub fn progress_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".cmsdl");
    PathBuf::from(s)
}

/// A single segment's saved state as read from a `.cmsdl` file.
#[derive(Copy, Clone, Debug, Default)]
pub struct SavedSegment {
    /// Inclusive end byte offset of this segment.
    pub end: u64,
    /// Next byte to write (progress position). 0 means not yet started.
    pub completed: u64,
}

/// Read the progress file at `path`.
///
/// Returns `None` when the file is absent, too small, or has an unrecognised
/// magic header.  Active slots are those where `end > 0`; unused slots have
/// `end == 0` and `completed == 0`.
pub fn read_progress(path: &Path) -> Option<[SavedSegment; MAX_SEGMENTS]> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < FILE_SIZE {
        return None;
    }
    if bytes[0..5] != MAGIC {
        return None;
    }
    let mut result = [SavedSegment::default(); MAX_SEGMENTS];
    for i in 0..MAX_SEGMENTS {
        let off = HEADER_SIZE + i * SLOT_SIZE;
        let end = u64::from_le_bytes(bytes[off..off + 8].try_into().ok()?);
        let completed = u64::from_le_bytes(bytes[off + 8..off + 16].try_into().ok()?);
        result[i] = SavedSegment { end, completed };
    }
    Some(result)
}

/// Returns the number of active segments in a saved progress array.
///
/// A slot is active if its `end > 0`.  Because progress files are only created
/// for multi-segment downloads (minimum 2 × MIN_SEGMENT_SIZE), slot 0 always
/// has `end > 0`.
fn active_count(saved: &[SavedSegment; MAX_SEGMENTS]) -> usize {
    (0..MAX_SEGMENTS)
        .rev()
        .find(|&i| saved[i].end > 0)
        .map(|i| i + 1)
        .unwrap_or(1)
}

/// Given saved segments and the expected file size, compute the resume
/// `(start, end)` ranges and the bytes already completed.
///
/// Returns `None` if the saved data is inconsistent with `expected_size`
/// (e.g. the file size changed on the server since the interrupted run).
///
/// Each segment's resume start is `max(natural_start, completed - RESUME_SHIFT)`.
/// `pre_completed` is the sum of bytes before each resume start that are
/// already on disk and need no network traffic; used to advance progress bars.
pub fn build_resume_ranges(
    saved: &[SavedSegment; MAX_SEGMENTS],
    expected_size: u64,
) -> Option<(Vec<(u64, u64)>, u64)> {
    let n = active_count(saved);
    // The last active segment must end at exactly byte `expected_size - 1`.
    if saved[n - 1].end + 1 != expected_size {
        return None;
    }

    let mut ranges = Vec::with_capacity(n);
    let mut pre_completed: u64 = 0;
    let mut natural_start: u64 = 0;

    for i in 0..n {
        let seg = &saved[i];
        let end = seg.end;
        let resume_start = seg.completed.saturating_sub(RESUME_SHIFT).max(natural_start);
        // Bytes before resume_start are already on disk.
        pre_completed += resume_start.saturating_sub(natural_start);
        ranges.push((resume_start, end));
        natural_start = end + 1;
    }

    Some((ranges, pre_completed))
}

/// In-flight download progress for up to [`MAX_SEGMENTS`] segments.
///
/// `FileProgress` is safe to share across segment threads via `&` references:
/// each thread writes its own slot atomically, and the write lock serialises
/// disk flushes.  Call [`delete`] on success to remove the sidecar file.
///
/// [`delete`]: FileProgress::delete
pub struct FileProgress {
    pub path: PathBuf,
    /// Inclusive end byte offsets (one per slot; 0 = unused).
    ends: [AtomicU64; MAX_SEGMENTS],
    /// Current progress byte offsets (one per slot).
    completed: [AtomicU64; MAX_SEGMENTS],
    /// Serialises concurrent flushes to the sidecar file.
    write_lock: Mutex<()>,
}

impl FileProgress {
    /// Create a new `FileProgress` from `ranges`, write the initial state to
    /// disk, and return it.
    ///
    /// Each slot i is populated from `ranges[i]` with `completed = start`.
    /// Slots beyond `ranges.len()` are zeroed (unused).
    pub fn new(dest: &Path, ranges: &[(u64, u64)]) -> io::Result<Self> {
        let path = progress_path(dest);
        let fp = FileProgress {
            path,
            ends: Default::default(),
            completed: Default::default(),
            write_lock: Mutex::new(()),
        };
        for (i, &(start, end)) in ranges.iter().enumerate().take(MAX_SEGMENTS) {
            fp.ends[i].store(end, Ordering::Relaxed);
            fp.completed[i].store(start, Ordering::Relaxed);
        }
        fp.flush()?;
        Ok(fp)
    }

    /// Restore a `FileProgress` from `saved` segments and the resume `ranges`.
    ///
    /// `ends` are taken from `saved` (preserving the original segment layout),
    /// and `completed` from the resume-shifted `ranges[i].0`.
    pub fn from_saved(
        dest: &Path,
        saved: &[SavedSegment; MAX_SEGMENTS],
        ranges: &[(u64, u64)],
    ) -> io::Result<Self> {
        let path = progress_path(dest);
        let fp = FileProgress {
            path,
            ends: Default::default(),
            completed: Default::default(),
            write_lock: Mutex::new(()),
        };
        for i in 0..MAX_SEGMENTS {
            fp.ends[i].store(saved[i].end, Ordering::Relaxed);
        }
        for (i, &(start, _)) in ranges.iter().enumerate().take(MAX_SEGMENTS) {
            fp.completed[i].store(start, Ordering::Relaxed);
        }
        fp.flush()?;
        Ok(fp)
    }

    /// Atomically record `completed` for `slot` and flush the state to disk.
    ///
    /// Callers should throttle calls to once per [`PROGRESS_FLUSH_INTERVAL`]
    /// bytes to avoid excessive disk I/O.
    pub fn update(&self, slot: usize, completed: u64) {
        if slot < MAX_SEGMENTS {
            self.completed[slot].store(completed, Ordering::Relaxed);
            let _ = self.flush();
        }
    }

    /// Write the current state to the `.cmsdl` sidecar file.
    pub fn flush(&self) -> io::Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        let mut buf = [0u8; FILE_SIZE];
        buf[0..5].copy_from_slice(&MAGIC);
        for i in 0..MAX_SEGMENTS {
            let off = HEADER_SIZE + i * SLOT_SIZE;
            buf[off..off + 8]
                .copy_from_slice(&self.ends[i].load(Ordering::Relaxed).to_le_bytes());
            buf[off + 8..off + 16]
                .copy_from_slice(&self.completed[i].load(Ordering::Relaxed).to_le_bytes());
        }
        std::fs::write(&self.path, &buf)
    }

    /// Remove the `.cmsdl` sidecar file.  Errors are silently ignored.
    pub fn delete(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_progress_through_file() {
        let dir = std::env::temp_dir();
        let dest = dir.join("cmsdl_test_roundtrip.bin");
        let ranges = vec![(0u64, 999u64), (1000u64, 1999u64), (2000u64, 2999u64)];
        let fp = FileProgress::new(&dest, &ranges).unwrap();
        fp.update(1, 1500);

        let path = progress_path(&dest);
        let saved = read_progress(&path).unwrap();
        assert_eq!(saved[0].end, 999);
        assert_eq!(saved[0].completed, 0);
        assert_eq!(saved[1].end, 1999);
        assert_eq!(saved[1].completed, 1500);
        assert_eq!(saved[2].end, 2999);
        assert_eq!(saved[2].completed, 2000);
        // Unused slots should be zero.
        assert_eq!(saved[3].end, 0);
        assert_eq!(saved[4].end, 0);

        fp.delete();
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn build_resume_ranges_applies_shift_and_pre_completed() {
        let mut saved = [SavedSegment::default(); MAX_SEGMENTS];
        // Two segments covering a 2000-byte file.
        saved[0] = SavedSegment { end: 999, completed: 600 };
        saved[1] = SavedSegment { end: 1999, completed: 1800 };

        let (ranges, pre) = build_resume_ranges(&saved, 2000).unwrap();
        assert_eq!(ranges.len(), 2);
        // Segment 0: resume_start = max(0, 600 - 16) = 584
        assert_eq!(ranges[0], (584, 999));
        // Segment 1: resume_start = max(1000, 1800 - 16) = max(1000, 1784) = 1784
        assert_eq!(ranges[1], (1784, 1999));
        // pre_completed = (584 - 0) + (1784 - 1000) = 584 + 784 = 1368
        assert_eq!(pre, 1368);
    }

    #[test]
    fn build_resume_ranges_rejects_wrong_size() {
        let mut saved = [SavedSegment::default(); MAX_SEGMENTS];
        saved[0] = SavedSegment { end: 999, completed: 500 };
        // last_end + 1 = 1000, but expected_size = 2000 → mismatch
        assert!(build_resume_ranges(&saved, 2000).is_none());
    }

    #[test]
    fn build_resume_ranges_clamps_to_natural_start() {
        let mut saved = [SavedSegment::default(); MAX_SEGMENTS];
        // Segment 0 barely started, completed < RESUME_SHIFT → saturating_sub gives 0
        saved[0] = SavedSegment { end: 999, completed: 5 };
        saved[1] = SavedSegment { end: 1999, completed: 1000 }; // exactly at natural start
        let (ranges, pre) = build_resume_ranges(&saved, 2000).unwrap();
        // resume_start for seg 0: max(0, 5 - 16) = max(0, 0 [saturating]) = 0
        assert_eq!(ranges[0].0, 0);
        // resume_start for seg 1: max(1000, 1000 - 16) = 1000
        assert_eq!(ranges[1].0, 1000);
        assert_eq!(pre, 0);
    }

    #[test]
    fn read_progress_rejects_bad_magic() {
        let dir = std::env::temp_dir();
        let path = dir.join("cmsdl_test_bad_magic.cmsdl");
        let mut buf = [0u8; FILE_SIZE];
        buf[0..5].copy_from_slice(b"XYZZY");
        std::fs::write(&path, &buf).unwrap();
        assert!(read_progress(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }
}
