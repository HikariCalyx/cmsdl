use std::path::{Path, PathBuf};

use clap::{ArgGroup, Parser, ValueEnum};

/// cmsdl - a CLI downloader for the Greater China region mushroom game.
#[derive(Parser, Debug)]
#[command(name = "cmsdl", version, about, long_about = None)]
#[command(group(
    ArgGroup::new("action")
        .required(true)
        .args(["check", "download", "get_bit_torrent", "patch", "create_shortcut", "create_patch"]),
))]
pub struct Cli {
    /// The region to operate on (case-insensitive).
    #[arg(value_enum, ignore_case = true)]
    pub region: Region,

    /// Check for available updates from the region.
    #[arg(long)]
    pub check: bool,

    /// Download the client from the region into the given directory.
    ///
    /// With `manual` region this is a URL to download (must be under
    /// mxdver0.jijiagames.com or mxdcclient.jijiagames.com).
    #[arg(long, value_name = "PATH|URL")]
    pub download: Option<String>,

    /// Download only the BitTorrent (.torrent) file for the latest version.
    #[arg(long)]
    pub get_bit_torrent: bool,

    /// Destination for --get-bit-torrent (a file path or an existing directory).
    /// Defaults to the torrent's own name in the current directory.
    #[arg(long, short = 'o', value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Operate on incremental patches for the region.
    ///
    /// Pass `list` to print every published patch. Pass a target version
    /// (e.g. `0.0.0.15`) or `latest` to apply patches up to that version to the
    /// client directory given as the second positional argument.
    #[arg(long, value_name = "VERSION|list")]
    pub patch: Option<String>,

    /// Client directory to download or patch (used with `--download`, or `--patch <version>`).
    #[arg(value_name = "CLIENT_DIR")]
    pub patch_target: Option<PathBuf>,

    /// Launch game after patching finishes.
    #[arg(long)]
    pub launch_after_patching: bool,

    /// Close the graphical window automatically once the operation finishes.
    ///
    /// Applies to the GUI patcher (`cms --patch <version> <dir>`) and the GUI
    /// downloader (`cms --download <dir>` / `tms --download <dir>`); ignored
    /// when `--no-gui` is set. On a patch launch failure or a download failure
    /// the window stays open so the error remains visible.
    #[arg(long, alias = "close-after-patching", alias = "close-after-downloading")]
    pub close_after_finishing: bool,

    /// Do not show the graphical window; use console output only.
    ///
    /// With `cms --patch <version> <dir>`: patch in the console instead of the
    /// GUI. With `cms --download <dir>` / `tms --download <dir>`: download in
    /// the console instead of the GUI. With `cms --create-shortcut`: the created
    /// shortcut launches the patcher in console mode (the `--no-gui` flag is
    /// embedded in it). On non-Windows platforms the GUI is never shown
    /// regardless of this flag.
    #[arg(long = "no-gui", alias = "no_gui")]
    pub no_gui: bool,

    /// Only download WZ files
    #[arg(long)]
    pub download_wz_only: bool,

    /// Delete files in the Data directory that are not listed in the manifest.
    ///
    /// With `--download`: runs after fetching the manifest and before downloading,
    /// removing any local files under `mxd/Data` (CMS) or `Data` (TMS) that are
    /// absent from the manifest.
    ///
    /// With `--patch latest`: runs after patching completes, removing stray files
    /// under `mxd/Data` that are not in the latest full client manifest.
    #[arg(long)]
    pub purge_wz_files: bool,

    /// Preserve original WZ files during patching by renaming mxd/Data to
    /// mxd/DataBk before applying patches that touch files under mxd/Data.
    ///
    /// When a patch manifest contains delta or new entries under mxd/Data, the
    /// directory is backed up and patched files are written to a fresh mxd/Data.
    /// Files outside mxd/Data are overwritten normally. If patching is interrupted
    /// the presence of mxd/Data/.incomplete triggers the same behaviour on resume,
    /// so this flag only needs to be passed once.
    #[arg(long, alias = "keep-old-wz")]
    pub keep_old_wz_files: bool,

