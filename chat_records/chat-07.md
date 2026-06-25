# Now let's work on tms module.

Exported: 6/24/2026, 4:11:58 PM

---

**User:**

Now let's work on tms module.
1. Check: it checks json from following URL.
http://maplestory-download.beanfun.com/maplestory/productInfo.json
2. download:
It will fetch these parts.
- baseUrl.
- executionPath.
- BasePath of execution Path, in this case, P2P2dQtkA2f8rU3
- Every paths in files section.
When download, it will combine url like these.
```
<baseUrl><basePath>/<path_of_every_files>
```
As well as:
```
<baseUrl><executionPath>
```
And download them to respective directories with 10 parallels, 5 segments.
Unlike cms module, you don't need to regenerate url for stall detection.
3. For --download-wz-only switch, only let it download paths start with Data/.

---

**Assistant:**

I'll read the existing code to understand the patterns before building the tms module.

Let me read the rest of cms.rs to understand the download machinery I'll need to reuse.

Now let me fetch the productInfo.json to understand its structure.

Now I understand the full picture. The key facts:
- `baseUrl` = `https://maplestory-download.beanfun.com/maplestory/download/`
- `executionPath` = `P2P2dQtkA2f8rU3/MapleStory.exe`, so its base path is `P2P2dQtkA2f8rU3`
- File URLs = `baseUrl + basePath + "/" + file.path`; execution URL = `baseUrl + executionPath`
- Files carry `sizeInBytes` and `sha256` for verification; URLs are static (no re-signing needed)

Let me add the needed dependencies first.

Now let me create the tms module:

Now wire up the module in main.rs and the dispatcher in downloader.rs:

Now unhide the TMS region so it's usable from the CLI:

TMS is already exposed. Now let's build to verify everything compiles.

