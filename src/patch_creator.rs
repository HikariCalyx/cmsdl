//! Patch creator: builds a KMST1125-format binary patch from two client
//! versions.
//!
//! Files and directories excluded from the *old* client are listed in
//! [`OLD_CLIENT_EXCLUSIONS`]; those excluded from the *new* client are listed
//! in [`NEW_CLIENT_EXCLUSIONS`].
//!
//! The entry point is [`create_patch`].

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// Exclusion lists & matching
// ---------------------------------------------------------------------------

/// Glob-style patterns for paths that must be excluded from the *old* client
/// directory when building a patch.
const OLD_CLIENT_EXCLUSIONS: &[&str] = &[
    "*.avi",
    "*.lnk",
    "*.log",
    "*.txt",
    "*.webm",
    r"*.$$$\*",
    "106690*",
    "16785939*",
    "589825*",
    "589826*",
    r"blob_storage\*",
    "Checksums.md5",
    "cmsdl.exe",
    "Data.wz",
    "GameLauncher.exe",
    "GameLauncherT.exe",
    r"GPK\*",
    r"GPUCache\*",
    r"HShield\*",
    "langID",
    r"Launcher3Configs\*",
    "LocalVersion3.xml",
    "Maple*.jpg",
    "Maple*.png",
    "MapleStory.exe",
    "MapleStoryN.exe",
    "MapleStoryT.exe",
    "MapleStoryTA.exe",
    "NGM*.*",
    "obd-manifest*",
    "Patcher.exe",
    "Setup.exe",
    r"SDO\*",
    r"VideoDecodeStats\*",
    r"XignCode3\*",
];

/// Glob-style patterns for paths that must be excluded from the *new* client
/// directory when building a patch.
const NEW_CLIENT_EXCLUSIONS: &[&str] = &[
    "*.avi",
    "*.lnk",
    "*.log",
    "*.txt",
    "*.webm",
    r"blob_storage\*",
    "cmsdl.exe",
    "Data.wz",
    "GameLauncher.exe",
    "GameLauncherT.exe",
    r"GPK\*",
    r"GPUCache\*",
    r"HShield\*",
    "langID",
    r"Launcher3Configs\*",
    "Maple*.jpg",
    "Maple*.png",
    "NGM*.*",
    "obd-manifest*",
    "Patcher.exe",
    r"SDO\*",
    r"VideoDecodeStats\*",
];

/// Return `true` if `relative_path` (relative to the old client root) should
/// be excluded when building a patch.
pub fn is_excluded_from_old_client(relative_path: &Path) -> bool {
    is_excluded_by(OLD_CLIENT_EXCLUSIONS, relative_path)
}

/// Return `true` if `relative_path` (relative to the new client root) should
/// be excluded when building a patch.
pub fn is_excluded_from_new_client(relative_path: &Path) -> bool {
    is_excluded_by(NEW_CLIENT_EXCLUSIONS, relative_path)
}

fn is_excluded_by(exclusions: &[&str], relative_path: &Path) -> bool {
    let forward = relative_path.to_string_lossy().replace('\\', "/");
    let filename = relative_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    exclusions.iter().any(|pattern| {
        let pat = pattern.replace('\\', "/");
        if let Some(dir_prefix) = pat.strip_suffix("/*") {
            let prefix_lower = dir_prefix.to_lowercase();
            let path_lower = forward.to_lowercase();
            path_lower.starts_with(&format!("{prefix_lower}/"))
        } else if pat.contains('/') {
            glob_match(&pat, &forward)
        } else {
            glob_match(&pat, &filename)
        }
    })
}

fn glob_match(pattern: &str, s: &str) -> bool {
    glob_match_bytes(
        pattern.to_lowercase().as_bytes(),
        s.to_lowercase().as_bytes(),
    )
}

fn glob_match_bytes(pat: &[u8], s: &[u8]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (Some(&b'*'), _) => {
            if glob_match_bytes(&pat[1..], s) {
                return true;
            }
            if s.first() == Some(&b'/') {
                return false;
            }
            !s.is_empty() && glob_match_bytes(pat, &s[1..])
        }
        (None, _) | (_, None) => false,
        (Some(&p), Some(&c)) => p == c && glob_match_bytes(&pat[1..], &s[1..]),
    }
}

// ---------------------------------------------------------------------------
// CRC-32  (polynomial 0x04C11DB7, init 0, non-reflected)
//
// Matches WzComparerR2 CheckSum.
// ---------------------------------------------------------------------------

const CRC_POLY: u32 = 0x04C11DB7;
const CRC_TOPBIT: u32 = 0x8000_0000;

fn build_crc_table() -> [u32; 256 * 8] {
    let mut table = [0u32; 256 * 8];
    for i in 0u32..256 {
        let mut remain = i << 24;
        for _ in 0..8 {
            remain = if (remain & CRC_TOPBIT) != 0 {
                (remain << 1) ^ CRC_POLY
            } else {
                remain << 1
            };
        }
        table[i as usize] = remain;
    }
    let mut i = 256u32;
    while i < (256 * 8) as u32 {
        let r = table[(i - 256) as usize];
        table[i as usize] = table[(r >> 24) as usize] ^ (r << 8);
        i += 1;
    }
    table
}

fn crc_table() -> &'static [u32; 256 * 8] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256 * 8]> = OnceLock::new();
    TABLE.get_or_init(build_crc_table)
}

