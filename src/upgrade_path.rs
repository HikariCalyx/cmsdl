//! Upgrade path check.
//!
//! Compares the total size of the incremental patch chain needed to reach a
//! target version against the size of the full client for that version, so the
//! caller can decide whether patching is cheaper than re-downloading.
//!
//! Only CMS and CMS_CW publish incremental patches. Because the result is a
//! diagnostic, expected failures are reported through a process exit code
//! (see the `EXIT_*` constants) rather than a Rust error.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::cli::Region;
use crate::cms;
use crate::cms_cw;
use crate::cms_patch;

/// Exit code: the patch chain is the same size as, or smaller than, the full
/// client for the target version.
pub const EXIT_OK: i32 = 0;
/// Exit code: no applicable patch chain exists for the requested version.
pub const EXIT_NO_PATCH: i32 = 1;
/// Exit code: the patch chain is larger than the full client for the version.
pub const EXIT_TOO_LARGE: i32 = 2;
/// Exit code: the current client version could not be read.
pub const EXIT_NO_VERSION: i32 = 3;
/// Exit code: the patch server could not be reached after retrying.
pub const EXIT_SERVER: i32 = 100;

/// Number of times a failing network operation is retried before giving up.
const NETWORK_RETRIES: usize = 3;

/// Pause between network retries.
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// cmsdl's own version, for the startup heading.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Run the upgrade path check for `region`, returning the process exit code.
///
/// `version` is either an explicit target version (e.g. `0.0.0.22`) or
/// `latest`; `target_dir` is the client directory to inspect.
pub fn run(
    region: Region,
    version: &str,
    target_dir: &Path,
    allow_insecure: bool,
    proxy: Option<&str>,
    verbose: bool,
) -> Result<i32> {
    match region {
        Region::Cms => run_inner(region, version, target_dir, allow_insecure, proxy, verbose),
        Region::CmsCw => cms::with_config(cms_cw::CW_CONFIG, || {
            run_inner(region, version, target_dir, allow_insecure, proxy, verbose)
        }),
        Region::Tms | Region::Manual => {
            bail!("--upgrade-path-check is only supported for region 'cms' (or 'cms_cw')")
        }
    }
}

/// The installed version, as read from disk.
enum InstalledVersion {
    /// An exact version string (e.g. `0.0.0.15`) read from `LocalVersion3.xml`.
    Known(String),
    /// Only the WZ major version read from `Base.wz` (CMS fallback); it is
    /// mapped to a patch version once the patch metadata is available.
    FromWz(i16),
}

/// The resolved patch plan: the inclusive package range `start..=end` that must
/// be applied to reach [`Plan::target_version`]. An empty range
/// (`start > end`) means the client is already at the target version.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    target_version: String,
    start: usize,
    end: usize,
}

