# In CMS module, could you create shortcuts after... (checkpoint)

Exported: 6/24/2026, 4:12:05 PM

---

**User:**

In CMS module, could you create shortcuts after download completes (when under Windows)?
The shortcut needs to be created from following location:
<download_path>\mxd\MapleStory.exe
With switch "--sqLauncher“
To desktop, start menu - programs, the current location of cmsdl binary, and named "MapleStory CN.lnk"?
If possible, you can also add "--skip-create-shortcut" so it will not be created on desktop and start menu, but the shortcut at same location as cmsdl binary will still be created.
You don't need to implement this feature to module tms.

---

**Assistant:**

I'll read the relevant files to understand the structure before implementing.

Let me look at the downloader module to understand how download flows and where to hook shortcut creation.

Let me see the end of `download_client` in cms.rs.

Now I understand the full flow. Let me implement the changes. First, add the CLI flag:

Now update main.rs to pass the flag:

Now update downloader.rs to thread the flag to CMS only:

Now update the `download_client` signature in cms.rs and add shortcut creation at the end:

Now update the doc comment for `download_client` and add the shortcut functions. Let me update the doc comment:

Now add the `create_shortcuts` functions. I'll append them after `download_client` (before the `SignedUrl` struct doc):

Now let me verify it compiles:

Build finished successfully. The exit code is a quirk of the piped command. Let me confirm with a clean check:

Build is clean. Let me check the README to keep flag documentation consistent:

The README doesn't document individual flags, so no change needed there.

Done. Here's what I implemented, scoped to the CMS module only:

**New CLI flag** (`src/cli.rs`): `--skip-create-shortcut` — when set, skips the desktop and Start Menu shortcuts but still creates the one next to the cmsdl binary.

**Wiring** (`src/main.rs`, `src/downloader.rs`): the flag is threaded through to `cms::download_client`. The TMS branch is untouched and doesn't receive the flag.

**Shortcut creation** (`src/cms.rs`): after a successful CMS download, `create_shortcuts` runs (Windows-only via `#[cfg(windows)]`, a no-op stub elsewhere). It creates `MapleStory CN.lnk` shortcuts that:
- Target `<download_path>\mxd\MapleStory.exe`
- Pass the `--sqLauncher` switch as arguments
- Set the working directory to `<download_path>\mxd` and use the exe as the icon source
- Are placed at: the current cmsdl binary location (always), plus the Desktop and Start Menu > Programs (unless `--skip-create-shortcut` is given)

The shortcuts are created by driving the `WScript.Shell` COM object through PowerShell, so no new crate dependency was added. Desktop and Programs folders are resolved at runtime via `[Environment]::GetFolderPath` for correctness with redirected profiles, and paths are single-quote-escaped to avoid injection issues. Shortcut failures are reported as warnings rather than failing the already-completed download.

A couple of notes worth flagging:
- If `--download-wz-only` is used, `MapleStory.exe` won't actually be present, so the shortcut would point at a missing file. I kept shortcut creation unconditional on success since you didn't specify otherwise — let me know if you'd rather skip it in wz-only mode.
- The `target_dir` path in the shortcut is stored as given; if it's a relative path the shortcut will resolve relative to its working directory behavior. Using an absolute download path avoids any ambiguity.

---

**User:**

Can it create a file named cmsdl.ver that contains version number (e.g. 0.0.0.15) at <downloadPath>/mxd directory after download/checksum completes?

---

**Assistant:**

I'll add a `cmsdl.ver` file write at `<download_path>/mxd` after a successful CMS download. The `version` is already available in `download_client`.

Done. After a successful CMS download, `download_client` now writes the version (e.g. `0.0.0.15`) to `<download_path>/mxd/cmsdl.ver`, creating the `mxd` directory if it doesn't already exist. This happens right before the shortcut creation step. A write failure is reported as a warning rather than failing the completed download.

Two notes:
- This covers the download path. The `repair` flow is still a TODO stub in `downloader.rs`, so there's no checksum-completion path to hook into yet. When that gets implemented, you'll likely want to call `write_version_file` there too — let me know if you'd like me to wire it up now in anticipation.
- The file contains just the bare version string with no trailing newline. Say the word if you'd prefer a newline or a different format.

---

**User:**

You can remove the stub repair flow now, as this is no longer necessary.

---

**Assistant:**

I'll remove the repair flow. Let me find all its references first.

That was wrong — I need to remove the repair field, not duplicate it. Let me check the current state of cli.rs:

There's now a duplicate. Let me remove both repair field blocks:

Now remove the `Repair` variant from the `Action` enum and the `action()` resolution:

Let me confirm no stray repair references remain:

Removed the stub repair flow entirely. Changes:

- `src/cli.rs`: dropped the hidden `--repair` argument and the `Action::Repair` variant, and removed its branch from `action()`.
- `src/main.rs`: removed the `Action::Repair` match arm.
- `src/downloader.rs`: deleted the `repair` function stub.

The build is clean and no `repair` references remain.

---

**User:**

In certain cases the download may fail due to unable to verify SSL cerfificate. Could you add switch "--allow-insecure" for bypassing the verification? And of course, you should warn user when this switch is supplied.

---

**Assistant:**

This requires routing HTTP through a configurable agent and adding an insecure TLS config. Let me first understand all the request sites.

This is a meaningful refactor touching all request sites. Let me check the rustls/ureq versions to pick the right insecure-TLS approach.

Let me check the release workflow targets and whether ring/aws-lc is in the lock file, to choose the simplest robust approach.

`ring` is the crypto provider (no aws-lc, no openssl), and builds span Windows/Linux/macOS. So a rustls custom verifier using the ring provider is the cleanest path (native-tls would drag in openssl on Linux/macOS). Let me read the full tms.rs and the remaining cms.rs request helpers to plan precise edits.

Let me check external usages of the public cms functions before changing their signatures.

I have the full picture. My plan: add a `net` module with an opt-in insecure rustls config (using the ring provider already in the tree), thread an `allow_insecure` flag through all request paths, and warn the user once at startup. Let me start.

First, add `rustls` as a direct dependency matching ureq's version/provider:

Now create the `net` module:

Now wire the CLI flag and the startup warning in main.rs:

Now update downloader.rs to thread the flag:

Now the cms.rs metadata functions. Let me read the exact current code for the functions I need to edit:

Now `get_client_file_list_info`:

Now `fetch_client_file_list_for`, `discover_latest_client_number_with`, and `fetch_initial_client_number`. Let me read the discover function precisely:

Now `fetch_initial_client_number`:

Now `download_client` in cms.rs — update signature and the agent construction. Let me read its opening:

Now remove the now-duplicate later agent construction:

Now tms.rs. Update `get_product_info`, `get_product_info_summary`, `http_get_bytes`, `download_torrent`, and `download_client`:

Now tms `download_client`:

Now let me build:

Build succeeded. Let me run a clean check for warnings (e.g. unused imports) and run the tests:

No build warnings. The crate is a binary, so let me run tests with `--bins`:

All 26 tests pass. Let me verify the CLI surface and that the insecure TLS path initializes without panicking by doing a quick live check against the real endpoint:

The `--allow-insecure` flag works end-to-end: the warning prints to stderr and the TMS check completed over HTTPS with certificate verification disabled (the ring-backed handshake and signature checks still run). The garbled product name is just a console GBK/UTF-8 display quirk, unrelated to this change.

Here's what I implemented:

**New flag** (`src/cli.rs`): `--allow-insecure`, a global flag available on all actions (check, download, get-bit-torrent).

**Startup warning** (`src/main.rs`): when the flag is set, a clear security warning is printed to stderr before anything runs, noting that TLS verification is disabled and exposes you to man-in-the-middle attacks.

**Shared networking module** (`src/net.rs`): all HTTP now goes through `ureq` agents built here. When `allow_insecure` is true, the agent uses a custom rustls `ClientConfig` whose certificate verifier accepts any certificate. It reuses the ring crypto provider that `ureq` already uses, so only the trust check is bypassed — the handshake and signature verification still function. The config is built once and cached.

**Threading the flag through** (`src/cms.rs`, `src/tms.rs`, `src/downloader.rs`): every request site now uses a configurable agent instead of the bare `ureq::get`. This covers the CMS control file, build-number probing, file-list fetch, and file downloads, plus the TMS manifest, torrent, and file downloads. Each download path reuses a single agent (with the existing stall/connect timeouts) for both metadata and file transfers.

Dependency-wise I added `rustls = "0.23"` with `default-features = false, features = ["ring", "std", "tls12", "logging"]` to match exactly what `ureq` already pulls in — so no new crates (no aws-lc-rs, no openssl) and the cross-platform release builds are unaffected. Build is clean with no warnings and all 26 tests pass.

One note: the flag deliberately applies to both regions since SSL issues aren't region-specific. Let me know if you'd rather scope it to CMS only.

---

**User:**

I saw allow_insecure has #[arg(long, global = true)] flag. Is it enabled by default?

---

**Assistant:**