    /// Launch the game through Locale Remulator when present.
    ///
    /// With `--create-shortcut`: if the LocaleRemulator directory with all
    /// required files exists under the target directory, the created shortcut
    /// will include `--lrhook` so subsequent launches also use Locale Remulator.
    ///
    /// With `--patch --launch-after-patching`: if the LocaleRemulator directory
    /// is present, the game is launched through `LRProc.exe` with the pre-
    /// configured profile GUID; otherwise a warning is printed and the game
    /// launches directly.
    #[arg(long)]
    pub lrhook: bool,

    /// Create a launcher shortcut for the CMS client at the given directory.
    /// (Windows only.)
    #[arg(long, value_name = "PATH")]
    pub create_shortcut: Option<PathBuf>,

    /// Create a KMST1125-format binary patch from two client directories.
    ///
    /// The generated patch file can be applied with WzComparerR2.
    #[arg(long)]
    pub create_patch: bool,

    /// Path to the *old* client directory (used with `--create-patch`).
    #[arg(long, value_name = "PATH")]
    pub old: Option<PathBuf>,

    /// Path to the *new* client directory (used with `--create-patch`).
    #[arg(long, value_name = "PATH")]
    pub new: Option<PathBuf>,

    /// Only validate and print information; do not download anything.
    ///
    /// Only valid with `manual --download`.  Combine with `--verbose` to see
    /// response headers and the resolved signed URL.
    #[arg(long)]
    pub dry_run: bool,

    /// Output the result as JSON (suppresses informational messages).
    ///
    /// With `--check`: prints a single JSON object with `region`, `build`,
    /// `version`, `files`, and `total_size` fields, or `{}` on failure.
    ///
    /// With `--patch list`: prints a JSON array of objects with `from`, `to`,
    /// `version_view`, and `size` (bytes) fields, or `[]` on failure.
    #[arg(long)]
    pub json: bool,

    /// Enable verbose output.
    ///
    /// With `--check`, also lists the files that would be downloaded (filtered
    /// when `--filter` or `--filter-regex` is supplied).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Keep only files whose path contains one of the given substrings.
    ///
    /// Pass a colon-separated list of conditions, e.g.
    /// `--filter="Base:Character:Etc"`. Backslashes are treated as forward
    /// slashes when matching. Cannot be combined with `--filter-regex`.
    #[arg(long, value_name = "COND1[:COND2...]", require_equals = true)]
    pub filter: Option<String>,

    /// Keep only files whose path matches one of the given regex patterns.
    ///
    /// Pass a colon-separated list of patterns, e.g.
    /// `--filter-regex="^mxd/Base":".wz$"`. Cannot be combined with `--filter`.
    #[arg(long, value_name = "PAT1[:PAT2...]", require_equals = true)]
    pub filter_regex: Option<String>,

    /// Invert the filter: keep only files that do NOT match.
    ///
    /// Can be used together with `--filter` or `--filter-regex`.
    #[arg(long)]
    pub invert_filter: bool,

    /// Use a specific build number instead of searching for the latest.
    ///
    /// Only valid with `--download` and `--check` for the `cms` region.
    /// The manifest for the given build number is fetched directly; an error is
    /// returned if the build does not exist on the server.
    #[arg(long, value_name = "NUMBER")]
    pub build: Option<u32>,

    /// With `--patch list`, show only patches whose major version is at least
    /// this number (e.g. `--build-since=225` shows V225.x and later).
    #[arg(long, value_name = "NUMBER")]
    pub build_since: Option<u32>,

    /// Skip TLS certificate verification for all downloads (INSECURE).
    ///
    /// Use only when a download fails because the server's certificate cannot
    /// be verified and you trust the network. This disables protection against
    /// man-in-the-middle attacks.
    #[arg(long, global = true)]
    pub allow_insecure: bool,

    /// Route all traffic through a proxy.
    ///
    /// Pass a URL to use a specific proxy, e.g. `--proxy=socks5://127.0.0.1:6000`
    /// or `--proxy=http://127.0.0.1:8080`. Pass the flag with no value
    /// (`--proxy`) to use the system proxy (taken from the `*_PROXY` environment
    /// variables, or the Windows Internet Settings on Windows).
    #[arg(long, global = true, value_name = "URL", num_args = 0..=1, require_equals = true)]
    pub proxy: Option<Option<String>>,
}

