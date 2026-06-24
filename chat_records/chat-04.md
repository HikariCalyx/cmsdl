# I found another issue. When the download path i...

Exported: 6/24/2026, 4:11:37 PM

---

**User:**

I found another issue. When the download path is quoted (when the path contains space), the directory cannot be created.

---

**Assistant:**

The root cause is the classic Windows command-line quoting gotcha. When a user runs something like:

```
cmsdl cms --download "D:\My Games\client\"
```

the trailing backslash escapes the closing quote, so the argument the program actually receives is `D:\My Games\client"` with a literal `"` stuck on the end. `"` is an illegal character in Windows paths, so `create_dir_all` fails. The same thing can happen if a stray leading/trailing quote survives for any other reason.

The fix is to sanitize the incoming path by stripping surrounding quote characters before it's used. I'll do that in `cli.rs` where the action is resolved.

I need to import `Path` in cli.rs:

The build succeeded (the stderr output is just cargo's normal compile messages being treated as errors by PowerShell's pipeline). The change compiles cleanly.

The fix sanitizes the download/repair path by trimming any surrounding quote characters before it's used. This handles the Windows case where a quoted path ending in a backslash (like `"D:\My Games\"`) leaves a stray trailing `"` in the argument, which is an illegal path character that was breaking `create_dir_all`.

---

**User:**

And how about the root directory of a drive in case on Windows?

---

**Assistant:**

Good edge case. Let me trace through what actually happens with the Windows argument parser for a drive root.

When a user types `"D:\"`, Windows' command-line parser (`CommandLineToArgvW`) treats the `\"` sequence as an *escaped literal quote*, so the backslash is consumed as an escape character. The program receives `D:"` — not `D:\"`. After my quote-trimming, that becomes `D:`.

The problem: `D:` on Windows is a *drive-relative* path (the current directory on drive D), not the root `D:\`. So the backslash that mattered got eaten, and the path now points somewhere unintended.

I'll add normalization: if the sanitized path is a bare drive specifier like `D:`, restore the trailing separator to make it the drive root `D:\`.

Builds clean. Here's how the drive-root case is now handled:

- `"D:\My Games\"` → shell delivers `D:\My Games"` → trimmed to `D:\My Games` ✓
- `"D:\"` → shell delivers `D:"` → trimmed to `D:` → normalized to `D:\` (drive root) ✓
- `D:\` (unquoted) → delivered as-is, untouched ✓

The new `is_bare_drive_spec` check catches the case where the meaningful root separator was consumed by the shell's quote-escaping, and restores it so the download targets the actual drive root rather than the drive's current directory.

---

**User:**

Could you embed the icon.ico when building on Windows target?

---

**Assistant:**

Found `src/icon.ico`. Windows release builds run on native MSVC runners, so the standard approach with the `winresource` crate (the maintained fork of `winres`) will work. I'll add a build script that only embeds the icon when the target OS is Windows.

Build succeeded with no icon-embed warning, meaning the resource compiler ran cleanly. Since CI builds with `--locked`, let me confirm `Cargo.lock` was updated with the new dependency.

Done. The icon is now embedded on Windows builds. Summary of changes:

- **`Cargo.toml`** — added `winresource = "0.1"` under `[build-dependencies]`.
- **`build.rs`** (new) — embeds `src/icon.ico` only when `CARGO_CFG_TARGET_OS == "windows"`, so Linux/macOS builds are unaffected and cross-compiles to Windows still get the icon. It re-runs only when the icon changes, and degrades to a warning (rather than failing the build) if the resource compiler isn't available.
- **`Cargo.lock`** — updated with `winresource 0.1.31`, so the CI `--locked` builds stay consistent.

The icon will appear on the `.exe` in Explorer, the taskbar, and file properties across all three Windows targets (x86_64, i686, aarch64), since those run on native MSVC runners that include the resource compiler.

---