Build succeeded (the exit code is just PowerShell treating cargo's stderr as an error). Let me clean up the one dead-code warning by actually using the declared install size, and run the tests.

All 24 tests pass with no warnings. Let me run a live smoke test of the `check` command to confirm end-to-end parsing works against the real endpoint.

The TMS module is complete and working. The live `check` confirms parsing works end-to-end (1222 files, V280, 65.87 GB).

Here's what I built in `src/tms.rs`, wired through `downloader.rs` and `main.rs`:

**Check** (`cmsdl tms --check`) fetches and parses `productInfo.json`, reporting product name, version, file count, and total size.

**Download** (`cmsdl tms --download <path>`):
- Derives the base path `P2P2dQtkA2f8rU3` from `executionPath`.
- Builds file URLs as `<baseUrl><basePath>/<file.path>` and the executable URL as `<baseUrl><executionPath>`, downloading each to its respective directory under the target.
- Runs 10 files in parallel, each split into up to 5 byte-range segments, matching the CMS module's concurrency.
- Verifies existing files via SHA-256 + size and skips ones already up to date.
- Stall handling re-requests the *same* static URL (no re-signing, per your note).

**`--download-wz-only`** keeps only file entries whose path starts with `Data/` and excludes the executable.

A few implementation notes worth flagging:
- `MapleStory.exe` appears both in `files` (with a checksum) and as the `executionPath`, producing an identical URL and local destination. I dedupe by local path and keep the checksummed file entry, so it isn't fetched twice. If a future manifest has an `executionPath` not present in `files`, it's still downloaded (size resolved via a HEAD request, falling back to single-stream).
- Added `serde`, `serde_json`, and `sha2` to `Cargo.toml`.
- 12 new unit tests cover URL building, base-path/filename extraction, dedup, wz-only filtering, JSON parsing, and range splitting. All 24 tests pass.

I didn't run a full download since it's ~66 GB; the path-building and filtering logic is covered by the unit tests instead.

---

**User:**

I also noticed a torrent file will be fetched as well:
https://maplestory-download.beanfun.com/maplestory/download/torrent/MS_V280.torrent
Could it implement --get-bit-torrent and --output switches for getting the torrent file only?

---

**Assistant:**

I'll add the `--get-bit-torrent` and `--output` switches. The torrent URL pattern is `<baseUrl>torrent/<productId>_<version>.torrent`, so I need to parse `productId` from the manifest too.

Let me update the CLI first:

Now add `productId` parsing and the torrent download logic to tms.rs:

Now update the sample test data to include `product_id`, add torrent tests, and wire up the dispatcher. First the test sample:

Now wire the new action into main.rs and downloader.rs:

I made an incomplete edit. Let me view and fix that area:

I need to restore the repair function signature and insert the new get_bit_torrent function:

Now let me build and test:

All 26 tests pass with no warnings. Now a live smoke test of the new switch:

`-o test_dl` was treated as a file path since `test_dl` didn't exist as a directory — correct behavior. Let me verify the directory case and default case, then clean up.

Both the directory and file cases work. Let me confirm CMS is rejected cleanly:

Everything works as expected. The `--get-bit-torrent` and `--output` switches are implemented.

Summary of changes:

**`src/cli.rs`** — Added `--get-bit-torrent` as a third mutually-exclusive action (alongside `--check`/`--download`) and `--output`/`-o <PATH>` for the destination. Added `Action::GetBitTorrent(Option<PathBuf>)`, applying the existing Windows path sanitizer to the output path.

**`src/tms.rs`** — Parse `productId` from the manifest, and added `download_torrent(output)` which:
- Builds the URL as `<baseUrl>torrent/<productId>_<version>.torrent` (e.g. `.../download/torrent/MS_V280.torrent`).
- Resolves the destination: an existing directory gets the torrent's own name appended; an explicit path is used as-is; omitting `--output` writes `<productId>_<version>.torrent` in the current directory.

**`src/downloader.rs`** / **`src/main.rs`** — Dispatch the new action. Only TMS publishes a torrent, so CMS returns a clear error.

Verified live: `-o <existing dir>` saves `dlout\MS_V280.torrent`, `-o <non-dir>` uses that exact path, and CMS exits with `region 'CMS' does not publish a BitTorrent file`. All 26 unit tests pass (added 2 for torrent URL/name and output resolution).

One note: `--output` only applies to `--get-bit-torrent`. If you'd like clap to actively reject `--output` when combined with `--check` or `--download` (rather than silently ignoring it), I can add a `requires`/conflicts rule — let me know.

---

**User:**

Now, could the TMS download combine HTTP and BitTorrent, and disable BitTorrent part with "--no-bit-torrent" switch?
i.e. download files from both HTTP source and BitTorrent source.

---

**Assistant:**

This is a substantial feature, so let me research the realistic implementation path before writing code. A full BitTorrent client from scratch isn't feasible, so I'll look at using an existing Rust BitTorrent library and how it handles combining HTTP (web seeds) with peer downloads.

Let me look at the librqbit library API specifically for programmatic usage.

docs.rs isn't rendering. Let me get the actual librqbit API from the source repository.

I have enough to make an informed recommendation. Before I write a large amount of code, let me get the exact current librqbit library API so the integration actually compiles. Let me fetch the repo's example.

I now have the real librqbit API. Let me check the latest published version to pin it correctly.

I now have the API (librqbit 8.1.1 matches the `Session`/`add_torrent`/`AddTorrent`/`AddTorrentResponse` API from the example). 

Before writing a large amount of code, let me be upfront about the one significant tradeoff this introduces, since it's hard to reverse:

**This adds a full BitTorrent engine (`librqbit`) plus the `tokio` async runtime to what is currently a lean synchronous tool** — hundreds of transitive crates and a notably larger binary/build time. I'll keep it behind a default-on `bittorrent` cargo feature so lean builds can opt out, and I cannot run a live swarm here, so I'll verify by compilation + the HTTP path. The design:

- **Default**: BitTorrent stage first (which also uses the HTTP CDN as a web seed if the torrent advertises one), then the existing HTTP pass verifies every file by SHA‑256 and downloads anything missing/corrupt — so both sources contribute and the install is guaranteed complete.
- **`--no-bit-torrent`**: skips the torrent stage (today's pure-HTTP behavior).

Let me implement it.

Now create the bittorrent module with feature-gated real and fallback implementations:

Now register the module and wire the CLI flag:

Now update `downloader::download` to pass through the flag:

Now update `tms::download_client` to add the BitTorrent stage. Let me view its current top:

Now I'll update the function signature, docs, and insert the BitTorrent stage before the HTTP agent setup:

Now add the `run_bittorrent_stage` helper. Let me place it right after `download_torrent`:

Now let me build with the default features (including bittorrent). This will pull many crates, so it'll take a while.

Build finished successfully — `librqbit` and my `bittorrent.rs` compiled against the real API. Let me run the tests, then verify the lean (no-BitTorrent) build also compiles.

Both the full and lean builds compile, and all 26 tests pass. Let me do a short, timed live run to confirm the BitTorrent engine actually initializes at runtime (then I'll stop it — I won't pull the full 66 GB).

The BitTorrent engine works at runtime — librqbit loaded the torrent (correctly read the 65.8 GB total), created the session, and is reporting live progress. That confirms the integration end-to-end. Let me stop the background process and clean up the test directory.

Cleaned up. Let me confirm the new flag appears in help and no process lingers:

Everything is in place and verified. The `--no-bit-torrent` flag shows in help, the test directory is cleaned up, and no process lingers.

## Summary

The TMS download now combines HTTP and BitTorrent, with `--no-bit-torrent` to disable the BitTorrent part.

**How it works** (`cmsdl tms --download <path>`):
1. **Stage 1 — BitTorrent**: fetches `MS_<version>.torrent` and hands it to `librqbit`, which downloads into the target directory using the swarm plus any HTTP web seed the torrent advertises. Live progress is printed.
2. **Stage 2 — HTTP**: the existing parallel HTTP downloader runs over the manifest, verifying every file by SHA‑256 and fetching only what's missing or corrupt. This guarantees a complete, correct install regardless of swarm health, and is where both sources "combine."

`--no-bit-torrent` skips stage 1 (pure HTTP, as before). The BitTorrent stage is also skipped automatically in `--download-wz-only` mode, since the torrent covers the whole client rather than just `Data/`.

**Files changed:**
- `Cargo.toml` — added `librqbit` + `tokio` behind a default-on `bittorrent` cargo feature.
- `src/bittorrent.rs` (new) — feature-gated wrapper that runs librqbit on a tokio runtime; a fallback returns an error when the feature is off, plus `is_available()`.
- `src/cli.rs` — added `--no-bit-torrent`.
- `src/downloader.rs` / `src/main.rs` — thread `use_bittorrent` through to TMS.
- `src/tms.rs` — `download_client` gained `use_bittorrent`, with the new `run_bittorrent_stage` orchestration.

**Verified:** full build (with BitTorrent) and `--no-default-features` lean build both compile; all 26 unit tests pass; and a short live run confirmed librqbit actually loads the torrent (correctly read 65.8 GB) and reports progress at runtime.

Two things worth flagging honestly:
- **Dependency weight**: this pulls in a full BT engine + tokio + reqwest (hundreds of transitive crates, larger binary, longer builds). That's why it's behind the `bittorrent` feature — `cargo build --no-default-features` keeps the original lean HTTP-only tool.
- **I couldn't test a real swarm completion here** (no peers, and it's 66 GB). The HTTP verify pass is the correctness backstop. One assumption I can't verify without running a full download: that the torrent lays files out at the same relative paths as the HTTP manifest. If a future torrent wraps everything in a top-level folder, BitTorrent's output wouldn't overlay the HTTP layout — the install would still be correct (HTTP re-fetches), just without the BT bandwidth savings. If you hit that, the fix is to detect the torrent's root folder and adjust the output dir; I can add that if you see it happen.

---
