//! GUI front-end for `cmsdl cms --download <dir>` and `cmsdl tms --download <dir>`.
//!
//! Runs the client download on a background thread while a layered window shows
//! progress (see [`crate::gui`]). The list of downloaded files (with
//! timestamps) is written to `<target>/cmsdl_downloader.log`. Falls back to the
//! console downloader when the GUI is unavailable (non-Windows or `--no-gui`).
//!
//! Window labels while downloading:
//!   * label2 (top):    `gui-downloader-client-version` — region, display
//!                      version, and the current transfer speed.
//!   * label1 (bottom): `gui-downloader-downloading` — files done, total
//!                      files, bytes done, and total bytes.
//! When the download finishes and `--close-after-finishing` was not given,
//! label2 shows `gui-downloader-complete`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use indicatif::ProgressBar;

use crate::cli::Region;
use crate::filter::FileFilter;
use crate::gui::{self, UiModel};
use crate::locale::tr;
use crate::progress::{self, format_size, format_speed, DownloadReporter};

/// Rolling transfer-speed estimator, updated from cumulative byte counts.
struct SpeedState {
    last: Instant,
    last_bytes: u64,
    speed: f64,
}

impl SpeedState {
    fn reset() -> Self {
        SpeedState { last: Instant::now(), last_bytes: 0, speed: 0.0 }
    }

    /// Update with the latest cumulative byte count; returns the current speed.
    fn tick(&mut self, downloaded: u64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64();
        if dt >= 0.3 {
            let db = downloaded.saturating_sub(self.last_bytes) as f64;
            self.speed = db / dt;
            self.last = now;
            self.last_bytes = downloaded;
        }
        self.speed
    }
}

/// Buffered log writer for `cmsdl_downloader.log`.
struct LogState {
    file: Option<File>,
    buffer: Vec<String>,
    path: PathBuf,
}

/// The GUI download reporter: maps download progress onto the shared
/// [`UiModel`] and records each completed file to the log file.
pub struct GuiDownloadReporter {
    ui: Arc<Mutex<UiModel>>,
    log: Mutex<LogState>,
    region: Mutex<String>,
    version: Mutex<String>,
    total_files: AtomicUsize,
    total_bytes: AtomicU64,
    speed: Mutex<SpeedState>,
    /// Set once the session is over, so a late progress tick cannot overwrite
    /// the terminal "complete"/error label.
    done: AtomicBool,
}

impl GuiDownloadReporter {
    fn new(ui: Arc<Mutex<UiModel>>, log_path: PathBuf) -> Self {
        GuiDownloadReporter {
            ui,
            log: Mutex::new(LogState { file: None, buffer: Vec::new(), path: log_path }),
            region: Mutex::new(String::new()),
            version: Mutex::new(String::new()),
            total_files: AtomicUsize::new(0),
            total_bytes: AtomicU64::new(0),
            speed: Mutex::new(SpeedState::reset()),
            done: AtomicBool::new(false),
        }
    }

    fn set_label1(&self, text: String) {
        if let Ok(mut m) = self.ui.lock() {
            m.label1 = text;
        }
    }

    fn set_label2(&self, text: String) {
        if let Ok(mut m) = self.ui.lock() {
            m.label2 = text;
        }
    }

    fn set_progress(&self, ratio: f32) {
        if let Ok(mut m) = self.ui.lock() {
            m.progress = ratio.clamp(0.0, 1.0);
        }
    }

    /// Open the log file (append) and flush any buffered lines. A timestamped
    /// header separates successive runs.
    fn open_log(&self) {
        let mut st = self.log.lock().unwrap();
        if st.file.is_some() {
            return;
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&st.path) {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "\n===== cmsdl download session {ts} =====");
            for line in st.buffer.drain(..) {
                let _ = writeln!(f, "{line}");
            }
            st.file = Some(f);
        }
    }
}

impl DownloadReporter for GuiDownloadReporter {
    fn log(&self, line: &str) {
        let mut st = self.log.lock().unwrap();
        match st.file.as_mut() {
            Some(f) => { let _ = writeln!(f, "{line}"); }
            None => st.buffer.push(line.to_string()),
        }
    }

    fn scanning(&self) {
        // Shown while the latest build is being discovered, before the file
        // list (and therefore the totals) is known.
        self.set_label2(tr("gui-patcher-scanning-update", &[]));
        self.set_label1(String::new());
        self.set_progress(0.0);
    }

    fn begin(&self, region: &str, version: &str, total_files: usize, total_bytes: u64) {
        *self.region.lock().unwrap() = region.to_string();
        *self.version.lock().unwrap() = version.to_string();
        self.total_files.store(total_files, Ordering::Relaxed);
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
        *self.speed.lock().unwrap() = SpeedState::reset();
        self.open_log();
        crate::progress::dl_log(&format!(
            "downloading {region} {version}: {total_files} file(s), {} total",
            format_size(total_bytes)
        ));
        // Prime the labels immediately so the window is not blank while the
        // first bytes are still in flight.
        self.update(0, 0);
    }

