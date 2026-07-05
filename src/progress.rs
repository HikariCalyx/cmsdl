//! Progress reporting abstraction shared between the console and GUI patchers.
//!
//! The patch/repair code calls the free functions here. When no reporter is
//! registered (the default, console mode) they are cheap no-ops and the
//! existing `indicatif` bars / `println!` output drive the UI. When the GUI
//! patcher registers a reporter, these calls feed the on-screen labels,
//! progress bar, and the `cmsdl_patcher.log` file instead.

use std::sync::{Arc, RwLock};

/// Sink for structured patch progress. All methods take `&self`; the
/// implementation is responsible for its own interior mutability and
/// thread-safety (download/repair report from worker threads).
pub trait Reporter: Send + Sync {
    /// A detailed log line (the "verbose procedure" for the log file).
    fn log(&self, line: &str);
    /// Scanning for updates (label2 = scanning, label1 cleared).
    fn scanning(&self);
    /// An update was found: installing from `current` to `target`.
    fn installing(&self, current: &str, target: &str);
    /// Begin downloading update package `index` of `count` (`total` bytes).
    fn begin_download(&self, index: usize, count: usize, total: u64);
    /// Report cumulative bytes downloaded for the current package.
    fn download_progress(&self, downloaded: u64);
    /// A package finished downloading and is now being extracted/applied.
    fn extracting(&self, index: usize, count: usize);
    /// Begin applying a package's files (resets the bar; `total` files).
    fn begin_apply(&self, total: usize);
    /// Report apply progress: `done` of `total` files, current `rel_path`.
    fn apply_progress(&self, done: usize, total: usize, rel_path: &str);
    /// Begin repairing corrupted files (`total` files, `total_bytes` total).
    fn begin_repair(&self, total: usize, total_bytes: u64);
    /// Report repair progress: `done` of `total` files, current `rel_path`,
    /// and cumulative `downloaded` bytes (for speed).
    fn repair_progress(&self, done: usize, total: usize, rel_path: &str, downloaded: u64);
    /// Purging stray WZ files not in the latest manifest (`--purge-wz-files`).
    fn purging(&self);
    /// Terminal state: show `msg` on label1; close the window when `close`.
    fn finish(&self, msg: &str, close: bool);
}

/// Sink for structured download progress, used by the GUI downloader.
///
/// Distinct from [`Reporter`] (which models the patch/repair flow), this trait
/// models the whole-client download flow: a fixed set of files whose combined
/// byte progress is tracked to a known total. All methods take `&self`; the
/// implementation handles its own interior mutability and thread-safety
/// (download workers report from many threads).
pub trait DownloadReporter: Send + Sync {
    /// A detailed log line (written to `cmsdl_downloader.log`).
    fn log(&self, line: &str);
    /// Begin a download session for `region` (e.g. `CMS`) at display `version`
    /// (e.g. `V226.1`), with `total_files` files totalling `total_bytes` bytes.
    fn begin(&self, region: &str, version: &str, total_files: usize, total_bytes: u64);
    /// Report cumulative progress: `files_done` files completed and
    /// `bytes_done` bytes transferred so far.
    fn update(&self, files_done: usize, bytes_done: u64);
    /// A single file finished downloading (recorded to the log with a timestamp).
    fn file_done(&self, rel_path: &str, size: u64);
    /// Purging stray files not in the manifest (`--purge-wz-files`), before the
    /// download begins.
    fn purging(&self);
    /// The download finished successfully; close the window when `close`.
    fn finish(&self, close: bool);
    /// The download failed; show `msg` and keep the window open.
    fn fail(&self, msg: &str);
}

static PATCH_REPORTER: RwLock<Option<Arc<dyn Reporter>>> = RwLock::new(None);
static DL_REPORTER: RwLock<Option<Arc<dyn DownloadReporter>>> = RwLock::new(None);

