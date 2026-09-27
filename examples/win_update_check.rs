//! Reports whether this PC currently has pending Windows updates.
//!
//! Answer only: one line on stdout (`true` or `false`) plus an exit code you can
//! branch on. There is deliberately no list of updates; per-check detail and any
//! diagnostics go to stderr.
//!
//! Deliberately does NOT use the Windows Update Agent (WUA) COM API: it reads
//! the markers Windows leaves behind while an update is on its way, so it is
//! cheap, never touches the network and needs no elevation. The trade-off is
//! that it only sees updates that have already been downloaded or installed; it
//! cannot discover updates that are merely available for download, because that
//! means asking the update service (i.e. WUA).
//!
//! All three markers live under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\`:
//!
//! - `WindowsUpdate\Auto Update\RebootRequired` -- an update was installed and
//!   is waiting for a restart
//! - `Component Based Servicing\RebootPending` -- a servicing operation is
//!   waiting for a restart
//! - `Component Based Servicing\PackagesPending` (one subkey per package) --
//!   packages are staged and still have to be installed
//!
//! Run it with:
//!
//! ```text
//! cargo run --release --example win_update_check
//! ```
//!
//! Exit codes: `0` = false (nothing pending), `1` = true (at least one marker),
//! `2` = the check itself failed (nothing could be read).
//!
//! The application runs the same check from `src/win_update.rs` (before
//! `--launch-after-patching`); this example is the standalone version.

#[cfg(not(windows))]
fn main() {
    eprintln!("win_update_check only runs on Windows.");
    // Same as `imp::EXIT_ERROR`.
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(imp::run());
}

#[cfg(windows)]
mod imp {
    use std::io::ErrorKind;

    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    const EXIT_FALSE: i32 = 0;
    const EXIT_TRUE: i32 = 1;
    const EXIT_ERROR: i32 = 2;

    /// Component Based Servicing (CBS) is the component store that applies
    /// update packages; it leaves these markers behind while a servicing
    /// operation is unfinished.
    const CBS_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing";
    /// Windows Update's own "restart to finish" marker.
    const WU_AUTO_UPDATE_KEY: &str =
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update";
    /// What one marker probe found.
    enum Probe {
        /// No evidence.
        Clear,
        /// Evidence of a pending update; the string explains it for stderr.
        Set(String),
        /// The marker could not be read, so its state is unknown.
        Unknown(String),
    }

    pub(super) fn run() -> i32 {
        eprintln!("checking for pending Windows updates (no Windows Update Agent) ...");

        let checks = [
            ("restart required to finish an update", wu_reboot_required()),
            ("servicing operation needs restart", cbs_reboot_pending()),
            ("update packages staged for install", cbs_packages_pending()),
        ];
        let total = checks.len();

        let mut pending = 0usize;
        let mut unknown = 0usize;
        for (label, probe) in checks {
            match probe {
                Probe::Clear => {}
                Probe::Set(detail) => {
                    eprintln!("  pending: {label} ({detail})");
                    pending += 1;
                }
                Probe::Unknown(detail) => {
                    eprintln!("  warning: cannot check {label}: {detail}");
                    unknown += 1;
                }
            }
        }

        if pending > 0 {
            println!("true");
            return EXIT_TRUE;
        }

        if unknown == total {
            eprintln!("error: none of the pending-update markers could be read");
            return EXIT_ERROR;
        }

        match unknown {
            0 => eprintln!("  no pending-update markers"),
            n => eprintln!("  no pending-update markers ({n} could not be read)"),
        }
        println!("false");
        EXIT_FALSE
    }

    /// Opens an HKLM subkey. `Ok(None)` means the key (and therefore its marker)
    /// is absent; `Err` means its state could not be read.
    fn open_key(key_path: &str) -> Result<Option<RegKey>, String> {
        match RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(key_path, KEY_READ) {
            Ok(key) => Ok(Some(key)),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    /// The marker is a value that only exists while it is set, whatever its type.
    fn value_exists(key_path: &str, value: &str, detail: &str) -> Probe {
        match open_key(key_path) {
            Err(err) => Probe::Unknown(err),
            Ok(None) => Probe::Clear,
            Ok(Some(key)) => match key.get_raw_value(value) {
                Ok(_) => Probe::Set(detail.to_string()),
                Err(err) if err.kind() == ErrorKind::NotFound => Probe::Clear,
                Err(err) => Probe::Unknown(err.to_string()),
            },
        }
    }

    fn wu_reboot_required() -> Probe {
        value_exists(
            WU_AUTO_UPDATE_KEY,
            "RebootRequired",
            r"the WindowsUpdate\Auto Update\RebootRequired marker is set",
        )
    }

    fn cbs_reboot_pending() -> Probe {
        value_exists(
            CBS_KEY,
            "RebootPending",
            r"the Component Based Servicing\RebootPending marker is set",
        )
    }

    /// `CBS\PackagesPending` holds one subkey per package the component store
    /// still has to finish installing.
    fn cbs_packages_pending() -> Probe {
        let key_path = format!(r"{CBS_KEY}\PackagesPending");
        match open_key(&key_path) {
            Err(err) => Probe::Unknown(err),
            Ok(None) => Probe::Clear,
            Ok(Some(key)) => {
                let count = key.enum_keys().filter(|name| name.is_ok()).count();
                if count == 0 {
                    Probe::Clear
                } else {
                    Probe::Set(format!("{count} package(s) staged under PackagesPending"))
                }
            }
        }
    }
}
