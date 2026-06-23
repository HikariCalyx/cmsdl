//! BitTorrent download support, behind the optional `bittorrent` feature.
//!
//! When the feature is enabled this wraps [`librqbit`] (which runs on a `tokio`
//! runtime) to download a `.torrent` into a target directory, using peers and
//! any HTTP web seeds the torrent advertises. When the feature is disabled the
//! entry point is still present but returns an error, so callers can degrade
//! gracefully to HTTP-only downloads.

use std::path::Path;

use anyhow::Result;

/// Download the contents of a `.torrent` (given as raw bytes) into `target_dir`.
///
/// Blocks until the download completes. Files are written under `target_dir`
/// using the layout described by the torrent.
#[cfg(feature = "bittorrent")]
pub fn download(torrent_bytes: Vec<u8>, target_dir: &Path) -> Result<()> {
    use anyhow::{bail, Context};
    use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session};
    use std::time::Duration;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create the async runtime")?;

    runtime.block_on(async move {
        let session = Session::new(target_dir.to_path_buf())
            .await
            .context("failed to create the BitTorrent session")?;

        let response = session
            .add_torrent(
                AddTorrent::from_bytes(torrent_bytes),
                Some(AddTorrentOptions {
                    // Reuse any files already present (e.g. from a previous run).
                    overwrite: true,
                    ..Default::default()
                }),
            )
            .await
            .context("failed to add the torrent to the session")?;

        let handle = match response {
            AddTorrentResponse::Added(_, handle) => handle,
            AddTorrentResponse::AlreadyManaged(_, handle) => handle,
            AddTorrentResponse::ListOnly(_) => bail!("torrent was added in list-only mode"),
        };

        // Print swarm/transfer stats periodically until the download finishes.
        let progress = tokio::spawn({
            let handle = handle.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    println!("  bittorrent: {}", handle.stats());
                }
            }
        });

        let result = handle.wait_until_completed().await;
        progress.abort();
        result.context("BitTorrent download did not complete")?;
        Ok(())
    })
}

/// Fallback used when the crate is built without the `bittorrent` feature.
#[cfg(not(feature = "bittorrent"))]
pub fn download(_torrent_bytes: Vec<u8>, _target_dir: &Path) -> Result<()> {
    anyhow::bail!("this build was compiled without BitTorrent support (the `bittorrent` feature)")
}

/// Whether BitTorrent support was compiled into this build.
pub const fn is_available() -> bool {
    cfg!(feature = "bittorrent")
}
