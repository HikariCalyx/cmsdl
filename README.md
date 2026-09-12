# cmsdl
[简体中文](https://github.com/HikariCalyx/cmsdl/tree/main/README.zh-CN.md)
[繁體中文](https://github.com/HikariCalyx/cmsdl/tree/main/README.zh-TW.md)

A downloader designed for Greater China region mushroom game.

*This project is not officially supported or endorsed by SQ Games and Black Orange Games*

## Why create this?
I don't want to use the bloated launcher developed by SQ Games at all.
This program is meant for a full replacement of CMS official v3 Launcher.

For chat records about how it was developed, see chat_records directory.

## Can I integrate CMSDL to my own project?
Yes of course, as long as it's not used for cheating in game.

## Will I get banned by operator because of this program?
No. This program is built with minimum previlege requirements in mind, and it does not require elevation at all. When you launch game with cmsdl, cmsdl will close itself after game is launched. Besides, anti-tampering is implemented in all regions of mushroom games since 2023: The server will refuse you from logging into the game if game data are corrupted or modified.

Feel free to review the source code. If you still have concerns, then don't use it.

Gaming hackers are unwelcome.

## Usage
### CMS
- Get latest CMS client:
```bash
~/cmsdl cms --check
```

- Download, or integrity check, or repair CMS client
```bash
~/cmsdl cms --download /path/to/cms/client
```
If the download was interrupted, you can rerun and it will continue to download.

- Download only files containing "_Canvas", "String", or "Reactor" in any path to /path/to/cms/client:
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor"
```

- Download only files not containing "_Canvas", "String", or "Reactor" in any path to /path/to/cms/client:
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- Download only files ending in ".wz" and files starting with "Maple" in any path to /path/to/cms/client:
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple"
```

- Download only files from all paths that do not end with .wz and do not start with "Maple" to /path/to/cms/client:
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

- List all available CMS patches
```bash
./cmsdl cms --patch list
```

- Patch CMS client to latest version (including minor patches), and/or launch the game after patching:
```bash
./cmsdl cms --patch latest /path/to/cms/client [--launch-after-patching]
```

- Create launch shortcut (Windows only)：
```bash
./cmsdl cms --create-shortcut /path/to/cms/client
```

### CMS Classic World (cms_cw)
`cms_cw` (MapleStory Classic World CN / 冒险岛怀旧服) supports the same commands as `cms`:
```bash
./cmsdl cms_cw --check
./cmsdl cms_cw --download /path/to/cms_cw/client
./cmsdl cms_cw --patch latest /path/to/cms_cw/client
./cmsdl cms_cw --create-shortcut /path/to/cms_cw/client
```

Any command that lists a region also accepts `cms_cw` instead of `cms`, including `--upgrade-path-check` below.

### TMS
- Get latest TMS client:
```bash
./cmsdl tms --check
```

- Download, or checksum check, or repair TMS client:
```bash
./cmsdl tms --download /path/to/tms/client
```

The client executable is kept current: when `MapleStory.exe` is missing, or is still the pristine copy listed in the manifest, the standalone executable hotfix (`ExePatch.dat`) published for the installed version is downloaded and installed instead. An executable that a previous hotfix already replaced is left untouched.

- Download only files containing "_Canvas", "String", or "Reactor" in any path to /path/to/tms/client:
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor"
```

- Download only files not containing "_Canvas", "String", or "Reactor" in any path to /path/to/tms/client:
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- Download only files ending in ".wz" and files starting with "Maple" in any path to /path/to/tms/client:
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple"
```

- Download only files from all paths that do not end with .wz and do not start with "Maple" to /path/to/tms/client:
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

- Patch TMS client to latest version, as well as minor patches:
```bash
./cmsdl tms --patch latest /path/to/tms/client
```

### Upgrade path check
Before patching, check whether the incremental patches needed to reach a version are actually smaller than downloading the full client for that version:
```bash
./cmsdl cms --upgrade-path-check latest /path/to/cms/client
./cmsdl cms_cw --upgrade-path-check latest /path/to/cms_cw/client
./cmsdl tms --upgrade-path-check latest /path/to/tms/client
```

The first argument is either a target version (e.g. `0.0.0.22` for CMS/CMS_CW, `281` for TMS) or `latest`. The installed client version is read from the client directory (`LocalVersion3.xml`, falling back to `Base.wz` for CMS; `Data/Base/Base.wz` for TMS).

Add `--verbose` (or `-v`) to also list every patch that would be applied and the size of each patch file.

When the upgrade path is not larger than the full client, the recommended `--patch` command is printed:
```
current version: 278
target version:  282
patches needed:  2 (9.19 GiB)
full client:     version V282 (67.74 GiB)
you may apply the patch with cmsdl.exe tms --patch 282 B:\tms_upgtest
```

Exit codes:

| Code | Meaning |
| ---- | ------- |
| 0 | The patches are the same size as, or smaller than, the full client. |
| 1 | No applicable patch can be found. |
| 2 | The required patches are larger than the full client; re-downloading is recommended. |
| 3 | The current client version cannot be read. |
| 4 | The requested target version is older than the installed client. |
| 100 | The patch server cannot be accessed (after retrying). |

Notes:
- CMS/CMS_CW compare against the full client whose version matches the target. If that full client is not published yet, the most recent published client is used instead (shown as a fallback).
- TMS only publishes the latest full-client manifest, so an explicit target version skips the full-client comparison. With `latest`, if the newest full client has no patch yet, the check falls back to the last version actually reachable through the patch server.
- For TMS, the standalone executable hotfix (`ExePatch.dat`) for the latest version is always included in the patch total — the patcher applies it after reaching the latest version, whether the client was already up to date or was just upgraded to it.
- The Windows installers run this check automatically before updating (GUI mode) and offer to reinstall the full client when no patch is applicable or when patching would be larger than the client itself.

### Extra Tips
If you'd like to ensure cmsdl runs under a "Game Accelerator", please rename the program to MapleStory.exe, so the "Game Accelerator" could capture the program, and download stuff.

When you get SSL related error, you may want to add `--allow-insecure` switch. This is not recommended.

If you'd like to download via proxy, you can add switch `--proxy`, so it will download via system proxy, or your preferred proxy server like `socks5://127.0.0.1:9000` - depends on the proxy app you're using.

## How can I launch the game without bloated launcher if I already downloaded client before v0.1.4 release?
Pass `--sqLauncher` switch to MapleStory.exe, so the game will run.

Alternately, you can add this in shortcut properties.

## Building
### Windows
1. Download and install Visual Studio Build Tools and Git.
- https://aka.ms/vs/stable/vs_BuildTools.exe
- https://git-scm.com/install/windows

2. Install Rust by downloading rustup-init.exe from Rust website.
- https://rust-lang.org/learn/get-started/

3. Clone this repository.
```pwsh
git clone https://github.com/HikariCalyx/cmsdl
cd cmsdl
```

4. Build it.
```pwsh
cargo build --release
```

5. Optional: You can compress the binary with [UPX](https://github.com/upx/upx/releases) for much smaller size.
```
cd target\release
upx cmsdl.exe
```

### Linux
1. We assume you're using Debian-based distro, like Ubuntu. Please install necessary build tools:
```bash
sudo apt update
sudo apt install build-essential curl git
```

2. Install Rust.
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

3. Clone this repository.
```bash
git clone https://github.com/HikariCalyx/cmsdl
cd cmsdl
```

4. Build it.
```bash
cargo build --release
```

### macOS
1. Install Xcode commandline tools.
```bash
xcode-select --install
```

2. Install Rust.
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

3. Clone this repository.
```bash
git clone https://github.com/HikariCalyx/cmsdl
cd cmsdl
```

4. Build it.
```bash
cargo build --release
```

## Credits
- @Deneo , for reverse-enginerring job
- @InWILL for Locale Remulator
- [choyang](https://x.com/choyang___) & [shio_rice](https://x.com/shio_rice0) for splash screen
- GUI part used [Maplestory OTF fonttype](https://maplestory.nexon.com/Media/Font)