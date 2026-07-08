//! GUI front-end for `cmsdl cms --patch <version> <dir>`.
//!
//! Runs the patch on a background thread while a layered window shows progress
//! (see [`crate::gui`]). All detailed procedure output is written to
//! `<target>/cmsdl_patcher.log`. Falls back to the console patcher when the
//! GUI is unavailable (non-Windows or `--no_gui`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{bail, Result};

use crate::gui::{self, UiModel};
use crate::gui_downloader::format_eta;
use crate::locale::tr;
use crate::progress::{self, format_speed, Reporter};

/// Download/repair context used to compute transfer speed.
struct SpeedState {
    last: Instant,
    last_bytes: u64,
    speed: f64,
}

impl SpeedState {
    fn reset() -> Self {
        SpeedState { last: Instant::now(), last_bytes: 0, speed: 0.0 }
    }

    /// Update with the latest cumulative byte count; returns current speed.
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

/// Current download package context (index/count/total bytes).
#[derive(Default, Clone, Copy)]
struct DownloadCtx {
    index: usize,
    count: usize,
    total: u64,
}

struct LogState {
    file: Option<File>,
    buffer: Vec<String>,
    path: PathBuf,
}

/// The GUI reporter: maps progress events onto the shared [`UiModel`], and
/// writes the detailed procedure to the log file.
pub struct GuiReporter {
    ui: Arc<Mutex<UiModel>>,
    log: Mutex<LogState>,
    dl: Mutex<DownloadCtx>,
    speed: Mutex<SpeedState>,
    last_ui: Mutex<Instant>,
    repair_total_bytes: Mutex<u64>,
}

impl GuiReporter {
    fn new(ui: Arc<Mutex<UiModel>>, log_path: PathBuf) -> Self {
        GuiReporter {
            ui,
            log: Mutex::new(LogState { file: None, buffer: Vec::new(), path: log_path }),
            dl: Mutex::new(DownloadCtx::default()),
            speed: Mutex::new(SpeedState::reset()),
            last_ui: Mutex::new(Instant::now()),
            repair_total_bytes: Mutex::new(0),
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

    fn set_label3(&self, text: String) {
        if let Ok(mut m) = self.ui.lock() {
            m.label3 = text;
        }
    }

    fn set_progress(&self, ratio: f32) {
        if let Ok(mut m) = self.ui.lock() {
            m.progress = ratio.clamp(0.0, 1.0);
        }
    }

    /// Throttle helper: returns true at most every ~100ms.
    fn should_update_ui(&self) -> bool {
        let mut last = self.last_ui.lock().unwrap();
        if last.elapsed().as_millis() >= 100 {
            *last = Instant::now();
            true
        } else {
            false
        }
    }

    /// Open the log file (append) and flush any buffered lines. Called when an
    /// update is confirmed (see requirement: log is created when an update is
    /// found). A timestamped header separates successive runs.
    fn open_log(&self) {
        let mut st = self.log.lock().unwrap();
        if st.file.is_some() {
            return;
        }
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&st.path) {
            let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(f, "\n===== cmsdl patch session {ts} =====");
            for line in st.buffer.drain(..) {
                let _ = writeln!(f, "{line}");
            }
            st.file = Some(f);
        }
    }
}

impl Reporter for GuiReporter {
    fn log(&self, line: &str) {
        let mut st = self.log.lock().unwrap();
        match st.file.as_mut() {
            Some(f) => { let _ = writeln!(f, "{line}"); }
            None => st.buffer.push(line.to_string()),
        }
    }

    fn scanning(&self) {
        self.set_label2(tr("gui-patcher-scanning-update", &[]));
        self.set_label1(String::new());
        self.set_label3(String::new());
        self.set_progress(0.0);
    }

    fn installing(&self, current: &str, target: &str) {
        // An update was found: create/append the log file now.
        self.open_log();
        self.set_label2(tr("gui-patcher-installing-update-from", &[current, target]));
        self.set_label3(String::new());
        self.set_progress(0.0);
    }

