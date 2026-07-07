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

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use md5::{Digest, Md5};

use crate::cms;
use crate::miniwzlib;
use crate::plog;

/// Name of the XML manifest stored at the root of the first zip.
const MANIFEST_NAME: &str = "patch_delta_direct.dat";

/// Number of files patched concurrently within a single zip part.
const PARALLEL_FILES: usize = 10;

/// Maximum number of byte-range segments used per zip download.
const SEGMENTS_PER_FILE: usize = 5;

/// Files smaller than this are downloaded with a single stream.
const MIN_SEGMENT_SIZE: u64 = 1 << 20; // 1 MiB

/// If no data arrives for this long, the connection is treated as stalled.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for establishing a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum consecutive stalls (no bytes received) tolerated for a download.
const MAX_STALL_RETRIES: usize = 30;

/// Pause before re-signing and resuming a stalled download.
const RESUME_BACKOFF: Duration = Duration::from_millis(500);

// --- keep-old-wz-files support ------------------------------------------------

/// Marker file inside `mxd/Data` that signals keep-old-wz-files mode is active.
/// Its presence (together with `mxd/DataBk`) means reads should be redirected
/// from `mxd/DataBk` and the session was interrupted.
const KEEP_OLD_DATA_MARKER: &str = "mxd/Data/.incomplete";

/// Check whether `key` (a backslash-separated path relative to the client root)
/// lives under the `mxd/Data` directory.
fn is_under_data(key: &str) -> bool {
    let lower = key.to_lowercase().replace('\\', "/");
    lower.starts_with("mxd/data/") || lower == "mxd/data"
}

/// Check whether keep-old-wz mode is active (both the backup directory and the
/// marker file exist).
fn is_keep_old_wz_active(target_dir: &Path) -> bool {
    target_dir.join("mxd").join("DataBk").is_dir()
        && target_dir.join(KEEP_OLD_DATA_MARKER).exists()
}

/// Check whether a [`Manifest`] contains any delta or new entries under
/// `mxd/Data`.
fn manifest_has_data_entries(manifest: &Manifest) -> bool {
    manifest.deltas.keys().any(|k| is_under_data(k))
        || manifest.news.keys().any(|k| is_under_data(k))
}

/// Set up the keep-old-wz directory layout: rename `mxd/Data` → `mxd/DataBk`,
/// create a fresh `mxd/Data`, and write the `.incomplete` marker.
///
/// Returns `true` when setup was performed (or was already in place); returns
/// `false` when the manifest does not contain any Data entries and there is
/// nothing to back up.
fn setup_keep_old_wz(target_dir: &Path, manifest: &Manifest) -> Result<bool> {
    let data_dir = target_dir.join("mxd").join("Data");
    let data_bk = target_dir.join("mxd").join("DataBk");
    let marker = target_dir.join(KEEP_OLD_DATA_MARKER);

    // Already set up from a previous patch in this session.
    if marker.exists() && data_bk.is_dir() {
        return Ok(true);
    }

    if !manifest_has_data_entries(manifest) {
        return Ok(false);
    }

    // The Data directory must exist to be backed up.  If it doesn't, this is
    // likely a fresh install or the layout is already in the desired state.
    if data_dir.is_dir() {
        // If DataBk already exists from a previous interrupted run, remove it
        // so the rename below succeeds (it was stale anyway — the marker was
        // absent, meaning we are starting fresh).
        if data_bk.exists() {
            std::fs::remove_dir_all(&data_bk)
                .with_context(|| format!("failed to remove stale {}", data_bk.display()))?;
        }
        std::fs::rename(&data_dir, &data_bk)
            .with_context(|| format!("failed to rename {} to {}", data_dir.display(), data_bk.display()))?;
    }
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("failed to create {}", data_dir.display()))?;

    // Copy all non-.wz files from the backup into the fresh Data directory so
    // configuration and other support files are available before patching begins.
    copy_non_wz_files(&data_bk, &data_dir)?;

    std::fs::write(&marker, "")
        .with_context(|| format!("failed to create {}", marker.display()))?;

    Ok(true)
}

/// Resolve the source path for a delta entry. In keep-old-wz mode, files under
/// `mxd/Data` are read from `mxd/DataBk` instead, while the patched result is
/// still written to `mxd/Data` via the normal target path.
fn source_path_for_delta(target_dir: &Path, source_key: &str) -> PathBuf {
    if is_keep_old_wz_active(target_dir) && is_under_data(source_key) {
        // Strip the "mxd/Data" prefix (with either separator style) and prepend
        // "mxd/DataBk".
        let rel = source_key
            .strip_prefix("mxd\\Data\\")
            .or_else(|| source_key.strip_prefix("mxd/Data/"))
            .or_else(|| source_key.strip_prefix("mxd\\Data"))
            .or_else(|| source_key.strip_prefix("mxd/Data"))
            .unwrap_or(source_key);
        target_dir.join("mxd").join("DataBk").join(rel)
    } else {
        rel_join(target_dir, source_key)
    }
}

