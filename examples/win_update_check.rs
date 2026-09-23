//! Reports whether this PC currently has pending Windows updates.
//!
//! Answer only: one line on stdout (`pending Windows updates: yes|no`) plus an
//! exit code you can branch on. There is deliberately no list of updates and no
//! "waiting for restart" reporting — progress and diagnostics go to stderr.
//!
//! Uses the Windows Update Agent (WUA) COM API (`Microsoft.Update.Session`)
//! through the `windows` crate — no PowerShell, no extra dependencies.
//!
//! Run it with:
//!
//! ```text
//! cargo run --release --example win_update_check
//! ```
//!
//! An optional first argument replaces the WUA search criteria (the default is
//! `IsInstalled=0 and IsHidden=0`, i.e. "not installed and not hidden"):
//!
//! ```text
//! cargo run --release --example win_update_check "IsInstalled=0 and Type='Software'"
//! ```
//!
//! Exit codes: `0` = nothing pending, `1` = at least one pending update,
//! `2` = the check itself failed (no elevation is needed; the search can take a
//! minute, and it needs the Windows Update service to be enabled).

#[cfg(not(windows))]
fn main() {
    eprintln!("win_update_check only runs on Windows.");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(imp::run());
}

#[cfg(windows)]
mod imp {
    use anyhow::{bail, Context, Result};
    use windows::core::BSTR;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::UpdateAgent::{
        orcSucceeded, orcSucceededWithErrors, ISearchResult, IUpdateSession, UpdateSession,
    };

    /// Default WUA criteria: every update Windows offers that is neither
    /// installed nor hidden by the user.
    const DEFAULT_CRITERIA: &str = "IsInstalled=0 and IsHidden=0";

    const EXIT_CLEAN: i32 = 0;
    const EXIT_PENDING: i32 = 1;
    const EXIT_ERROR: i32 = 2;

    pub(super) fn run() -> i32 {
        let criteria = std::env::args()
            .nth(1)
            .unwrap_or_else(|| DEFAULT_CRITERIA.to_string());

        // Progress and diagnostics go to stderr so stdout stays a single
        // machine-readable line.
        eprintln!("searching for Windows updates (criteria: {criteria}) ...");
        let outcome = match check(&criteria) {
            Ok(outcome) => outcome,
            Err(err) => {
                eprintln!("error: {err:#}");
                return EXIT_ERROR;
            }
        };

        if outcome.incomplete {
            eprintln!("warning: the search completed with errors, so pending updates may have been missed:");
            for warning in &outcome.warnings {
                eprintln!("  - {warning}");
            }
        }

        if outcome.count > 0 {
            println!("pending Windows updates: yes");
            EXIT_PENDING
        } else {
            println!("pending Windows updates: no");
            EXIT_CLEAN
        }
    }

    /// The answer itself, plus just enough context to know how much to trust it.
    struct Outcome {
        count: i32,
        /// WUA reported `orcSucceededWithErrors`: some services/categories could
        /// not be searched, so `count` may be too low.
        incomplete: bool,
        warnings: Vec<String>,
    }

    fn check(criteria: &str) -> Result<Outcome> {
        unsafe {
            // Microsoft.Update.Session is registered as "Both", so either
            // apartment works; MTA keeps this console app single-threaded.
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() {
                bail!("CoInitializeEx failed: {hr:?}");
            }
            let result = query(criteria);
            CoUninitialize();
            result
        }
    }

    fn query(criteria: &str) -> Result<Outcome> {
        let session: IUpdateSession = unsafe { CoCreateInstance(&UpdateSession, None, CLSCTX_ALL) }
            .context(
                "cannot create a Windows Update session (is the Windows Update service enabled?)",
            )?;

        let searcher = unsafe { session.CreateUpdateSearcher() }
            .context("cannot create an update searcher")?;

        let found = unsafe { searcher.Search(&BSTR::from(criteria)) }
            .with_context(|| format!("searching for updates failed (criteria: {criteria})"))?;

        let code = unsafe { found.ResultCode() }.context("cannot read the search result code")?;
        if code != orcSucceeded && code != orcSucceededWithErrors {
            bail!("the update search did not complete (result code {code:?})");
        }

        let count = unsafe { found.Updates() }
            .and_then(|updates| unsafe { updates.Count() })
            .context("cannot read the update list")?;

        Ok(Outcome {
            count,
            incomplete: code == orcSucceededWithErrors,
            warnings: read_warnings(&found),
        })
    }

    /// `ISearchResult::Warnings` lists the services/categories that could not be
    /// searched; an empty list means the result is complete.
    fn read_warnings(found: &ISearchResult) -> Vec<String> {
        let Ok(collection) = (unsafe { found.Warnings() }) else {
            return Vec::new();
        };
        let Ok(count) = (unsafe { collection.Count() }) else {
            return Vec::new();
        };
        (0..count)
            .filter_map(|i| unsafe { collection.get_Item(i) }.ok())
            .map(|warning| {
                let message = unsafe { warning.Message() }
                    .map(|text| text.to_string())
                    .unwrap_or_default();
                match unsafe { warning.HResult() } {
                    Ok(hr) if hr != 0 => format!("{message} (0x{hr:08X})"),
                    _ => message,
                }
            })
            .collect()
    }
}