No, it's not enabled by default. `global = true` does not mean "on by default" — it controls *scope*, not the default value.

Here's what each part means for `#[arg(long, global = true)]` on a `bool` field:

- `long` makes it a `--allow-insecure` flag.
- Because the field type is `bool`, clap treats it as a `flag` whose default is `false`. It's only `true` when the user actually passes `--allow-insecure` on the command line.
- `global = true` just means the flag is recognized at any subcommand level (it "propagates down" to subcommands), so it can appear before or after a subcommand. It has no effect on the default.

So with no flag passed, `cli.allow_insecure` is `false`, and `net::agent_builder(false)` returns a normal agent with full TLS verification. Insecure mode only activates when the user explicitly types `--allow-insecure`, at which point the startup warning fires.

You can confirm it quickly:

Without the flag, there's no insecure warning and the run uses normal certificate verification. The warning only appeared in the earlier run because I explicitly passed `--allow-insecure`.

So to summarize: `global = true` controls *where* the flag can appear (any subcommand position), not whether it's on. A `bool` arg defaults to `false`, so verification stays enabled unless the user opts in.

---

**User:**

Now, could you add a "--proxy" argument? If no proxy address specified, it will try to use system proxy. Otherwise, it will use proxy address supplied. e.g. --proxy="socks5://127.0.0.1:6000" to allow it use socks5 proxy hosted on 127.0.0.1, port 6000.

---

**Assistant:**

I'll add a `--proxy` argument with optional value: bare `--proxy` uses the system proxy, and `--proxy=URL` uses the given URL (including `socks5://`). This needs ureq's SOCKS support, system-proxy detection, and threading a proxy setting alongside the existing `allow_insecure`.

Let me start with dependencies:

I mistakenly added `winresource` (that's a build-dependency). Let me fix that:

Now the CLI flag. The `Option<Option<String>>` type gives the three-way distinction (absent / present-no-value / present-with-value):

The `default_missing_value` would turn bare `--proxy` into `Some(Some(""))`. I want `Some(None)` for the system-proxy case, so let me remove it and treat an empty string defensively in resolution:

Now extend net.rs with proxy support and system-proxy detection:

Now main.rs — resolve the proxy and pass it through:

Now downloader.rs — add `proxy` param to all three functions:

Now cms.rs — update the two agent-building public functions:

Now tms.rs — update the three agent-building functions:

Now build:

Clean build, no warnings. Let me verify proxy resolution behaves correctly. First the bare `--proxy` (system) and an explicit proxy:

Bare `--proxy` correctly detected no system proxy and connected directly. Let me see the explicit-proxy output on its own:

SOCKS5 proxy routing works — ureq attempted to connect through `127.0.0.1:6000` (refused because nothing is listening there, as expected). Let me run the test suite to confirm nothing regressed:

All 26 tests pass. The `--proxy` argument is implemented and verified end-to-end.

Here's what was added:

**Argument** (`src/cli.rs`): `--proxy` is an `Option<Option<String>>` global flag with `require_equals`, giving three states:
- absent → no proxy (direct connection)
- `--proxy` (no value) → use the system proxy
- `--proxy=URL` → use that URL, e.g. `--proxy=socks5://127.0.0.1:6000` or `--proxy=http://127.0.0.1:8080`

**Resolution and detection** (`src/net.rs`):
- `resolve_proxy` turns the CLI value into the final proxy URL and prints which proxy (or direct connection) is in use.
- System-proxy detection checks the standard `*_PROXY` environment variables first (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`, plus lowercase), then on Windows reads `HKCU\...\Internet Settings` (`ProxyEnable` / `ProxyServer`) via the `winreg` crate. It handles both the bare `host:port` form and the protocol-specific `http=...;https=...;socks=...` form, preferring https → http → socks.
- A URL without a scheme defaults to `http://`. An invalid proxy URL produces a warning and falls back to a direct connection.
- `agent_builder`/`agent` now take `proxy: Option<&str>` and call `ureq::Proxy::new` when set.

**Wiring**: the resolved proxy is threaded from `main.rs` through `downloader.rs` into every CMS/TMS agent-construction site, alongside the existing `allow_insecure` flag.

**Dependencies** (`Cargo.toml`): enabled ureq's `socks-proxy` feature (for `socks4`/`socks5` support) and added `winreg` as a Windows-only dependency for registry-based system-proxy detection.

A note on scheme support: HTTP CONNECT proxies (`http://`) and SOCKS (`socks4://`, `socks5://`) work. ureq doesn't support an `https://` proxy connection (TLS to the proxy itself), so stick to those schemes.

---
