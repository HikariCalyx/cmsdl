# cmsdl
一款为大中华区蘑菇游戏设计的下载器。

*本项目未获得SQ Games和Black Orange Games的官方支持或认可*

## 为什么要开发这个？
我完全不想用SQ Games官方开发的臃肿启动器。开发的主要目标是作为官方启动器的完全替代品。

如需了解它的开发过程，请查看 chat_records 目录。

## 我能否将CMSDL整合进我的项目？
只要不是用于游戏作弊，我们欢迎您将 CMSDL 整合进您的项目。

## 我会因为使用它而被运营商封号吗？

不会。本程序的设计权限要求极低，完全不需要提升权限。使用 cmsdl 启动游戏后，cmsdl 会在游戏启动后自动关闭。此外，自 2023 年起，所有地区的蘑菇游戏都已实装防篡改机制：如果游戏数据损坏或被修改，服务器将拒绝您登录游戏。

欢迎查看源代码。如果您仍对安全性有疑虑，那就不要使用。

我们不欢迎外挂玩家。

## 使用方法
### 国服 (冒险岛 Online)
- 检查最新的CMS客户端：
```bash
./cmsdl cms --check
```

- 下载/修复/检查CMS客户端：
```bash
./cmsdl cms --download /path/to/cms/client
```

如果下载中断，重新开启时会从中断的地方继续。

- 只下载所有路径中包含_Canvas，String，Reactor的文件到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor"
```

- 只下载所有路径中不包含_Canvas，String，Reactor的文件到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- 只下载所有路径中所有以 .wz 结尾的文件，以及 Maple 开头的文件到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple"
```

- 只下载所有路径中所有不以 .wz 结尾的文件，且不以 Maple 开头的文件到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

- 查看已有的补丁文件
```bash
./cmsdl cms --patch list
```

- 更新CMS客户端到最新版本（包括minor patch），如果需要，请在更新完成后启动客户端：
```bash
./cmsdl cms --patch latest /path/to/cms/client [--launch-after-patching]
```

- 创建启动快捷方式（仅限 Windows）：
```bash
./cmsdl cms --create-shortcut /path/to/cms/client
```


### 国服怀旧服 (cms_cw)
`cms_cw`（冒险岛怀旧服 / MapleStory Classic World CN）支持与 `cms` 相同的命令：
```bash
./cmsdl cms_cw --check
./cmsdl cms_cw --download /path/to/cms_cw/client
./cmsdl cms_cw --patch latest /path/to/cms_cw/client
./cmsdl cms_cw --create-shortcut /path/to/cms_cw/client
```

任何需要指定大区的命令都可以用 `cms_cw` 替代 `cms`，包括下方的升级路径检查。

### 台服 (新枫之谷)
- 检查最新的TMS客户端：
```bash
./cmsdl tms --check
```

- 下载/修复/检查TMS客户端：
```bash
./cmsdl tms --download /path/to/tms/client
```

客户端主程序会保持最新：当 `MapleStory.exe` 缺失，或仍是清单中列出的原始版本时，会改为下载并安装当前版本对应的独立主程序热修复（`ExePatch.dat`）；已被热修复替换过的主程序不会被再次覆盖。

- 只下载所有路径中包含_Canvas，String，Reactor的文件到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor"
```

- 只下载所有路径中不包含_Canvas，String，Reactor的文件到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- 只下载所有路径中所有以 .wz 结尾的文件，以及 Maple 开头的文件到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple"
```

- 只下载所有路径中所有不以 .wz 结尾的文件，且不以 Maple 开头的文件到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

### 升级路径检查
在更新之前，可以先检查更新到指定版本所需的增量补丁是否比直接下载该版本的完整客户端更小：
```bash
./cmsdl cms --upgrade-path-check latest /path/to/cms/client
./cmsdl cms_cw --upgrade-path-check latest /path/to/cms_cw/client
./cmsdl tms --upgrade-path-check latest /path/to/tms/client
```

第一个参数是目标版本（CMS/CMS_CW 形如 `0.0.0.22`，TMS 形如 `281`）或 `latest`。程序会从客户端目录读取当前版本（CMS 读取 `LocalVersion3.xml`，读取失败时回退到 `Base.wz`；TMS 读取 `Data/Base/Base.wz`）。

加上 `--verbose`（或 `-v`）还会列出所有将被应用的补丁，以及每个补丁文件的大小。

当升级路径不大于完整客户端时，程序会打印建议执行的命令：
```
current version: 278
target version:  282
patches needed:  2 (9.19 GiB)
full client:     version V282 (67.74 GiB)
you may apply the patch with cmsdl.exe tms --patch 282 B:\tms_upgtest
```

退出代码：

| 代码 | 含义 |
| ---- | ---- |
| 0 | 补丁总大小不超过完整客户端。 |
| 1 | 找不到可用的补丁。 |
| 2 | 所需补丁比完整客户端更大，建议重新下载。 |
| 3 | 无法读取当前客户端版本。 |
| 4 | 指定的目标版本早于当前客户端版本。 |
| 100 | 无法访问补丁服务器（重试后仍然失败）。 |

说明：
- CMS/CMS_CW 会与该目标版本对应的完整客户端比较。如果该完整客户端尚未发布，则改用最近已发布的客户端（会标注为回退）。
- TMS 只发布最新完整客户端的清单，因此指定具体版本时会跳过完整客户端比较。使用 `latest` 时，如果最新完整客户端还没有对应的补丁，程序会回退到补丁服务器上实际可到达的最后一个版本。
- 对于 TMS，如果客户端已经是最新大版本，还会把独立可执行文件热修复（`ExePatch.dat`）的大小计入。
- Windows 安装程序在更新前（图形界面模式）会自动执行此检查；当没有可用补丁，或补丁比客户端本身更大时，会提示重新安装完整客户端。

### 额外说明
如果你想确保cmsdl走网游加速器，请将 cmsdl 程序更名为 MapleStory.exe，以便网游加速器捕捉到。

如果你看到SSL错误，你可以加上参数 `--allow-insecure`（不推荐）。

如果你想走代理下载，可以加上参数 `--proxy` 以通过系统代理下载。或者你也可以自行指定代理服务器地址： `socks5://127.0.0.1:9000`

## 编译
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