    fn begin_download(&self, index: usize, count: usize, total: u64) {
        *self.dl.lock().unwrap() = DownloadCtx { index, count, total };
        *self.speed.lock().unwrap() = SpeedState::reset();
        self.set_label3(String::new());
        self.set_progress(0.0);
        // Show the label immediately with a 0 speed.
        self.render_download(0);
    }

    fn download_progress(&self, downloaded: u64) {
        let ctx = *self.dl.lock().unwrap();
        if ctx.total > 0 {
            self.set_progress(downloaded as f32 / ctx.total as f32);
        }
        if self.should_update_ui() {
            self.render_download(downloaded);
        }
    }

    fn extracting(&self, index: usize, count: usize) {
        // Shown between download and apply; for a package that is a single huge
        // file the subsequent patch step reports no per-file progress until it
        // completes, so this label reassures the user work is ongoing.
        self.set_label3(String::new());
        self.set_label1(tr(
            "gui-patcher-extracting-package",
            &[&index.to_string(), &count.to_string()],
        ));
    }

    fn begin_apply(&self, _total: usize) {
        // Requirement 6: reset the progress bar to 0 for the apply phase.
        self.set_label3(String::new());
        self.set_progress(0.0);
    }

    fn apply_progress(&self, done: usize, total: usize, rel_path: &str) {
        if total > 0 {
            self.set_progress(done as f32 / total as f32);
        }
        if self.should_update_ui() || done == total {
            self.set_label1(tr(
                "gui-patcher-applying-update",
                &[&done.to_string(), &total.to_string(), rel_path],
            ));
        }
    }

    fn begin_repair(&self, _total: usize, total_bytes: u64) {
        *self.repair_total_bytes.lock().unwrap() = total_bytes;
        *self.speed.lock().unwrap() = SpeedState::reset();
        self.set_label3(String::new());
        self.set_progress(0.0);
    }

    fn repair_progress(&self, done: usize, total: usize, rel_path: &str, downloaded: u64) {
        let total_bytes = *self.repair_total_bytes.lock().unwrap();
        if total_bytes > 0 {
            self.set_progress(downloaded as f32 / total_bytes as f32);
        }
        let speed = self.speed.lock().unwrap().tick(downloaded);
        if self.should_update_ui() || done == total {
            self.set_label1(tr(
                "gui-patcher-repairing-file",
                &[&done.to_string(), &total.to_string(), rel_path, &format_speed(speed)],
            ));
        }
    }

    fn purging(&self) {
        self.set_label1(tr("gui-patcher-purge", &[]));
        self.set_label3(String::new());
        self.set_progress(0.0);
    }

