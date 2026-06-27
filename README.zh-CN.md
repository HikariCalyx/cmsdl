# cmsdl
一款为大中华区蘑菇游戏设计的下载器。

## 为什么要开发这个？
我完全不想用SQ Games官方开发的臃肿启动器。开发的主要目标是作为官方启动器的完全替代品。

如需了解它的开发过程，请查看 chat_records 目录。

## 使用方法
### 国服
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


### 台服
- 检查最新的TMS客户端：
```bash
./cmsdl tms --check
```

- 下载/修复/检查TMS客户端：
```bash
./cmsdl tms --download /path/to/tms/client
```

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