use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::cli::Region;
use crate::cms;
use crate::filter::FileFilter;
use crate::tms;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check for available updates from the given region.
///
/// When a `filter` is supplied the displayed file count and total size reflect
/// only the matching files.  When `verbose` is set, the matching file paths are
/// also listed.  Either condition requires fetching the full file list; without
/// either, only the lightweight summary is fetched.
///
/// When `json` is set, output is emitted as a single JSON object and all
/// informational messages are suppressed.  On failure `{}` is printed and the
/// function returns `Ok(())`.
pub fn check(region: Region, filter: Option<&FileFilter>, verbose: bool, json: bool, allow_insecure: bool, proxy: Option<&str>) -> Result<()> {
    if !json {
        println!("cmsdl {VERSION}: checking for updates from region '{region}'.");
    }

    match region {
        Region::Cms => {
            if json {
                match cms::get_client_file_list_info(allow_insecure, proxy) {
                    Ok(info) => {
                        let output = serde_json::json!({
                            "region": "cms",
                            "build": info.build_number,
                            "version": info.version,
                            "files": info.file_count,
                            "total_size": info.total_size,
                        });
                        println!("{output}");
                    }
                    Err(_) => println!("{{}}"),
                }
            } else if verbose || filter.is_some() {
                println!("scanning for the latest build version...");
                let (info, entries) = cms::get_client_file_list_full(allow_insecure, proxy)?;
                let (show_count, show_size) = filtered_totals(&entries, filter, info.file_count, info.total_size);
                println!("  build:      {}", info.build_number);
                println!("  version:    {}", info.version);
                println!("  files:      {show_count}");
                println!(
                    "  total size: {:.2} GB ({} bytes)",
                    show_size as f64 / 1_073_741_824.0,
                    with_thousands_separator(show_size)
                );
                if verbose {
                    println!();
                    print_matching_files(&entries, filter);
                }
            } else {
                println!("scanning for the latest build version...");
                let info = cms::get_client_file_list_info(allow_insecure, proxy)?;
                println!("  build:      {}", info.build_number);
                println!("  version:    {}", info.version);
                println!("  files:      {}", info.file_count);
                println!(
                    "  total size: {:.2} GB ({} bytes)",
                    info.total_size as f64 / 1_073_741_824.0,
                    with_thousands_separator(info.total_size)
                );
            }
        }
        Region::Tms => {
            if json {
                match tms::get_product_info_summary(allow_insecure, proxy) {
                    Ok(info) => {
                        let output = serde_json::json!({
                            "region": "tms",
                            "build": 0,
                            "version": info.version,
                            "files": info.file_count,
                            "total_size": info.total_size,
                        });
                        println!("{output}");
                    }
                    Err(_) => println!("{{}}"),
                }
            } else if verbose || filter.is_some() {
                let (info, entries) = tms::get_product_info_full(allow_insecure, proxy)?;
                let (show_count, show_size) = filtered_totals(&entries, filter, info.file_count, info.total_size);
                println!("  product:    {}", info.product_name);
                println!("  version:    {}", info.version);
                println!("  files:      {show_count}");
                println!(
                    "  total size: {:.2} GB ({} bytes)",
                    show_size as f64 / 1_073_741_824.0,
                    with_thousands_separator(show_size)
                );
                if verbose {
                    println!();
                    print_matching_files(&entries, filter);
                }
            } else {
                let info = tms::get_product_info_summary(allow_insecure, proxy)?;
                println!("  product:    {}", info.product_name);
                println!("  version:    {}", info.version);
                println!("  files:      {}", info.file_count);
                println!(
                    "  total size: {:.2} GB ({} bytes)",
                    info.total_size as f64 / 1_073_741_824.0,
                    with_thousands_separator(info.total_size)
                );
            }
        }
    }

    Ok(())
}

/// Return `(count, total_size)` for the subset of `entries` that match
/// `filter`, or the precomputed `(all_count, all_size)` when no filter is set.
fn filtered_totals(
    entries: &[(String, u64)],
    filter: Option<&FileFilter>,
    all_count: usize,
    all_size: u64,
) -> (usize, u64) {
    match filter {
        None => (all_count, all_size),
        Some(f) => {
            let count = entries.iter().filter(|(p, _)| f.matches(p)).count();
            let size = entries.iter().filter(|(p, _)| f.matches(p)).map(|(_, s)| *s).sum();
            (count, size)
        }
    }
}

/// Print the subset of `entries` whose paths match `filter` (or all entries
/// when `filter` is `None`).
fn print_matching_files(entries: &[(String, u64)], filter: Option<&FileFilter>) {
    let matched: Vec<&str> = entries
        .iter()
        .filter(|(p, _)| filter.map_or(true, |f| f.matches(p)))
        .map(|(p, _)| p.as_str())
        .collect();

    if matched.is_empty() {
        println!("no files match the given filter.");
    } else {
        println!("{} file(s){}:", matched.len(), if filter.is_some() { " match" } else { "" });
        for p in &matched {
            println!("  {p}");
        }
    }
}