    fn finish(&self, msg: &str, close: bool) {
        self.set_label3(String::new());
        if !msg.is_empty() {
            self.set_label1(msg.to_string());
        }
        if close {
            if let Ok(mut m) = self.ui.lock() {
                m.should_close = true;
            }
        }
    }
}

impl GuiReporter {
    /// Render the label1 "downloading" line for the current package + speed,
    /// and update the ETA label.
    fn render_download(&self, downloaded: u64) {
        let ctx = *self.dl.lock().unwrap();
        let speed = self.speed.lock().unwrap().tick(downloaded);
        self.set_label1(tr(
            "gui-patcher-downloading-update",
            &[&ctx.index.to_string(), &ctx.count.to_string(), &format_speed(speed)],
        ));
        // ETA: remaining bytes / current speed.
        let remaining = ctx.total.saturating_sub(downloaded);
        let eta_secs = if speed > 0.0 { remaining as f64 / speed } else { 0.0 };
        let eta_text = if eta_secs > 1.0 {
            format_eta(eta_secs)
        } else {
            String::new()
        };
        self.set_label3(eta_text);
    }
}

/// Run the patch procedure with the GUI. Validates the client directory up
/// front (requirement 2: an invalid path shows no window and returns an error,
/// which propagates to a non-zero exit code). Otherwise shows the window and
/// patches on a background thread.
pub fn run_gui_patch(
    target: &Path,
    version: &str,
    launch_after: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
    purge_wz_files: bool,
    lrhook: bool,
    close_after_finishing: bool,
    keep_old_wz_files: bool,
) -> Result<()> {
    // Validate the client directory before showing anything. (Done before
    // hiding the console so an invalid-path error is still visible when run
    // from a terminal.)
    if !target.join("mxd").is_dir() {
        bail!(
            "no 'mxd' directory found in {}; not a CMS client directory",
            target.display()
        );
    }

    // Hide the empty console window when we own it (launched from Explorer /
    // a shortcut). Left untouched when launched from a real terminal.
    crate::gui::hide_own_console();

    let ui = UiModel::new();
    let reporter = Arc::new(GuiReporter::new(Arc::clone(&ui), target.join("cmsdl_patcher.log")));
    progress::set_reporter(reporter.clone() as Arc<dyn Reporter>);

    // Run the patch on a background thread so the message loop stays live.
    let target_buf = target.to_path_buf();
    let version_buf = version.to_string();
    let proxy_buf = proxy.map(|s| s.to_string());
    let ui_for_result = Arc::clone(&ui);
    std::thread::spawn(move || {
        let proxy = proxy_buf.as_deref();
        let res = run_patch_flow(
            &target_buf, &version_buf, launch_after,
            allow_insecure, proxy, purge_wz_files, lrhook, close_after_finishing,
            keep_old_wz_files,
        );
        if let Err(e) = &res {
            crate::plog!("error: {e:#}");
            // Surface the failure on the status line; leave the window open.
            progress::finish(&format!("{e}"), false);
        } else if let Ok(mut m) = ui_for_result.lock() {
            m.exit_code = 0;
        }
    });

    // Block on the window's message loop until it closes.
    let ui_hold = Arc::clone(&ui);
    gui::run_window(ui)?;

    let exit_code = ui_hold.lock().unwrap().exit_code;
    if exit_code != 0 {
        anyhow::bail!("patch failed (exit code {exit_code})");
    }
    Ok(())
}

/// The patch + optional launch flow, reporting through `progress::*`.
fn run_patch_flow(
    target: &Path,
    version: &str,
    launch_after: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
    purge_wz_files: bool,
    lrhook: bool,
    close_after_finishing: bool,
    keep_old_wz_files: bool,
) -> Result<()> {
    use crate::patch::{apply_patches, launch_client, PatchOutcome};

    let outcome = apply_patches(target, version, allow_insecure, proxy, purge_wz_files, keep_old_wz_files)?;

    // Choose the terminal label based on whether an update was applied and
    // whether we were asked to launch the game (requirements 9 and 10).
    let (done_key, launch_key) = match outcome {
        PatchOutcome::Updated => {
            ("gui-patcher-patch-successful", "gui-patcher-patch-successful-launch")
        }
        PatchOutcome::AlreadyUpToDate => {
            ("gui-patcher-nopatch-successful", "gui-patcher-nopatch-successful-launch")
        }
    };

    if launch_after {
        // Show the "launching" message, then attempt the (UAC-elevated) launch.
        progress::finish(&tr(launch_key, &[]), false);
        match launch_client(target, lrhook) {
            Ok(()) => {
                crate::plog!("launch requested; closing patcher.");
                progress::finish("", true); // close the window
            }
            Err(e) => {
                // Keep the window open on a launch failure so the error stays
                // visible, even when --close-after-finishing was requested.
                crate::plog!("launch failed or was declined: {e:#}");
                progress::finish(&tr("gui-patcher-launch-fail", &[]), false);
            }
        }
    } else {
        // Show the final status; close automatically when requested.
        progress::finish(&tr(done_key, &[]), close_after_finishing);
    }

    Ok(())
}
