# cmsdl
A downloader designed for Greater China region mushroom game.

## Why create this?
I don't want to use the bloated launcher developed by SQ Games at all.
This program is meant for a full replacement of CMS official v3 Launcher.

For chat records about how it was developed, see chat_records directory.

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
On Windows, it will also create shortcuts at same location of download path, Desktop, and Start Menu - Programs when download completes.

If the download was interrupted, you can rerun and it will continue to download.

- List all available CMS patches
```bash
~/cmsdl cms --patch list
```

- Patch CMS client to latest version (including minor patches), and/or launch the game after patching:
```bash
~/cmsdl cms --patch latest /path/to/cms/client [--launch-after-patching]
```

### TMS
- Get latest TMS client:
```bash
~/cmsdl tms --check
```

- Download, or checksum check, or repair TMS client:
```bash
~/cmsdl tms --download /path/to/tms/client
```

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
https://rust-lang.org/learn/get-started/

3. Clone this repository.
```pwsh
git clone https://github.com/HikariCalyx/cmsdl
cd cmsdl
```

4. Build it.
```pwsh
cargo build --release
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
@Deneo , for reverse-enginerring job