/// Register the active reporter (called once by the GUI patcher).
pub fn set_reporter(r: Arc<dyn Reporter>) {
    *PATCH_REPORTER.write().unwrap() = Some(r);
}

/// Register the active download reporter (called once by the GUI downloader).
pub fn set_download_reporter(r: Arc<dyn DownloadReporter>) {
    *DL_REPORTER.write().unwrap() = Some(r);
}

/// Whether a download reporter is currently registered (i.e. GUI download mode).
pub fn dl_active() -> bool {
    DL_REPORTER.read().unwrap().is_some()
}

fn with_dl(f: impl FnOnce(&dyn DownloadReporter)) {
    if let Some(r) = DL_REPORTER.read().unwrap().as_ref() {
        f(r.as_ref());
    }
}

pub fn dl_begin(region: &str, version: &str, total_files: usize, total_bytes: u64) {
    with_dl(|r| r.begin(region, version, total_files, total_bytes));
}
pub fn dl_update(files_done: usize, bytes_done: u64) {
    with_dl(|r| r.update(files_done, bytes_done));
}
pub fn dl_file_done(rel_path: &str, size: u64) {
    with_dl(|r| r.file_done(rel_path, size));
}
pub fn dl_purging() { with_dl(|r| r.purging()); }
pub fn dl_finish(close: bool) { with_dl(|r| r.finish(close)); }
pub fn dl_fail(msg: &str) { with_dl(|r| r.fail(msg)); }
/// Emit a detailed line to the download reporter's log, when one is registered.
pub fn dl_log(line: &str) { with_dl(|r| r.log(line)); }

/// Whether a reporter is currently registered (i.e. GUI mode).
pub fn active() -> bool {
    PATCH_REPORTER.read().unwrap().is_some()
}

fn with(f: impl FnOnce(&dyn Reporter)) {
    if let Some(r) = PATCH_REPORTER.read().unwrap().as_ref() {
        f(r.as_ref());
    }
}

/// Emit a detailed line: to the reporter's log in GUI mode, otherwise stdout.
/// Preserves the exact console output when no reporter is registered.
pub fn line(msg: &str) {
    let guard = PATCH_REPORTER.read().unwrap();
    match guard.as_ref() {
        Some(r) => r.log(msg),
        None => println!("{msg}"),
    }
}

pub fn scanning() { with(|r| r.scanning()); }
pub fn installing(current: &str, target: &str) { with(|r| r.installing(current, target)); }
pub fn begin_download(index: usize, count: usize, total: u64) {
    with(|r| r.begin_download(index, count, total));
}
pub fn download_progress(downloaded: u64) { with(|r| r.download_progress(downloaded)); }
pub fn extracting(index: usize, count: usize) { with(|r| r.extracting(index, count)); }
pub fn begin_apply(total: usize) { with(|r| r.begin_apply(total)); }
pub fn apply_progress(done: usize, total: usize, rel_path: &str) {
    with(|r| r.apply_progress(done, total, rel_path));
}
pub fn begin_repair(total: usize, total_bytes: u64) { with(|r| r.begin_repair(total, total_bytes)); }
pub fn repair_progress(done: usize, total: usize, rel_path: &str, downloaded: u64) {
    with(|r| r.repair_progress(done, total, rel_path, downloaded));
}
pub fn purging() { with(|r| r.purging()); }
pub fn finish(msg: &str, close: bool) { with(|r| r.finish(msg, close)); }

/// `plog!("...")` — a detailed procedure line (see [`line`]).
#[macro_export]
macro_rules! plog {
    ($($arg:tt)*) => { $crate::progress::line(&format!($($arg)*)) };
}

/// Format a byte-rate as a human-readable string like `1.2 MiB/s`.
pub fn format_speed(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KiB/s", "MiB/s", "GiB/s"];
    let mut v = bytes_per_sec;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

/// Format a byte count as a human-readable string like `1.23 GiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}