fn run_inner(
    region: Region,
    version_arg: &str,
    target_dir: &Path,
    allow_insecure: bool,
    proxy: Option<&str>,
    verbose: bool,
) -> Result<i32> {
    // Echo the requested target verbatim (so `latest` stays `latest`).
    println!(
        "cmsdl {VERSION}: checking upgrade path for region '{region}' to version {version_arg}."
    );

    // ── 1. Current client version ──────────────────────────────────────────
    let installed = match detect_installed_version(region, target_dir) {
        Some(v) => v,
        None => {
            println!("error: current client version cannot be read");
            return Ok(EXIT_NO_VERSION);
        }
    };

    // ── 2. Resolve the patch chain to the target version ───────────────────
    let data = match retry(NETWORK_RETRIES, || cms::get_patch_data(allow_insecure, proxy)) {
        Ok(d) => d,
        Err(_) => {
            println!("error: patch server cannot be accessed");
            return Ok(EXIT_SERVER);
        }
    };

    // Turn the on-disk reading into an exact version string. The `Base.wz`
    // fallback only carries the WZ major version, so it is matched against the
    // published patches' display versions.
    let current = match installed {
        InstalledVersion::Known(v) => v,
        InstalledVersion::FromWz(wz) => {
            match data
                .packages
                .iter()
                .find(|p| cms_patch::version_view_matches(&p.version_view, wz))
            {
                Some(pkg) => pkg.to.clone(),
                None => {
                    println!("error: current client version cannot be read");
                    return Ok(EXIT_NO_VERSION);
                }
            }
        }
    };

    let plan = match plan_patches(&data.packages, &current, version_arg) {
        Some(p) => p,
        None => {
            println!("error: no applicable patch can be found");
            return Ok(EXIT_NO_PATCH);
        }
    };
    let target_version = plan.target_version;
    let selected: &[cms::PatchPackage] = if plan.start > plan.end {
        &[]
    } else {
        &data.packages[plan.start..=plan.end]
    };

    // ── 3. Sum of all patch sizes ──────────────────────────────────────────
    let agent = crate::net::agent(allow_insecure, proxy);
    let challenge = match retry(NETWORK_RETRIES, || cms::get_challenge_key(&agent)) {
        Ok(c) => c,
        Err(_) => {
            println!("error: patch server cannot be accessed");
            return Ok(EXIT_SERVER);
        }
    };

    let mut patch_sizes: Vec<u64> = Vec::with_capacity(selected.len());
    let mut patch_parts: Vec<Vec<(String, u64)>> = Vec::new();
    for pkg in selected {
        let parts = match retry(NETWORK_RETRIES, || {
            cms::get_patch_file_sizes(&agent, &challenge, &data.base_url, &pkg.file_list_url)
        }) {
            Ok(p) => p,
            Err(_) => {
                println!("error: patch server cannot be accessed");
                return Ok(EXIT_SERVER);
            }
        };
        patch_sizes.push(parts.iter().map(|(_, size)| *size).sum());
        if verbose {
            patch_parts.push(parts);
        }
    }
    let patch_sum: u64 = patch_sizes.iter().sum();

    // ── 4. Full client size for the target version ─────────────────────────
    let client = match retry(NETWORK_RETRIES, || {
        cms::client_size_for_version(&agent, &challenge, &target_version)
    }) {
        Ok(c) => c,
        Err(_) => {
            println!("error: patch server cannot be accessed");
            return Ok(EXIT_SERVER);
        }
    };

    // ── 5. Human-readable summary ──────────────────────────────────────────
    let view_of = |v: &str| -> Option<&str> {
        data.packages
            .iter()
            .find(|p| p.to == v)
            .map(|p| p.version_view.as_str())
            .filter(|s| !s.is_empty())
    };
    let fmt_ver = |v: &str| -> String {
        match view_of(v) {
            Some(view) => format!("{v} ({view})"),
            None => v.to_string(),
        }
    };

    println!("current version: {}", fmt_ver(&current));
    println!("target version:  {}", fmt_ver(&target_version));
    println!(
        "patches needed:  {} ({})",
        selected.len(),
        crate::progress::format_size(patch_sum)
    );
    match &client {
        Some(cs) => {
            let note = if cs.exact {
                String::new()
            } else {
                " [fallback: target full client not available]".to_string()
            };
            println!(
                "full client:     build {} version {} ({}){}",
                cs.build,
                fmt_ver(&cs.version),
                crate::progress::format_size(cs.total_size),
                note
            );
        }
        None => {
            eprintln!("warning: could not retrieve full client data for comparison.");
        }
    }

    // ── 5b. Detailed upgrade path (--verbose) ──────────────────────────────
    if verbose {
        if selected.is_empty() {
            println!("upgrade path:     (already at the target version; no patches needed)");
        } else {
            println!("upgrade path:");
            for (pkg, parts) in selected.iter().zip(&patch_parts) {
                let view = if pkg.version_view.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", pkg.version_view)
                };
                let total: u64 = parts.iter().map(|(_, size)| *size).sum();
                println!(
                    "  {} -> {}{}: {} ({} file(s))",
                    pkg.from,
                    pkg.to,
                    view,
                    crate::progress::format_size(total),
                    parts.len()
                );
                for (url, size) in parts {
                    println!("      {url}: {}", crate::progress::format_size(*size));
                }
            }
        }
    }

    // ── 6. Comparison and exit code ────────────────────────────────────────
    let command = format!(
        "{} {} --patch {target_version} {}",
        process_name(),
        region.code(),
        target_dir.display()
    );

    match &client {
        Some(cs) if cs.exact => {
            let code = decide(patch_sum, cs.total_size, true);
            if code == EXIT_TOO_LARGE {
                println!(
                    "warning: required patches to {target_version} version are larger than \
                     the {target_version} client itself"
                );
            } else {
                println!("you may apply the patch with {command}");
            }
            Ok(code)
        }
        Some(cs) => {
            eprintln!(
                "warning: no full client found for version {target_version}; \
                 compared against version {} instead.",
                cs.version
            );
            if patch_sum > cs.total_size {
                eprintln!(
                    "warning: required patches to {target_version} version are larger than \
                     the {} client itself",
                    cs.version
                );
            }
            println!("you may apply the patch with {command}");
            Ok(EXIT_OK)
        }
        None => {
            println!("you may apply the patch with {command}");
            Ok(EXIT_OK)
        }
    }
}

/// Read the installed client version from `target_dir`.
///
/// For CMS, a missing or corrupt `LocalVersion3.xml` falls back to the WZ major
/// version stored in `Base.wz`. For CMS_CW there is no fallback: a missing or
/// corrupt `LocalVersion3.xml` yields `None`.
fn detect_installed_version(region: Region, target_dir: &Path) -> Option<InstalledVersion> {
    if let Some(v) = cms_patch::read_version_from_local_xml(target_dir) {
        return Some(InstalledVersion::Known(v));
    }

    if region == Region::CmsCw {
        return None;
    }

    // CMS fallback: `<data_dir>/Data/Base/Base.wz`.
    let wz_path = target_dir
        .join(cms::data_dir())
        .join("Data")
        .join("Base")
        .join("Base.wz");
    let wz = crate::miniwzlib::get_wz_version(&wz_path).ok()?;
    if wz.version == 0 {
        return None;
    }
    Some(InstalledVersion::FromWz(wz.version))
}

