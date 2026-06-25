mod cli;
mod cms;
mod downloader;
mod filter;
mod net;
mod patch;
mod tms;

use anyhow::Result;
use clap::Parser;

use cli::{Action, Cli, PatchAction};
use filter::FileFilter;

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.allow_insecure {
        eprintln!(
            "WARNING: --allow-insecure is set; TLS certificate verification is \
             DISABLED for all connections. This removes protection against \
             man-in-the-middle attacks. Only use this on a network you trust."
        );
    }

    let file_filter = build_filter(&cli)?;

    let proxy = net::resolve_proxy(cli.proxy.as_ref());
    let proxy = proxy.as_deref();
    let verbose = cli.verbose > 0;

    match cli.action()? {
        Action::Check => downloader::check(cli.region, file_filter.as_ref(), verbose, cli.json, cli.allow_insecure, proxy)?,
        Action::Download(path) => downloader::download(
            cli.region,
            &path,
            cli.download_wz_only,
            file_filter.as_ref(),
            cli.allow_insecure,
            proxy,
        )?,
        Action::GetBitTorrent(output) => {
            downloader::get_bit_torrent(cli.region, output.as_deref(), cli.allow_insecure, proxy)?
        }
        Action::Patch(PatchAction::List) => {
            downloader::patch_list(cli.region, cli.allow_insecure, proxy)?
        }
        Action::Patch(PatchAction::Apply { version, target }) => downloader::patch_apply(
            cli.region,
            &version,
            &target,
            cli.launch_after_patching,
            cli.allow_insecure,
            proxy,
        )?,
        Action::CreateShortcut(path) => downloader::create_shortcut(cli.region, &path)?,
    }

    Ok(())
}

/// Validate and build a [`FileFilter`] from the CLI arguments.
fn build_filter(cli: &Cli) -> Result<Option<FileFilter>> {
    match (&cli.filter, &cli.filter_regex) {
        (Some(_), Some(_)) => {
            anyhow::bail!("--filter and --filter-regex cannot be used together");
        }
        (Some(f), None) => Ok(Some(FileFilter::from_substrings(f, cli.invert_filter)?)),
        (None, Some(r)) => Ok(Some(FileFilter::from_regexes(r, cli.invert_filter)?)),
        (None, None) => {
            if cli.invert_filter {
                anyhow::bail!("--invert-filter requires --filter or --filter-regex");
            }
            Ok(None)
        }
    }
}
