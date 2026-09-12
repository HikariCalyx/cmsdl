//! Guard against running the exact same cmsdl command line twice at once.
//!
//! A lock file is created under the OS temp directory for every distinct
//! command line (all arguments after the program name, hashed). The lock is
//! held for the lifetime of the process, so a second invocation of the same
//! command sees `WouldBlock` and exits immediately instead of starting a
//! duplicate operation. Different command lines are free to run concurrently.
//!
//! Arguments are normalised before hashing so that spelling differences that
//! refer to the same thing do not create separate locks:
//!
//! - the region token is lower-cased, since clap matches it case-insensitively
//!   (`cms` == `CMS`, `tms` == `TMS`, ...);
//! - on Windows, drive-absolute paths are lower-cased and `/`/`\` separators
//!   are unified (e.g. `C:\Games\CMS` and `c:/games/cms` map to the same key).
//!
//! All other tokens (flags, URLs, regexes, versions, ...) are hashed
//! byte-for-byte.
//!
//! Lock files are cleaned up: when an operation finishes cleanly its lock file
//! is removed, and every invocation prunes lock files left behind by earlier
//! (possibly crashed) runs. Files still held by a live process are never
//! touched, so cleanup can never break an in-progress run.

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Directory (under the OS temp folder) holding one lock file per command line.
fn lock_dir() -> PathBuf {
    std::env::temp_dir().join("cmsdl-locks")
}

/// Compute the lock file name for a list of command-line arguments.
///
/// The region token is case-folded first and every token is then path-
/// normalised on Windows; the results are joined with NUL separators so that
/// e.g. `["a b"]` and `["a", "b"]` produce distinct keys.
fn lock_file_name_for<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut hasher = Sha256::new();
    for arg in normalize_region_token(args) {
        hasher.update(normalize_arg(&arg).as_bytes());
        hasher.update([0u8]);
    }
    format!("{}.lock", hex::encode(hasher.finalize()))
}

/// The accepted region names (canonical lower-case forms used by clap).
const REGION_NAMES: [&str; 4] = ["cms", "cms_cw", "tms", "manual"];

/// Lower-case the region token in a raw argument list.
///
/// clap accepts the region case-insensitively (`cms` == `CMS`), so the token
/// that actually is the region is case-folded before hashing, keeping e.g.
/// `cmsdl cms --download X` and `cmsdl CMS --download X` on the same lock.
///
/// The region is the first *bare* positional (a token not starting with `-`)
/// whose value is one of the known region names, so it is found correctly even
/// when options precede it (e.g. `cmsdl --check cms`). Every other token is
/// returned unchanged.
fn normalize_region_token<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen_region = false;
    args.into_iter()
        .map(|arg| {
            let s = arg.as_ref();
            if !seen_region && !s.starts_with('-') && is_region_name(s) {
                seen_region = true;
                s.to_ascii_lowercase()
            } else {
                s.to_owned()
            }
        })
        .collect()
}

/// Return `true` if `s` (case-insensitively) is one of the region names.
fn is_region_name(s: &str) -> bool {
    REGION_NAMES.iter().any(|r| r.eq_ignore_ascii_case(s))
}

/// Normalise a single command-line argument for duplicate detection.
///
/// On Windows only, a token that clearly names a drive-absolute path - either
/// the whole token or the value after the first `=` (e.g. `--download=C:\...`)
/// - has its drive letter and path lower-cased and `\` separators converted to
/// `/`, mirroring the case-insensitive filesystem. Everything else is returned
/// unchanged, so flags, URLs, regex patterns, versions, and other values stay
/// byte-for-byte identical in the key. On non-Windows platforms the argument
/// is always returned unchanged, because paths are case-sensitive there.
fn normalize_arg(arg: &str) -> String {
    if !cfg!(windows) {
        return arg.to_owned();
    }

    match arg.find('=') {
        Some(i) => {
            let (name, value) = arg.split_at(i + 1); // keep the '='
            if is_drive_path(value) {
                format!("{name}{}", normalize_drive_path(value))
            } else {
                arg.to_owned()
            }
        }
        None if is_drive_path(arg) => normalize_drive_path(arg),
        None => arg.to_owned(),
    }
}