/// Resolve the patch chain from `current` to the requested `version_arg`
/// (a concrete version or `latest`).
///
/// Returns `None` when no applicable chain exists: unknown target version, an
/// empty patch list, an unknown starting point, or a requested downgrade.
fn plan_patches(
    packages: &[cms::PatchPackage],
    current: &str,
    version_arg: &str,
) -> Option<Plan> {
    let target_version = if version_arg.eq_ignore_ascii_case("latest") {
        packages.last()?.to.clone()
    } else {
        version_arg.to_owned()
    };
    let end = packages.iter().position(|p| p.to == target_version)?;

    // Already at the target: an empty patch range.
    if current == target_version {
        return Some(Plan {
            target_version,
            start: end + 1,
            end,
        });
    }

    // A downgrade is not a valid upgrade path.
    if cms_patch::version_newer_than(current, &target_version) {
        return None;
    }

    let start = packages.iter().position(|p| p.from == current)?;
    if start > end {
        return None;
    }
    Some(Plan {
        target_version,
        start,
        end,
    })
}

/// Decide the exit code from a comparison. Only an exact full-client match can
/// report [`EXIT_TOO_LARGE`]; the fallback comparison is advisory.
fn decide(patch_sum: u64, client_size: u64, exact: bool) -> i32 {
    if exact && patch_sum > client_size {
        EXIT_TOO_LARGE
    } else {
        EXIT_OK
    }
}

/// The executable's own file name, used in the suggested patch command.
fn process_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "cmsdl.exe".to_string())
}

/// Retry a fallible operation up to `retries` extra times (so at most
/// `retries + 1` attempts) with a fixed [`RETRY_DELAY`] between attempts.
fn retry<T>(retries: usize, f: impl FnMut() -> Result<T>) -> Result<T> {
    retry_with_delay(retries, RETRY_DELAY, f)
}

/// Like [`retry`] but with an explicit delay (zero disables sleeping).
fn retry_with_delay<T>(
    retries: usize,
    delay: Duration,
    mut f: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut last = anyhow::anyhow!("no attempts made");
    for attempt in 0..=retries {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                last = e;
                if attempt < retries && !delay.is_zero() {
                    std::thread::sleep(delay);
                }
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cms::PatchPackage;

    fn pkg(from: &str, to: &str, view: &str) -> PatchPackage {
        PatchPackage {
            from: from.to_owned(),
            to: to.to_owned(),
            version_view: view.to_owned(),
            file_list_url: String::new(),
        }
    }

    fn chain() -> Vec<PatchPackage> {
        vec![
            pkg("0.0.0.14", "0.0.0.15", "V225.6"),
            pkg("0.0.0.15", "0.0.0.16", "V226.1"),
            pkg("0.0.0.16", "0.0.0.17", "V226.2"),
        ]
    }

    #[test]
    fn plan_resolves_latest() {
        let plan = plan_patches(&chain(), "0.0.0.15", "latest").unwrap();
        assert_eq!(plan.target_version, "0.0.0.17");
        assert_eq!((plan.start, plan.end), (1, 2));
    }

    #[test]
    fn plan_resolves_explicit_version() {
        let plan = plan_patches(&chain(), "0.0.0.14", "0.0.0.16").unwrap();
        assert_eq!(plan.target_version, "0.0.0.16");
        assert_eq!((plan.start, plan.end), (0, 1));
    }

    #[test]
    fn plan_already_at_target_is_empty() {
        let plan = plan_patches(&chain(), "0.0.0.17", "latest").unwrap();
        assert!(plan.start > plan.end);
    }

    #[test]
    fn plan_rejects_unknown_target() {
        assert!(plan_patches(&chain(), "0.0.0.15", "9.9.9.9").is_none());
    }

    #[test]
    fn plan_rejects_unknown_start() {
        assert!(plan_patches(&chain(), "0.0.0.99", "latest").is_none());
    }

    #[test]
    fn plan_rejects_downgrade() {
        assert!(plan_patches(&chain(), "0.0.0.17", "0.0.0.15").is_none());
    }

    #[test]
    fn plan_rejects_empty_patch_list() {
        assert!(plan_patches(&[], "0.0.0.15", "latest").is_none());
    }

    #[test]
    fn decide_only_warns_on_exact_match() {
        assert_eq!(decide(100, 100, true), EXIT_OK);
        assert_eq!(decide(101, 100, true), EXIT_TOO_LARGE);
        // A fallback comparison is advisory even when the patches are larger.
        assert_eq!(decide(1000, 10, false), EXIT_OK);
    }

    #[test]
    fn retry_returns_first_success() {
        let mut attempts = 0;
        let out: Result<u32> = retry_with_delay(3, Duration::ZERO, || {
            attempts += 1;
            Ok(42)
        });
        assert_eq!(out.unwrap(), 42);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn retry_uses_all_attempts_before_giving_up() {
        let mut attempts = 0;
        let out: Result<u32> = retry_with_delay(3, Duration::ZERO, || {
            attempts += 1;
            anyhow::bail!("nope")
        });
        assert!(out.is_err());
        assert_eq!(attempts, 4); // initial attempt + 3 retries
    }
}
