mod cli;
mod cms;
mod downloader;
mod filter;
mod gui;
mod gui_downloader;
mod gui_patch;
mod is_hdd;
mod keep_awake;
mod locale;
mod manual;
mod metered;
mod miniwzlib;
mod net;
mod patch;
mod patch_builder;
mod progress;
mod resume;
mod taskprogress;
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
///
/// Setting `ENABLE_EXTENDED_FLAGS` re-specifies the input mode, which on some
/// Windows builds drops `ENABLE_PROCESSED_INPUT`. Without that flag the console
/// stops generating `CTRL_C_EVENT` (Ctrl+C is delivered as raw `^C` input
/// instead), so Ctrl+C no longer terminates the process while Ctrl+Break —
/// which does not depend on that flag — still does. We therefore re-assert
/// `ENABLE_PROCESSED_INPUT` explicitly.
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
        const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
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
        // interpreted correctly. Keep ENABLE_PROCESSED_INPUT on so Ctrl+C
        // continues to raise CTRL_C_EVENT (see the note above).
        let new_mode =
            (mode | ENABLE_EXTENDED_FLAGS | ENABLE_PROCESSED_INPUT) & !ENABLE_QUICK_EDIT_MODE;
        if new_mode == mode {
            // Already in the desired state.
            return;
        }

        // SAFETY: handle is valid; mode flags are well-known console flags.
        let _ = unsafe { SetConsoleMode(handle, new_mode) };
        // Failure is deliberately ignored — this is a cosmetic convenience.
    }
}

/// Install a console control handler so Ctrl+C (and Ctrl+Break) reliably
/// terminate the process.
///
/// Since the patcher can run a Win32 message loop on the main thread while the
/// actual work happens on background threads, we install an explicit handler
/// that force-exits the process. The handler runs on its own OS thread, so it
/// works regardless of what the main thread is currently blocked on.
fn install_ctrl_handler() {
    #[cfg(windows)]
    {
        const CTRL_C_EVENT: u32 = 0;
        const CTRL_BREAK_EVENT: u32 = 1;

        extern "system" fn handler(ctrl_type: u32) -> i32 {
            extern "system" {
                fn ExitProcess(code: u32);
            }
            if ctrl_type == CTRL_C_EVENT || ctrl_type == CTRL_BREAK_EVENT {
                // 130 == 128 + SIGINT, the conventional "terminated by Ctrl+C".
                // SAFETY: ExitProcess is always safe to call; it does not return.
                unsafe { ExitProcess(130) };
            }
            0 // FALSE: fall through to the default handler for other events.
        }

        extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        // SAFETY: all calls below use well-defined arguments.
        unsafe {
            // Clear any inherited "ignore Ctrl+C" attribute. A process launched
            // with CREATE_NEW_PROCESS_GROUP (or whose parent called
            // SetConsoleCtrlHandler(NULL, TRUE)) inherits a flag that suppresses
            // CTRL_C_EVENT while still delivering CTRL_BREAK_EVENT — which is
            // exactly the "Ctrl+Break works, Ctrl+C doesn't" symptom. Passing
            // (NULL, FALSE) restores normal Ctrl+C processing.
            SetConsoleCtrlHandler(None, 0);
            // Register our handler (routine, add = TRUE).
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }
}

fn main() -> Result<()> {
    install_ctrl_handler();

    disable_quick_edit();

    // `is_metered` is a stand-alone probe that needs no region argument, so it
    // is handled before clap to keep the interface simple.
    //
    // NSIS callers can use either:
    //   ExecWait '"cmsdl.exe" is_metered' $0   ; $0 = exit code (0/1)
    //   nsExec::ExecToStack '"cmsdl.exe" is_metered'
    //   Pop $0  ; exit code
    //   Pop $1  ; stdout text: "0" or "1"
    if std::env::args().nth(1).as_deref() == Some("is_metered") {
        let metered = metered::is_android_metered()
            || metered::is_iphone_tethering()
            || metered::is_windows_metered();
        println!("{}", metered as u8);
        std::process::exit(metered as i32);
    }

    // `is_hdd` is a stand-alone probe that checks whether the given path
    // resides on a mechanical hard disk.  It takes a single path argument.
    //
    // NSIS callers can use either:
    //   ExecWait '"cmsdl.exe" is_hdd "$INSTDIR"' $0   ; $0 = exit code (0/1)
    //   nsExec::ExecToStack '"cmsdl.exe" is_hdd "$INSTDIR"'
    //   Pop $0  ; exit code
    //   Pop $1  ; stdout text: "0" or "1"
    if std::env::args().nth(1).as_deref() == Some("is_hdd") {
        let path = std::env::args().nth(2).unwrap_or_else(|| String::from("."));
        let hdd = is_hdd::is_hdd(std::path::Path::new(&path));
        println!("{}", hdd as u8);
        std::process::exit(hdd as i32);
    }

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
            cli.no_gui,
            cli.close_after_finishing,
        )?,
        Action::GetBitTorrent(output) => {
            downloader::get_bit_torrent(cli.region, output.as_deref(), cli.allow_insecure, proxy)?
        }
        Action::Patch(PatchAction::List) => {
            downloader::patch_list(cli.region, cli.allow_insecure, proxy, cli.json)?
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
            cli.no_gui,
            cli.close_after_finishing,
            cli.keep_old_wz_files,
        )?,
        Action::CreateShortcut(path) => downloader::create_shortcut(cli.region, &path, cli.lrhook, cli.no_gui, cli.close_after_finishing)?,
        Action::CreatePatch { old_dir, new_dir, out_file } => {
            patch_builder::create_patch(&old_dir, &new_dir, &out_file)?
        }
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