/// Recursively copy all files whose extension is **not** `.wz` from `src` to
/// `dst`, preserving the relative directory structure.
fn copy_non_wz_files(src: &Path, dst: &Path) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(src)
        .with_context(|| format!("failed to read {}", src.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(src).unwrap_or(&path);
        let dest = dst.join(rel);

        if path.is_dir() {
            copy_non_wz_files(&path, &dest)?;
        } else if !path.extension().map(|e| e.eq_ignore_ascii_case("wz")).unwrap_or(false) {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::copy(&path, &dest)
                .with_context(|| format!("failed to copy {} to {}", path.display(), dest.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------

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

    let pb = if crate::progress::active() {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(items.len() as u64)
    };
    pb.set_style(ProgressStyle::with_template("    [{pos}/{len}] {wide_msg}").unwrap());
    pb.enable_steady_tick(Duration::from_millis(120));
    crate::progress::begin_apply(items.len());

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
                    let rel = match &item.kind {
                        Kind::Delta(src) => { pb.set_message(format!("patching {src}")); src.clone() }
                        Kind::New(tgt) => { pb.set_message(format!("adding {tgt}")); tgt.clone() }
                    };

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
                    crate::progress::apply_progress(pb.position() as usize, items.len(), &rel);

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
    let source_path = source_path_for_delta(target_dir, source_key);
    // The final destination is always under the real target directory
    // (mxd/Data, not mxd/DataBk).
    let dest_path = rel_join(target_dir, source_key);

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
            replace_file(&patched_path, &dest_path)?;
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
pub(crate) struct PatchZip {
    /// Path of the zip relative to the patch `baseUrl` (e.g. `/5_..._1.zip`).
    pub(crate) url: String,
    /// Expected MD5 of the zip.
    pub(crate) md5: String,
    /// Expected size in bytes, as a string.
    pub(crate) size: String,
}

/// A patch's `FileList.dat`: a base URL plus the ordered list of zip parts.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct PatchFileList {
    #[serde(rename = "baseUrl")]
    pub(crate) base_url: String,
    #[serde(rename = "fileList")]
    pub(crate) file_list: Vec<PatchZip>,
}

/// Read the installed version from `<target>/mxd/LocalVersion3.xml`, if any.
fn read_installed_version(target_dir: &Path) -> Option<String> {
    read_version_from_local_xml(target_dir)
}

/// Parse the installed version from `<target>/mxd/LocalVersion3.xml`, if any.
fn read_version_from_local_xml(target_dir: &Path) -> Option<String> {
    let path = target_dir.join("mxd").join("LocalVersion3.xml");
    let contents = std::fs::read_to_string(path).ok()?;
    let tag = "<zone5_8848_v3>";
    let start = contents.find(tag)? + tag.len();
    let end = contents.find("</zone5_8848_v3>")?;
    let json: serde_json::Value = serde_json::from_str(&contents[start..end]).ok()?;
    let v = json.get("version")?.get("v")?.as_str()?.to_owned();
    if v.is_empty() { None } else { Some(v) }
}

/// Write the installed `version` and `version_view` to
/// `<target>/mxd/LocalVersion3.xml`.
fn write_installed_version(target_dir: &Path, version: &str, version_view: &str) -> Result<()> {
    let mxd = target_dir.join("mxd");
    std::fs::create_dir_all(&mxd)
        .with_context(|| format!("failed to create {}", mxd.display()))?;
    let xml_path = mxd.join("LocalVersion3.xml");
    let xml = format!(
        r#"<?xmlversion="1.0"encoding="utf-8"?><Root><zone5_8848_v3>{{"product_name":"zone5_8848_v3","version":{{"v":"{version}","view":"{version_view}"}}}}</zone5_8848_v3></Root>"#
    );
    std::fs::write(&xml_path, &xml)
        .with_context(|| format!("failed to write {}", xml_path.display()))
}

/// Launch the patched client (`<target>/mxd/MapleStory.exe --sqLauncher`).
///
/// The process is spawned without waiting, so cmsdl can exit while the game
/// keeps running.
///
/// When `lrhook` is true and all LocaleRemulator files exist under
/// `<target_dir>/LocaleRemulator/`, the game is launched through `LRProc.exe`
/// with the pre-configured Locale Remulator profile GUID.
/// Otherwise a warning is printed and the game launches directly.
pub fn launch_client(target_dir: &Path, lrhook: bool) -> Result<()> {
    let mxd = target_dir.join("mxd");
    let exe = mxd.join("MapleStory.exe");
    if !exe.exists() {
        bail!("cannot launch: {} not found", exe.display());
    }

    if lrhook && crate::cms::locale_remulator_available(target_dir) {
        let lr_proc = target_dir.join("LocaleRemulator").join("LRProc.exe");
        println!(
            "launching \"{}\" --sqLauncher with Locale Remulator",
            exe.display()
        );
        launch_with_lr(&lr_proc, &exe)
    } else {
        if lrhook {
            println!("warning: Locale Remulator doesn't exist. launching game directly.");
        }
        println!("launching {} --sqLauncher", exe.display());
        launch_exe(&exe, &mxd)
    }
}

/// Launch the game through Locale Remulator.
///
/// Uses the pre-configured profile GUID `55fbcb37-1d64-4344-8dd2-731cd6150f52`.
/// Calls `ShellExecuteW` with the `runas` verb so UAC elevation works properly
/// (no PowerShell intermediary means no console flash and no suppressed UAC).
#[cfg(windows)]
fn launch_with_lr(lr_proc: &Path, exe: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

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

    let guid = "55fbcb37-1d64-4344-8dd2-731cd6150f52";
    let params_str = format!("{guid} \"{}\" --sqLauncher", exe.display());
    let params = to_wide(OsStr::new(&params_str));
    let file = to_wide(lr_proc.as_os_str());
    let dir = to_wide(
        lr_proc
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .as_os_str(),
    );
    let operation = to_wide(OsStr::new("runas"));

    const SW_SHOW: i32 = 5;
    // Values ≤ 32 indicate failure (see ShellExecute documentation).
    let ret = unsafe {
        ShellExecuteW(
            0,
            operation.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            dir.as_ptr(),
            SW_SHOW,
        )
    };

    if ret <= 32 {
        bail!(
            "could not launch the client with Locale Remulator \
             (ShellExecute error {ret}; the UAC elevation prompt may have been declined)"
        );
    }
    Ok(())
}

/// Stub for non-Windows platforms.
#[cfg(not(windows))]
fn launch_with_lr(_lr_proc: &Path, _exe: &Path) -> Result<()> {
    bail!("Locale Remulator is only supported on Windows")
}

/// Launch `exe --sqLauncher` through the Windows shell.
///
/// `MapleStory.exe` ships a manifest that requests administrator rights, so a
/// plain `CreateProcess` (`std::process::Command`) fails with OS error 740
/// (`ERROR_ELEVATION_REQUIRED`). We use `ShellExecuteW` directly instead of
/// PowerShell — the shell sees the executable's manifest and triggers the UAC
/// prompt automatically, without any intermediate console window.
#[cfg(windows)]
fn launch_exe(exe: &Path, working_dir: &Path) -> Result<()> {
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

    let file = to_wide(exe.as_os_str());
    let params = to_wide(OsStr::new("--sqLauncher"));
    let dir = to_wide(working_dir.as_os_str());

    const SW_SHOW: i32 = 5;
    // Null operation = default "open" verb; the executable's own manifest
    // (requireAdministrator) triggers UAC automatically.
    let ret = unsafe {
        ShellExecuteW(0, ptr::null(), file.as_ptr(), params.as_ptr(), dir.as_ptr(), SW_SHOW)
    };

    if ret <= 32 {
        bail!(
            "could not launch the client (ShellExecute error {ret}; \
             the UAC elevation prompt may have been declined)"
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

/// Try to detect the installed version from `<target>/mxd/Data/Base/Base.wz`.
///
/// Returns `Some((package_index, to_version))` if a matching patch package is
/// found in `packages`, so the patcher can resume from that point.
/// `LocalVersion3.xml` is written immediately.
fn try_detect_version_from_wz(
    target_dir: &Path,
    packages: &[cms::PatchPackage],
) -> Option<(usize, String)> {
    let wz_path = target_dir.join("mxd").join("Data").join("Base").join("Base.wz");
    if !wz_path.exists() {
        return None;
    }
    let wz = miniwzlib::get_wz_version(&wz_path).ok()?;
    if wz.version == 0 {
        return None;
    }

    // Match WZ version against the numeric part of each package's version_view
    // (e.g.  "V225.1" → 225).
    for (i, pkg) in packages.iter().enumerate() {
        if version_view_matches(&pkg.version_view, wz.version) {
            let _ = write_installed_version(target_dir, &pkg.to, &pkg.version_view);
            return Some((i, pkg.to.clone()));
        }
    }
    None
}

/// Extract the leading integer from a version-view string like `"V225.1"`,
/// together with the position immediately after the digits.
fn parse_version_view_number(s: &str) -> Option<(i16, usize)> {
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let end = s[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map_or(s.len(), |off| start + off);
    let digits = &s[start..end];
    if digits.is_empty() {
        return None;
    }
    let num: i16 = digits.parse().ok()?;
    Some((num, end))
}

/// Check whether `version_view` corresponds to the given WZ version.
///
/// Accepts `V225.1` and `V225`; rejects internal/test builds like `V225_2G`.
fn version_view_matches(version_view: &str, wz_version: i16) -> bool {
    if let Some((num, end)) = parse_version_view_number(version_view) {
        if num != wz_version {
            return false;
        }
        // Only match if the next character is '.', end-of-string, or we're
        // at the end of the numeric prefix — reject underscore ('_') suffixes
        // used by internal/test builds (e.g. V225_2G).
        let suffix = &version_view[end..];
        suffix.is_empty() || suffix.starts_with('.')
    } else {
        false
    }
}

/// Result of [`apply_patches`]: whether any patch was actually applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOutcome {
    /// One or more patches were applied (or repair ran).
    Updated,
    /// The client was already at the requested version; nothing to do.
    AlreadyUpToDate,
}

/// Apply incremental patches to bring the client under `target_dir` up to
/// `max_version` (a version like `0.0.0.15`, or `latest` for the newest).
///
/// The starting point is taken from `<target>/mxd/LocalVersion3.xml` when
/// present and known; otherwise every patch up to the target version is
/// applied in order.
pub fn apply_patches(
    target_dir: &Path,
    max_version: &str,
    allow_insecure: bool,
    proxy: Option<&str>,
    purge_wz_files: bool,
    keep_old_wz_files: bool,
) -> Result<PatchOutcome> {
    // 1. The client must already be present.
    if !target_dir.join("mxd").is_dir() {
        bail!(
            "no 'mxd' directory found in {}; not a CMS client directory",
            target_dir.display()
        );
    }

    // 2. Fetch the patch metadata and resolve the version chain.
    crate::progress::scanning();
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
        plog!("already at version {final_version}; nothing to do.");
        return Ok(PatchOutcome::AlreadyUpToDate);
    }
    let start_idx = match installed.as_deref() {
        Some(v) => {
            if let Some(pos) = data.packages.iter().position(|p| p.from == v) {
                pos
            } else {
                // The recorded version is not a known patch starting point
                // (e.g. a newer or non-standard build) — fall back to WZ
                // detection before defaulting to the beginning of the chain.
                plog!(
                    "warning: recorded version '{v}' is not a known patch starting point; \
                     attempting to detect version from Base.wz."
                );
                if let Some((idx, detected_ver)) =
                    try_detect_version_from_wz(target_dir, &data.packages)
                {
                    plog!("detected version {detected_ver} from Base.wz.");
                    if detected_ver == final_version {
                        plog!("already at the target version; nothing to do.");
                        return Ok(PatchOutcome::AlreadyUpToDate);
                    }
                    idx + 1
                } else {
                    plog!("warning: could not detect version from Base.wz; starting from the beginning of the patch chain.");
                    0
                }
            }
        }
        None => {
            // LocalVersion3.xml does not provide a version —
            // try to detect it from Base.wz.
            if let Some((idx, detected_ver)) =
                try_detect_version_from_wz(target_dir, &data.packages)
            {
                if detected_ver == final_version {
                    plog!("detected {detected_ver} from Base.wz; already at the target version.");
                    return Ok(PatchOutcome::AlreadyUpToDate);
                }
                // The client is already at the `to` version of this package,
                // so the next patch to apply is the following one.
                idx + 1
            } else {
                0
            }
        }
    };
    if start_idx > final_idx {
        bail!(
            "installed version is newer than the requested target '{final_version}'"
        );
    }

    let selected = &data.packages[start_idx..=final_idx];
    let current_version = selected.first().unwrap().from.clone();

    // Map an internal version (e.g. "0.0.0.14") to its human "version view"
    // (e.g. "V225.6"). A package's `version_view` describes its `to` version,
    // so a version's view is the view of the package that produces it.
    let view_of = |v: &str| -> Option<&str> {
        data.packages.iter().find(|p| p.to == v).map(|p| p.version_view.as_str())
    };
    // "0.0.0.14 (V225.6)" when a view is known, else just the raw version.
    let fmt_ver = |v: &str| -> String {
        match view_of(v) {
            Some(view) => format!("{v} ({view})"),
            None => v.to_string(),
        }
    };

    // An update was found: report the installation range (opens the log file
    // in GUI mode). The GUI shows the version views only.
    let current_view = view_of(&current_version).unwrap_or(current_version.as_str());
    let final_view = view_of(&final_version).unwrap_or(final_version.as_str());
    crate::progress::installing(current_view, final_view);
    plog!(
        "applying {} patch(es): {} -> {}",
        selected.len(),
        fmt_ver(&current_version),
        fmt_ver(&final_version)
    );

    let agent = crate::net::agent_builder(allow_insecure, proxy)
        .timeout_read(STALL_TIMEOUT)
        .timeout_connect(CONNECT_TIMEOUT)
        .build();
    let challenge = cms::get_challenge_key(&agent).context("failed to obtain challenge code")?;
    let ver2_base = cms::strip_download_host(&data.base_url).to_owned();

    // Pre-fetch each patch's FileList.dat to learn how many zip parts each one
    // has, so the GUI can present a single continuous "part X of <grand total>"
    // counter across all patches instead of restarting at 1 for each patch.
    // These files are tiny JSON; the per-patch download re-fetches its own.
    let mut zip_counts: Vec<usize> = Vec::with_capacity(selected.len());
    for pkg in selected {
        let path = format!("{ver2_base}{}", pkg.file_list_url);
        let json = download_signed_text(&agent, &challenge, &path)
            .context("failed to download patch FileList.dat")?;
        let fl: PatchFileList =
            serde_json::from_str(&json).context("failed to parse patch FileList.dat")?;
        zip_counts.push(fl.file_list.len());
    }
    let global_total: usize = zip_counts.iter().sum();

    // If a previous keep-old-wz run was interrupted the marker still exists;
    // resume with the same mode automatically.
    let effective_keep = keep_old_wz_files || target_dir.join(KEEP_OLD_DATA_MARKER).exists();

    // 4-5. Apply each patch in turn. The version marker is only advanced when a
    // patch's every zip part applied with no corrupted files.
    let mut last_corrupted: Vec<String> = Vec::new();
    let mut global_offset: usize = 0;
    for (idx, pkg) in selected.iter().enumerate() {
        let mut corrupted = Vec::new();
        let from_view = view_of(&pkg.from).unwrap_or("");
        apply_one_patch(
            &agent, &challenge, &ver2_base, pkg, from_view,
            global_offset, global_total, target_dir, &mut corrupted,
            effective_keep,
        )?;
        global_offset += zip_counts[idx];
        if corrupted.is_empty() {
            write_installed_version(target_dir, &pkg.to, &pkg.version_view)?;
            plog!("  patched to {} ({})", pkg.to, pkg.version_view);
        } else {
            plog!(
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
        plog!("patching successful: now at version {final_version}.");
        cleanup_keep_old_wz_marker(target_dir);
        return Ok(PatchOutcome::Updated);
    }

    last_corrupted.sort();
    last_corrupted.dedup();
    plog!("\n{} file(s) could not be patched:", last_corrupted.len());
    for f in &last_corrupted {
        plog!("  {f}");
    }

    if max_version.eq_ignore_ascii_case("latest") {
        plog!("\nrepairing corrupted files from the latest full index...");
        let still_failed =
            cms::replace_files_from_latest(target_dir, &last_corrupted, allow_insecure, proxy)?;
        if still_failed.is_empty() {
            let final_view = data.packages.iter()
                .find(|p| p.to == final_version)
                .map(|p| p.version_view.as_str())
                .unwrap_or("");
            write_installed_version(target_dir, &final_version, final_view)?;
            plog!("all corrupted files were repaired; now at version {final_version}.");
            cleanup_keep_old_wz_marker(target_dir);
        } else {
            plog!("{} file(s) still could not be repaired:", still_failed.len());
            for f in &still_failed {
                plog!("  {f}");
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

    // Purge stray files under mxd/Data/ that are not in the latest manifest.
    if purge_wz_files && max_version.eq_ignore_ascii_case("latest") {
        plog!("");
        cms::purge_wz_files_after_patch(target_dir, allow_insecure, proxy)?;
    }

    cleanup_keep_old_wz_marker(target_dir);
    Ok(PatchOutcome::Updated)
}

/// Remove the keep-old-wz `.incomplete` marker from `mxd/Data` if present.
fn cleanup_keep_old_wz_marker(target_dir: &Path) {
    let marker = target_dir.join(KEEP_OLD_DATA_MARKER);
    let _ = std::fs::remove_file(&marker);
}

/// Download, then apply, every zip part of a single patch.
///
/// # Resume behaviour
///
/// After the first zip is downloaded and its manifest extracted, two files are
/// written to `<target>/patchdata/`:
///
/// * `patch_delta_direct.dat` — the raw manifest XML, so it is available
///   without re-downloading zip 0 on resume.
/// * `.incomplete` — a plain-text file; each time a zip is fully applied its
///   name is appended as a new line.
///
/// On the next run the function detects these two files, loads the manifest
/// from disk, skips every zip already listed in `.incomplete`, and continues
/// from where patching stopped.  Both files are removed once the patch
/// completes successfully.
fn apply_one_patch(
    agent: &ureq::Agent,
    challenge: &str,
    ver2_base: &str,
    pkg: &cms::PatchPackage,
    from_view: &str,
    global_offset: usize,
    global_total: usize,
    target_dir: &Path,
    corrupted: &mut Vec<String>,
    keep_old_wz: bool,
) -> Result<()> {
    if from_view.is_empty() {
        plog!("\npatch {} -> {} ({})", pkg.from, pkg.to, pkg.version_view);
    } else {
        plog!("\npatch {} ({}) -> {} ({})", pkg.from, from_view, pkg.to, pkg.version_view);
    }

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

    let incomplete_path = patchdata.join(".incomplete");
    let saved_manifest_path = patchdata.join(MANIFEST_NAME);

    // --- Determine resume state. ---
    let resuming = incomplete_path.exists() && saved_manifest_path.exists();
    let completed_zips: HashSet<String>;
    let mut manifest: Option<Manifest>;

    if resuming {
        completed_zips = read_completed_zips(&incomplete_path);
        let xml = std::fs::read_to_string(&saved_manifest_path)
            .with_context(|| format!("failed to read saved manifest {}", saved_manifest_path.display()))?;
        manifest = Some(parse_manifest(&xml)
            .context("failed to parse saved patch manifest")?);
        plog!("  resuming: {}/{} zip(s) already applied.",
            completed_zips.len(), filelist.file_list.len());
        // Re-establish keep-old-wz layout if needed (the setup call is
        // idempotent — if DataBk already exists this is a no-op).
        if keep_old_wz {
            if let Some(ref m) = manifest {
                setup_keep_old_wz(target_dir, m)?;
            }
        }
    } else {
        completed_zips = std::collections::HashSet::new();
        manifest = None;
    }

    let total = filelist.file_list.len();
    for (i, zip) in filelist.file_list.iter().enumerate() {
        let zip_name = zip.url.rsplit(['/', '\\']).next().unwrap_or("part.zip").to_owned();

        // Skip zip parts that were fully applied in a previous run.
        if completed_zips.contains(&zip_name) {
            plog!("  [{}/{}] skipping {zip_name} (already applied).", i + 1, total);
            continue;
        }

        let size: u64 = zip.size.trim().parse().unwrap_or(0);
        let zip_path = patchdata.join(&zip_name);
        let sign_path = format!("{zip_base}{}", zip.url);

        plog!(
            "  [{}/{}] downloading {zip_name} ({:.2} MiB)...",
            i + 1,
            total,
            size as f64 / (1024.0 * 1024.0)
        );
        // GUI shows a single continuous counter across all patches; the debug
        // log above keeps the per-patch [i/total] form.
        crate::progress::begin_download(global_offset + i + 1, global_total, size);
        download_signed_to_file(agent, challenge, &sign_path, &zip_path, size, &zip.md5)
            .with_context(|| format!("failed to download {zip_name}"))?;

        // The manifest lives in the first zip.  Save it to disk so a resumed
        // run can load it without re-downloading zip 0, then create the
        // `.incomplete` sentinel.
        if i == 0 {
            let xml = read_manifest_xml_from_zip(&zip_path)?;
            std::fs::write(&saved_manifest_path, &xml)
                .with_context(|| format!("failed to save manifest to {}", saved_manifest_path.display()))?;
            if !incomplete_path.exists() {
                std::fs::write(&incomplete_path, "")
                    .with_context(|| format!("failed to create {}", incomplete_path.display()))?;
            }
            manifest = Some(parse_manifest(&xml)?);
            // Set up keep-old-wz layout before any files are processed.
            if keep_old_wz {
                if let Some(ref m) = manifest {
                    setup_keep_old_wz(target_dir, m)?;
                }
            }
        }

        let m = manifest
            .as_ref()
            .ok_or_else(|| anyhow!("patch manifest missing from the first zip"))?;

        // Signal the extract/apply phase before the (potentially long) work of
        // patching this zip's files begins.
        crate::progress::extracting(global_offset + i + 1, global_total);
        plog!("  [{}/{}] extracting {zip_name}...", i + 1, total);

        let stats = apply_zip(&zip_path, m, target_dir, &patchdata, corrupted)?;
        plog!(
            "    applied: {} patched, {} added, {} skipped, {} corrupted",
            stats.patched, stats.added, stats.skipped, stats.corrupted
        );

        // Record this zip as done, then remove it from disk.
        append_completed_zip(&incomplete_path, &zip_name)
            .with_context(|| format!("failed to record {zip_name} as applied"))?;
        let _ = std::fs::remove_file(&zip_path);
    }

    // Apply deletions once, after all parts have been processed.
    if let Some(m) = &manifest {
        for del in &m.deletions {
            let p = rel_join(target_dir, del);
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
            // In keep-old-wz mode also remove from the backup so stale files
            // don't linger there.
            if is_keep_old_wz_active(target_dir) && is_under_data(del) {
                let bk = source_path_for_delta(target_dir, del);
                if bk.exists() {
                    let _ = std::fs::remove_file(&bk);
                }
            }
        }
    }

    // Drop the per-patch working files and resume state.
    let _ = std::fs::remove_dir_all(patchdata.join("patchfile"));
    let _ = std::fs::remove_dir_all(patchdata.join("patched"));
    let _ = std::fs::remove_file(&incomplete_path);
    let _ = std::fs::remove_file(&saved_manifest_path);
    Ok(())
}

/// Extract the raw XML text of `patch_delta_direct.dat` from a zip part.
fn read_manifest_xml_from_zip(zip_path: &Path) -> Result<String> {
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
    Ok(xml)
}

/// Read and parse `patch_delta_direct.dat` from a zip part.
// fn read_manifest_from_zip(zip_path: &Path) -> Result<Manifest> {
//     parse_manifest(&read_manifest_xml_from_zip(zip_path)?)
// }

/// Return the set of zip file names already fully applied, as recorded in the
/// `.incomplete` sidecar (one name per line).
fn read_completed_zips(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Append `zip_name` as a new line to the `.incomplete` sidecar file.
fn append_completed_zip(path: &Path, zip_name: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "{zip_name}")
        .with_context(|| format!("failed to write to {}", path.display()))
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

/// Download a signed `path` to `dest` using up to [`SEGMENTS_PER_FILE`] parallel
/// byte-range segments (falling back to a single resumable stream for small
/// files or servers without range support), then verify against `expected_md5`.
///
/// When a `.cmsdl` sidecar file exists next to `dest` and `dest` is present
/// with the expected size, each segment is resumed from its saved offset (minus
/// a 16-byte safety margin).  The sidecar is removed on success.
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

    // Early resume: if the ZIP is already fully on disk with matching size + MD5,
    // skip re-download (covers the case where download succeeded but patching
    // was interrupted before the zip was marked complete in `.incomplete`).
    if dest.exists() {
        if let Ok(meta) = dest.metadata() {
            if meta.len() == size && !expected_md5.is_empty() {
                if let Ok(got) = md5_file_upper(dest) {
                    if got.eq_ignore_ascii_case(expected_md5) {
                        plog!("  {} already present and verified (skipping download).",
                            dest.file_name().unwrap_or_default().to_string_lossy());
                        crate::progress::download_progress(size);
                        return Ok(());
                    }
                }
            }
        }
    }

    let pb = if crate::progress::active() {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(size)
    };
    pb.set_style(
        ProgressStyle::with_template(
            "    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({binary_bytes_per_sec}, ETA {eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb.enable_steady_tick(Duration::from_millis(120));

    let segments = effective_segments(size, SEGMENTS_PER_FILE);
    let probe_url = cms::build_signed_url(challenge, cms::get_current_utc8_time(), path);

    if segments <= 1 || size == 0 || !supports_ranges(agent, &probe_url) {
        // Single resumable stream: remove any stale progress sidecar then download.
        let _ = std::fs::remove_file(crate::resume::progress_path(dest));
        download_single_stream(agent, challenge, path, dest, size, &pb)?;
    } else {
        // --- Determine ranges: resume from saved progress or start fresh. ---
        let progress_path = crate::resume::progress_path(dest);
        let saved_opt = crate::resume::read_progress(&progress_path)
            .filter(|_| dest.exists())
            .filter(|_| dest.metadata().map_or(false, |m| m.len() == size));

        let ranges: Vec<(u64, u64)>;
        let progress: crate::resume::FileProgress;

        if let Some(saved) = saved_opt
            .and_then(|s| crate::resume::build_resume_ranges(&s, size).map(|(r, pre)| (s, r, pre)))
        {
            let (saved_segs, resume_ranges, pre_completed) = saved;
            pb.inc(pre_completed);
            crate::progress::download_progress(pb.position());
            progress = crate::resume::FileProgress::from_saved(dest, &saved_segs, &resume_ranges)
                .with_context(|| {
                    format!("failed to write progress file {}", progress_path.display())
                })?;
            ranges = resume_ranges;
        } else {
            // Fresh download: pre-allocate the file so segments can write at offsets.
            {
                let file = std::fs::File::create(dest)
                    .with_context(|| format!("failed to create {}", dest.display()))?;
                file.set_len(size)
                    .with_context(|| format!("failed to size {}", dest.display()))?;
            }
            let fresh_ranges = compute_ranges(size, segments);
            progress = crate::resume::FileProgress::new(dest, &fresh_ranges).with_context(
                || format!("failed to create progress file {}", progress_path.display()),
            )?;
            ranges = fresh_ranges;
        }

        let first_err: Mutex<Option<anyhow::Error>> = Mutex::new(None);

        std::thread::scope(|scope| {
            let progress = &progress;
            let handles: Vec<_> = ranges
                .iter()
                .enumerate()
                .map(|(slot, &(start, end))| {
                    let pb = &pb;
                    let first_err = &first_err;
                    scope.spawn(move || {
                        if let Err(e) =
                            download_segment(agent, challenge, path, dest, start, end, pb, progress, slot)
                        {
                            let mut s = first_err.lock().unwrap();
                            if s.is_none() {
                                *s = Some(e);
                            }
                        }
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        });

        if let Some(e) = first_err.into_inner().unwrap() {
            pb.finish_and_clear();
            return Err(e);
        }

        progress.delete();
    }

    pb.finish_and_clear();

    // Verify integrity.
    if !expected_md5.is_empty() {
        let got = md5_file_upper(dest)?;
        if !got.eq_ignore_ascii_case(expected_md5) {
            bail!("downloaded file checksum mismatch (expected {expected_md5}, got {got})");
        }
    }
    Ok(())
}

/// Download the whole file as one resumable stream, re-signing the URL on each
/// stall. Used for small files and servers that ignore range requests.
fn download_single_stream(
    agent: &ureq::Agent,
    challenge: &str,
    path: &str,
    dest: &Path,
    size: u64,
    pb: &ProgressBar,
) -> Result<()> {
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
        let url = cms::build_signed_url(challenge, cms::get_current_utc8_time(), path);
        let _ = stream_range(agent, &url, &mut file, &mut pos, pb);

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
                bail!("download stalled with no progress after {MAX_STALL_RETRIES} retries");
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    file.flush().ok();
    Ok(())
}

/// Download a single byte range `[start, end]` of a signed `path` into `dest`,
/// resuming (with a freshly-signed URL) from the current offset on each stall.
fn download_segment(
    agent: &ureq::Agent,
    challenge: &str,
    path: &str,
    dest: &Path,
    start: u64,
    end: u64,
    pb: &ProgressBar,
    progress: &crate::resume::FileProgress,
    slot: usize,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(dest)
        .with_context(|| format!("failed to open {}", dest.display()))?;

    let mut pos = start;
    let mut stalls = 0usize;

    while pos <= end {
        let before = pos;
        let url = cms::build_signed_url(challenge, cms::get_current_utc8_time(), path);
        let _ = stream_bounded(agent, &url, &mut file, &mut pos, end, pb, progress, slot);

        // Record progress at every reconnect boundary.
        progress.update(slot, pos);

        if pos > end {
            return Ok(());
        }
        if pos > before {
            stalls = 0;
        } else {
            stalls += 1;
            if stalls > MAX_STALL_RETRIES {
                bail!("download segment stalled after {MAX_STALL_RETRIES} retries");
            }
        }
        std::thread::sleep(RESUME_BACKOFF);
    }
    Ok(())
}

/// Probe whether the server honours HTTP range requests for `url`.
fn supports_ranges(agent: &ureq::Agent, url: &str) -> bool {
    match agent.get(url).set("Range", "bytes=0-0").call() {
        Ok(resp) => resp.status() == 206,
        Err(_) => false,
    }
}

/// Split `size` bytes into `segments` contiguous inclusive `[start, end]` ranges.
fn compute_ranges(size: u64, segments: usize) -> Vec<(u64, u64)> {
    let segments = segments.max(1) as u64;
    let chunk = size / segments;
    let mut ranges = Vec::with_capacity(segments as usize);
    let mut start = 0u64;
    for i in 0..segments {
        let end = if i == segments - 1 {
            size - 1
        } else {
            start + chunk - 1
        };
        ranges.push((start, end));
        start = end + 1;
    }
    ranges
}

/// Decide how many segments to use for a file of the given size.
fn effective_segments(size: u64, max_segments: usize) -> usize {
    if max_segments <= 1 || size == 0 {
        return 1;
    }
    let by_size = (size / MIN_SEGMENT_SIZE).max(1) as usize;
    by_size.min(max_segments).max(1)
}

/// Stream a range request starting at `*pos` (open-ended) into `file`, advancing
/// `*pos` as bytes arrive. If the server ignores the range (HTTP 200 with
/// `*pos > 0`), the download restarts from the beginning.
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
        crate::progress::download_progress(pb.position());
    }
    Ok(())
}

/// Stream a bounded range request from `*pos` to `end` (inclusive) into `file`,
/// advancing `*pos` and incrementing the shared progress bar as bytes arrive.
///
/// Progress is flushed to `progress` every [`crate::resume::PROGRESS_FLUSH_INTERVAL`]
/// bytes so that an interruption loses at most one flush interval of work.
fn stream_bounded(
    agent: &ureq::Agent,
    url: &str,
    file: &mut std::fs::File,
    pos: &mut u64,
    end: u64,
    pb: &ProgressBar,
    progress: &crate::resume::FileProgress,
    slot: usize,
) -> Result<()> {
    let resp = agent
        .get(url)
        .set("Range", &format!("bytes={}-{}", *pos, end))
        .call()
        .context("HTTP range request failed")?;
    let mut reader = resp.into_reader();

    file.seek(SeekFrom::Start(*pos))
        .context("failed to seek before resuming")?;

    let mut buf = [0u8; 64 * 1024];
    let mut since_flush: u64 = 0;
    loop {
        let n = reader.read(&mut buf).context("failed to read response body")?;
        if n == 0 {
            break;
        }
        // Never write past the segment boundary.
        let remaining = (end + 1).saturating_sub(*pos) as usize;
        if remaining == 0 {
            break;
        }
        let take = n.min(remaining);
        file.write_all(&buf[..take]).context("failed to write to disk")?;
        *pos += take as u64;
        since_flush += take as u64;
        pb.inc(take as u64);
        crate::progress::download_progress(pb.position());
        if since_flush >= crate::resume::PROGRESS_FLUSH_INTERVAL {
            progress.update(slot, *pos);
            since_flush = 0;
        }
        if take < n {
            break;
        }
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

        let xml = read_manifest_xml_from_zip(&zip_path).unwrap();
        let manifest = parse_manifest(&xml).unwrap();
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

    #[test]
    fn parse_version_view_number_basic() {
        assert_eq!(parse_version_view_number("V225.1"), Some((225, 4)));
        assert_eq!(parse_version_view_number("v442"), Some((442, 4)));
        assert_eq!(parse_version_view_number("225"), Some((225, 3)));
        assert_eq!(parse_version_view_number("V0.2"), Some((0, 2)));
    }

    #[test]
    fn parse_version_view_number_empty() {
        assert_eq!(parse_version_view_number(""), None);
        assert_eq!(parse_version_view_number("V"), None);
        assert_eq!(parse_version_view_number("abc"), None);
    }

    #[test]
    fn version_view_matches_detection() {
        assert!(version_view_matches("V225.1", 225));
        assert!(version_view_matches("v442", 442));
        assert!(version_view_matches("225", 225));
        // Internal/test builds with underscore suffix should NOT match.
        assert!(!version_view_matches("V225_2G", 225));
        assert!(!version_view_matches("V225.1", 226));
        assert!(!version_view_matches("V225.1", 0));
    }

    #[test]
    fn detect_version_from_sample_wz() {
        // The sample Base.wz (version 225) is in target/Base.wz —
        // copy it into a fake mxd tree and verify detection.
        let tmp = std::env::temp_dir().join("cmsdl_test_wz_detect");
        let _ = std::fs::remove_dir_all(&tmp);
        let mxd_data_base = tmp.join("mxd").join("Data").join("Base");
        std::fs::create_dir_all(&mxd_data_base).unwrap();
        std::fs::copy("target/Base.wz", mxd_data_base.join("Base.wz")).unwrap();

        let packages = vec![
            cms::PatchPackage {
                from: "0.0.0.1".into(),
                to: "0.0.0.2".into(),
                version_view: "V224.1".into(),
                file_list_url: String::new(),
            },
            cms::PatchPackage {
                from: "0.0.0.3".into(),
                to: "0.0.0.4".into(),
                version_view: "V225.1".into(),
                file_list_url: String::new(),
            },
            cms::PatchPackage {
                from: "0.0.0.5".into(),
                to: "0.0.0.6".into(),
                version_view: "V226.1".into(),
                file_list_url: String::new(),
            },
        ];

        let result = try_detect_version_from_wz(&tmp, &packages);
        assert!(result.is_some());
        let (idx, ver) = result.unwrap();
        assert_eq!(idx, 1, "should match V225.1 (second package)");
        assert_eq!(ver, "0.0.0.4");

        // Verify the marker file was written.
        assert!(tmp.join("mxd").join("LocalVersion3.xml").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_version_no_match() {
        let tmp = std::env::temp_dir().join("cmsdl_test_wz_nomatch");
        let _ = std::fs::remove_dir_all(&tmp);
        let mxd_data_base = tmp.join("mxd").join("Data").join("Base");
        std::fs::create_dir_all(&mxd_data_base).unwrap();
        std::fs::copy("target/Base.wz", mxd_data_base.join("Base.wz")).unwrap();

        // No package with version_view matching 225.
        let packages = vec![
            cms::PatchPackage {
                from: "0.0.0.1".into(),
                to: "0.0.0.2".into(),
                version_view: "V999.1".into(),
                file_list_url: String::new(),
            },
        ];

        assert!(try_detect_version_from_wz(&tmp, &packages).is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_version_no_wz_file() {
        let tmp = std::env::temp_dir().join("cmsdl_test_wz_nofile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("mxd")).unwrap();

        let packages = vec![];
        assert!(try_detect_version_from_wz(&tmp, &packages).is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
