//! Taskbar / dock progress integration.
//!
//! On Windows the progress is shown on the console window's taskbar button via
//! the `ITaskbarList3` COM interface.  On macOS and Linux a best-effort attempt
//! is made using the OSC 9;4 escape sequence recognised by iTerm2, WezTerm,
//! and other terminal emulators that forward the sequence to the OS-level
//! dock / taskbar API.
//!
//! Call [`watch`] with a cloned [`indicatif::ProgressBar`] and the total byte
//! count to start a background polling thread.  Drop or call
//! [`TaskbarHandle::finish`] when the download completes to clear the
//! indicator.

use std::sync::mpsc;
use std::time::Duration;

use indicatif::ProgressBar;

/// Handle returned by [`watch`].
///
/// The background thread runs until this is dropped or [`TaskbarHandle::finish`]
/// is called explicitly.  Dropping while the download is still in progress
/// clears the taskbar/dock indicator immediately.
pub struct TaskbarHandle {
    stop_tx: mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TaskbarHandle {
    /// Stop the watcher thread and clear the taskbar/dock progress indicator.
    pub fn finish(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for TaskbarHandle {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Start a background thread that polls `pb` every ~200 ms and reflects its
/// current position as a native taskbar / dock progress value in the range
/// `[0, total]`.
///
/// Returns a [`TaskbarHandle`] that clears the indicator when dropped.
/// When `total` is zero the function is a no-op that returns an inert handle.
pub fn watch(pb: ProgressBar, total: u64) -> TaskbarHandle {
    let (stop_tx, stop_rx) = mpsc::channel::<()>();

    let thread = std::thread::spawn(move || {
        if total == 0 {
            return;
        }
        let tp = PlatformProgress::new();
        loop {
            let pos = pb.position();
            tp.set_progress(pos, total);

            if stop_rx.try_recv().is_ok() || pb.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        tp.finish();
    });

    TaskbarHandle {
        stop_tx,
        thread: Some(thread),
    }
}

// ---------------------------------------------------------------------------
// Windows — ITaskbarList3 via the `windows` crate
// ---------------------------------------------------------------------------

#[cfg(windows)]
struct PlatformProgress {
    hwnd: windows::Win32::Foundation::HWND,
    /// `None` when the console window handle could not be obtained or COM
    /// initialisation / object creation failed.
    taskbar: Option<windows::Win32::UI::Shell::ITaskbarList3>,
    /// Whether stderr is a TTY.  When true, OSC 9;4 sequences are also
    /// emitted so that Windows Terminal (which uses ConPTY and therefore
    /// returns an invisible HWND from `GetConsoleWindow`) can show taskbar
    /// progress on its own visible window.
    osc_tty: bool,
}

#[cfg(windows)]
impl PlatformProgress {
    fn new() -> Self {
        use std::io::IsTerminal as _;
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};

        let osc_tty = std::io::stderr().is_terminal();

        unsafe {
            // Obtain the HWND of the console window.  Returns NULL when stdout
            // is not attached to a console (e.g. output is piped).
            extern "system" {
                fn GetConsoleWindow() -> isize;
            }
            let raw = GetConsoleWindow();
            if raw == 0 {
                return Self {
                    hwnd: HWND(std::ptr::null_mut()),
                    taskbar: None,
                    osc_tty,
                };
            }
            let hwnd = HWND(raw as *mut core::ffi::c_void);

            // Initialise COM for this thread; ignore errors — the main thread
            // may have already initialised it with a compatible apartment model.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let taskbar: Option<ITaskbarList3> =
                CoCreateInstance(&TaskbarList, None, CLSCTX_ALL).ok();

            if let Some(ref tb) = taskbar {
                let _ = tb.HrInit();
            }

            Self { hwnd, taskbar, osc_tty }
        }
    }

    fn set_progress(&self, current: u64, total: u64) {
        use windows::Win32::UI::Shell::TBPF_NORMAL;

        // ITaskbarList3: works for classic cmd.exe / PowerShell with conhost.exe.
        if let Some(ref tb) = self.taskbar {
            unsafe {
                let _ = tb.SetProgressState(self.hwnd, TBPF_NORMAL);
                let _ = tb.SetProgressValue(self.hwnd, current, total);
            }
        }

        // OSC 9;4: Windows Terminal processes this and shows progress on its
        // own visible window, which GetConsoleWindow() does not return.
        if self.osc_tty && total > 0 {
            use std::io::Write as _;
            let pct = ((current as f64 / total as f64) * 100.0).round() as u64;
            let pct = pct.min(100);
            let _ = write!(std::io::stderr(), "\x1b]9;4;4;{pct}\x1b\\");
        }
    }

    fn finish(&self) {
        use windows::Win32::UI::Shell::TBPF_NOPROGRESS;

        if let Some(ref tb) = self.taskbar {
            unsafe {
                let _ = tb.SetProgressState(self.hwnd, TBPF_NOPROGRESS);
            }
        }

        if self.osc_tty {
            use std::io::Write as _;
            let _ = write!(std::io::stderr(), "\x1b]9;4;0;0\x1b\\");
        }
    }
}

// SAFETY: `PlatformProgress` is created and used exclusively within the
// watcher thread; it is never actually sent across threads.  The raw pointer
// inside `HWND` is a handle value, not a live allocation owned by this type.
#[cfg(windows)]
unsafe impl Send for PlatformProgress {}

// ---------------------------------------------------------------------------
// macOS / Linux — OSC 9;4 terminal escape sequence (best-effort)
//
// This sequence is recognised by iTerm2 (macOS dock progress), WezTerm,
// Windows Terminal, and certain other emulators that forward it to the OS
// taskbar / dock API.  On terminals that do not support it the bytes are
// silently discarded.
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
struct PlatformProgress {
    /// Whether stderr was a TTY at the time the watcher thread started.
    /// Escape sequences are only emitted when this is true to avoid
    /// polluting piped / redirected output in headless environments
    /// (SSH without a PTY, cron jobs, CI pipelines, etc.).
    is_tty: bool,
}

#[cfg(not(windows))]
impl PlatformProgress {
    fn new() -> Self {
        use std::io::IsTerminal as _;
        Self {
            is_tty: std::io::stderr().is_terminal(),
        }
    }

    fn set_progress(&self, current: u64, total: u64) {
        use std::io::Write as _;

        if !self.is_tty || total == 0 {
            return;
        }
        let pct = ((current as f64 / total as f64) * 100.0).round() as u64;
        let pct = pct.min(100);
        // OSC 9;4 format: ESC ] 9 ; 4 ; <state> ; <progress> ST
        //   state 4 = normal progress, progress = 0-100
        let _ = write!(std::io::stderr(), "\x1b]9;4;4;{pct}\x1b\\");
    }

    fn finish(&self) {
        use std::io::Write as _;

        if !self.is_tty {
            return;
        }
        // state 0 clears the progress indicator.
        let _ = write!(std::io::stderr(), "\x1b]9;4;0;0\x1b\\");
    }
}
