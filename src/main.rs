mod cli;
mod cms;
mod downloader;
mod net;
mod patch;
mod tms;

use anyhow::Result;
use clap::Parser;

use cli::{Action, Cli, PatchAction};

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.allow_insecure {
        eprintln!(
            "WARNING: --allow-insecure is set; TLS certificate verification is \
             DISABLED for all connections. This removes protection against \
             man-in-the-middle attacks. Only use this on a network you trust."
        );
    }

    let proxy = net::resolve_proxy(cli.proxy.as_ref());
    let proxy = proxy.as_deref();

    match cli.action()? {
        Action::Check => downloader::check(cli.region, cli.allow_insecure, proxy)?,
        Action::Download(path) => downloader::download(
            cli.region,
            &path,
            cli.download_wz_only,
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