/// Download the client for the given region into `path`.
///
/// When `wz_only` is set, only data files are downloaded (for CMS, paths under
/// `mxd/Data`).
///
/// When `filter` is given, only files whose path matches the filter are
/// downloaded.
///
/// When `allow_insecure` is set, TLS certificate verification is disabled for
/// all requests. When `proxy` is given, all requests are routed through it.
pub fn download(
    region: Region,
    path: &Path,
    wz_only: bool,
    filter: Option<&FileFilter>,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    println!(
        "cmsdl {VERSION}: downloading client for region '{region}' into '{}'.",
        path.display()
    );

    // Ensure the target directory exists so the sentinel file can be placed inside it.
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create target directory {}", path.display()))?;

    // Create a sentinel file that signals an in-progress or incomplete download.
    // It is removed only on success, so a leftover file indicates a failed run.
    let sentinel = path.join(format!(".incomplete_{region}"));
    std::fs::File::create(&sentinel)
        .with_context(|| format!("failed to create sentinel file {}", sentinel.display()))?;

    match region {
        Region::Cms => cms::download_client(path, wz_only, filter, allow_insecure, proxy)?,
        Region::Tms => tms::download_client(path, wz_only, filter, allow_insecure, proxy)?,
    }

    std::fs::remove_file(&sentinel)
        .with_context(|| format!("failed to remove sentinel file {}", sentinel.display()))?;

    Ok(())
}

/// Download the BitTorrent file for the latest version of the given region.
///
/// Only the TMS region publishes a torrent file.
pub fn get_bit_torrent(
    region: Region,
    output: Option<&Path>,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    match region {
        Region::Tms => {
            println!("cmsdl {VERSION}: fetching torrent file for region '{region}'.");
            tms::download_torrent(output, allow_insecure, proxy)?;
        }
        Region::Cms => {
            bail!("region '{region}' does not publish a BitTorrent file");
        }
    }

    Ok(())
}

/// List the published incremental patches for the given region.
///
/// Only the CMS region publishes patch metadata.
pub fn patch_list(region: Region, allow_insecure: bool, proxy: Option<&str>) -> Result<()> {
    match region {
        Region::Cms => {
            println!("cmsdl {VERSION}: listing patches for region '{region}'.");
            let patches = cms::get_patch_data(allow_insecure, proxy)?.packages;

            if patches.is_empty() {
                println!("no patches published.");
                return Ok(());
            }

            println!();
            println!("{:<12}  {:<12}  {}", "FROM", "TO", "VERSION VIEW");
            println!("{:<12}  {:<12}  {}", "----", "--", "------------");
            for p in &patches {
                println!("{:<12}  {:<12}  {}", p.from, p.to, p.version_view);
            }
            println!();
            println!("{} patch(es) total.", patches.len());
        }
        Region::Tms => {
            bail!("region '{region}' does not publish patch metadata");
        }
    }

    Ok(())
}

/// Apply incremental patches up to `version` (or `latest`) into `target`.
///
/// When `launch_after` is set, the client is launched once patching completes
/// successfully. Only the CMS region supports patching.
pub fn patch_apply(
    region: Region,
    version: &str,
    target: &Path,
    launch_after: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    match region {
        Region::Cms => {
            // If a sentinel file from a previous incomplete download exists,
            // the client directory may be in an inconsistent state. Fall back
            // to a full download rather than attempting to patch.
            let sentinel = target.join(format!(".incomplete_{region}"));
            if sentinel.exists() {
                println!(
                    "cmsdl {VERSION}: incomplete download marker detected at '{}'; \
                     performing a full client download instead of patching.",
                    sentinel.display()
                );
                download(Region::Cms, target, false, None, allow_insecure, proxy)?;
                create_shortcut(Region::Cms, target)?;
                if launch_after {
                    crate::patch::launch_client(target)?;
                }
                return Ok(());
            }

            println!(
                "cmsdl {VERSION}: patching region '{region}' client at '{}' up to '{version}'.",
                target.display()
            );
            crate::patch::apply_patches(target, version, allow_insecure, proxy)?;
            if launch_after {
                crate::patch::launch_client(target)?;
            }
        }
        Region::Tms => {
            bail!("region '{region}' does not support patching");
        }
    }

    Ok(())
}

/// Create a launcher shortcut for the CMS client at `target_path`.
///
/// Only supported for the CMS region on Windows.
pub fn create_shortcut(region: Region, target_path: &Path) -> Result<()> {
    match region {
        Region::Cms => cms::create_shortcut(target_path)?,
        Region::Tms => bail!("region '{region}' does not support shortcut creation"),
    }
    Ok(())
}

/// Format an integer with `,` thousands separators (e.g. `70776930990` -> `70,776,930,990`).
fn with_thousands_separator(value: u64) -> String {
    let digits = value.to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3);

    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }

    out
}