/// Return `true` if `s` starts with a Windows drive path like `C:\` or `c:/`.
///
/// Requiring a separator right after the colon keeps the test specific enough
/// that a stray `http:` or `C:foo` is never mistaken for a path.
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && (b[2] == b'\\' || b[2] == b'/')
}

/// Lower-case `s` and unify `\` and `/` separators into `/`.
fn normalize_drive_path(s: &str) -> String {
    s.to_ascii_lowercase().replace('\\', "/")
}

/// Lock file name for this process's command line. The program path (argv[0])
/// is excluded, so the same logical command maps to the same lock regardless
/// of where cmsdl was invoked from.
fn lock_file_name() -> String {
    lock_file_name_for(std::env::args().skip(1))
}

/// Try to take an exclusive lock on the file at `path`.
///
/// Returns `Ok(Some(file))` when the lock was acquired (keep `file` alive for
/// the duration of the operation), `Ok(None)` when another process already
/// holds it, and `Err` only for genuine I/O failures.
fn acquire_at(path: &Path) -> Result<Option<File>> {
    // Open without truncating so an existing (locked) file is never disturbed.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The acquired single-instance lock.
///
/// Keeps the lock file open (and therefore locked) for as long as the guard is
/// alive, and remembers where the file lives so it can be removed afterwards.
pub struct SingleInstanceGuard {
    _file: File,
    path: PathBuf,
}

impl SingleInstanceGuard {
    /// Release the lock and remove the lock file (best effort).
    ///
    /// Called once the operation has finished cleanly. The file is unlinked
    /// *while the lock is still held*, so no other process can acquire it in
    /// the gap between releasing and deleting it. On error paths the guard is
    /// simply dropped instead, which releases the lock but leaves the tiny
    /// empty file behind for the next invocation's prune to remove.
    pub fn cleanup(self) {
        let Self { _file, path } = self;
        let _ = std::fs::remove_file(&path);
        drop(_file); // close the handle, releasing the lock
    }
}

/// Try to acquire the single-instance lock for the current command line.
///
/// - `Ok(Some(guard))` - this process may proceed; hold the guard until done,
///   then call [`SingleInstanceGuard::cleanup`] once the operation finishes.
/// - `Ok(None)` - another cmsdl is already running this exact command.
/// - `Err(e)` - the lock could not be set up (e.g. an unwritable temp dir);
///   the caller should treat this as best-effort and continue.
///
/// After acquiring, lock files left behind by earlier runs are pruned (see
/// [`prune_stale_locks`]).
pub fn acquire() -> Result<Option<SingleInstanceGuard>> {
    let dir = lock_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(lock_file_name());

    match acquire_at(&path)? {
        Some(file) => {
            prune_stale_locks(&dir, &path);
            Ok(Some(SingleInstanceGuard { _file: file, path }))
        }
        None => Ok(None),
    }
}

/// Best-effort removal of `.lock` files in `dir` that no live process holds.
///
/// A leftover lock file is one whose exclusive lock can be taken right now: if
/// some other cmsdl were still running that command, the lock would be held and
/// the attempt would fail. Files that are held are left untouched, as is the
/// caller's own lock file (`own_path`). Errors are ignored - this is purely
/// housekeeping, so it can never block the actual operation.
fn prune_stale_locks(dir: &Path, own_path: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Never touch the lock we just acquired, or non-lock files.
        if path == own_path {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        // If we can lock it, no one else is using it: delete it while still
        // holding the lock so no other process can grab it mid-cleanup.
        if let Ok(Some(_file)) = acquire_at(&path) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_file_name_is_deterministic() {
        let a = lock_file_name_for(["cms", "--download", "C:\\Games\\CMS"]);
        let b = lock_file_name_for(["cms", "--download", "C:\\Games\\CMS"]);
        assert_eq!(a, b);
    }

    #[test]
    fn lock_file_name_distinguishes_arguments() {
        assert_ne!(
            lock_file_name_for(["cms", "--download", "a"]),
            lock_file_name_for(["cms", "--download", "b"])
        );
        // The NUL separator keeps ["a b"] distinct from ["a", "b"].
        assert_ne!(lock_file_name_for(["a b"]), lock_file_name_for(["a", "b"]));
    }

    #[cfg(windows)]
    #[test]
    fn drive_paths_are_case_and_separator_normalized() {
        assert_eq!(normalize_arg(r"C:\Games\CMS"), "c:/games/cms");
        assert_eq!(normalize_arg("C:/Games/CMS"), "c:/games/cms");
        // The value after `=` is normalised, the flag name is left alone.
        assert_eq!(
            normalize_arg(r"--download=C:\Games\CMS"),
            "--download=c:/games/cms"
        );
        // Both spellings therefore map to the same lock key.
        assert_eq!(
            lock_file_name_for([r"cms", "--download", r"C:\Games\CMS"]),
            lock_file_name_for([r"cms", "--download", "c:/games/cms"])
        );
    }

    #[test]
    fn non_path_values_are_not_normalized() {
        // Regexes, URLs, versions, and filters are case-sensitive and are
        // hashed byte-for-byte.
        assert_eq!(
            normalize_arg(r"--filter-regex=Data\.wz"),
            r"--filter-regex=Data\.wz"
        );
        assert_eq!(normalize_arg("https://host/File.WZ"), "https://host/File.WZ");
        assert_eq!(normalize_arg("--patch=0.0.0.15"), "--patch=0.0.0.15");
        assert_eq!(normalize_arg("V280"), "V280");
        // `C:foo` is not a path (no separator after the colon).
        assert_eq!(normalize_arg("C:foo"), "C:foo");
    }

    #[test]
    fn region_token_is_case_normalized() {
        // `cms` and `CMS` (region in different case) map to the same key.
        assert_eq!(
            lock_file_name_for(["cms", "--download", "dir"]),
            lock_file_name_for(["CMS", "--download", "dir"])
        );
        // Flags before the region are handled too.
        assert_eq!(
            lock_file_name_for(["--check", "cms"]),
            lock_file_name_for(["--check", "CMS"])
        );
        // The first bare positional that matches a region name is the region;
        // a later path value merely named like a region is left alone.
        assert_eq!(
            lock_file_name_for(["tms", "--download", "Manual"]),
            lock_file_name_for(["TMS", "--download", "Manual"])
        );
        // Different regions still produce different keys.
        assert_ne!(
            lock_file_name_for(["cms", "--check"]),
            lock_file_name_for(["tms", "--check"])
        );
    }

    #[test]
    fn second_lock_on_same_file_is_denied() {
        let dir = std::env::temp_dir().join("cmsdl_single_instance_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.lock");

        let first = acquire_at(&path).unwrap();
        assert!(first.is_some());

        // A second exclusive lock on the same file must be refused.
        let second = acquire_at(&path).unwrap();
        assert!(second.is_none());

        // Dropping the first lock releases it for a third acquirer.
        drop(first);
        let third = acquire_at(&path).unwrap();
        assert!(third.is_some());
    }

    #[test]
    fn guard_cleanup_removes_lock_file() {
        let dir = std::env::temp_dir().join("cmsdl_guard_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("guard.lock");

        let guard = match acquire_at(&path).unwrap() {
            Some(file) => SingleInstanceGuard {
                _file: file,
                path: path.clone(),
            },
            None => panic!("lock should have been acquired"),
        };
        assert!(path.exists(), "lock file should exist while held");

        guard.cleanup();
        assert!(!path.exists(), "lock file should be removed on cleanup");

        // A fresh run can now take the lock again without any stale file.
        let again = acquire_at(&path).unwrap();
        assert!(again.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_removes_unlocked_files_but_keeps_locked_ones() {
        let dir = std::env::temp_dir().join("cmsdl_prune_test");
        std::fs::create_dir_all(&dir).unwrap();

        // A "live" lock: acquired by us, must survive pruning.
        let live_path = dir.join("live.lock");
        let live = acquire_at(&live_path).unwrap();
        assert!(live.is_some());

        // A stale lock file: not held by anyone, must be pruned.
        let stale_path = dir.join("stale.lock");
        std::fs::write(&stale_path, b"").unwrap();

        // An unrelated file: must not be touched.
        let other_path = dir.join("unrelated.txt");
        std::fs::write(&other_path, b"hello").unwrap();

        // Our "own" file in this simulation (not actually on disk).
        let own = dir.join("own.lock");

        prune_stale_locks(&dir, &own);

        assert!(live_path.exists(), "a locked file must be kept");
        assert!(!stale_path.exists(), "an unlocked file must be pruned");
        assert!(other_path.exists(), "unrelated files must be left alone");

        drop(live);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
