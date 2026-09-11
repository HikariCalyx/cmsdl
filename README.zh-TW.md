# cmsdl
一款為大中華區蘑菇遊戲設計的下載器。

*本專案未獲得SQ Games和Black Orange Games的官方支持或認可*

## 為什麼要開發這個？
我完全不想用SQ Games官方開發的臃腫啟動器。開發的主要目標是作為官方啟動器的完全替代品。

如需了解它的開發過程，請查看 chat_records 目錄。

## 我能否將CMSDL整合進我的專案？
只要不是用於遊戲作弊，我們歡迎您將 CMSDL 整合進您的專案。

## 我會因為使用它而被營運商封鎖帳號嗎？

不會。本程式設計上所需的權限極低，完全不需要提升權限。使用 cmsdl 啟動遊戲後，cmsdl 會在遊戲啟動後自動關閉。此外，自 2023 年起，所有地區的蘑菇遊戲都已實裝防篡改機制：如果遊戲資料損毀或被修改，伺服器將拒絕您登入遊戲。

歡迎查看原始碼。如果您仍對安全性有疑慮，那就不要使用。

我們不歡迎外掛玩家。

## 使用方法
### 陸服 (冒險島 Online)
- 檢查最新的CMS客戶端：
```bash
./cmsdl cms --check
```

- 下載/修復/檢查CMS客戶端：
```bash
./cmsdl cms --download /path/to/cms/client
```

如果下載中斷，重新開啟時會從中斷的地方繼續。

- 只下載所有路徑中包含_Canvas，String，Reactor的檔案到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor"
```

- 只下載所有路徑中不包含_Canvas，String，Reactor的檔案到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- 只下載所有路徑中所有以 .wz 結尾的檔案，以及 Maple 開頭的檔案到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple"
```

- 只下載所有路徑中所有不以 .wz 結尾的檔案，且不以 Maple 開頭的檔案到 /path/to/cms/client：
```bash
./cmsdl cms --download /path/to/cms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

- 查看已有的補丁檔案
```bash
./cmsdl cms --patch list
```

- 更新CMS客戶端到最新版本（包括minor patch），如果需要，請在更新完成後啟動客戶端：
```bash
./cmsdl cms --patch latest /path/to/cms/client [--launch-after-patching]
```

- 建立啟動捷徑（僅限 Windows）：
```bash
./cmsdl cms --create-shortcut /path/to/cms/client
```


### 台服 (新楓之谷)
- 檢查最新的TMS客戶端：
```bash
./cmsdl tms --check
```

- 下載/修復/檢查TMS客戶端：
```bash
./cmsdl tms --download /path/to/tms/client
```

- 只下載所有路徑中包含_Canvas，String，Reactor的檔案到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor"
```

- 只下載所有路徑中不包含_Canvas，String，Reactor的檔案到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter="_Canvas:String:Reactor" --invert-filter
```

- 只下載所有路徑中所有以 .wz 結尾的檔案，以及 Maple 開頭的檔案到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple"
```

- 只下載所有路徑中所有不以 .wz 結尾的檔案，且不以 Maple 開頭的檔案到 /path/to/tms/client：
```bash
./cmsdl tms --download /path/to/tms/client --filter-regex=".wz$":"^Maple" --invert-filter
```

### 額外說明
如果你想確保cmsdl走遊戲加速器，請將 cmsdl 程式更名為 MapleStory.exe，以便遊戲加速器捕捉到。

如果你看到SSL錯誤，你可以加上參數 `--allow-insecure`（不建議）。

如果你想走代理下載，可以加上參數 `--proxy` 以透過系統代理下載。或者你也可以自行指定代理伺服器位址： `socks5://127.0.0.1:9000`

## 編譯
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
