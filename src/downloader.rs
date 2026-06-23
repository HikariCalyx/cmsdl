use std::path::Path;

use anyhow::Result;

use crate::cli::Region;
use crate::cms;

/// Check for available updates from the given region.
pub fn check(region: Region) -> Result<()> {
    println!("cmsdl: checking for updates from region '{region}'.");

    match region {
        Region::Cms => {
            let info = cms::get_client_file_list_info()?;
            println!("  version:    {}", info.version);
            println!("  files:      {}", info.file_count);
            println!(
                "  total size: {:.2} GB ({} bytes)",
                info.total_size as f64 / 1_073_741_824.0,
                with_thousands_separator(info.total_size)
            );
        }
        Region::Tms => {
            // TODO: implement update check logic for the TMS region.
        }
    }

    Ok(())
}

/// Download the client for the given region into `path`.
pub fn download(region: Region, path: &Path) -> Result<()> {
    println!(
        "cmsdl: downloading client for region '{region}' into '{}'.",
        path.display()
    );

    match region {
        Region::Cms => cms::download_client(path)?,
        Region::Tms => {
            // TODO: implement the download logic for the TMS region.
        }
    }

    Ok(())
}

/// Verify checksums and repair corrupted files for the given region at `path`.
pub fn repair(region: Region, path: &Path) -> Result<()> {
    println!(
        "cmsdl: repairing client for region '{region}' at '{}'.",
        path.display()
    );

    // TODO: implement the checksum and repair logic here.

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
