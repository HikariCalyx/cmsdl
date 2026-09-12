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

- 查看已有的更新檔
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


### 陸服懷舊服 (cms_cw)
`cms_cw`（冒險島懷舊服 / MapleStory Classic World CN）支援與 `cms` 相同的指令：
```bash
./cmsdl cms_cw --check
./cmsdl cms_cw --download /path/to/cms_cw/client
./cmsdl cms_cw --patch latest /path/to/cms_cw/client
./cmsdl cms_cw --create-shortcut /path/to/cms_cw/client
```

任何需要指定大區的指令都可以用 `cms_cw` 替代 `cms`，包括下方的升級路徑檢查。

### 台服 (新楓之谷)
- 檢查最新的TMS客戶端：
```bash
./cmsdl tms --check
```

- 下載/修復/檢查TMS客戶端：
```bash
./cmsdl tms --download /path/to/tms/client
```

客戶端主程式會保持最新：當 `MapleStory.exe` 缺失，或仍是清單中列出的原始版本時，會改為下載並安裝目前版本對應的獨立主程式 Hotfix（`ExePatch.dat`）；已被 Hotfix 替換過的主程式不會被再次覆蓋。

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

### 升級路徑檢查
在更新之前，可以先檢查更新到指定版本所需的增量更新檔是否比直接下載該版本的完整客戶端更小：
```bash
./cmsdl cms --upgrade-path-check latest /path/to/cms/client
./cmsdl cms_cw --upgrade-path-check latest /path/to/cms_cw/client
./cmsdl tms --upgrade-path-check latest /path/to/tms/client
```

第一個參數是目標版本（CMS/CMS_CW 形如 `0.0.0.22`，TMS 形如 `281`）或 `latest`。程式會從客戶端目錄讀取目前版本（CMS 讀取 `LocalVersion3.xml`，讀取失敗時回退到 `Base.wz`；TMS 讀取 `Data/Base/Base.wz`）。

加上 `--verbose`（或 `-v`）還會列出所有將被套用的更新檔，以及每個更新檔的大小。

當升級路徑不大於完整客戶端時，程式會印出建議執行的指令：
```
current version: 278
target version:  282
patches needed:  2 (9.19 GiB)
full client:     version V282 (67.74 GiB)
you may apply the patch with cmsdl.exe tms --patch 282 B:\tms_upgtest
```

退出代碼：

| 代碼 | 含義 |
| ---- | ---- |
| 0 | 更新檔總大小不超過完整客戶端。 |
| 1 | 找不到可用的更新檔。 |
| 2 | 所需更新檔比完整客戶端更大，建議重新下載。 |
| 3 | 無法讀取目前客戶端版本。 |
| 4 | 指定的目標版本早於目前客戶端版本。 |
| 100 | 無法存取更新伺服器（重試後仍然失敗）。 |

說明：
- CMS/CMS_CW 會與該目標版本對應的完整客戶端比較。如果該完整客戶端尚未發布，則改用最近已發布的客戶端（會標註為回退）。
- TMS 只發布最新完整客戶端的清單，因此指定具體版本時會跳過完整客戶端比較。使用 `latest` 時，如果最新完整客戶端還沒有對應的更新檔，程式會回退到更新伺服器上實際可到達的最後一個版本。
- 對於 TMS，如果客戶端已經是最新大版本，還會把獨立可執行檔熱修復（`ExePatch.dat`）的大小計入。
- Windows 安裝程式在更新前（圖形介面模式）會自動執行此檢查；當沒有可用更新檔，或更新檔比客戶端本身更大時，會提示重新安裝完整客戶端。

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
