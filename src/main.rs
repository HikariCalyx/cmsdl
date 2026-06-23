mod cli;
mod cms;
mod downloader;

use anyhow::Result;
use clap::Parser;

use cli::{Action, Cli};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.action() {
        Action::Check => downloader::check(cli.region)?,
        Action::Download(path) => downloader::download(cli.region, &path, cli.download_wz_only)?,
        Action::Repair(path) => downloader::repair(cli.region, &path)?,
    }

    Ok(())
}
