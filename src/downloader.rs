use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::cli::Region;
use crate::cms;
use crate::tms;

/// Check for available updates from the given region.
pub fn check(region: Region, allow_insecure: bool, proxy: Option<&str>) -> Result<()> {
    println!("cmsdl: checking for updates from region '{region}'.");

    match region {
        Region::Cms => {
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
        Region::Tms => {
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

    Ok(())
}

/// Download the client for the given region into `path`.
///
/// When `wz_only` is set, only data files are downloaded (for CMS, paths under
/// `mxd/Data`).
///
/// When `allow_insecure` is set, TLS certificate verification is disabled for
/// all requests. When `proxy` is given, all requests are routed through it.
pub fn download(
    region: Region,
    path: &Path,
    wz_only: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    println!(
        "cmsdl: downloading client for region '{region}' into '{}'.",
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
        Region::Cms => cms::download_client(path, wz_only, allow_insecure, proxy)?,
        Region::Tms => tms::download_client(path, wz_only, allow_insecure, proxy)?,
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
            println!("cmsdl: fetching torrent file for region '{region}'.");
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
            println!("cmsdl: listing patches for region '{region}'.");
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
                    "cmsdl: incomplete download marker detected at '{}'; \
                     performing a full client download instead of patching.",
                    sentinel.display()
                );
                download(Region::Cms, target, false, allow_insecure, proxy)?;
                create_shortcut(Region::Cms, target)?;
                if launch_after {
                    crate::patch::launch_client(target)?;
                }
                return Ok(());
            }

            println!(
                "cmsdl: patching region '{region}' client at '{}' up to '{version}'.",
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