/// Streaming CRC-32 update.
pub fn crc32_update(mut crc: u32, buf: &[u8]) -> u32 {
    let table = crc_table();
    let mut data = buf;

    while data.len() >= 8 {
        crc ^= u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let idx = [
            ((crc >> 24) & 0xff) as usize,
            ((crc >> 16) & 0xff) as usize,
            ((crc >> 8) & 0xff) as usize,
            (crc & 0xff) as usize,
            data[4] as usize,
            data[5] as usize,
            data[6] as usize,
            data[7] as usize,
        ];
        crc = table[idx[0] + 0x700]
            ^ table[idx[1] + 0x600]
            ^ table[idx[2] + 0x500]
            ^ table[idx[3] + 0x400]
            ^ table[idx[4] + 0x300]
            ^ table[idx[5] + 0x200]
            ^ table[idx[6] + 0x100]
            ^ table[idx[7]];
        data = &data[8..];
    }
    for &b in data {
        crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

pub fn crc32(buf: &[u8]) -> u32 {
    crc32_update(0, buf)
}

pub fn crc32_file(path: &Path) -> Result<u32> {
    let mut file = File::open(path)?;
    let mut crc: u32 = 0;
    let mut buf = [0u8; 0x4000];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        crc = crc32_update(crc, &buf[..n]);
    }
    Ok(crc)
}

// ---------------------------------------------------------------------------
// Patch format constants
// ---------------------------------------------------------------------------

const PATCH_MAGIC: &[u8; 8] = b"WzPatch\x1A";
const PATCH_VERSION: i32 = 3;
const FOOTER_MAGIC: u32 = 0xf2f7fbf3;
const FOOTER_MAGIC64: u64 = FOOTER_MAGIC as u64; // zero-extended to 8 bytes

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EntryType {
    /// Type byte 0x00 — new file, embedded data follows.
    Create = 0,
    /// Type byte 0x01 — rebuild from old + instructions.
    Rebuild = 1,
    /// Type byte 0x02 — delete from old client.
    Delete = 2,
}

impl EntryType {
    /// Sort key for patch entries: Delete → Create → Rebuild.
    fn sort_order(self) -> u8 {
        match self {
            EntryType::Delete => 0,
            EntryType::Create => 1,
            EntryType::Rebuild => 2,
        }
    }
}

struct PatchEntry {
    path: String,
    entry_type: EntryType,
    new_full_path: Option<PathBuf>,
    old_full_path: Option<PathBuf>,
    new_crc: u32,
    old_crc: Option<u32>,
}

// ---------------------------------------------------------------------------
// Rebuild instruction diffing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum RebuildInst {
    FromPatch { len: usize, data: Vec<u8> },
    Fill { len: usize, byte: u8 },
    FromOld { len: usize, offset: usize, from_file: String },
}

/// Compare old & new bytes; emit rebuild instructions.
///
/// For old files ≤ 256 KiB uses an exact O(n·m) scan.  For larger files falls
/// back to a hash-indexed O(n) approach that is ~1000× faster on multi-GB WZ
/// files while still finding most matches in incremental patches.
///
/// `min_match` is the shortest run we will emit as `FromOld`.
/// `on_progress` is called with `(bytes_processed, total_bytes)`.
fn diff_files(
    old: &[u8],
    new: &[u8],
    from_file: &str,
    min_match: usize,
    on_progress: &dyn Fn(usize, usize),
) -> Vec<RebuildInst> {
    // Small files: exact byte-by-byte scan (fast enough for ≤ 256 KiB).
    if old.len() <= 256 * 1024 {
        return diff_files_exact(old, new, from_file, min_match, on_progress);
    }
    diff_files_hash(old, new, from_file, min_match, on_progress)
}

/// Exact O(n·m) diff — correct for all inputs, practical only for small files.
fn diff_files_exact(
    old: &[u8],
    new: &[u8],
    from_file: &str,
    min_match: usize,
    on_progress: &dyn Fn(usize, usize),
) -> Vec<RebuildInst> {
    let mut insts: Vec<RebuildInst> = Vec::new();
    let new_len = new.len();
    let old_len = old.len();
    let mut pos = 0usize;
    let mut last_reported = 0usize;
    let report_interval = (new_len / 200).max(64 * 1024).min(2 * 1024 * 1024);

    while pos < new_len {
        let mut best_len = 0usize;
        let mut best_off = 0usize;

        let search_limit = if old_len > 0 {
            pos.min(old_len.saturating_sub(1))
        } else {
            0
        };

        for off in 0..=search_limit {
            let max = (old_len - off).min(new_len - pos);
            let mut ml = 0;
            while ml < max && old[off + ml] == new[pos + ml] {
                ml += 1;
            }
            if ml > best_len {
                best_len = ml;
                best_off = off;
                if ml >= min_match {
                    break;
                }
            }
        }

        if best_len >= min_match {
            insts.push(RebuildInst::FromOld {
                len: best_len,
                offset: best_off,
                from_file: from_file.to_owned(),
            });
            pos += best_len;
        } else {
            let start = pos;
            while pos < new_len {
                let mut has_match = false;
                if old_len > 0 {
                    let search_limit2 = pos.min(old_len.saturating_sub(1));
                    for off in 0..=search_limit2 {
                        let max = (old_len - off).min(new_len - pos);
                        let mut ml = 0;
                        while ml < max && old[off + ml] == new[pos + ml] {
                            ml += 1;
                        }
                        if ml >= min_match {
                            has_match = true;
                            break;
                        }
                    }
                }
                if has_match || pos - start >= 64 * 1024 {
                    break;
                }
                pos += 1;
            }
            let data = new[start..pos].to_vec();
            if !data.is_empty() {
                insts.push(RebuildInst::FromPatch {
                    len: data.len(),
                    data,
                });
            }
        }
        if pos - last_reported >= report_interval {
            on_progress(pos, new_len);
            last_reported = pos;
        }
    }

    on_progress(new_len, new_len);
    coalesce_instructions(insts)
}

