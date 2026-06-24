//! CMS incremental patch application.
//!
//! A patch upgrades the client from one version to the next. Each patch is
//! described by a `FileList.dat` (a JSON list of zip parts) and the zips
//! themselves. The first zip contains `patch_delta_direct.dat`, an XML manifest
//! describing, for every affected file:
//!
//! - which files are patched with an HDiffPatch `*.hdiff` delta
//!   ([`Manifest::deltas`]), with the source, hdiff and result MD5s;
//! - which files are newly added ([`Manifest::news`]);
//! - which files are deleted ([`Manifest::deletions`]).
//!
//! Actual byte-level patching of `HDIFFSF20` deltas is delegated to the
//! `hdiffpatch-rs` crate.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use md5::{Digest, Md5};

use crate::cms;

/// Name of the XML manifest stored at the root of the first zip.
const MANIFEST_NAME: &str = "patch_delta_direct.dat";

/// Name of the version marker written under `<target>/mxd`.
const VERSION_FILE_NAME: &str = "cmsdl.ver";

/// Number of files patched concurrently within a single zip part.
const PARALLEL_FILES: usize = 10;

/// If no data arrives for this long, the connection is treated as stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for establishing a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum consecutive stalls (no bytes received) tolerated for a download.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before re-signing and resuming a stalled download.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

/// A single file to be patched with an HDiffPatch delta.
#[derive(Debug, Clone)]
pub(crate) struct DeltaEntry {
    /// Expected MD5 of the source file, before patching.
    pub origin_md5: String,
    /// Expected MD5 of the patched file.
    pub result_md5: String,
    /// Expected MD5 of the `.hdiff` delta file itself.
    pub delta_md5: String,
}

/// The parsed `patch_delta_direct.dat` manifest.
#[derive(Debug, Default, Clone)]
pub(crate) struct Manifest {
    /// Map of source file path (backslash form, e.g. `mxd\Data\Base\Base.wz`)
    /// to its delta information.
    pub deltas: HashMap<String, DeltaEntry>,
    /// Reverse lookup: zip hdiff path (backslash) -> source file path.
    pub delta_by_hdiff: HashMap<String, String>,
    /// Map of newly added file path -> its path within the zip.
    pub news: HashMap<String, String>,
    /// Reverse lookup: zip path (backslash) -> target file path.
    pub new_by_zip: HashMap<String, String>,
    /// Result MD5s, keyed by target file path (covers new files when present).
    pub result_md5: HashMap<String, String>,
    /// Files to delete (backslash form).
    pub deletions: Vec<String>,
}

/// Per-zip application tallies.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ZipStats {
    pub patched: usize,
    pub added: usize,
    pub skipped: usize,
    pub corrupted: usize,
}

