use std::path::Path;

use anyhow::{bail, Result};

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
/// When `skip_create_shortcut` is set, the CMS post-download step skips creating
/// the desktop and Start Menu shortcuts (the one next to the binary is still
/// created). This flag does not apply to TMS.
///
/// When `allow_insecure` is set, TLS certificate verification is disabled for
/// all requests. When `proxy` is given, all requests are routed through it.
pub fn download(
    region: Region,
    path: &Path,
    wz_only: bool,
    skip_create_shortcut: bool,
    allow_insecure: bool,
    proxy: Option<&str>,
) -> Result<()> {
    println!(
        "cmsdl: downloading client for region '{region}' into '{}'.",
        path.display()
    );

    match region {
        Region::Cms => {
            cms::download_client(path, wz_only, skip_create_shortcut, allow_insecure, proxy)?
        }
        Region::Tms => tms::download_client(path, wz_only, allow_insecure, proxy)?,
    }

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