/// Hash-indexed diff — O(n) build + O(n) scan.  Stores the first occurrence
/// of each 8-byte prefix in `old`.  Fast on multi-GB files; may miss matches
/// when the same 8-byte prefix maps to a later, better-aligned position.
fn diff_files_hash(
    old: &[u8],
    new: &[u8],
    from_file: &str,
    min_match: usize,
    on_progress: &dyn Fn(usize, usize),
) -> Vec<RebuildInst> {
    let mut insts: Vec<RebuildInst> = Vec::new();
    let new_len = new.len();
    let old_len = old.len();
    let mut pos = 0usize;
    let mut last_reported = 0usize;
    let report_interval = (new_len / 200).max(64 * 1024).min(2 * 1024 * 1024);

    // ---- Phase 1: build hash index of the old file ----------------------

    let mut old_index: HashMap<u64, usize> = HashMap::with_capacity(old_len / 4);
    for i in 0..old_len.saturating_sub(7) {
        let key = u64::from_le_bytes(old[i..i + 8].try_into().unwrap());
        old_index.entry(key).or_insert(i);
    }

    // ---- Phase 2: scan new file, emit instructions -----------------------

    while pos < new_len {
        let mut found = false;

        // Try the hash index.
        if pos + 8 <= new_len {
            let key = u64::from_le_bytes(new[pos..pos + 8].try_into().unwrap());
            if let Some(&old_pos) = old_index.get(&key) {
                let max_extend = (old_len - old_pos).min(new_len - pos);
                let mut ml = 0;
                while ml < max_extend && old[old_pos + ml] == new[pos + ml] {
                    ml += 1;
                }
                if ml >= min_match {
                    insts.push(RebuildInst::FromOld {
                        len: ml,
                        offset: old_pos,
                        from_file: from_file.to_owned(),
                    });
                    pos += ml;
                    found = true;
                }
            }
        }

        // Trailing bytes: short local scan.
        if !found && new_len - pos < 8 && new_len - pos >= min_match {
            let remaining = new_len - pos;
            for off in 0..old_len.saturating_sub(min_match) {
                let max = (old_len - off).min(remaining);
                let mut ml = 0;
                while ml < max && old[off + ml] == new[pos + ml] {
                    ml += 1;
                }
                if ml >= min_match {
                    insts.push(RebuildInst::FromOld {
                        len: ml,
                        offset: off,
                        from_file: from_file.to_owned(),
                    });
                    pos += ml;
                    found = true;
                    break;
                }
            }
        }

        if !found {
            // Collect non-matching bytes.
            let start = pos;
            pos += 1;
            while pos < new_len && pos - start < 64 * 1024 {
                if pos + 8 <= new_len {
                    let key = u64::from_le_bytes(new[pos..pos + 8].try_into().unwrap());
                    if let Some(&old_pos) = old_index.get(&key) {
                        let max_extend = (old_len - old_pos).min(new_len - pos);
                        let mut ml = 0;
                        while ml < max_extend && old[old_pos + ml] == new[pos + ml] {
                            ml += 1;
                        }
                        if ml >= min_match {
                            break;
                        }
                    }
                }
                pos += 1;
            }
            let data = new[start..pos].to_vec();
            if !data.is_empty() {
                insts.push(RebuildInst::FromPatch {
                    len: data.len(),
                    data,
                });
            }
        }

        if pos - last_reported >= report_interval {
            on_progress(pos, new_len);
            last_reported = pos;
        }
    }

    on_progress(new_len, new_len);
    coalesce_instructions(insts)
}

/// Merge adjacent instructions of the same kind:
/// * Consecutive `FromPatch` instructions are joined.
/// * Consecutive `Fill` instructions with the same byte are joined.
fn coalesce_instructions(insts: Vec<RebuildInst>) -> Vec<RebuildInst> {
    let mut merged: Vec<RebuildInst> = Vec::with_capacity(insts.len());
    for inst in insts {
        let should_merge = match (&inst, merged.last()) {
            (
                RebuildInst::Fill { byte: b1, .. },
                Some(RebuildInst::Fill { byte: b2, .. }),
            ) => b1 == b2,
            (
                RebuildInst::FromPatch { .. },
                Some(RebuildInst::FromPatch { .. }),
            ) => true,
            _ => false,
        };
        if should_merge {
            match (&inst, merged.last_mut()) {
                (
                    RebuildInst::Fill { len: l1, .. },
                    Some(RebuildInst::Fill { len: l2, .. }),
                ) => {
                    *l2 += l1;
                }
                (
                    RebuildInst::FromPatch { data: d1, .. },
                    Some(RebuildInst::FromPatch {
                        data: d2, len: l2, ..
                    }),
                ) => {
                    d2.extend_from_slice(d1);
                    *l2 += d1.len();
                }
                _ => unreachable!(),
            }
        } else {
            merged.push(inst);
        }
    }
    merged
}

