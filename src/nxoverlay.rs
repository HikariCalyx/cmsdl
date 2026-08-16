//! Best-effort cleanup of Nexon's `NxOverlay` overlay caches.
//! Source: https://www.inven.co.kr/board/maple/5974/7034700
//!
//! After a client is patched, the per-game overlay caches under
//! `%LocalAppData%\Nexon\<game>\NxOverlay` can still reference the old
//! client version and misbehave until the launcher rebuilds them.  Removing
//! them once a patch completes lets the next launch start from a clean slate.

/// Remove every `NxOverlay` directory found under `%LocalAppData%\Nexon\*`
/// (including a stray `%LocalAppData%\Nexon\NxOverlay` itself).
///
/// Best-effort: a missing environment variable or directory, unreadable
/// entries, and individual removal failures are logged and ignored.
///
/// Returns `true` when at least one overlay cache was removed.  When a GUI
/// reporter is registered, the status line is updated while the cleanup runs.
pub fn clear_nxoverlay() -> bool {
    #[cfg(windows)]
    {
        // In GUI mode, surface the cleanup step on the status line.
        if crate::progress::active() {
            crate::progress::nxoverlay();
        }
        clear_nxoverlay_impl()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn clear_nxoverlay_impl() -> bool {
    use std::path::Path;

    let Some(local_appdata) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let nexon = Path::new(&local_appdata).join("Nexon");
    if !nexon.is_dir() {
        return false;
    }

    let entries = match std::fs::read_dir(&nexon) {
        Ok(entries) => entries,
        Err(e) => {
            crate::plog!("warning: could not scan '{}': {e}", nexon.display());
            return false;
        }
    };

    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name.to_string_lossy().eq_ignore_ascii_case("nxoverlay") {
            // A stray top-level NxOverlay directory.
            if remove_overlay(&path) {
                removed += 1;
            }
            continue;
        }
        // The usual layout: %LocalAppData%\Nexon\<game>\NxOverlay.
        let overlay = path.join("NxOverlay");
        if overlay.is_dir() && remove_overlay(&overlay) {
            removed += 1;
        }
    }

    if removed == 0 {
        crate::plog!(
            "no NxOverlay overlay caches found under '{}'.",
            nexon.display()
        );
        false
    } else {
        crate::plog!(
            "removed {} NxOverlay overlay cache(s) under '{}'.",
            removed,
            nexon.display()
        );
        true
    }
}

#[cfg(windows)]
fn remove_overlay(dir: &std::path::Path) -> bool {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {
            crate::plog!("removed stale overlay cache '{}'.", dir.display());
            true
        }
        Err(e) => {
            crate::plog!("warning: failed to remove '{}': {e}", dir.display());
            false
        }
    }
}
