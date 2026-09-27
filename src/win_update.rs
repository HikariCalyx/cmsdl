//! Pending Windows Update detection without the Windows Update Agent (WUA) COM
//! API.
//!
//! [`pending_updates`] only reads the local markers Windows leaves behind while
//! an update is on its way, so it is instant, network-free and needs no
//! elevation. The trade-off is that it can only see updates that have already
//! been downloaded or installed; it cannot discover updates that are merely
//! available for download (that would need WUA).
//!
//! Markers, all under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\`:
//!
//! - `WindowsUpdate\Auto Update\RebootRequired` -- an update was installed and
//!   is waiting for a restart
//! - `Component Based Servicing\RebootPending` -- a servicing operation is
//!   waiting for a restart
//! - `Component Based Servicing\PackagesPending` (one subkey per package) --
//!   packages are staged and still have to be installed
//!
//! `examples/win_update_check.rs` is a standalone version of the same check.

use anyhow::Result;

/// Component Based Servicing (CBS) is the component store that applies update
/// packages; it leaves these markers behind while a servicing operation is
/// unfinished.
#[cfg(windows)]
const CBS_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing";
/// Windows Update's own "restart to finish" marker.
#[cfg(windows)]
const WU_AUTO_UPDATE_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update";

/// Marker names as they appear in the log.
#[cfg(windows)]
const REBOOT_REQUIRED: &str = r"WindowsUpdate\Auto Update\RebootRequired";
#[cfg(windows)]
const REBOOT_PENDING: &str = r"Component Based Servicing\RebootPending";
#[cfg(windows)]
const PACKAGES_PENDING: &str = r"Component Based Servicing\PackagesPending";

/// Returns `true` when a Windows update is already staged or installed and is
/// waiting to finish (usually a restart).
///
/// Markers that cannot be read are treated as "nothing pending" and logged, so a
/// failed check can never block the caller by itself.
#[cfg(windows)]
pub fn pending_updates() -> bool {
    let found = pending_markers();
    if found.is_empty() {
        crate::plog!("windows update check: no pending-update markers are set");
        false
    } else {
        crate::plog!("windows update check: found {}", found.join(", "));
        true
    }
}

/// Non-Windows stub: there is no Windows Update.
#[cfg(not(windows))]
pub fn pending_updates() -> bool {
    false
}

/// Opens the Windows Update page in the Settings app (`ms-settings:windowsupdate`).
///
/// Uses `ShellExecuteW` (the `ms-settings:` protocol is handled by the shell),
/// so no console window flashes and the caller keeps running.
#[cfg(windows)]
pub fn open_settings() -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            lpOperation: *const u16,
            lpFile: *const u16,
            lpParameters: *const u16,
            lpDirectory: *const u16,
            nShowCmd: i32,
        ) -> isize;
    }

    fn to_wide(s: &OsStr) -> Vec<u16> {
        let mut v: Vec<u16> = s.encode_wide().collect();
        v.push(0);
        v
    }

    let uri = to_wide(OsStr::new("ms-settings:windowsupdate"));
    const SW_SHOW: i32 = 5;
    // Null operation = the default "open" verb.
    let ret = unsafe {
        ShellExecuteW(
            0,
            ptr::null(),
            uri.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOW,
        )
    };

    // Values <= 32 indicate failure (see the ShellExecute documentation).
    if ret <= 32 {
        anyhow::bail!("could not open the Windows Update settings page (ShellExecute error {ret})");
    }
    Ok(())
}

/// Non-Windows stub: the `ms-settings:` protocol only exists on Windows.
#[cfg(not(windows))]
pub fn open_settings() -> Result<()> {
    anyhow::bail!("opening the Windows Update settings page is only supported on Windows")
}

/// Collects a description of every pending-update marker that is set.
#[cfg(windows)]
fn pending_markers() -> Vec<String> {
    let mut found = Vec::new();

    match registry_value_exists(WU_AUTO_UPDATE_KEY, "RebootRequired") {
        Ok(true) => found.push(REBOOT_REQUIRED.to_string()),
        Ok(false) => {}
        Err(err) => crate::plog!("windows update check: cannot read {REBOOT_REQUIRED}: {err}"),
    }
    match registry_value_exists(CBS_KEY, "RebootPending") {
        Ok(true) => found.push(REBOOT_PENDING.to_string()),
        Ok(false) => {}
        Err(err) => crate::plog!("windows update check: cannot read {REBOOT_PENDING}: {err}"),
    }
    let packages_pending = format!(r"{CBS_KEY}\PackagesPending");
    match registry_subkey_count(&packages_pending) {
        Ok(0) => {}
        Ok(count) => found.push(format!("{PACKAGES_PENDING} ({count} package(s))")),
        Err(err) => crate::plog!("windows update check: cannot read {PACKAGES_PENDING}: {err}"),
    }

    found
}

/// `Ok(true)` when the value exists, `Ok(false)` when it (or its key) does not,
/// `Err` when it could not be read at all.
///
/// `KEY_WOW64_64KEY` is requested explicitly: the markers only exist in the
/// 64-bit view of HKLM, so a 32-bit build would otherwise be redirected to
/// WOW6432Node and find nothing.
#[cfg(windows)]
fn registry_value_exists(key_path: &str, value: &str) -> std::io::Result<bool> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let key = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(key_path, KEY_READ | KEY_WOW64_64KEY)
    {
        Ok(key) => key,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    match key.get_raw_value(value) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

/// Number of subkeys under `key_path`; `Ok(0)` when the key does not exist.
#[cfg(windows)]
fn registry_subkey_count(key_path: &str) -> std::io::Result<usize> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let key = match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(key_path, KEY_READ | KEY_WOW64_64KEY)
    {
        Ok(key) => key,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err),
    };
    Ok(key.enum_keys().filter(|name| name.is_ok()).count())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A missing key (or value) must read as "not set" rather than as an error,
    /// otherwise every check on a clean machine would look like a failure. The
    /// path below is read-only and is not created by anything.
    #[test]
    fn missing_markers_are_not_pending() {
        const MISSING_KEY: &str = r"SOFTWARE\cmsdl-nonexistent-win-update-test";
        assert!(!registry_value_exists(MISSING_KEY, "RebootRequired").unwrap());
        assert_eq!(registry_subkey_count(MISSING_KEY).unwrap(), 0);
        assert_eq!(
            registry_subkey_count(&format!(r"{MISSING_KEY}\PackagesPending")).unwrap(),
            0
        );
    }

    /// `pending_updates` must be a safe, panic-free read on any machine.
    #[test]
    fn pending_updates_does_not_panic() {
        let _ = pending_updates();
    }
}