// ---------------------------------------------------------------------------
// Directory scanning
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileInfo {
    rel: String,
    crc: u32,
}

/// Walk `root`, collecting every non-excluded file as `(rel_path, abs_path)`.
fn collect_paths(
    root: &Path,
    is_excluded: fn(&Path) -> bool,
) -> Result<Vec<(String, PathBuf)>> {
    let mut paths = Vec::new();
    collect_paths_impl(root, root, &mut paths, is_excluded)?;
    Ok(paths)
}

fn collect_paths_impl(
    root: &Path,
    current: &Path,
    paths: &mut Vec<(String, PathBuf)>,
    is_excluded: fn(&Path) -> bool,
) -> Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('/', "\\");

        if is_excluded(Path::new(&rel)) {
            continue;
        }

        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_paths_impl(root, &path, paths, is_excluded)?;
        } else if ft.is_file() {
            paths.push((rel, path));
        }
    }
    Ok(())
}

fn scan_client(
    root: &Path,
    label: &str,
    is_excluded: fn(&Path) -> bool,
) -> Result<BTreeMap<String, FileInfo>> {
    // Phase 1: collect paths.
    let spinner = ProgressBar::new_spinner()
        .with_style(
            ProgressStyle::with_template("{spinner:.green} {msg}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
    spinner.enable_steady_tick(Duration::from_millis(80));
    spinner.set_message(format!("{label}: enumerating..."));

    let paths = collect_paths(root, is_excluded)?;
    let total = paths.len();
    spinner.set_message(format!("{label}: checksumming {total} files..."));

    // Phase 2: CRC-32 in parallel.
    let results: Arc<Mutex<BTreeMap<String, FileInfo>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let counter = AtomicUsize::new(0);

    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(total.max(1));

    std::thread::scope(|s| {
        let chunk_size = ((total + num_threads - 1) / num_threads).max(1);
        for chunk in paths.chunks(chunk_size) {
            let results = Arc::clone(&results);
            let counter = &counter;
            let spinner = &spinner;
            s.spawn(move || {
                for (rel, abs) in chunk {
                    let crc = match crc32_file(abs) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    {
                        let mut map = results.lock().unwrap();
                        map.insert(rel.to_lowercase(), FileInfo {
                            rel: rel.clone(),
                            crc,
                        });
                    }
                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 16 == 0 {
                        spinner.set_message(format!(
                            "checksumming… ({n}/{total} files)"
                        ));
                    }
                }
            });
        }
    });

    spinner.finish_and_clear();
    let map = results.lock().unwrap().clone();
    eprintln!("  {label}: found {} files (after exclusions)", map.len());
    Ok(map)
}

// ---------------------------------------------------------------------------
// Patch file writer
// ---------------------------------------------------------------------------

/// Pre-computed diff for a single Rebuild entry.
struct RebuildDiff {
    insts: Vec<RebuildInst>,
}

fn write_patch(out_path: &Path, entries: &[PatchEntry]) -> Result<()> {
    let total = entries.len();
    let width = total.to_string().len();

    // ---- Phase 1: pre-compute rebuild diffs in parallel -----------------

    let mut rebuild_indices: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.entry_type == EntryType::Rebuild)
        .map(|(i, _)| i)
        .collect();

    // Sort by file size ascending — small files finish first, keeping all
    // threads busy and giving the user visible progress sooner.
    rebuild_indices.sort_by_key(|&i| {
        entries[i]
            .new_full_path
            .as_deref()
            .and_then(|p| fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0)
    });

    let rebuild_count = rebuild_indices.len();
    let diffs: Arc<Mutex<BTreeMap<usize, RebuildDiff>>> =
        Arc::new(Mutex::new(BTreeMap::new()));

    if rebuild_count > 0 {
        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(rebuild_count.max(1));

        let mp = MultiProgress::new();

        // Overall counter.
        let total_bar = mp.add(
            ProgressBar::new(rebuild_count as u64)
                .with_style(
                    ProgressStyle::with_template(
                        "  {spinner:.green} [{bar:30.cyan/blue}] {pos}/{len} files"
                    )
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
                    .progress_chars("=>-"),
                ),
        );
        total_bar.enable_steady_tick(Duration::from_millis(80));

        // Per-thread bars (one per worker thread).
        let thread_bars: Vec<ProgressBar> = (0..num_threads)
            .map(|i| {
                mp.add(
                    ProgressBar::new(100)
                        .with_style(
                            ProgressStyle::with_template(&format!(
                                "  {{prefix:>2}} {{msg:.<60}} [{{bar:20.cyan/blue}}] {{percent}}%"
                            ))
                            .unwrap()
                            .progress_chars("=>-"),
                        )
                        .with_prefix(format!("T{i}")),
                )
            })
            .collect();

        let thread_bars = Arc::new(thread_bars);

        // Shared work cursor — each thread atomically grabs the next job.
        let rebuild_indices: Arc<[usize]> = rebuild_indices.into();
        let cursor = AtomicUsize::new(0);

        std::thread::scope(|s| {
            for thread_idx in 0..num_threads {
                let diffs = Arc::clone(&diffs);
                let total_bar = &total_bar;
                let thread_bars = Arc::clone(&thread_bars);
                let rebuild_indices = Arc::clone(&rebuild_indices);
                let cursor = &cursor;
                let entries_ptr = entries.as_ptr();
                let entries_len = entries.len();
                let entries_ref: &[PatchEntry] =
                    unsafe { std::slice::from_raw_parts(entries_ptr, entries_len) };
                s.spawn(move || {
                    let my_bar = &thread_bars[thread_idx % thread_bars.len()];
                    loop {
                        let i = cursor.fetch_add(1, Ordering::Relaxed);
                        if i >= rebuild_indices.len() {
                            break;
                        }
                        let idx = rebuild_indices[i];
                        let entry = &entries_ref[idx];
                        let short = shorten_path(&entry.path, 58);
                        my_bar.set_message(short.clone());
                        my_bar.set_position(0);

                        let old_data = match fs::read(entry.old_full_path.as_deref().unwrap()) {
                            Ok(d) => d,
                            Err(_) => {
                                my_bar.set_message(format!("{short} (read err)"));
                                total_bar.inc(1);
                                continue;
                            }
                        };
                        let new_data = match fs::read(entry.new_full_path.as_deref().unwrap()) {
                            Ok(d) => d,
                            Err(_) => {
                                my_bar.set_message(format!("{short} (read err)"));
                                total_bar.inc(1);
                                continue;
                            }
                        };

                        let insts = diff_files(
                            &old_data,
                            &new_data,
                            &entry.path,
                            16,
                            &|done, total| {
                                if total > 0 {
                                    my_bar.set_position((done * 100 / total) as u64);
                                    my_bar.set_message(short.clone());
                                }
                            },
                        );

                        diffs.lock().unwrap().insert(idx, RebuildDiff { insts });
                        total_bar.inc(1);
                    }
                    my_bar.finish_with_message("done");
                });
            }
        });

        mp.clear().ok();
    }

    let diffs = Arc::try_unwrap(diffs)
        .unwrap_or_else(|_| unreachable!())
        .into_inner()
        .unwrap();

    // ---- Phase 2: build uncompressed data -------------------------------

    let mut uncompressed: Vec<u8> = Vec::new();

    // KMST1125 file-hash list.
    let rebuild_entries: Vec<&PatchEntry> = entries
        .iter()
        .filter(|e| e.entry_type == EntryType::Rebuild && e.old_crc.is_some())
        .collect();

    uncompressed.extend_from_slice(&(rebuild_entries.len() as i32).to_le_bytes());
    for entry in &rebuild_entries {
        write_prefixed_string(&mut uncompressed, &entry.path);
        uncompressed.extend_from_slice(&entry.old_crc.unwrap().to_le_bytes());
    }

    // Patch entries.
    for (i, entry) in entries.iter().enumerate() {
        let (label, extra) = match entry.entry_type {
            EntryType::Create => {
                let sz = entry
                    .new_full_path
                    .as_deref()
                    .and_then(|p| fs::metadata(p).ok())
                    .map(|m| format_size(m.len()));
                ("Create ", sz)
            }
            EntryType::Rebuild => {
                let sz = entry
                    .new_full_path
                    .as_deref()
                    .and_then(|p| fs::metadata(p).ok())
                    .map(|m| format_size(m.len()));
                ("Rebuild", sz)
            }
            EntryType::Delete => ("Delete ", None),
        };
        match extra {
            Some(sz) => eprintln!("  [{:>width$}/{total}] {label} {sz:>8}  {}", i + 1, entry.path),
            None => eprintln!("  [{:>width$}/{total}] {label}          {}", i + 1, entry.path),
        }
        match entry.entry_type {
            EntryType::Create => write_create_entry(&mut uncompressed, entry)?,
            EntryType::Rebuild => {
                if let Some(diff) = diffs.get(&i) {
                    write_rebuild_entry_from_diff(
                        &mut uncompressed,
                        entry,
                        &diff.insts,
                    );
                }
            }
            EntryType::Delete => write_delete_entry(&mut uncompressed, entry)?,
        }
    }

    // ---- Phase 3: zlib compress -----------------------------------------
    //
    // Single-threaded by necessity: the reference patcher (InflateStream)
    // wraps .NET DeflateStream which reads a single contiguous raw-deflate
    // stream.  Concatenating parallel chunks would require a zlib header
    // per chunk and the patcher only strips the first one.

    use flate2::write::ZlibEncoder;
    use flate2::Compression;

    let mut compressed = Vec::new();
    {
        let mut enc = ZlibEncoder::new(&mut compressed, Compression::default());
        enc.write_all(&uncompressed)?;
        enc.finish()?;
    }

    // ---- Phase 4: patch block -------------------------------------------

    let mut patch_block: Vec<u8> = Vec::new();
    patch_block.extend_from_slice(PATCH_MAGIC);
    patch_block.extend_from_slice(&PATCH_VERSION.to_le_bytes());

    let comp_crc = crc32(&compressed);
    patch_block.extend_from_slice(&comp_crc.to_le_bytes());
    patch_block.extend_from_slice(&compressed);

    // ---- Phase 5: final file with 64-bit footer -------------------------
    //
    // The reference patcher's TrySplit checks for a 64-bit footer when the
    // file does NOT start with "MZ".  A 32-bit footer is only used inside
    // self-extracting ("MZ") patches.

    let notice = b"Created by cmsdl patch creator\r\n";
    let patch_block_len = patch_block.len() as i64;
    let notice_len = notice.len() as i64;

    let mut file = BufWriter::new(
        File::create(out_path)
            .with_context(|| format!("cannot create {}", out_path.display()))?,
    );
    file.write_all(&patch_block)?;
    file.write_all(notice)?;
    file.write_all(&patch_block_len.to_le_bytes())?;
    file.write_all(&notice_len.to_le_bytes())?;
    file.write_all(&FOOTER_MAGIC64.to_le_bytes())?;
    file.flush()?;

    Ok(())
}

// ---- Entry writers -----------------------------------------------------

fn write_prefixed_string(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    buf.extend_from_slice(&(b.len() as i32).to_le_bytes());
    buf.extend_from_slice(b);
}

fn has_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map_or(false, |e| !e.is_empty())
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Truncate a path to at most `max` chars, keeping the filename intact.
fn shorten_path(path: &str, max: usize) -> String {
    if path.len() <= max {
        return path.to_string();
    }
    let filename = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_else(|| path.into());
    let keep = filename.len() + 3; // "…\\" + filename
    if keep >= max {
        let trunc = max.saturating_sub(3);
        if trunc > 0 {
            format!("…{}", &filename[filename.len() - trunc..])
        } else {
            filename[..max.min(filename.len())].to_string()
        }
    } else {
        format!("…\\{filename}")
    }
}

fn write_create_entry(buf: &mut Vec<u8>, entry: &PatchEntry) -> Result<()> {
    buf.extend_from_slice(entry.path.as_bytes());
    buf.push(EntryType::Create as u8);

    if !has_extension(&entry.path) {
        return Ok(());
    }

    let data = fs::read(entry.new_full_path.as_deref().unwrap())
        .with_context(|| format!("cannot read new {}", entry.path))?;
    buf.extend_from_slice(&(data.len() as i32).to_le_bytes());
    buf.extend_from_slice(&entry.new_crc.to_le_bytes());
    buf.extend_from_slice(&data);
    Ok(())
}

fn write_rebuild_entry_from_diff(
    buf: &mut Vec<u8>,
    entry: &PatchEntry,
    insts: &[RebuildInst],
) {
    buf.extend_from_slice(entry.path.as_bytes());
    buf.push(EntryType::Rebuild as u8);
    buf.extend_from_slice(&entry.new_crc.to_le_bytes());

    for inst in insts {
        match inst {
            RebuildInst::FromPatch { len, data } => {
                let cmd = 0x8000_0000u32 | (*len as u32 & 0x0FFF_FFFF);
                buf.extend_from_slice(&cmd.to_le_bytes());
                buf.extend_from_slice(data);
            }
            RebuildInst::Fill { len, byte } => {
                let cmd =
                    0xC000_0000u32 | ((*len as u32 & 0x0F_FFFF) << 8) | (*byte as u32);
                buf.extend_from_slice(&cmd.to_le_bytes());
            }
            RebuildInst::FromOld {
                len,
                offset,
                from_file,
            } => {
                let cmd = *len as u32;
                buf.extend_from_slice(&cmd.to_le_bytes());
                buf.extend_from_slice(&(*offset as i32).to_le_bytes());
                write_prefixed_string(buf, from_file);
            }
        }
    }
    buf.extend_from_slice(&0i32.to_le_bytes());
}

fn write_delete_entry(buf: &mut Vec<u8>, entry: &PatchEntry) -> Result<()> {
    buf.extend_from_slice(entry.path.as_bytes());
    buf.push(EntryType::Delete as u8);
    Ok(())
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Build a KMST1125 patch from `old_dir` and `new_dir`, writing the result
/// to `out_file`.  Files whose checksum is unchanged between old and new are
/// silently omitted.
pub fn create_patch(old_dir: &Path, new_dir: &Path, out_file: &Path) -> Result<()> {
    println!(
        "cmsdl {VERSION}: creating patch {} from {} to {}.",
        out_file.display(),
        old_dir.display(),
        new_dir.display()
    );

    // ---- Phase 1: scan old client ---------------------------------------
    eprintln!("Scanning old client: {}", old_dir.display());
    let old_files = scan_client(old_dir, "old", is_excluded_from_old_client)?;

    // ---- Phase 2: scan new client ---------------------------------------
    eprintln!("Scanning new client: {}", new_dir.display());
    let new_files = scan_client(new_dir, "new", is_excluded_from_new_client)?;

    // ---- Phase 3: compare -----------------------------------------------
    eprintln!("Comparing files...");

    let mut entries: Vec<PatchEntry> = Vec::new();
    let mut unchanged = 0usize;

    // Files present in new client.
    for (key_lower, new_info) in &new_files {
        if let Some(old_info) = old_files.get(key_lower) {
            if old_info.crc == new_info.crc {
                unchanged += 1;
                continue;
            }
            entries.push(PatchEntry {
                path: new_info.rel.clone(),
                entry_type: EntryType::Rebuild,
                new_full_path: Some(new_dir.join(&new_info.rel)),
                old_full_path: Some(old_dir.join(&old_info.rel)),
                new_crc: new_info.crc,
                old_crc: Some(old_info.crc),
            });
        } else {
            entries.push(PatchEntry {
                path: new_info.rel.clone(),
                entry_type: EntryType::Create,
                new_full_path: Some(new_dir.join(&new_info.rel)),
                old_full_path: None,
                new_crc: new_info.crc,
                old_crc: None,
            });
        }
    }

    // Files only in old client → Delete.
    for (key_lower, old_info) in &old_files {
        if !new_files.contains_key(key_lower) {
            entries.push(PatchEntry {
                path: old_info.rel.clone(),
                entry_type: EntryType::Delete,
                new_full_path: None,
                old_full_path: Some(old_dir.join(&old_info.rel)),
                new_crc: 0,
                old_crc: Some(old_info.crc),
            });
        }
    }

    let n_create = entries.iter().filter(|e| e.entry_type == EntryType::Create).count();
    let n_rebuild = entries.iter().filter(|e| e.entry_type == EntryType::Rebuild).count();
    let n_delete = entries.iter().filter(|e| e.entry_type == EntryType::Delete).count();
    let n_total = entries.len();

    eprintln!(
        "  Create: {n_create}  Rebuild: {n_rebuild}  Delete: {n_delete}  \
         Unchanged: {unchanged}  → {n_total} patch entries"
    );

    // Sort: Delete → Create → Rebuild, then by path.
    entries.sort_by(|a, b| {
        a.entry_type
            .sort_order()
            .cmp(&b.entry_type.sort_order())
            .then_with(|| a.path.cmp(&b.path))
    });

    // ---- Phase 4: write patch file --------------------------------------
    eprintln!("Writing patch to {}", out_file.display());
    write_patch(out_file, &entries)?;

    eprintln!("Done.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- exclusion helpers -----------------------------------------------

    fn old_excluded(p: &str) -> bool {
        is_excluded_from_old_client(Path::new(p))
    }
    fn new_excluded(p: &str) -> bool {
        is_excluded_from_new_client(Path::new(p))
    }

    // -- old client ------------------------------------------------------

    #[test]
    fn old_exact_matches() {
        for p in [
            "cmsdl.exe", "Data.wz", "Checksums.md5", "langID",
            "LocalVersion3.xml", "MapleStory.exe", "MapleStoryN.exe",
            "MapleStoryT.exe", "MapleStoryTA.exe", "Patcher.exe",
            "GameLauncher.exe", "GameLauncherT.exe",
        ] {
            assert!(old_excluded(p), "{p} should be excluded");
        }
    }

    #[test]
    fn old_prefix_wildcards() {
        assert!(old_excluded("106690_something"));
        assert!(old_excluded("589825data"));
        assert!(old_excluded("589826data"));
        assert!(old_excluded("16785939file"));
        assert!(old_excluded("obd-manifest.xml"));
        assert!(old_excluded("obd-manifest"));
    }

    #[test]
    fn old_extension_wildcards() {
        for p in ["movie.avi", "shortcut.lnk", "output.log", "readme.txt", "clip.webm"] {
            assert!(old_excluded(p), "{p}");
        }
    }

    #[test]
    fn old_mixed_wildcards() {
        for p in [
            "MapleTitle.jpg", "MapleBackground.png",
            "NGMUnity.dll", "NGMPatcher.exe",
        ] {
            assert!(old_excluded(p), "{p}");
        }
    }

    #[test]
    fn old_directory_exclusions() {
        for p in [
            r"blob_storage\somefile.dat",
            r"GPK\somefile",
            r"GPUCache\f_000",
            r"VideoDecodeStats\stats.json",
            r"Launcher3Configs\config.xml",
            r"SDO\data.bin",
        ] {
            assert!(old_excluded(p), "{p}");
        }
    }

    #[test]
    fn old_not_excluded() {
        for p in [
            "mxd/Base.wz", "mxd/Character.wz", r"mxd\Data\monster.wz", "client.exe",
        ] {
            assert!(!old_excluded(p), "{p}");
        }
    }

    #[test]
    fn old_case_insensitive() {
        for p in ["CMSDL.EXE", "data.WZ", "MOVIE.AVI", r"GPK\FILE"] {
            assert!(old_excluded(p), "{p}");
        }
    }

    #[test]
    fn old_wildcards_in_subdirs() {
        for p in [
            "subdir/movie.avi", r"subdir\shortcut.lnk", "deep/nested/output.log",
            "subdir/readme.txt", "subdir/clip.webm",
            "subdir/NGMUnity.dll", "subdir/MapleTitle.jpg",
            "subdir/MapleBackground.png", "subdir/obd-manifest.xml",
        ] {
            assert!(old_excluded(p), "{p}");
        }
    }

    // -- new client ------------------------------------------------------

    #[test]
    fn new_exact_matches() {
        for p in [
            "cmsdl.exe", "Data.wz", "langID", "Patcher.exe",
            "GameLauncher.exe", "GameLauncherT.exe",
        ] {
            assert!(new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_extension_wildcards() {
        for p in ["movie.avi", "shortcut.lnk", "output.log", "readme.txt", "clip.webm"] {
            assert!(new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_mixed_wildcards() {
        for p in [
            "MapleTitle.jpg", "MapleBackground.png",
            "NGMUnity.dll", "NGMPatcher.exe", "obd-manifest.xml",
        ] {
            assert!(new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_directory_exclusions() {
        for p in [
            r"blob_storage\somefile.dat", r"GPK\somefile", r"GPUCache\f_000",
            r"VideoDecodeStats\stats.json", r"Launcher3Configs\config.xml",
            r"SDO\data.bin",
        ] {
            assert!(new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_not_excluded() {
        for p in [
            "mxd/Base.wz", "mxd/Character.wz", r"mxd\Data\monster.wz",
            "client.exe", "Checksums.md5", "LocalVersion3.xml",
            "MapleStory.exe", "106690_something",
        ] {
            assert!(!new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_case_insensitive() {
        for p in ["CMSDL.EXE", "data.WZ", "MOVIE.AVI", r"GPK\FILE"] {
            assert!(new_excluded(p), "{p}");
        }
    }

    #[test]
    fn new_wildcards_in_subdirs() {
        for p in [
            "subdir/movie.avi", r"subdir\shortcut.lnk", "deep/nested/output.log",
            "subdir/readme.txt", "subdir/clip.webm",
            "subdir/NGMUnity.dll", "subdir/MapleTitle.jpg",
            "subdir/MapleBackground.png", "subdir/obd-manifest.xml",
        ] {
            assert!(new_excluded(p), "{p}");
        }
    }

    // -- CRC-32 ----------------------------------------------------------

    #[test]
    fn crc32_empty() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn crc32_consistent() {
        let a = crc32(b"hello");
        let b = crc32(b"hello");
        assert_eq!(a, b);
        assert_ne!(a, crc32(b"world"));
    }

    #[test]
    fn crc32_streaming() {
        let data = b"The quick brown fox jumps over the lazy dog";
        let full = crc32(data);
        let mut crc = 0u32;
        crc = crc32_update(crc, &data[..10]);
        crc = crc32_update(crc, &data[10..]);
        assert_eq!(full, crc);
    }

    // -- diff ------------------------------------------------------------

    #[test]
    fn diff_identical() {
        let data = b"abcdefghijklmnop";
        let insts = diff_files(data, data, "test.bin", 4, &|_, _| {});
        assert_eq!(insts.len(), 1, "identical files → single FromOld");
        match &insts[0] {
            RebuildInst::FromOld { len, offset, from_file } => {
                assert_eq!(*len, 16);
                assert_eq!(*offset, 0);
                assert_eq!(from_file, "test.bin");
            }
            _ => panic!("expected FromOld"),
        }
    }

    #[test]
    fn diff_different() {
        let old = b"abcdefghijklmnop";
        let new = b"1234567890ABCDEF";
        let insts = diff_files(old, new, "test.bin", 4, &|_, _| {});
        let patch_bytes: usize = insts
            .iter()
            .filter_map(|i| match i {
                RebuildInst::FromPatch { len, .. } => Some(*len),
                _ => None,
            })
            .sum();
        assert_eq!(patch_bytes, 16, "all new bytes go into FromPatch");
    }

    #[test]
    fn diff_mixed() {
        let old = b"AAAA BBBB CCCC";
        let new = b"AAAA XXXX CCCC";
        let insts = diff_files(old, new, "test.bin", 4, &|_, _| {});
        // FromOld(5) + FromPatch(4) + FromOld(5)
        assert_eq!(insts.len(), 3);
        assert!(matches!(insts[0], RebuildInst::FromOld { len: 5, .. }));
        assert!(matches!(insts[1], RebuildInst::FromPatch { len: 4, .. }));
        assert!(matches!(insts[2], RebuildInst::FromOld { len: 5, .. }));
    }

    #[test]
    fn diff_prepend() {
        let old = b"world";
        let new = b"hello world";
        let insts = diff_files(old, new, "test.bin", 4, &|_, _| {});
        // FromPatch("hello ") + FromOld("world")
        assert_eq!(insts.len(), 2);
    }

    #[test]
    fn diff_truncate() {
        let old = b"hello world";
        let new = b"hello";
        let insts = diff_files(old, new, "test.bin", 4, &|_, _| {});
        // FromOld("hello")
        assert_eq!(insts.len(), 1);
        assert!(matches!(insts[0], RebuildInst::FromOld { len: 5, .. }));
    }

    // -- round-trip ------------------------------------------------------

    #[test]
    fn round_trip() -> Result<()> {
        let tmp = std::env::temp_dir().join("cmsdl_patch_creator_test");
        let old_dir = tmp.join("old");
        let new_dir = tmp.join("new");
        let patch_file = tmp.join("test.patch");

        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&old_dir)?;
        fs::create_dir_all(&new_dir)?;

        fs::write(old_dir.join("same.txt"), b"unchanged")?;
        fs::write(new_dir.join("same.txt"), b"unchanged")?;

        fs::write(old_dir.join("mod.bin"), b"old data here 1234")?;
        fs::write(new_dir.join("mod.bin"), b"new data here 1234")?;

        fs::write(old_dir.join("del.dat"), b"to delete")?;
        fs::write(new_dir.join("add.dat"), b"brand new")?;

        create_patch(&old_dir, &new_dir, &patch_file)?;
        assert!(patch_file.exists());
        assert!(fs::metadata(&patch_file)?.len() > 0);

        let _ = fs::remove_dir_all(&tmp);
        Ok(())
    }
}
