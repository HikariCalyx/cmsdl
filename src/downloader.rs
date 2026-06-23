use std::path::Path;

use anyhow::Result;

use crate::cli::Region;
use crate::cms;

/// Check for available updates from the given region.
pub fn check(region: Region) -> Result<()> {
    println!("cmsdl: checking for updates from region '{region}'.");

    match region {
        Region::Cms => {
            let file_list = cms::get_client_file_list()?;
            print!("{file_list}");
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

    // TODO: implement the download logic here.

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
