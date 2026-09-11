//! Upgrade path check.
//!
//! Compares the total size of the incremental patch chain needed to reach a
//! target version against the size of the full client for that version, so the
//! caller can decide whether patching is cheaper than re-downloading.
//!
//! CMS and CMS_CW publish a patch manifest, so the chain is derived from it;
//! TMS publishes individual `.patch` files, so its chain is discovered by
//! probing the patch server. Because the result is a diagnostic, expected
//! failures are reported through a process exit code (see the `EXIT_*`
//! constants) rather than a Rust error.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

use crate::cli::Region;
use crate::cms;
use crate::cms_cw;
use crate::cms_patch;
use crate::progress::format_size;
use crate::tms;
use crate::tms_patch;

/// Exit code: the patch chain is the same size as, or smaller than, the full
/// client for the target version.
pub const EXIT_OK: i32 = 0;
/// Exit code: no applicable patch chain exists for the requested version.
pub const EXIT_NO_PATCH: i32 = 1;
/// Exit code: the patch chain is larger than the full client for the version.
pub const EXIT_TOO_LARGE: i32 = 2;
/// Exit code: the current client version could not be read.
pub const EXIT_NO_VERSION: i32 = 3;
/// Exit code: the requested target version is older than the installed client.
pub const EXIT_TARGET_OLDER: i32 = 4;
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
    // Echo the requested target verbatim (so `latest` stays `latest`).
    println!(
        "cmsdl {VERSION}: checking upgrade path for region '{region}' to version {version}."
    );

    match region {
        Region::Cms => run_inner(region, version, target_dir, allow_insecure, proxy, verbose),
        Region::CmsCw => cms::with_config(cms_cw::CW_CONFIG, || {
            run_inner(region, version, target_dir, allow_insecure, proxy, verbose)
        }),
        Region::Tms => run_tms(version, target_dir, allow_insecure, proxy, verbose),
        Region::Manual => {
            bail!("--upgrade-path-check is only supported for region 'cms', 'cms_cw', or 'tms'")
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
        Err(e) => {
            debug_net("fetching patch metadata", &e);
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
        Ok(p) => p,
        Err(PlanError::TargetOlder) => {
            println!("error: target client version is older than current client version");
            return Ok(EXIT_TARGET_OLDER);
        }
        Err(PlanError::NoPatch) => {
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
        Err(e) => {
            debug_net("obtaining challenge key", &e);
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
            Err(e) => {
                debug_net("fetching patch file sizes", &e);
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
        Err(e) => {
            debug_net("locating the target full client", &e);
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

// ── TMS ─────────────────────────────────────────────────────────────────────

/// A single TMS `.patch` file in the upgrade chain.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TmsPatch {
    from: i16,
    to: i16,
    size: u64,
}

impl TmsPatch {
    /// The CDN file name for this patch (e.g. `00280to00285.patch`).
    fn file_name(&self) -> String {
        format!("{:05}to{:05}.patch", self.from, self.to)
    }
}

/// Upgrade-path check for the TMS region.
///
/// TMS publishes individual `.patch` files (one per version hop) rather than a
/// patch manifest, so the chain is discovered by probing the patch server for
/// the largest available jump at each step.
fn run_tms(
    version_arg: &str,
    target_dir: &Path,
    allow_insecure: bool,
    proxy: Option<&str>,
    verbose: bool,
) -> Result<i32> {
    // ── 1. Current client version (Data/Base/Base.wz) ─────────────────────
    let current = match read_tms_base_version(target_dir) {
        Some(v) => v,
        None => {
            println!("error: current client version cannot be read");
            return Ok(EXIT_NO_VERSION);
        }
    };

    let agent = crate::net::agent(allow_insecure, proxy);
    // The full-client manifest (and therefore the client check) is only used
    // when the target was requested as `latest`.
    let check_client = version_arg.eq_ignore_ascii_case("latest");

    // ── 2. Target version ─────────────────────────────────────────────────
    let target: i16 = if check_client {
        match retry(NETWORK_RETRIES, || tms_patch::get_latest_version(&agent)) {
            Ok(v) => v,
            Err(e) => {
                debug_net("resolving the latest TMS version", &e);
                println!("error: patch server cannot be accessed");
                return Ok(EXIT_SERVER);
            }
        }
    } else {
        match version_arg.parse::<i16>() {
            Ok(v) if v > 0 => v,
            _ => {
                println!("error: no applicable patch can be found");
                return Ok(EXIT_NO_PATCH);
            }
        }
    };

    if current > target {
        println!("error: target client version is older than current client version");
        return Ok(EXIT_TARGET_OLDER);
    }

    // ── 3. Required patch chain ───────────────────────────────────────────
    let plan = match plan_tms_chain(&agent, current, target) {
        Ok(plan) => plan,
        Err(e) => {
            debug_net("discovering the TMS patch chain", &e);
            println!("error: patch server cannot be accessed");
            return Ok(EXIT_SERVER);
        }
    };
    let chain = plan.chain;

    // The full client can be published before the patch that reaches it. When
    // that happens for `latest` (and at least one patch is available), fall back
    // to the last version actually reachable through the patch server. When no
    // patch at all leaves the current version — e.g. a client that is not a TMS
    // client — there is no upgrade path.
    let target = match resolve_tms_target(check_client, current, target, plan.reached) {
        TmsTarget::Reached => target,
        TmsTarget::Fallback(reached) => {
            eprintln!(
                "warning: version {target} is not yet available on the patch server; \
                 falling back to the last available version {reached}."
            );
            reached
        }
        TmsTarget::NoPatch => {
            println!("error: no applicable patch can be found");
            return Ok(EXIT_NO_PATCH);
        }
    };
    let major_sum: u64 = chain.iter().map(|p| p.size).sum();

    // ── 4. Full client size for the target version (only for `latest`) ────
    // The TMS download server only publishes the latest client manifest, so an
    // explicit (typically older) target has nothing meaningful to compare
    // against. In that case the client check is skipped entirely.
    let check_client = version_arg.eq_ignore_ascii_case("latest");
    let mut client_size: u64 = 0;
    let mut client_version = String::new();
    let mut exact = false;
    let mut minor: Option<u64> = None;

    if check_client {
        let info = match retry(NETWORK_RETRIES, || tms::get_product_info(&agent)) {
            Ok(i) => i,
            Err(e) => {
                debug_net("fetching the TMS product manifest", &e);
                println!("error: patch server cannot be accessed");
                return Ok(EXIT_SERVER);
            }
        };
        client_size = info.files.iter().map(|f| f.size_in_bytes).sum();
        exact = parse_tms_version_number(&info.version) == Some(target);
        client_version = info.version;

        // When the client is already on the latest major version the patcher
        // still fetches the standalone executable hotfix (`ExePatch.dat`), so
        // its size counts towards the patch route.
        if exact && current == target {
            minor = probe_exe_patch_size(&agent, target);
        }
    }
    let patch_sum = major_sum + minor.unwrap_or(0);

    // ── 5. Human-readable summary ─────────────────────────────────────────
    println!("current version: {current}");
    println!("target version:  {target}");
    println!(
        "patches needed:  {} ({})",
        chain.len(),
        format_size(major_sum)
    );
    if let Some(size) = minor {
        println!(
            "minor patch:     V{target} ExePatch.dat ({})",
            format_size(size)
        );
    }
    if check_client {
        let note = if exact {
            String::new()
        } else {
            " [fallback: target full client not available]".to_string()
        };
        println!(
            "full client:     version {client_version} ({}){}",
            format_size(client_size),
            note
        );
    } else {
        println!(
            "full client:     (skipped: TMS only publishes the latest manifest for an explicit target)"
        );
    }

    // ── 5b. Detailed upgrade path (--verbose) ─────────────────────────────
    if verbose {
        if chain.is_empty() && minor.is_none() {
            println!("upgrade path:     (already at the target version; no patches needed)");
        } else {
            println!("upgrade path:");
            for p in &chain {
                println!(
                    "  {:05} -> {:05}: {} (1 file(s))",
                    p.from,
                    p.to,
                    format_size(p.size)
                );
                println!("      {}: {}", p.file_name(), format_size(p.size));
            }
            if let Some(size) = minor {
                println!(
                    "  minor patch (V{target}): {} (1 file(s))",
                    format_size(size)
                );
                println!("      ExePatch.dat: {}", format_size(size));
            }
        }
    }

    // ── 6. Comparison and exit code ───────────────────────────────────────
    let command = format!(
        "{} {} --patch {target} {}",
        process_name(),
        Region::Tms.code(),
        target_dir.display()
    );

    if !check_client {
        // Explicit target: the server only publishes the latest manifest, so
        // there is nothing meaningful to compare against.
        println!("you may apply the patch with {command}");
        return Ok(EXIT_OK);
    }

    if exact {
        let code = decide(patch_sum, client_size, true);
        if code == EXIT_TOO_LARGE {
            println!(
                "warning: required patches to {target} version are larger than \
                 the {target} client itself"
            );
        } else {
            println!("you may apply the patch with {command}");
        }
        Ok(code)
    } else {
        eprintln!(
            "warning: no full client found for version {target}; \
             compared against version {client_version} instead."
        );
        if patch_sum > client_size {
            eprintln!(
                "warning: required patches to {target} version are larger than \
                 the {client_version} client itself"
            );
        }
        println!("you may apply the patch with {command}");
        Ok(EXIT_OK)
    }
}

/// Read the client version from `<target_dir>/Data/Base/Base.wz`.
fn read_tms_base_version(target_dir: &Path) -> Option<i16> {
    let wz_path = target_dir.join("Data").join("Base").join("Base.wz");
    let wz = crate::miniwzlib::get_wz_version(&wz_path).ok()?;
    if wz.version == 0 {
        None
    } else {
        Some(wz.version)
    }
}

/// Parse the numeric version out of a TMS product version string
/// (e.g. `"V280"` → `280`).
fn parse_tms_version_number(version: &str) -> Option<i16> {
    let start = version.find(|c: char| c.is_ascii_digit())?;
    let tail = &version[start..];
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse::<i16>().ok()
}

/// A resolved TMS upgrade path.
struct TmsPlan {
    /// The patches to apply, in order.
    chain: Vec<TmsPatch>,
    /// The version the chain reaches. Equals the requested target when fully
    /// reachable, otherwise the last version the patch server can reach.
    reached: i16,
}

/// What to do with the requested TMS target after the chain search.
#[derive(Debug, PartialEq, Eq)]
enum TmsTarget {
    /// The requested target is reachable as-is.
    Reached,
    /// Fall back to the last reachable version.
    Fallback(i16),
    /// No applicable patch chain exists.
    NoPatch,
}

/// Decide the effective TMS target from the chain search result.
///
/// A partial chain is only acceptable for `latest`, where the full client may
/// be published before its patch. Falling back requires that at least one patch
/// was found (`reached > current`); an explicit unreachable target, or a client
/// with no patch leaving its current version, has no upgrade path.
fn resolve_tms_target(check_client: bool, current: i16, target: i16, reached: i16) -> TmsTarget {
    if reached >= target {
        TmsTarget::Reached
    } else if check_client && reached > current {
        TmsTarget::Fallback(reached)
    } else {
        TmsTarget::NoPatch
    }
}

/// Discover the TMS patch chain from `current` up to `target`.
///
/// Mirrors the patcher's greedy strategy: at each step, probe from the target
/// version downwards and take the first (largest) patch that exists. The search
/// stops at the last reachable version; `Err` is returned only when the patch
/// server cannot be reached at all.
fn plan_tms_chain(agent: &ureq::Agent, current: i16, target: i16) -> Result<TmsPlan> {
    plan_tms_chain_with(current, target, |from, to| probe_tms_patch(agent, from, to))
}

/// Testable core of [`plan_tms_chain`] with an injectable probe.
///
/// A probe error (`Err`) is treated as a transient failure for that candidate
/// and skipped, mirroring the patcher's retry-with-closer-target behaviour. The
/// search stops when no patch leaves a version; `Err` is returned only when no
/// probe ever received a response (the server is unreachable).
fn plan_tms_chain_with<F>(
    current: i16,
    target: i16,
    mut probe: F,
) -> Result<TmsPlan>
where
    F: FnMut(i16, i16) -> Result<Option<u64>>,
{
    let mut chain = Vec::new();
    let mut cur = current;
    let mut any_response = false;
    while cur < target {
        let mut found: Option<(i16, u64)> = None;
        for candidate in (cur + 1..=target).rev() {
            match probe(cur, candidate) {
                Ok(Some(size)) => {
                    any_response = true;
                    found = Some((candidate, size));
                    break;
                }
                Ok(None) => any_response = true,
                Err(e) => debug_net(&format!("probe {cur:05}->{candidate:05}"), &e),
            }
        }
        match found {
            Some((to, size)) => {
                chain.push(TmsPatch { from: cur, to, size });
                cur = to;
            }
            None if any_response => break,
            None => return Err(anyhow::anyhow!("patch server cannot be accessed")),
        }
    }
    Ok(TmsPlan { chain, reached: cur })
}

/// Probe the TMS patch server for the patch from `from` to `to`.
fn probe_tms_patch(agent: &ureq::Agent, from: i16, to: i16) -> Result<Option<u64>> {
    probe_with_https_fallback(agent, &tms_patch::build_patch_url(from, to))
}

/// Probe `http_url`, falling back to the HTTPS equivalent only when the plain
/// HTTP request fails at the transport level.
///
/// A definitive answer from either scheme (`Ok(Some)` / `Ok(None)`) is
/// returned as-is; `Err` means both schemes failed to connect.
fn probe_with_https_fallback(agent: &ureq::Agent, http_url: &str) -> Result<Option<u64>> {
    match probe_tms_file(agent, http_url) {
        Ok(found) => Ok(found),
        Err(primary) => {
            let https = http_url.replacen("http://", "https://", 1);
            match probe_tms_file(agent, &https) {
                Ok(found) => Ok(found),
                Err(_) => Err(primary),
            }
        }
    }
}

/// Probe a single TMS patch URL for its size.
fn probe_tms_file(agent: &ureq::Agent, url: &str) -> Result<Option<u64>> {
    let head = match agent.head(url).call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(_, _)) => return Ok(None),
        Err(ureq::Error::Transport(t)) => {
            return Err(anyhow::anyhow!("transport error: {t}"))
        }
    };
    if let Some(n) = head
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
    {
        return Ok(Some(n));
    }

    // Some servers omit Content-Length on HEAD; fall back to a ranged GET.
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => Ok(probe_response_size(&resp)),
        Err(ureq::Error::Status(_, _)) => Ok(None),
        Err(ureq::Error::Transport(t)) => Err(anyhow::anyhow!("transport error: {t}")),
    }
}

/// Extract the total file size from a ranged-GET response.
fn probe_response_size(resp: &ureq::Response) -> Option<u64> {
    if let Some(range) = resp.header("Content-Range") {
        if let Some(total) = range.split('/').nth(1) {
            if let Ok(n) = total.parse::<u64>() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    resp.header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| *n > 0)
}

/// Probe the standalone executable hotfix (`ExePatch.dat`) size for `version`.
///
/// Best-effort: any failure (including a transport error) is treated as "no
/// minor patch", matching the patcher's non-fatal handling.
fn probe_exe_patch_size(agent: &ureq::Agent, version: i16) -> Option<u64> {
    probe_with_https_fallback(agent, &tms_patch::build_exe_patch_url(version))
        .ok()
        .flatten()
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

/// Why [`plan_patches`] could not produce a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanError {
    /// No applicable patch chain exists for the requested version.
    NoPatch,
    /// The requested target is older than the installed client version.
    TargetOlder,
}

/// Resolve the patch chain from `current` to the requested `version_arg`
/// (a concrete version or `latest`).
///
/// Returns [`PlanError::TargetOlder`] when the target predates the installed
/// version, and [`PlanError::NoPatch`] for any other missing chain (unknown
/// target, empty patch list, unknown starting point).
fn plan_patches(
    packages: &[cms::PatchPackage],
    current: &str,
    version_arg: &str,
) -> std::result::Result<Plan, PlanError> {
    let target_version = if version_arg.eq_ignore_ascii_case("latest") {
        packages.last().ok_or(PlanError::NoPatch)?.to.clone()
    } else {
        version_arg.to_owned()
    };

    // A downgrade is not a valid upgrade path. Checked before the target
    // lookup so an older-but-unpublished version still reports a downgrade.
    if cms_patch::version_newer_than(current, &target_version) {
        return Err(PlanError::TargetOlder);
    }

    let end = packages
        .iter()
        .position(|p| p.to == target_version)
        .ok_or(PlanError::NoPatch)?;

    // Already at the target: an empty patch range.
    if current == target_version {
        return Ok(Plan {
            target_version,
            start: end + 1,
            end,
        });
    }

    let start = packages
        .iter()
        .position(|p| p.from == current)
        .ok_or(PlanError::NoPatch)?;
    if start > end {
        return Err(PlanError::NoPatch);
    }
    Ok(Plan {
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

/// Print a diagnostic for a failed network operation on debug builds only.
///
/// Release builds stay quiet so the exit-code contract is unaffected; debug
/// builds (`cargo run` / `cargo build`) show the underlying error chain.
fn debug_net(context: &str, err: &anyhow::Error) {
    if cfg!(debug_assertions) {
        eprintln!("debug: {context}: {err:#}");
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
        assert_eq!(
            plan_patches(&chain(), "0.0.0.15", "9.9.9.9"),
            Err(PlanError::NoPatch)
        );
    }

    #[test]
    fn plan_rejects_unknown_start() {
        // A non-standard version older than the target is not a known start.
        assert_eq!(
            plan_patches(&chain(), "0.0.0.15.5", "latest"),
            Err(PlanError::NoPatch)
        );
    }

    #[test]
    fn plan_rejects_downgrade() {
        assert_eq!(
            plan_patches(&chain(), "0.0.0.17", "0.0.0.15"),
            Err(PlanError::TargetOlder)
        );
    }

    #[test]
    fn plan_rejects_older_unknown_target_as_downgrade() {
        // 0.0.0.5 is not a published patch target, but it predates the client.
        assert_eq!(
            plan_patches(&chain(), "0.0.0.15", "0.0.0.5"),
            Err(PlanError::TargetOlder)
        );
    }

    #[test]
    fn plan_rejects_empty_patch_list() {
        assert_eq!(
            plan_patches(&[], "0.0.0.15", "latest"),
            Err(PlanError::NoPatch)
        );
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

    #[test]
    fn tms_chain_picks_largest_jump() {
        let available = [(280, 285), (285, 290)];
        let plan = plan_tms_chain_with(280, 290, |from, to| {
            Ok(available.contains(&(from, to)).then_some(to as u64))
        })
        .unwrap();
        assert_eq!(plan.reached, 290);
        assert_eq!(
            plan.chain,
            vec![
                TmsPatch { from: 280, to: 285, size: 285 },
                TmsPatch { from: 285, to: 290, size: 290 },
            ]
        );
    }

    #[test]
    fn tms_chain_stops_before_unreachable_target() {
        let available = [(280, 281)];
        let plan = plan_tms_chain_with(280, 285, |from, to| {
            Ok(available.contains(&(from, to)).then_some(1))
        })
        .unwrap();
        assert_eq!(plan.reached, 281);
        assert_eq!(plan.chain, vec![TmsPatch { from: 280, to: 281, size: 1 }]);
    }

    #[test]
    fn tms_chain_empty_when_at_target() {
        let plan = plan_tms_chain_with(285, 285, |_, _| Ok(Some(1))).unwrap();
        assert!(plan.chain.is_empty());
        assert_eq!(plan.reached, 285);
    }

    #[test]
    fn tms_chain_mirrors_greedy_patcher() {
        // The largest available jump (280 -> 286) leads to a dead end, so the
        // chain stops there even though 280 -> 285 -> 290 exists.
        let available = [(280, 286), (280, 285), (285, 290)];
        let plan = plan_tms_chain_with(280, 290, |from, to| {
            Ok(available.contains(&(from, to)).then_some(1))
        })
        .unwrap();
        assert_eq!(plan.reached, 286);
        assert_eq!(plan.chain, vec![TmsPatch { from: 280, to: 286, size: 1 }]);
    }

    #[test]
    fn tms_chain_falls_back_to_last_reachable_version() {
        // 283 (the published full client) has no patch yet, but 282 does.
        let available = [(280, 282)];
        let plan = plan_tms_chain_with(280, 283, |from, to| {
            Ok(available.contains(&(from, to)).then_some(11))
        })
        .unwrap();
        assert_eq!(plan.reached, 282);
        assert_eq!(plan.chain, vec![TmsPatch { from: 280, to: 282, size: 11 }]);
    }

    #[test]
    fn tms_chain_tolerates_transient_probe_errors() {
        let plan = plan_tms_chain_with(280, 282, |from, to| match (from, to) {
            (280, 281) => Ok(Some(7)),
            (281, 282) => Ok(Some(9)),
            _ => anyhow::bail!("timeout"),
        })
        .unwrap();
        assert_eq!(plan.reached, 282);
        assert_eq!(
            plan.chain,
            vec![
                TmsPatch { from: 280, to: 281, size: 7 },
                TmsPatch { from: 281, to: 282, size: 9 },
            ]
        );
    }

    #[test]
    fn tms_target_keeps_reachable_target() {
        assert_eq!(resolve_tms_target(true, 280, 282, 282), TmsTarget::Reached);
        assert_eq!(resolve_tms_target(false, 280, 281, 281), TmsTarget::Reached);
    }

    #[test]
    fn tms_target_falls_back_only_for_latest_with_progress() {
        assert_eq!(
            resolve_tms_target(true, 280, 283, 282),
            TmsTarget::Fallback(282)
        );
        // No patch at all leaves the current version (e.g. not a TMS client).
        assert_eq!(resolve_tms_target(true, 253, 282, 253), TmsTarget::NoPatch);
        // An explicit unreachable target has no path.
        assert_eq!(resolve_tms_target(false, 280, 285, 282), TmsTarget::NoPatch);
    }

    #[test]
    fn tms_chain_errors_when_server_unreachable() {
        let out = plan_tms_chain_with(280, 285, |_, _| anyhow::bail!("unreachable"));
        assert!(out.is_err());
    }

    #[test]
    fn parses_tms_version_number() {
        assert_eq!(parse_tms_version_number("V280"), Some(280));
        assert_eq!(parse_tms_version_number("280"), Some(280));
        assert_eq!(parse_tms_version_number(""), None);
        assert_eq!(parse_tms_version_number("V"), None);
    }
}