/// Parse the `patch_delta_direct.dat` XML into a [`Manifest`].
pub(crate) fn parse_manifest(xml: &str) -> Result<Manifest> {
    let doc = roxmltree::Document::parse(xml).context("invalid patch manifest XML")?;

    // Collect `<TagSubItem Key=".." Value=".."/>` pairs under a given parent.
    let collect = |parent: &str| -> Vec<(String, String)> {
        doc.descendants()
            .find(|n| n.has_tag_name(parent))
            .map(|p| {
                p.children()
                    .filter(|c| c.is_element())
                    .filter_map(|c| {
                        let k = c.attribute("Key")?;
                        let v = c.attribute("Value")?;
                        Some((k.to_owned(), v.to_owned()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let delta_paths = collect("DeltaPathInfo"); // source -> Pkg\..hdiff
    let new_paths = collect("NewPathInfo"); // target -> Pkg\..
    let del_paths = collect("DelPathInfo"); // path -> path
    let delta_md5 = collect("DeltaMD5Info"); // mxd\..hdiff -> md5
    let origin_md5 = collect("OriginMD5Info"); // source -> md5
    let result_md5 = collect("ResultMD5Info"); // source -> md5

    let delta_md5: HashMap<String, String> = delta_md5.into_iter().collect();
    let origin_md5: HashMap<String, String> = origin_md5.into_iter().collect();
    let result_md5_map: HashMap<String, String> = result_md5.into_iter().collect();

    let mut manifest = Manifest::default();

    for (source, hdiff_zip_path) in delta_paths {
        // The DeltaMD5Info key is the hdiff path without the leading `Pkg\`.
        let hdiff_key = strip_pkg_prefix(&hdiff_zip_path);
        let entry = DeltaEntry {
            origin_md5: origin_md5.get(&source).cloned().unwrap_or_default(),
            result_md5: result_md5_map.get(&source).cloned().unwrap_or_default(),
            delta_md5: delta_md5.get(&hdiff_key).cloned().unwrap_or_default(),
        };
        manifest
            .delta_by_hdiff
            .insert(norm_backslash(&hdiff_zip_path), source.clone());
        manifest.deltas.insert(source, entry);
    }

    for (target, zip_path) in new_paths {
        manifest
            .new_by_zip
            .insert(norm_backslash(&zip_path), target.clone());
        manifest.news.insert(target, zip_path);
    }

    manifest.result_md5 = result_md5_map;
    manifest.deletions = del_paths.into_iter().map(|(k, _)| k).collect();

    Ok(manifest)
}

/// Remove a leading `Pkg\` (or `Pkg/`) prefix from a zip path.
fn strip_pkg_prefix(path: &str) -> String {
    let p = path.strip_prefix("Pkg\\").or_else(|| path.strip_prefix("Pkg/"));
    p.unwrap_or(path).to_owned()
}

/// Normalize a path to use backslash separators (the form used in the manifest).
fn norm_backslash(s: &str) -> String {
    s.replace('/', "\\")
}

/// Join a backslash/forward-slash relative `key` onto `base` as a real path.
fn rel_join(base: &Path, key: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for comp in key.split(['\\', '/']).filter(|s| !s.is_empty()) {
        p.push(comp);
    }
    p
}

/// Compute the uppercase hex MD5 of a file's contents, streaming from disk.
fn md5_file_upper(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode_upper(hasher.finalize()))
}

/// Stream the current zip entry to `dest`, creating parent directories.
fn extract_entry<R: Read>(entry: &mut R, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut out = std::fs::File::create(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;
    std::io::copy(entry, &mut out).with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}

/// Apply HDiffPatch `diff` to `source`, producing `dest`. Returns `false` if the
/// patch could not be applied.
fn apply_hdiff(source: &Path, diff: &Path, dest: &Path) -> bool {
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    hdiffpatch_rs::patchers::HDiff::new(
        source.to_string_lossy().into_owned(),
        diff.to_string_lossy().into_owned(),
        dest.to_string_lossy().into_owned(),
    )
    .apply()
}

/// Apply every patch entry contained in a single zip part.
///
/// `corrupted` accumulates the source paths that could not be patched (missing
/// source, checksum mismatch, or a failed patch). The manifest (parsed from the
/// first zip) drives which entries are deltas vs. new files.
///
/// Up to [`PARALLEL_FILES`] files are patched concurrently. Each worker opens
/// its own handle to the (on-disk) zip, so reads do not contend, and every file
/// writes to its own unique temporary paths.
pub(crate) fn apply_zip(
    zip_path: &Path,
    manifest: &Manifest,
    target_dir: &Path,
    patchdata: &Path,
    corrupted: &mut Vec<String>,
) -> Result<ZipStats> {
    let patchfile_dir = patchdata.join("patchfile");
    let patched_dir = patchdata.join("patched");

    // First pass: enumerate the entries that this zip actually contributes.
    let items = {
        let file = std::fs::File::open(zip_path)
            .with_context(|| format!("failed to open {}", zip_path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("invalid zip {}", zip_path.display()))?;
        let mut items: Vec<WorkItem> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let name = norm_backslash(entry.name());
            if name.eq_ignore_ascii_case(MANIFEST_NAME) {
                continue;
            }
            if let Some(source_key) = manifest.delta_by_hdiff.get(&name) {
                items.push(WorkItem {
                    index: i,
                    name,
                    kind: Kind::Delta(source_key.clone()),
                });
            } else if let Some(target_key) = manifest.new_by_zip.get(&name) {
                items.push(WorkItem {
                    index: i,
                    name,
                    kind: Kind::New(target_key.clone()),
                });
            }
            // Unknown entries (not referenced by the manifest) are ignored.
        }
        items
    };

    let pb = ProgressBar::new(items.len() as u64);
    pb.set_style(ProgressStyle::with_template("    [{pos}/{len}] {wide_msg}").unwrap());
    pb.enable_steady_tick(Duration::from_millis(120));

    let next = AtomicUsize::new(0);
    let patched = AtomicUsize::new(0);
    let added = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let corrupted_count = AtomicUsize::new(0);
    let local_corrupted: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

    let workers = PARALLEL_FILES.min(items.len()).max(1);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                // Each worker reads from its own archive handle.
                let archive = std::fs::File::open(zip_path)
                    .map_err(anyhow::Error::from)
                    .and_then(|f| zip::ZipArchive::new(f).map_err(anyhow::Error::from));
                let mut archive = match archive {
                    Ok(a) => a,
                    Err(e) => {
                        *first_err.lock().unwrap() = Some(e);
                        return;
                    }
                };

                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= items.len() {
                        break;
                    }
                    let item = &items[idx];
                    match &item.kind {
                        Kind::Delta(src) => pb.set_message(format!("patching {src}")),
                        Kind::New(tgt) => pb.set_message(format!("adding {tgt}")),
                    }

                    let result = (|| -> Result<Outcome> {
                        let mut entry = archive.by_index(item.index)?;
                        match &item.kind {
                            Kind::Delta(source_key) => apply_delta_entry(
                                &mut entry,
                                source_key,
                                manifest,
                                target_dir,
                                &patchfile_dir,
                                &patched_dir,
                                &item.name,
                            ),
                            Kind::New(target_key) => apply_new_entry(
                                &mut entry,
                                target_key,
                                manifest,
                                target_dir,
                                &patchfile_dir,
                                &patched_dir,
                                &item.name,
                            ),
                        }
                    })();
                    pb.inc(1);

                    match result {
                        Ok(Outcome::Patched) => {
                            patched.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Outcome::Added) => {
                            added.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Outcome::Skipped) => {
                            skipped.fetch_add(1, Ordering::Relaxed);
                        }
                        Ok(Outcome::Corrupted) => {
                            corrupted_count.fetch_add(1, Ordering::Relaxed);
                            if let Kind::Delta(src) = &item.kind {
                                local_corrupted.lock().unwrap().push(src.clone());
                            }
                        }
                        Ok(Outcome::None) => {}
                        Err(e) => {
                            let mut slot = first_err.lock().unwrap();
                            if slot.is_none() {
                                *slot = Some(e);
                            }
                            next.store(items.len(), Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }
    });
    pb.finish_and_clear();

    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }

    corrupted.extend(local_corrupted.into_inner().unwrap());
    Ok(ZipStats {
        patched: patched.into_inner(),
        added: added.into_inner(),
        skipped: skipped.into_inner(),
        corrupted: corrupted_count.into_inner(),
    })
}

/// What a single entry's processing resulted in.
enum Outcome {
    Patched,
    Added,
    Skipped,
    Corrupted,
    None,
}

/// The role of a zip entry to be processed.
enum Kind {
    /// A delta patch for an existing file (carries the source path).
    Delta(String),
    /// A newly added file (carries the target path).
    New(String),
}

/// A unit of work dispatched to a patch worker thread.
struct WorkItem {
    index: usize,
    name: String,
    kind: Kind,
}

/// Patch a single existing file from its `.hdiff` delta (see module docs).
fn apply_delta_entry<R: Read>(
    entry: &mut R,
    source_key: &str,
    manifest: &Manifest,
    target_dir: &Path,
    patchfile_dir: &Path,
    patched_dir: &Path,
    zip_name: &str,
) -> Result<Outcome> {
    let de = &manifest.deltas[source_key];
    let source_path = rel_join(target_dir, source_key);

    // The source must exist and currently match the expected origin checksum.
    if !source_path.exists() {
        return Ok(Outcome::Corrupted);
    }
    let current = md5_file_upper(&source_path)?;
    if current.eq_ignore_ascii_case(&de.result_md5) {
        // Already at the target version: nothing to do.
        return Ok(Outcome::Skipped);
    }
    if !current.eq_ignore_ascii_case(&de.origin_md5) {
        return Ok(Outcome::Corrupted);
    }

    // Extract the hdiff and verify its own checksum before applying.
    let hdiff_path = rel_join(patchfile_dir, zip_name);
    extract_entry(entry, &hdiff_path)?;
    let hdiff_md5 = md5_file_upper(&hdiff_path)?;
    if !hdiff_md5.eq_ignore_ascii_case(&de.delta_md5) {
        let _ = std::fs::remove_file(&hdiff_path);
        return Ok(Outcome::Corrupted);
    }

    // Patch to a temporary file, verify, then atomically replace the source.
    let patched_path = rel_join(patched_dir, source_key);
    let applied = apply_hdiff(&source_path, &hdiff_path, &patched_path);
    let outcome = if applied {
        let patched_md5 = md5_file_upper(&patched_path).unwrap_or_default();
        if patched_md5.eq_ignore_ascii_case(&de.result_md5) {
            replace_file(&patched_path, &source_path)?;
            Outcome::Patched
        } else {
            Outcome::Corrupted
        }
    } else {
        Outcome::Corrupted
    };

    let _ = std::fs::remove_file(&patched_path);
    let _ = std::fs::remove_file(&hdiff_path);
    Ok(outcome)
}

/// Install a newly added file (raw, or applied onto an empty base if it is a
/// `.hdiff`).
fn apply_new_entry<R: Read>(
    entry: &mut R,
    target_key: &str,
    manifest: &Manifest,
    target_dir: &Path,
    patchfile_dir: &Path,
    patched_dir: &Path,
    zip_name: &str,
) -> Result<Outcome> {
    let target_path = rel_join(target_dir, target_key);

    if zip_name.to_ascii_lowercase().ends_with(".hdiff") {
        // Apply the delta onto an empty file (no pre-patch checksum to verify).
        let hdiff_path = rel_join(patchfile_dir, zip_name);
        extract_entry(entry, &hdiff_path)?;

        let empty_path = hdiff_path.with_extension("hdiff.empty");
        std::fs::File::create(&empty_path)
            .with_context(|| format!("failed to create {}", empty_path.display()))?;

        let patched_path = rel_join(patched_dir, target_key);
        let applied = apply_hdiff(&empty_path, &hdiff_path, &patched_path);
        let mut outcome = Outcome::None;
        if applied {
            // Verify against the result checksum when the manifest provides one.
            let ok = manifest
                .result_md5
                .get(target_key)
                .map(|want| {
                    md5_file_upper(&patched_path)
                        .map(|got| got.eq_ignore_ascii_case(want))
                        .unwrap_or(false)
                })
                .unwrap_or(true);
            if ok {
                replace_file(&patched_path, &target_path)?;
                outcome = Outcome::Added;
            }
        }
        let _ = std::fs::remove_file(&patched_path);
        let _ = std::fs::remove_file(&hdiff_path);
        let _ = std::fs::remove_file(&empty_path);
        Ok(outcome)
    } else {
        // Plain new file: extract straight to its destination.
        extract_entry(entry, &target_path)?;
        Ok(Outcome::Added)
    }
}

/// Move `from` to `to`, falling back to copy+delete across filesystems.
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .with_context(|| format!("failed to copy into {}", to.display()))?;
    let _ = std::fs::remove_file(from);
    Ok(())
}

/// A single zip part listed in a patch's `FileList.dat`.
#[derive(Debug, Clone, serde::Deserialize)]
struct PatchZip {
    /// Path of the zip relative to the patch `baseUrl` (e.g. `/5_..._1.zip`).
    url: String,
    /// Expected MD5 of the zip.
    md5: String,
    /// Expected size in bytes, as a string.
    size: String,
}

/// A patch's `FileList.dat`: a base URL plus the ordered list of zip parts.
#[derive(Debug, Clone, serde::Deserialize)]
struct PatchFileList {
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "fileList")]
    file_list: Vec<PatchZip>,
}

/// Read the installed version recorded at `<target>/mxd/cmsdl.ver`, if any.
fn read_installed_version(target_dir: &Path) -> Option<String> {
    let path = target_dir.join("mxd").join(VERSION_FILE_NAME);
    let v = std::fs::read_to_string(path).ok()?.trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Write the installed `version` to `<target>/mxd/cmsdl.ver`.
fn write_installed_version(target_dir: &Path, version: &str) -> Result<()> {
    let mxd = target_dir.join("mxd");
    std::fs::create_dir_all(&mxd)
        .with_context(|| format!("failed to create {}", mxd.display()))?;
    let path = mxd.join(VERSION_FILE_NAME);
    std::fs::write(&path, version).with_context(|| format!("failed to write {}", path.display()))
}

/// Launch the patched client (`<target>/mxd/MapleStory.exe --sqLauncher`).
///
/// The process is spawned without waiting, so cmsdl can exit while the game
/// keeps running.
pub fn launch_client(target_dir: &Path) -> Result<()> {
    let mxd = target_dir.join("mxd");
    let exe = mxd.join("MapleStory.exe");
    if !exe.exists() {
        bail!("cannot launch: {} not found", exe.display());
    }
    println!("launching {} --sqLauncher", exe.display());
    launch_exe(&exe, &mxd)
}

/// Launch `exe --sqLauncher` through the Windows shell.
///
/// `MapleStory.exe` ships a manifest that requests administrator rights, so a
/// plain `CreateProcess` (`std::process::Command`) fails with OS error 740
/// (`ERROR_ELEVATION_REQUIRED`). Launching via the shell (`ShellExecute`, driven
/// here by PowerShell's `Start-Process`) lets Windows show the UAC prompt so the
/// user can elevate manually.
#[cfg(windows)]
fn launch_exe(exe: &Path, working_dir: &Path) -> Result<()> {
    use std::process::Command;

    // Single-quote a value for PowerShell by doubling embedded single quotes.
    fn ps_q(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    let script = format!(
        "Start-Process -FilePath {} -ArgumentList '--sqLauncher' -WorkingDirectory {}",
        ps_q(&exe.to_string_lossy()),
        ps_q(&working_dir.to_string_lossy()),
    );

    let status = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .status()
        .context("failed to launch the client via PowerShell Start-Process")?;

    if !status.success() {
        bail!(
            "could not launch the client (the UAC elevation prompt may have been declined)"
        );
    }
    Ok(())
}

/// Launch `exe --sqLauncher` directly on non-Windows platforms.
#[cfg(not(windows))]
fn launch_exe(exe: &Path, working_dir: &Path) -> Result<()> {
    std::process::Command::new(exe)
        .arg("--sqLauncher")
        .current_dir(working_dir)
        .spawn()
        .with_context(|| format!("failed to launch {}", exe.display()))?;
    Ok(())
}

/// Apply incremental patches to bring the client under `target_dir` up to
/// `max_version` (a version like `0.0.0.15`, or `latest` for the newest).
///
/// The starting point is taken from `<target>/mxd/cmsdl.ver` when present and
/// known; otherwise every patch up to the target version is applied in order.
pub fn apply_patches(
    target_dir: &Path,
    max_version: &str,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    // 1. The client must already be present.
    if !target_dir.join("mxd").is_dir() {
        bail!(
            "no 'mxd' directory found in {}; not a CMS client directory",
            target_dir.display()
        );
    }

    // 2. Fetch the patch metadata and resolve the version chain.
    let data = cms::get_patch_data(allow_insecure, proxy)?;
    if data.packages.is_empty() {
        bail!("no patches are published");
    }

    let final_version = if max_version.eq_ignore_ascii_case("latest") {
        data.packages.last().unwrap().to.clone()
    } else {
        max_version.to_owned()
    };

    let final_idx = data
        .packages
        .iter()
        .position(|p| p.to == final_version)
        .ok_or_else(|| {
            anyhow!(
                "version '{final_version}' is not a patch target; \
                 run `--patch list` to see available versions"
            )
        })?;

    // 3. Determine where to start.
    let installed = read_installed_version(target_dir);
    if installed.as_deref() == Some(final_version.as_str()) {
        println!("already at version {final_version}; nothing to do.");
        return Ok(());
    }
    let start_idx = match installed.as_deref() {
        Some(v) => data
            .packages
            .iter()
            .position(|p| p.from == v)
            .unwrap_or(0),
        None => 0,
    };
    if start_idx > final_idx {
        bail!(
            "installed version is newer than the requested target '{final_version}'"
        );
    }

    let selected = &data.packages[start_idx..=final_idx];
    println!(
        "applying {} patch(es): {} -> {}",
        selected.len(),
        selected.first().unwrap().from,
        final_version
    );

    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();
    let challenge = cms::get_challenge_key(&agent).context("failed to obtain challenge code")?;
    let ver2_base = cms::strip_download_host(&data.base_url).to_owned();

    // 4-5. Apply each patch in turn. The version marker is only advanced when a
    // patch's every zip part applied with no corrupted files.
    let mut last_corrupted: Vec<String> = Vec::new();
    for pkg in selected {
        let mut corrupted = Vec::new();
        apply_one_patch(&agent, &challenge, &ver2_base, pkg, target_dir, &mut corrupted)?;
        if corrupted.is_empty() {
            write_installed_version(target_dir, &pkg.to)?;
            println!("  patched to {} ({})", pkg.to, pkg.version_view);
        } else {
            println!(
                "  patch to {} ({}) completed with {} corrupted file(s); \
                 version marker left at the previous version",
                pkg.to,
                pkg.version_view,
                corrupted.len()
            );
        }
        last_corrupted = corrupted;
    }

    // Clean up the working directory.
    let _ = std::fs::remove_dir_all(target_dir.join("patchdata"));

    // 6. Report, and optionally repair corrupted files from the latest index.
    if last_corrupted.is_empty() {
        println!("patching successful: now at version {final_version}.");
        return Ok(());
    }

    last_corrupted.sort();
    last_corrupted.dedup();
    println!("\n{} file(s) could not be patched:", last_corrupted.len());
    for f in &last_corrupted {
        println!("  {f}");
    }

    if max_version.eq_ignore_ascii_case("latest") {
        println!("\nrepairing corrupted files from the latest full index...");
        let still_failed =
            cms::replace_files_from_latest(target_dir, &last_corrupted, allow_insecure, proxy)?;
        if still_failed.is_empty() {
            write_installed_version(target_dir, &final_version)?;
            println!("all corrupted files were repaired; now at version {final_version}.");
        } else {
            println!("{} file(s) still could not be repaired:", still_failed.len());
            for f in &still_failed {
                println!("  {f}");
            }
            bail!("patching completed with {} unrepaired file(s)", still_failed.len());
        }
    } else {
        bail!(
            "patching completed with {} corrupted file(s); \
             re-run with `--patch latest` to repair them",
            last_corrupted.len()
        );
    }

    Ok(())
}

/// Download, then apply, every zip part of a single patch.
fn apply_one_patch(
    agent: &ureq::Agent,
    challenge: &str,
    ver2_base: &str,
    pkg: &cms::PatchPackage,
    target_dir: &Path,
    corrupted: &mut Vec<String>,
) -> Result<()> {
    println!("\npatch {} -> {} ({})", pkg.from, pkg.to, pkg.version_view);

    // Fetch this patch's FileList.dat (signed).
    let filelist_path = format!("{ver2_base}{}", pkg.file_list_url);
    let filelist_json = download_signed_text(agent, challenge, &filelist_path)
        .context("failed to download patch FileList.dat")?;
    let filelist: PatchFileList =
        serde_json::from_str(&filelist_json).context("failed to parse patch FileList.dat")?;
    let zip_base = cms::strip_download_host(&filelist.base_url).to_owned();

    let patchdata = target_dir.join("patchdata");
    std::fs::create_dir_all(&patchdata)
        .with_context(|| format!("failed to create {}", patchdata.display()))?;

    let mut manifest: Option<Manifest> = None;

    for (i, zip) in filelist.file_list.iter().enumerate() {
        let size: u64 = zip.size.trim().parse().unwrap_or(0);
        let zip_name = zip.url.rsplit(['/', '\\']).next().unwrap_or("part.zip");
        let zip_path = patchdata.join(zip_name);
        let sign_path = format!("{zip_base}{}", zip.url);

        println!(
            "  [{}/{}] downloading {zip_name} ({:.2} MiB)...",
            i + 1,
            filelist.file_list.len(),
            size as f64 / (1024.0 * 1024.0)
        );
        download_signed_to_file(agent, challenge, &sign_path, &zip_path, size, &zip.md5)
            .with_context(|| format!("failed to download {zip_name}"))?;

        // The manifest lives in the first zip and drives every part.
        if i == 0 {
            manifest = Some(read_manifest_from_zip(&zip_path)?);
        }
        let m = manifest
            .as_ref()
            .ok_or_else(|| anyhow!("patch manifest missing from the first zip"))?;

        let stats = apply_zip(&zip_path, m, target_dir, &patchdata, corrupted)?;
        println!(
            "    applied: {} patched, {} added, {} skipped, {} corrupted",
            stats.patched, stats.added, stats.skipped, stats.corrupted
        );

        let _ = std::fs::remove_file(&zip_path);
    }

    // Apply deletions once, after all parts have been processed.
    if let Some(m) = &manifest {
        for del in &m.deletions {
            let p = rel_join(target_dir, del);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    // Drop the per-patch working files (keep patchdata for the next patch).
    let _ = std::fs::remove_dir_all(patchdata.join("patchfile"));
    let _ = std::fs::remove_dir_all(patchdata.join("patched"));
    Ok(())
}

/// Read and parse `patch_delta_direct.dat` from a zip part.
fn read_manifest_from_zip(zip_path: &Path) -> Result<Manifest> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("failed to open {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("invalid zip {}", zip_path.display()))?;
    let mut entry = archive
        .by_name(MANIFEST_NAME)
        .with_context(|| format!("{MANIFEST_NAME} not found in {}", zip_path.display()))?;
    let mut xml = String::new();
    entry
        .read_to_string(&mut xml)
        .context("failed to read patch manifest")?;
    parse_manifest(&xml)
}

/// GET a signed `path` and return its body as text.
fn download_signed_text(agent: &ureq::Agent, challenge: &str, path: &str) -> Result<String> {
    let t = cms::get_current_utc8_time();
    let url = cms::build_signed_url(challenge, t, path);
    let resp = agent.get(&url).call().context("HTTP request failed")?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).context("failed to read response body")?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Download a signed `path` to `dest`, resuming (with a freshly-signed URL) on
/// stalls, then verifying the MD5 against `expected_md5`.
fn download_signed_to_file(
    agent: &ureq::Agent,
    challenge: &str,
    path: &str,
    dest: &Path,
    size: u64,
    expected_md5: &str,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let pb = ProgressBar::new(size);
    pb.set_style(
        ProgressStyle::with_template(
            "    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    let mut pos = 0u64;
    let mut stalls = 0usize;

    while size == 0 || pos < size {
        let before = pos;
        let t = cms::get_current_utc8_time();
        let url = cms::build_signed_url(challenge, t, path);
        let _ = stream_range(agent, &url, &mut file, &mut pos, &pb);

        if size != 0 && pos >= size {
            break;
        }
        if size == 0 && pos > 0 {
            // Unknown length: a clean end of stream means we are done.
            break;
        }

        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                pb.finish_and_clear();
                bail!("download stalled with no progress after {MAX_STALL_RETRIES} retries");
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    pb.finish_and_clear();
    file.flush().ok();
    drop(file);

    // Verify integrity.
    if !expected_md5.is_empty() {
        let got = md5_file_upper(dest)?;
        if !got.eq_ignore_ascii_case(expected_md5) {
            bail!("downloaded file checksum mismatch (expected {expected_md5}, got {got})");
        }
    }
    Ok(())
}

/// Stream a range request starting at `*pos` into `file`, advancing `*pos` as
/// bytes arrive. If the server ignores the range (HTTP 200 with `*pos > 0`),
/// the download restarts from the beginning.
fn stream_range(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    pb: &ProgressBar,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-", *pos))
        .call()
        .context("HTTP range request failed")?;
    let status = resp.status();
    let mut reader = resp.into_reader();

    if status == 200 && *pos != 0 {
        // Server ignored the range; restart from the top.
        *pos = 0;
        pb.set_position(0);
    }
    file.seek(SeekFrom::Start(*pos))
        .context("failed to seek before resuming")?;

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("failed to write to disk")?;
        *pos += n as u64;
        pb.set_position(*pos);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy_dir_all(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            let from = e.path();
            let to = dst.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir_all(&from, &to);
            } else {
                std::fs::copy(&from, &to).unwrap();
            }
        }
    }

    /// Downloads the real first zip of the 0.0.0.14 -> 0.0.0.15 patch and applies
    /// it (via the parallel path) to a copy of the user-provided source files,
    /// verifying the present files reach their expected result MD5.
    #[test]
    #[ignore]
    fn tmp_parallel_apply_first_zip() {
        let data = cms::get_patch_data(false, None).unwrap();
        let pkg = data.packages.iter().find(|p| p.from == "0.0.0.14").unwrap();
        let ver2_base = cms::strip_download_host(&data.base_url).to_owned();

        let agent = crate::net::agent_builder(false, None)
            .timeout_read(STALL_TIMEOUT)
            .timeout_connect(CONNECT_TIMEOUT)
            .build();
        let challenge = cms::get_challenge_key(&agent).unwrap();

        let filelist_path = format!("{ver2_base}{}", pkg.file_list_url);
        let filelist_json = download_signed_text(&agent, &challenge, &filelist_path).unwrap();
        let filelist: PatchFileList = serde_json::from_str(&filelist_json).unwrap();
        let zip_base = cms::strip_download_host(&filelist.base_url).to_owned();

        let work = Path::new("_sample_par");
        let _ = std::fs::remove_dir_all(work);
        copy_dir_all(Path::new("target/test_patch"), work);
        let patchdata = work.join("patchdata");
        std::fs::create_dir_all(&patchdata).unwrap();

        let zip0 = &filelist.file_list[0];
        let size: u64 = zip0.size.trim().parse().unwrap();
        let zip_path = patchdata.join("part0.zip");
        download_signed_to_file(
            &agent,
            &challenge,
            &format!("{zip_base}{}", zip0.url),
            &zip_path,
            size,
            &zip0.md5,
        )
        .unwrap();

        let manifest = read_manifest_from_zip(&zip_path).unwrap();
        let mut corrupted = Vec::new();
        let stats = apply_zip(&zip_path, &manifest, work, &patchdata, &mut corrupted).unwrap();
        eprintln!("stats: {stats:?}");

        for (rel, want) in [
            ("mxd/Data/Base/Base.wz", "7AEE6EAE079C5CF5C99E0584A6761929"),
            ("mxd/Data/Base/Base_000.wz", "6E2C62612C629DF16FA3C0B278ABE684"),
            ("mxd/Canvas.dll", "53A3A73BF425C2826E580F69F908D206"),
        ] {
            let got = md5_file_upper(&work.join(rel)).unwrap();
            eprintln!("{rel}: {got}");
            assert_eq!(got, want, "mismatch for {rel}");
        }
        assert_eq!(stats.patched, 3);
        let _ = std::fs::remove_dir_all(work);
    }

    const SAMPLE_XML: &str = r#"<XMLROOT>
<DeltaPathInfo>
<DeltaPathSubItem Key="mxd\Data\Base\Base.wz" Value="Pkg\mxd\Data\Base\Base.wz.hdiff"/>
</DeltaPathInfo>
<NewPathInfo>
<NewPathSubItem Key="mxd\new.dll" Value="Pkg\mxd\new.dll"/>
</NewPathInfo>
<DelPathInfo>
<DelPathSubItem Key="mxd\old.dll" Value="mxd\old.dll"/>
</DelPathInfo>
<DeltaMD5Info>
<DeltaMD5SubItem Key="mxd\Data\Base\Base.wz.hdiff" Value="1CE15A117D4BBB02EAC14089A9F34D84"/>
</DeltaMD5Info>
<OriginMD5Info>
<OriginMD5SubItem Key="mxd\Data\Base\Base.wz" Value="02A21289288E2FB5A448E61989265649"/>
</OriginMD5Info>
<ResultMD5Info>
<ResultMD5SubItem Key="mxd\Data\Base\Base.wz" Value="7AEE6EAE079C5CF5C99E0584A6761929"/>
</ResultMD5Info>
</XMLROOT>"#;

    #[test]
    fn parses_manifest_sections() {
        let m = parse_manifest(SAMPLE_XML).unwrap();
        assert_eq!(m.deltas.len(), 1);
        assert_eq!(m.news.len(), 1);
        assert_eq!(m.deletions, vec!["mxd\\old.dll".to_string()]);

        let base = &m.deltas["mxd\\Data\\Base\\Base.wz"];
        assert_eq!(base.origin_md5, "02A21289288E2FB5A448E61989265649");
        assert_eq!(base.result_md5, "7AEE6EAE079C5CF5C99E0584A6761929");
        assert_eq!(base.delta_md5, "1CE15A117D4BBB02EAC14089A9F34D84");
        assert_eq!(
            m.delta_by_hdiff["Pkg\\mxd\\Data\\Base\\Base.wz.hdiff"],
            "mxd\\Data\\Base\\Base.wz"
        );
        assert_eq!(m.new_by_zip["Pkg\\mxd\\new.dll"], "mxd\\new.dll");
    }

    #[test]
    fn strips_pkg_prefix() {
        assert_eq!(strip_pkg_prefix("Pkg\\mxd\\a.hdiff"), "mxd\\a.hdiff");
        assert_eq!(strip_pkg_prefix("Pkg/mxd/a.hdiff"), "mxd/a.hdiff");
        assert_eq!(strip_pkg_prefix("mxd\\a.hdiff"), "mxd\\a.hdiff");
    }

    #[test]
    fn rel_join_handles_mixed_separators() {
        let p = rel_join(Path::new("/base"), "mxd\\Data/Base\\Base.wz");
        let expected: PathBuf = ["/base", "mxd", "Data", "Base", "Base.wz"].iter().collect();
        assert_eq!(p, expected);
    }
}
