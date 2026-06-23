use std::path::{Path, PathBuf};

use clap::{ArgGroup, Parser, ValueEnum};

/// cmsdl - a CLI downloader for the Greater China region mushroom game.
#[derive(Parser, Debug)]
#[command(name = "cmsdl", version, about, long_about = None)]
#[command(group(
    ArgGroup::new("action")
        .required(true)
        .args(["check", "download", "get_bit_torrent"]),
))]
pub struct Cli {
    /// The region to operate on (case-insensitive).
    #[arg(value_enum, ignore_case = true)]
    pub region: Region,

    /// Check for available updates from the region.
    #[arg(long)]
    pub check: bool,

    /// Download the client from the region into the given directory.
    #[arg(long, value_name = "PATH")]
    pub download: Option<PathBuf>,

    /// Download only the BitTorrent (.torrent) file for the latest version.
    #[arg(long)]
    pub get_bit_torrent: bool,

    /// Destination for --get-bit-torrent (a file path or an existing directory).
    /// Defaults to the torrent's own name in the current directory.
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Verify checksums and repair corrupted files in the given directory.
    #[arg(long, value_name = "PATH", hide = true)]
    pub repair: Option<PathBuf>,

    /// Only download WZ files
    #[arg(long)]
    pub download_wz_only: bool,

    /// Disable the BitTorrent source; download over HTTP only (TMS only).
    #[arg(long)]
    pub no_bit_torrent: bool,

    /// Increase output verbosity (can be repeated, e.g. -vv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true, hide = true)]
    pub verbose: u8,
}

/// Supported game regions.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// Mainland region, officially known as 冒险岛Online in Chinese.
    Cms,
    /// Taiwan and SARs region, officially known as 新楓之谷 in Chinese.
    Tms,
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Region::Cms => "CMS",
            Region::Tms => "TMS",
        };
        f.write_str(code)
    }
}

/// The resolved action to perform, derived from the CLI flags.
#[derive(Debug)]
pub enum Action {
    Check,
    Download(PathBuf),
    GetBitTorrent(Option<PathBuf>),
    Repair(PathBuf),
}

impl Cli {
    /// Resolve the mutually exclusive action flags into a single [`Action`].
    ///
    /// The arg group guarantees exactly one of the flags is set.
    pub fn action(&self) -> Action {
        if self.check {
            Action::Check
        } else if let Some(path) = &self.download {
            Action::Download(sanitize_path(path))
        } else if self.get_bit_torrent {
            Action::GetBitTorrent(self.output.as_deref().map(sanitize_path))
        } else if let Some(path) = &self.repair {
            Action::Repair(sanitize_path(path))
        } else {
            unreachable!("clap ArgGroup guarantees exactly one action is set")
        }
    }
}

/// Strip surrounding quote characters from a path argument.
///
/// On Windows, a quoted path ending in a backslash (e.g. `"D:\My Games\"`)
/// has its closing quote escaped by the shell, so the program receives a
/// literal trailing `"` (e.g. `D:\My Games\"`). A leading `"` can survive in
/// similar situations. Both are illegal in Windows paths and break directory
/// creation, so we trim any matching/stray surrounding quotes here.
fn sanitize_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let trimmed = s.trim().trim_matches(|c| c == '"' || c == '\'');

    // On Windows, `"D:\"` is parsed by the shell as the escaped sequence
    // `D:"` (the backslash escapes the closing quote and is consumed), which
    // trims down to a bare drive specifier `D:`. That is a *drive-relative*
    // path (current dir on D:), not the root. Restore the separator so it
    // resolves to the drive root `D:\` as the user intended.
    if is_bare_drive_spec(trimmed) {
        return PathBuf::from(format!("{trimmed}\\"));
    }

    PathBuf::from(trimmed)
}

/// Return `true` if `s` is a bare Windows drive specifier like `C:` or `d:`
/// (a drive letter followed by a colon and nothing else).
fn is_bare_drive_spec(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}