    fn update(&self, files_done: usize, bytes_done: u64) {
        if self.done.load(Ordering::Relaxed) {
            return;
        }
        let total_files = self.total_files.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);
        let speed = self.speed.lock().unwrap().tick(bytes_done);

        let region = self.region.lock().unwrap().clone();
        let version = self.version.lock().unwrap().clone();

        self.set_label2(tr(
            "gui-downloader-client-version",
            &[&region, &version, &format_speed(speed)],
        ));
        self.set_label1(tr(
            "gui-downloader-downloading",
            &[
                &files_done.to_string(),
                &total_files.to_string(),
                &format_size(bytes_done),
                &format_size(total_bytes),
            ],
        ));
        if total_bytes > 0 {
            self.set_progress(bytes_done as f32 / total_bytes as f32);
        }
    }

    fn file_done(&self, rel_path: &str, size: u64) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        self.log(&format!("[{ts}] {rel_path} ({})", format_size(size)));
    }

    fn purging(&self) {
        // Shown before the download begins, while stray files are removed.
        self.set_label1(tr("gui-patcher-purge", &[]));
        self.set_progress(0.0);
    }

    fn finish(&self, close: bool) {
        self.done.store(true, Ordering::Relaxed);
        self.set_progress(1.0);
        crate::progress::dl_log("download complete.");
        if close {
            if let Ok(mut m) = self.ui.lock() {
                m.should_close = true;
            }
        } else {
            self.set_label2(tr("gui-downloader-complete", &[]));
        }
    }

    fn fail(&self, msg: &str) {
        self.done.store(true, Ordering::Relaxed);
        crate::progress::dl_log(&format!("download failed: {msg}"));
        // Surface the failure on the status line; leave the window open.
        self.set_label1(msg.to_string());
    }
}

/// Background monitor that mirrors the overall progress bar's byte position and
/// a shared completed-file counter onto the download reporter. Runs only while
/// a download reporter is registered (GUI mode); otherwise it is inert.
pub struct DownloadMonitor {
    stop_tx: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl DownloadMonitor {
    /// Stop the monitor thread and flush a final progress update.
    pub fn finish(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for DownloadMonitor {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Start a background thread that polls `total_pb` (cumulative bytes) and
/// `done_files` (completed file count) every ~150 ms and forwards them to the
/// download reporter. Returns an inert handle in console mode.
pub fn watch_download(total_pb: ProgressBar, done_files: Arc<AtomicUsize>) -> DownloadMonitor {
    if !progress::dl_active() {
        return DownloadMonitor { stop_tx: None, thread: None };
    }

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::spawn(move || loop {
        let bytes = total_pb.position();
        let done = done_files.load(Ordering::Relaxed);
        progress::dl_update(done, bytes);

        if stop_rx.try_recv().is_ok() || total_pb.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    });

    DownloadMonitor { stop_tx: Some(stop_tx), thread: Some(thread) }
}

/// Run the client download with the GUI. Shows the window and downloads on a
/// background thread; the window closes automatically when
/// `close_after_finishing` is set and the download succeeds.
#[allow(clippy::too_many_arguments)]
pub fn run_gui_download(
    region: Region,
    path: &std::path::Path,
    wz_only: bool,
    filter: Option<&FileFilter>,
    allow_insecure: bool,
    proxy: Option<&str>,
    build: Option<u32>,
    purge_wz_files: bool,
    close_after_finishing: bool,
) -> Result<()> {
    // Hide the empty console window when we own it (launched from Explorer /
    // a shortcut). Left untouched when launched from a real terminal.
    crate::gui::hide_own_console();

    let ui = UiModel::new();
    let reporter = Arc::new(GuiDownloadReporter::new(
        Arc::clone(&ui),
        path.join("cmsdl_downloader.log"),
    ));
    progress::set_download_reporter(reporter as Arc<dyn DownloadReporter>);

    // Own the arguments so the download can run on a background thread while the
    // window's message loop stays live on this (main) thread.
    let path_buf = path.to_path_buf();
    let proxy_buf = proxy.map(|s| s.to_string());
    let filter_owned = filter.cloned();

    std::thread::spawn(move || {
        let proxy = proxy_buf.as_deref();
        let res = crate::downloader::run_download_core(
            region,
            &path_buf,
            wz_only,
            filter_owned.as_ref(),
            allow_insecure,
            proxy,
            build,
            purge_wz_files,
        );
        match res {
            Ok(()) => progress::dl_finish(close_after_finishing),
            Err(e) => progress::dl_fail(&format!("{e}")),
        }
    });

    // Block on the window's message loop until it closes.
    gui::run_window(ui)
}
