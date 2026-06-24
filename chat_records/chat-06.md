# Could you add a switch named "--download-wz-onl...

Exported: 6/24/2026, 4:11:52 PM

---

**User:**

Could you add a switch named "--download-wz-only"? In CMS case, when this is specified, only file paths start with mxd/Data will be downloaded.

---

**Assistant:**

I'll look at the relevant files to understand the structure before making changes.

Let me see the rest of cms.rs and main.rs to understand how things connect.

Now I understand the structure. Let me add the flag and wire it through. Starting with the CLI:

Now update `download_client` in cms.rs to accept the flag and filter entries:

Let me verify the build compiles:

Build succeeded. Let me check if the README documents flags, so I can keep it consistent:

Done. I added a `--download-wz-only` switch and wired it through:

- `src/cli.rs`: added the `download_wz_only: bool` flag (`--download-wz-only`).
- `src/main.rs`: passes the flag into `downloader::download`.
- `src/downloader.rs`: `download` now takes `wz_only` and forwards it to `cms::download_client`.
- `src/cms.rs`: `download_client` accepts `wz_only`; when set, it filters the parsed entries to keep only those whose path is under `mxd/Data` (matched case-insensitively on the forward-slash-normalized `file_location`), and prints how many data files remain.

The filter runs after parsing the full file list, so the skip/checksum logic and progress bars all operate on the reduced set. The flag is ignored for `--check` (which only reports totals) and only takes effect with `--download` for the CMS region. The project compiles cleanly.

---
