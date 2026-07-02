mod cli;
mod cms;
mod downloader;
mod filter;
mod gui_test;
mod manual;
mod miniwzlib;
mod net;
mod patch;
mod resume;
mod tms;

use anyhow::Result;
use clap::Parser;

use cli::{Action, Cli, PatchAction};
use filter::FileFilter;

/// On Windows, disable QuickEdit mode on the console so that a stray mouse
/// click does not freeze the process (the default "QuickEdit" behaviour
/// pauses output and blocks the application as soon as the user selects
/// any text or clicks inside the console window).
///
/// This is a fire-and-forget best-effort call; when the standard handle is
/// not a console (e.g., a pipe), it simply returns.
fn disable_quick_edit() {
    #[cfg(windows)]
    {
        // External Windows Console functions we need.
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> isize;
            fn GetConsoleMode(hConsoleHandle: isize, lpMode: *mut u32) -> i32;
            fn SetConsoleMode(hConsoleHandle: isize, dwMode: u32) -> i32;
        }

        const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6u32; // -10
        const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
        const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;

        // SAFETY: GetStdHandle with STD_INPUT_HANDLE is always safe to call.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle == -1 {
            // Not a console (redirected / piped input) — nothing to do.
            return;
        }

        let mut mode: u32 = 0;
        // SAFETY: handle has been validated; lpMode points to a valid u32.
        if unsafe { GetConsoleMode(handle, &mut mode) } == 0 {
            // Could not query console mode — bail out silently.
            return;
        }

        // Clear QuickEdit mode.  On recent Windows builds we must also
        // set ENABLE_EXTENDED_FLAGS first so that the mode bits are
        // interpreted correctly.
        let new_mode = (mode | ENABLE_EXTENDED_FLAGS) & !ENABLE_QUICK_EDIT_MODE;
        if new_mode == mode {
            // Already in the desired state.
            return;
        }

        // SAFETY: handle is valid; mode flags are well-known console flags.
        let _ = unsafe { SetConsoleMode(handle, new_mode) };
        // Failure is deliberately ignored — this is a cosmetic convenience.
    }
}

fn main() -> Result<()> {
    // Handle `cmsdl gui_test` before clap parsing so it doesn't conflict with
    // the required `region` positional argument or the action arg-group.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("gui_test") {
        return gui_test::run_gui_test();
    }

    disable_quick_edit();

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
        Action::Check => downloader::check(cli.region, file_filter.as_ref(), verbose, cli.json, cli.allow_insecure, proxy, cli.build, cli.build_since)?,
        Action::Download(path) => downloader::download(
            cli.region,
            &path,
            cli.download_wz_only,
            file_filter.as_ref(),
            cli.allow_insecure,
            proxy,
            cli.build,
            cli.purge_wz_files,
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
            cli.purge_wz_files,
            cli.lrhook,
        )?,
        Action::CreateShortcut(path) => downloader::create_shortcut(cli.region, &path, cli.lrhook)?,
        Action::ManualDownload { url, target_dir, output } => downloader::manual_download(
            &url,
            &target_dir,
            output.as_deref(),
            cli.dry_run,
            verbose,
            cli.allow_insecure,
            proxy,
        )?,
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