/// Supported game regions.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// Mainland region, officially known as 冒险岛Online in Chinese.
    Cms,
    /// Taiwan and SARs region, officially known as 新楓之谷 in Chinese.
    Tms,
    /// Manual single-file download from a signed CMS CDN URL.
    Manual,
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Region::Cms => "CMS",
            Region::Tms => "TMS",
            Region::Manual => "manual",
        };
        f.write_str(code)
    }
}

/// The patch sub-action selected by `--patch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchAction {
    /// List every published incremental patch.
    List,
    /// Apply patches up to `version` (or the latest) into `target`.
    Apply { version: String, target: PathBuf },
}

/// The resolved action to perform, derived from the CLI flags.
#[derive(Debug)]
pub enum Action {
    Check,
    Download(PathBuf),
    /// Manual single-file download: URL, target directory, custom output filename.
    ManualDownload {
        url: String,
        target_dir: PathBuf,
        output: Option<PathBuf>,
    },
    GetBitTorrent(Option<PathBuf>),
    Patch(PatchAction),
    CreateShortcut(PathBuf),
    CreatePatch {
        old_dir: PathBuf,
        new_dir: PathBuf,
        out_file: PathBuf,
    },
}

impl Cli {
    /// Resolve the mutually exclusive action flags into a single [`Action`].
    ///
    /// The arg group guarantees exactly one of the flags is set.
    pub fn action(&self) -> anyhow::Result<Action> {
        if self.purge_wz_files && self.download.is_none() && self.patch.is_none() {
            anyhow::bail!(
                "--purge-wz-files requires --download or --patch"
            );
        }

        let action = if self.check {
            Action::Check
        } else if let Some(arg) = &self.download {
            if self.region == Region::Manual {
                let target_dir = self.patch_target.clone().ok_or_else(|| {
                    anyhow::anyhow!(
                        "manual download requires a target directory, e.g. \
                         `cmsdl manual --download <url> /path/to/target`"
                    )
                })?;
                Action::ManualDownload {
                    url: arg.clone(),
                    target_dir: sanitize_path(&target_dir),
                    output: self.output.as_deref().map(sanitize_path),
                }
            } else {
                Action::Download(sanitize_path(Path::new(arg)))
            }
        } else if self.get_bit_torrent {
            Action::GetBitTorrent(self.output.as_deref().map(sanitize_path))
        } else if let Some(path) = &self.create_shortcut {
            if self.region == Region::Tms {
                anyhow::bail!(
                    "--create-shortcut is not supported for region 'tms'; \
                     shortcut creation is only available for 'cms'"
                );
            }
            Action::CreateShortcut(sanitize_path(path))
        } else if let Some(patch) = &self.patch {
            if self.region == Region::Tms {
                anyhow::bail!(
                    "--patch is not supported for region 'tms'; \
                     incremental patching is only available for 'cms'"
                );
            }
            if patch.eq_ignore_ascii_case("list") {
                Action::Patch(PatchAction::List)
            } else {
                let target = self.patch_target.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "applying patches requires a client directory, e.g. \
                         `cmsdl cms --patch {patch} /path/to/client`"
                    )
                })?;
                Action::Patch(PatchAction::Apply {
                    version: patch.clone(),
                    target: sanitize_path(target),
                })
            }
        } else if self.create_patch {
            if self.region != Region::Manual {
                anyhow::bail!(
                    "--create-patch is only supported for region 'manual'; \
                     use `cmsdl manual --create-patch ...`"
                );
            }
            let old_dir = self.old.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--create-patch requires --old <PATH>")
            })?;
            let new_dir = self.new.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--create-patch requires --new <PATH>")
            })?;
            let out_file = self.output.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--create-patch requires --output <PATH>")
            })?;
            Action::CreatePatch {
                old_dir: sanitize_path(old_dir),
                new_dir: sanitize_path(new_dir),
                out_file: sanitize_path(out_file),
            }
        } else {
            unreachable!("clap ArgGroup guarantees exactly one action is set")
        };
        Ok(action)
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
