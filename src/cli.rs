use std::path::PathBuf;

use clap::{ArgGroup, Parser, ValueEnum};

/// cmsdl - a CLI downloader for the Greater China region mushroom game.
#[derive(Parser, Debug)]
#[command(name = "cmsdl", version, about, long_about = None)]
#[command(group(
    ArgGroup::new("action")
        .required(true)
        .args(["check", "download", "repair"]),
))]
pub struct Cli {
    /// The region to operate on (case-insensitive, e.g. cms, CMS, cMs).
    #[arg(value_enum, ignore_case = true)]
    pub region: Region,

    /// Check for available updates from the region.
    #[arg(long)]
    pub check: bool,

    /// Download the client from the region into the given directory.
    #[arg(long, value_name = "PATH")]
    pub download: Option<PathBuf>,

    /// Verify checksums and repair corrupted files in the given directory.
    #[arg(long, value_name = "PATH")]
    pub repair: Option<PathBuf>,

    /// Increase output verbosity (can be repeated, e.g. -vv).
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
}

/// Supported game regions.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// Mainland China region.
    Cms,
    /// Taiwan region.
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
            Action::Download(path.clone())
        } else if let Some(path) = &self.repair {
            Action::Repair(path.clone())
        } else {
            unreachable!("clap ArgGroup guarantees exactly one action is set")
        }
    }
}
