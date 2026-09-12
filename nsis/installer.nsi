; MapleStory CN CMSDL Installer
; NSIS Installer Script

; ============================================================================
; Configuration
; ============================================================================

SetCompressor /solid /final lzma
Unicode true

!include "MUI2.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"
!include "FileFunc.nsh"

; Window show/hide flags used to reveal the advanced-options controls
; (guarded because one of the included headers already defines them).
!ifndef SW_HIDE
  !define SW_HIDE 0
!endif
!ifndef SW_SHOW
  !define SW_SHOW 5
!endif

; Version
!define VERSION "4.228.1.3"

; Product Info (English)
!define PRODUCT_NAME "MapleStory CN"

; Product Info (Simplified Chinese) - Change this to your desired name
!define PRODUCT_NAME_ZH "冒险岛"

; Registry key (no spaces)
!define REG_KEY "MapleStoryCN"

!define PRODUCT_PUBLISHER "Hikari Calyx Tech"
!define PRODUCT_WEB_SITE "https://github.com/HikariCalyx/cmsdl"

; Installation Directory default (resolved to the actual system drive at runtime)
!define INSTALL_DIR "C:"

; Installer Icon - must be defined before MUI2 settings
!define MUI_ICON "icon.ico"
!define MUI_UNICON "icon.ico"

; Welcome/Finish page bitmap (left-side image)
!define MUI_WELCOMEFINISHPAGE_BITMAP "cms_inst.bmp"

; ============================================================================
; Variables
; ============================================================================

; Operation mode: "1" = Install (full download), "2" = Update (patch), "3" = Update CMSDL, "4" = MSVC
Var InstallMode
Var Dialog
Var RadioInstall
Var RadioUpdate
Var RadioUpdateCMSDL
Var RadioMSVC
Var RadioFixSDOLogin
Var LinkTroubleshooting
Var CheckNoLR
Var CheckConsole
Var LrHookFlag
Var NoGuiFlag
Var CloseFlag
Var BuildFlag
Var BuildFlagCW
; Game variant selection
Var CheckCMS
Var CheckCMSCW
Var InstallCMS
Var InstallCMSCW
; Finish page launch selection
Var RadioNoLaunch
Var RadioLaunchCMS
Var RadioLaunchCMSCW
Var LaunchVariant
; Advanced CMS client options (variant page)
Var CheckAdvOpt
Var CheckSpecificVer
Var RadioSpecificBuild
Var RadioLatest
Var EditBuildNumber
Var CheckNoUninstaller
Var CheckNoShortcut
Var AdvOptFlag
Var SpecificVerFlag
Var BuildChoiceFlag
Var NoUninstallerFlag
Var NoShortcutFlag
Var BuildNumber
; Result of the upgrade-path check: "patch", "reinstall" or "abort".
Var UpgradeAction
; Heading prefix (client name + newline) for client-specific dialogs.
Var UpgradeHeading

; ============================================================================
; MUI2 Settings
; ============================================================================

; Installer pages
!insertmacro MUI_PAGE_WELCOME
Page custom ModeSelectPage ModeSelectPageLeave
Page custom VariantSelectPage VariantSelectPageLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE DirectoryPagePre
!insertmacro MUI_PAGE_DIRECTORY
!undef MUI_PAGE_CUSTOMFUNCTION_PRE
!insertmacro MUI_PAGE_INSTFILES
!define MUI_PAGE_CUSTOMFUNCTION_SHOW FinishPageShow
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE FinishPageLeave
!insertmacro MUI_PAGE_FINISH

; Uninstaller pages
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; Language - English
!insertmacro MUI_LANGUAGE "English"

; Language - Simplified Chinese
!insertmacro MUI_LANGUAGE "SimpChinese"

; ============================================================================
; Language Strings - English
; ============================================================================

LangString STR_DOWNLOADING ${LANG_ENGLISH} "Downloading game files..."
LangString STR_PATCHING ${LANG_ENGLISH} "Updating game files..."
LangString STR_DOWNLOAD_FAILED ${LANG_ENGLISH} "Game file download failed with error code $0."
LangString STR_PATCH_FAILED ${LANG_ENGLISH} "Game update failed with error code $0."
LangString STR_MSVC_FAILED ${LANG_ENGLISH} "MSVC runtime installation failed with error code $0."
LangString STR_SHORTCUT_FAILED ${LANG_ENGLISH} "Shortcut creation failed with error code $0."
LangString STR_LAUNCH_PROMPT ${LANG_ENGLISH} "Installation completed. Would you like to launch the game now?"
LangString STR_UNSUPPORTED_OS ${LANG_ENGLISH} "This application requires Windows 10 or later on x64 architecture.$\nYour system does not meet the requirements."
LangString STR_UNSUPPORTED_ARCH ${LANG_ENGLISH} "This application requires x64 architecture.$\nYour system is not x64 compatible."
LangString STR_PRODUCT_NAME ${LANG_ENGLISH} "${PRODUCT_NAME}"
LangString STR_MODE_TITLE ${LANG_ENGLISH} "Choose Operation"
LangString STR_MODE_SUBTITLE ${LANG_ENGLISH} "Select whether to install or update the game."
LangString STR_MODE_INSTALL ${LANG_ENGLISH} "Install or repair (download the full game)"
LangString STR_MODE_UPDATE ${LANG_ENGLISH} "Update (update an existing game installation)"
LangString STR_MODE_UPDATE_CMSDL ${LANG_ENGLISH} "Update CMSDL"
LangString STR_MODE_MSVC ${LANG_ENGLISH} "Repair Runtime (VCRUNTIME140.dll missing, etc)"
LangString STR_MODE_FIX_SDOLOGIN ${LANG_ENGLISH} "Fix SDOLogin error"
LangString STR_FIX_SDOLOGIN_WARNING ${LANG_ENGLISH} "Fixing the SDOLogin error will clear all your existing account records.$\nDo you want to continue?"
LangString STR_FIX_SDOLOGIN_NO_GAME ${LANG_ENGLISH} "MapleStory.exe was not found in the selected directory. Please select a valid game installation."
LangString STR_FIX_SDOLOGIN_FAILED ${LANG_ENGLISH} "SDOLogin fix failed with error code $0."
LangString STR_FIX_SDOLOGIN_UAC_FIREWALL ${LANG_ENGLISH} "The SDOLogin fix requires administrator privileges to add firewall rules. Please click Yes to continue."
LangString STR_FIX_SDOLOGIN_UAC_RETRY ${LANG_ENGLISH} "Firewall rules could not be added (administrator privileges required). Would you like to retry?"
LangString STR_LINK_TROUBLESHOOTING ${LANG_ENGLISH} "Troubleshooting (Simplified Chinese only)"
LangString STR_UPDATE_ABORT ${LANG_ENGLISH} "No existing game installation was found in the selected directory. Update cannot continue."
LangString STR_NO_WRITE_PERMISSION ${LANG_ENGLISH} "The target folder cannot be written to.$\nPlease run this installer with Administrator privileges."
LangString STR_DO_NOT_INCLUDE_LR ${LANG_ENGLISH} "Do not include Locale Remulator"
LangString STR_USE_CONSOLE_TYPE ${LANG_ENGLISH} "Use the console-type CMSDL interface"
LangString STR_REMOVE_OFFICIAL_LAUNCHER ${LANG_ENGLISH} "Would you like to remove the official game launcher? Removing it does not affect game launching."
LangString STR_REMOVE_OFFICIAL_LAUNCHER_UAC ${LANG_ENGLISH} "You're currently running official launcher, but you didn't close it. Once you finish closing, please click Retry."
LangString STR_METERED_WARNING ${LANG_ENGLISH} "Your network connection is metered.$\nDownloading the game may incur additional costs.$\n$\nDo you want to continue?"
LangString STR_IS_HDD_WARNING ${LANG_ENGLISH} "You are using a mechanical hard drive, and updating game files may be very slow.$\n$\nDo you want to continue?"
LangString STR_VARIANT_TITLE ${LANG_ENGLISH} "Choose Game Variant"
LangString STR_VARIANT_SUBTITLE ${LANG_ENGLISH} "Select which game variants to install."
LangString STR_VARIANT_CMS ${LANG_ENGLISH} "MapleStory CN (CMS)"
LangString STR_VARIANT_CMS_CW ${LANG_ENGLISH} "MapleStory Classic World CN (cms_cw)"
LangString STR_CLIENT_CMS ${LANG_ENGLISH} "MapleStory CN"
LangString STR_CLIENT_CMS_CW ${LANG_ENGLISH} "MapleStory Classic World CN"
LangString STR_VARIANT_ERROR ${LANG_ENGLISH} "You must select at least one game variant to install."
LangString STR_DOWNLOADING_CMS_CW ${LANG_ENGLISH} "Downloading MapleStory Classic World CN game files..."
LangString STR_DOWNLOAD_CMS_CW_FAILED ${LANG_ENGLISH} "MapleStory Classic World CN download failed with error code $0."
LangString STR_FINISH_NO_LAUNCH ${LANG_ENGLISH} "Do not launch"
LangString STR_FINISH_LAUNCH_CMS ${LANG_ENGLISH} "Launch MapleStory CN"
LangString STR_FINISH_LAUNCH_CMS_CW ${LANG_ENGLISH} "Launch MapleStory Classic World CN"
LangString STR_LAUNCH_PROMPT_CMS_CW ${LANG_ENGLISH} "Installation completed. Would you like to launch MapleStory Classic World CN now?"
LangString STR_CLOSE_QIHOO_360_TOTAL_SECURITY ${LANG_ENGLISH} "Please close or uninstall 360 Total Security and click Retry. If you do not want to close it or cannot close it, click Abort to exit the installation."
LangString STR_VARIANT_ADVANCED ${LANG_ENGLISH} "Show advanced options for CMS client"
LangString STR_VARIANT_SPECIFIC_VER ${LANG_ENGLISH} "Install a specific version of the client"
LangString STR_VARIANT_SPECIFIC_BUILD ${LANG_ENGLISH} "Specific build number:"
LangString STR_VARIANT_LATEST ${LANG_ENGLISH} "Latest version"
LangString STR_VARIANT_NO_UNINSTALLER ${LANG_ENGLISH} "Do not create uninstaller"
LangString STR_VARIANT_NO_SHORTCUT ${LANG_ENGLISH} "Do not create shortcut"
LangString STR_VARIANT_BUILD_INVALID ${LANG_ENGLISH} "Please enter a valid build number (digits only)."
LangString STR_CHECKING_UPGRADE ${LANG_ENGLISH} "Checking if upgrade path is applicable..."
LangString STR_UPGRADE_NO_PATCH ${LANG_ENGLISH} "No patches cannot be found. Would you like to reinstall the latest version of game? "
LangString STR_UPGRADE_TOO_LARGE ${LANG_ENGLISH} "Patch files required to latest version are larger than latest client, we recommend a reinstallation. Would you like to reinstall? "
LangString STR_UPGRADE_NO_CLIENT ${LANG_ENGLISH} "No valid client data can be found. Would you like to reinstall the latest version of game? "
LangString STR_UPGRADE_RETRY ${LANG_ENGLISH} "Unable to get latest patch info. Would you like to retry?"

; ============================================================================
; Language Strings - Simplified Chinese
; ============================================================================

LangString STR_DOWNLOADING ${LANG_SIMPCHINESE} "正在下载游戏文件..."
LangString STR_PATCHING ${LANG_SIMPCHINESE} "正在更新游戏文件..."
LangString STR_DOWNLOAD_FAILED ${LANG_SIMPCHINESE} "游戏文件下载失败，错误代码：$0。"
LangString STR_PATCH_FAILED ${LANG_SIMPCHINESE} "游戏更新失败，错误代码：$0。"
LangString STR_MSVC_FAILED ${LANG_SIMPCHINESE} "MSVC 运行时安装失败，错误代码：$0。"
LangString STR_SHORTCUT_FAILED ${LANG_SIMPCHINESE} "创建快捷方式失败，错误代码：$0。"
LangString STR_LAUNCH_PROMPT ${LANG_SIMPCHINESE} "安装完成。您要立即启动游戏吗？"
LangString STR_UNSUPPORTED_OS ${LANG_SIMPCHINESE} "此应用程序需要 Windows 10 或更高版本（x64 架构）。$\n请升级操作系统后再使用。"
LangString STR_UNSUPPORTED_ARCH ${LANG_SIMPCHINESE} "此应用程序需要 x64 架构。$\n您的系统不兼容 x64。"
LangString STR_PRODUCT_NAME ${LANG_SIMPCHINESE} "${PRODUCT_NAME_ZH}"
LangString STR_MODE_TITLE ${LANG_SIMPCHINESE} "选择操作"
LangString STR_MODE_SUBTITLE ${LANG_SIMPCHINESE} "请选择是安装还是更新游戏。"
LangString STR_MODE_INSTALL ${LANG_SIMPCHINESE} "安装或修复（下载完整游戏）"
LangString STR_MODE_UPDATE ${LANG_SIMPCHINESE} "更新（更新现有游戏）"
LangString STR_MODE_UPDATE_CMSDL ${LANG_SIMPCHINESE} "升级 CMSDL"
LangString STR_MODE_MSVC ${LANG_SIMPCHINESE} "修复运行时（VCRUNTIME140.dll 丢失等错误）"
LangString STR_MODE_FIX_SDOLOGIN ${LANG_SIMPCHINESE} "尝试修复登录器错误（点击大区卡死、R6025报错等）"
LangString STR_FIX_SDOLOGIN_WARNING ${LANG_SIMPCHINESE} "修复登录器将会清除您原有的所有账号记录。$\n您还想继续吗？"
LangString STR_FIX_SDOLOGIN_NO_GAME ${LANG_SIMPCHINESE} "在所选目录中未找到 MapleStory.exe。请选择有效的游戏安装目录。"
LangString STR_FIX_SDOLOGIN_FAILED ${LANG_SIMPCHINESE} "登录器修复失败，错误代码：$0。"
LangString STR_FIX_SDOLOGIN_UAC_FIREWALL ${LANG_SIMPCHINESE} "登录器修复需要管理员权限以添加防火墙规则。请点击“是”以继续。"
LangString STR_FIX_SDOLOGIN_UAC_RETRY ${LANG_SIMPCHINESE} "无法添加防火墙规则（需要管理员权限）。您要重试吗？"
LangString STR_LINK_TROUBLESHOOTING ${LANG_SIMPCHINESE} "使用遇到问题了？点击查看帮助"
LangString STR_UPDATE_ABORT ${LANG_SIMPCHINESE} "在所选目录中未找到现有的游戏安装。无法继续更新。"
LangString STR_NO_WRITE_PERMISSION ${LANG_SIMPCHINESE} "无法写入目标文件夹。$\n请以管理员身份运行此安装程序。"
LangString STR_DO_NOT_INCLUDE_LR ${LANG_SIMPCHINESE} "你不应该看到这个选项"
LangString STR_USE_CONSOLE_TYPE ${LANG_SIMPCHINESE} "使用命令行样式的CMSDL界面"
LangString STR_REMOVE_OFFICIAL_LAUNCHER ${LANG_SIMPCHINESE} "您想要移除官方游戏启动器吗？移除该启动器不会影响启动游戏。"
LangString STR_REMOVE_OFFICIAL_LAUNCHER_UAC ${LANG_SIMPCHINESE} "您当前正在运行官方启动器，但尚未关闭它。关闭后，请点击重试。"
LangString STR_METERED_WARNING ${LANG_SIMPCHINESE} "您的网络连接为按流量计费的连接。$\n下载游戏可能会产生额外费用。$\n$\n您是否要继续？"
LangString STR_IS_HDD_WARNING ${LANG_SIMPCHINESE} "您正在使用机械硬盘，游戏文件的更新可能会非常缓慢。$\n$\n您是否要继续？"
LangString STR_VARIANT_TITLE ${LANG_SIMPCHINESE} "选择游戏版本"
LangString STR_VARIANT_SUBTITLE ${LANG_SIMPCHINESE} "选择要安装的游戏版本。"
LangString STR_VARIANT_CMS ${LANG_SIMPCHINESE} "冒险岛正式服"
LangString STR_VARIANT_CMS_CW ${LANG_SIMPCHINESE} "冒险岛怀旧服"
LangString STR_CLIENT_CMS ${LANG_SIMPCHINESE} "冒险岛正式服"
LangString STR_CLIENT_CMS_CW ${LANG_SIMPCHINESE} "冒险岛怀旧服"
LangString STR_VARIANT_ERROR ${LANG_SIMPCHINESE} "您必须至少选择一个游戏版本。"
LangString STR_DOWNLOADING_CMS_CW ${LANG_SIMPCHINESE} "正在下载冒险岛怀旧服游戏文件..."
LangString STR_DOWNLOAD_CMS_CW_FAILED ${LANG_SIMPCHINESE} "冒险岛怀旧服下载失败，错误代码：$0。"
LangString STR_FINISH_NO_LAUNCH ${LANG_SIMPCHINESE} "不启动"
LangString STR_FINISH_LAUNCH_CMS ${LANG_SIMPCHINESE} "启动冒险岛正式服"
LangString STR_FINISH_LAUNCH_CMS_CW ${LANG_SIMPCHINESE} "启动冒险岛怀旧服"
LangString STR_LAUNCH_PROMPT_CMS_CW ${LANG_SIMPCHINESE} "安装完成。您要立即启动冒险岛怀旧服吗？"
LangString STR_CLOSE_QIHOO_360_TOTAL_SECURITY ${LANG_SIMPCHINESE} "请关闭或卸载 360 安全卫士后，点击重试按钮。若不愿意关闭或无法关闭，可点击中止按钮退出安装。"
LangString STR_VARIANT_ADVANCED ${LANG_SIMPCHINESE} "为正式服客户端显示高级选项"
LangString STR_VARIANT_SPECIFIC_VER ${LANG_SIMPCHINESE} "安装指定版本的客户端"
LangString STR_VARIANT_SPECIFIC_BUILD ${LANG_SIMPCHINESE} "指定构建号："
LangString STR_VARIANT_LATEST ${LANG_SIMPCHINESE} "最新版本"
LangString STR_VARIANT_NO_UNINSTALLER ${LANG_SIMPCHINESE} "不创建卸载程序"
LangString STR_VARIANT_NO_SHORTCUT ${LANG_SIMPCHINESE} "不创建快捷方式"
LangString STR_VARIANT_BUILD_INVALID ${LANG_SIMPCHINESE} "请输入有效的构建号（仅限数字）。"
LangString STR_CHECKING_UPGRADE ${LANG_SIMPCHINESE} "正在检查升级路径是否可用..."
LangString STR_UPGRADE_NO_PATCH ${LANG_SIMPCHINESE} "未找到可用的补丁。是否要重新安装最新版本的游戏？"
LangString STR_UPGRADE_TOO_LARGE ${LANG_SIMPCHINESE} "升级到最新版本所需的补丁文件比最新客户端更大，我们建议重新安装。是否要重新安装？"
LangString STR_UPGRADE_NO_CLIENT ${LANG_SIMPCHINESE} "未找到有效的客户端数据。是否要重新安装最新版本的游戏？"
LangString STR_UPGRADE_RETRY ${LANG_SIMPCHINESE} "无法获取最新的补丁信息。是否要重试？"

; ============================================================================
; Installer Attributes
; ============================================================================

; Product name resolves at runtime based on selected language
Name "$(STR_PRODUCT_NAME) ${VERSION}"
OutFile "MapleStoryCN-${VERSION}-installer.exe"
InstallDir "${INSTALL_DIR}"
InstallDirRegKey HKCU "Software\${REG_KEY}" "InstallDir"

; No elevation required
RequestExecutionLevel user

ShowInstDetails show
ShowUnInstDetails show
BrandingText "Powered by CMSDL"

; ============================================================================
; Initialize
; ============================================================================

Function .onInit
  ; Resolve the install directory to the actual system drive (e.g. D:) when
  ; no previous installation path is stored in the registry. This cannot be
  ; done at compile time because $%SystemDrive% is a Windows-only env var.
  ReadRegStr $R0 HKCU "Software\${REG_KEY}" "InstallDir"
  ${If} $R0 == ""
    ReadEnvStr $R0 SystemDrive
    ${If} $R0 != ""
      StrCpy $INSTDIR $R0
    ${EndIf}
  ${EndIf}

  ; Default operation mode is Install
  StrCpy $InstallMode "1"

  ; Default to the graphical patcher (console mode off). In GUI mode the
  ; window auto-closes when finished; --close-after-finishing is omitted when
  ; --no-gui is selected.
  StrCpy $NoGuiFlag ""
  StrCpy $CloseFlag " --close-after-finishing"

  ; Select language based on OS language (Simplified Chinese = 0804).
  ; Set this first so the requirement-check message boxes are localized.
  StrCpy $LANGUAGE ${LANG_ENGLISH}
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Control\Nls\Language" "Default"
  StrCmp $0 "0804" 0 +2
    StrCpy $LANGUAGE ${LANG_SIMPCHINESE}

  ; Locale Remulator is only useful when the system language is NOT
  ; Simplified Chinese (legacy locale-based app compat is not needed).
  ; For Simplified Chinese systems, leave the flag empty.
  StrCpy $LrHookFlag " --lrhook"
  StrCmp $0 "0804" 0 +2
    StrCpy $LrHookFlag ""

  ; Check if system is x64
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "$(STR_UNSUPPORTED_ARCH)"
    Quit
  ${EndIf}

  ; Check Windows version (Windows 10 and later)
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP "$(STR_UNSUPPORTED_OS)"
    Quit
  ${EndIf}

  ; Set the automatic build flag used for a full install.
  Call SetDefaultBuildFlag

  ; Default game variants: install both.
  StrCpy $InstallCMS "1"
  StrCpy $InstallCMSCW "0"
  StrCpy $LaunchVariant "1"

  ; Advanced CMS client options default to off; "latest version" is the
  ; default when the specific-version sub-option is enabled.
  StrCpy $AdvOptFlag ""
  StrCpy $SpecificVerFlag ""
  StrCpy $BuildChoiceFlag ""
  StrCpy $NoUninstallerFlag ""
  StrCpy $NoShortcutFlag ""
  StrCpy $BuildNumber ""
FunctionEnd

; ============================================================================
; Helper: automatic build flag for a full install
; ============================================================================

; If the current date is on or before October 20, 2026, add --build 1120 (CMS)
; and --build 1126 (CMS CW) to the download commands (required for specific
; game build rollouts).
Function SetDefaultBuildFlag
  ${GetTime} "" "L" $0 $1 $2 $3 $4 $5 $6
  ; $2 = year (4 digits), $1 = month, $0 = day of month
  StrCpy $BuildFlag ""
  StrCpy $BuildFlagCW ""
  IntCmp $2 2026 yearEq beforeCutoff afterCutoff
  yearEq:
    IntCmp $1 10 monthEq beforeCutoff afterCutoff
  monthEq:
    IntCmp $0 20 beforeCutoff beforeCutoff afterCutoff
  beforeCutoff:
    StrCpy $BuildFlag "--build 1120"
    StrCpy $BuildFlagCW "--build 1126"
  afterCutoff:
FunctionEnd

; ============================================================================
; Mode Selection Page (Install vs Update)
; ============================================================================

Function ModeSelectPage
  !insertmacro MUI_HEADER_TEXT "$(STR_MODE_TITLE)" "$(STR_MODE_SUBTITLE)"

  nsDialogs::Create 1018
  Pop $Dialog
  StrCmp $Dialog "error" modeDone

  ${NSD_CreateRadioButton} 10u 6u 95% 12u "$(STR_MODE_INSTALL)"
  Pop $RadioInstall
  ${NSD_CreateRadioButton} 10u 24u 95% 12u "$(STR_MODE_UPDATE)"
  Pop $RadioUpdate

  ${NSD_CreateRadioButton} 10u 42u 95% 12u "$(STR_MODE_UPDATE_CMSDL)"
  Pop $RadioUpdateCMSDL
  ${NSD_CreateRadioButton} 10u 60u 95% 12u "$(STR_MODE_MSVC)"
  Pop $RadioMSVC
  ${NSD_CreateRadioButton} 10u 78u 95% 12u "$(STR_MODE_FIX_SDOLOGIN)"
  Pop $RadioFixSDOLogin

  ; Console-mode opt-in checkbox (always available). When checked, the created
  ; shortcut and the post-install launch pass --no-gui so the patcher runs in
  ; the console instead of the graphical window.
  ${NSD_CreateCheckbox} 10u 96u 95% 12u "$(STR_USE_CONSOLE_TYPE)"
  Pop $CheckConsole
  ; Restore previous state if the user went back.
  StrCmp $NoGuiFlag " --no-gui" 0 +2
    ${NSD_Check} $CheckConsole

  ${NSD_CreateLink} 10u 128u 95% 12u "$(STR_LINK_TROUBLESHOOTING)"
  Pop $LinkTroubleshooting
  ${NSD_OnClick} $LinkTroubleshooting OpenTroubleshootingLink

  ; Locale Remulator opt-out checkbox (only visible on non-zh-CN systems).
  StrCmp $LrHookFlag "" restoreSelection
    ${NSD_CreateCheckbox} 10u 112u 95% 12u "$(STR_DO_NOT_INCLUDE_LR)"
    Pop $CheckNoLR
    ; Restore previous checkbox state if going back.
    StrCmp $LrHookFlag " --lrhook" 0 +2
      ${NSD_Uncheck} $CheckNoLR
      Goto restoreSelection
    ${NSD_Check} $CheckNoLR

  restoreSelection:
  ; Restore previous selection
  StrCmp $InstallMode "2" selUpdate
  StrCmp $InstallMode "3" selUpdateCMSDL
  StrCmp $InstallMode "4" selMSVC
  StrCmp $InstallMode "5" selFixSDOLogin
    ${NSD_Check} $RadioInstall
    Goto modeShow
  selUpdate:
    ${NSD_Check} $RadioUpdate
    Goto modeShow
  selUpdateCMSDL:
    ${NSD_Check} $RadioUpdateCMSDL
    Goto modeShow
  selMSVC:
    ${NSD_Check} $RadioMSVC
    Goto modeShow
  selFixSDOLogin:
    ${NSD_Check} $RadioFixSDOLogin

  modeShow:
  nsDialogs::Show
  modeDone:
FunctionEnd

Function ModeSelectPageLeave
  ${NSD_GetState} $RadioUpdate $0
  StrCmp $0 "1" setUpdate
  ${NSD_GetState} $RadioUpdateCMSDL $0
  StrCmp $0 "1" setUpdateCMSDL
  ${NSD_GetState} $RadioMSVC $0
  StrCmp $0 "1" setMSVC
  ${NSD_GetState} $RadioFixSDOLogin $0
  StrCmp $0 "1" setFixSDOLogin
    StrCpy $InstallMode "1"
    Goto leaveDone
  setUpdate:
    StrCpy $InstallMode "2"
    Goto leaveDone
  setUpdateCMSDL:
    StrCpy $InstallMode "3"
    Goto leaveDone
  setMSVC:
    StrCpy $InstallMode "4"
    Goto leaveDone
  setFixSDOLogin:
    StrCpy $InstallMode "5"
  leaveDone:
    ; If the opt-out checkbox exists and is checked, clear the lrhook flag.
    StrCmp $LrHookFlag "" doneLR
      ${NSD_GetState} $CheckNoLR $0
      ${If} $0 == 1
        StrCpy $LrHookFlag ""
      ${Else}
        StrCpy $LrHookFlag " --lrhook"
      ${EndIf}
    doneLR:

    ; Console-mode checkbox: set the --no-gui flag when checked. In GUI mode
    ; also request auto-close after patching; omit it when --no-gui is set.
    ${NSD_GetState} $CheckConsole $0
    ${If} $0 == 1
      StrCpy $NoGuiFlag " --no-gui"
      StrCpy $CloseFlag ""
    ${Else}
      StrCpy $NoGuiFlag ""
      StrCpy $CloseFlag " --close-after-finishing"
    ${EndIf}

FunctionEnd

; ============================================================================
; Variant Selection Page (only shown for MODE_INSTALL)
; ============================================================================

Function VariantSelectPage
  ; Show for install and update modes; skip otherwise.
  StrCmp $InstallMode "1" variantShow
  StrCmp $InstallMode "2" variantShow
  Goto variantSkip
variantShow:

  !insertmacro MUI_HEADER_TEXT "$(STR_VARIANT_TITLE)" "$(STR_VARIANT_SUBTITLE)"

  nsDialogs::Create 1018
  Pop $Dialog
  StrCmp $Dialog "error" variantDone

  ${NSD_CreateCheckbox} 10u 6u 95% 12u "$(STR_VARIANT_CMS)"
  Pop $CheckCMS
  ${NSD_CreateCheckbox} 10u 22u 95% 12u "$(STR_VARIANT_CMS_CW)"
  Pop $CheckCMSCW

  ; Restore previous variant selection if the user went back.
  StrCmp $InstallCMS "1" 0 +2
    ${NSD_Check} $CheckCMS
  StrCmp $InstallCMSCW "1" 0 +2
    ${NSD_Check} $CheckCMSCW

  ; Advanced CMS client options are only offered for a fresh install. In
  ; Update mode these controls are not created, so the checkbox is hidden.
  ${If} $InstallMode == 1
    ; --- Advanced CMS client options (revealed by the checkbox below) ---
    ${NSD_CreateCheckbox} 10u 40u 95% 12u "$(STR_VARIANT_ADVANCED)"
    Pop $CheckAdvOpt
    ${NSD_OnClick} $CheckAdvOpt UpdateAdvancedUI

    ${NSD_CreateCheckbox} 10u 58u 95% 12u "$(STR_VARIANT_SPECIFIC_VER)"
    Pop $CheckSpecificVer
    ${NSD_OnClick} $CheckSpecificVer UpdateAdvancedUI

    ${NSD_CreateRadioButton} 26u 76u 132u 12u "$(STR_VARIANT_SPECIFIC_BUILD)"
    Pop $RadioSpecificBuild
    ${NSD_AddStyle} $RadioSpecificBuild ${WS_GROUP}
    ${NSD_OnClick} $RadioSpecificBuild UpdateAdvancedUI

    ${NSD_CreateText} 164u 76u 72u 12u ""
    Pop $EditBuildNumber
    ${NSD_OnChange} $EditBuildNumber OnBuildTextChange

    ${NSD_CreateRadioButton} 26u 94u 95% 12u "$(STR_VARIANT_LATEST)"
    Pop $RadioLatest
    ${NSD_OnClick} $RadioLatest UpdateAdvancedUI

    ${NSD_CreateCheckbox} 10u 112u 95% 12u "$(STR_VARIANT_NO_UNINSTALLER)"
    Pop $CheckNoUninstaller
    ${NSD_OnClick} $CheckNoUninstaller UpdateAdvancedUI

    ${NSD_CreateCheckbox} 10u 128u 95% 12u "$(STR_VARIANT_NO_SHORTCUT)"
    Pop $CheckNoShortcut
    ${NSD_OnClick} $CheckNoShortcut UpdateAdvancedUI

    ; Restore previous advanced state if the user went back.
    StrCmp $AdvOptFlag "1" 0 +2
      ${NSD_Check} $CheckAdvOpt
    StrCmp $SpecificVerFlag "1" 0 +2
      ${NSD_Check} $CheckSpecificVer
    ${If} $BuildChoiceFlag == "1"
      ${NSD_Check} $RadioSpecificBuild
      ${NSD_SetText} $EditBuildNumber "$BuildNumber"
    ${Else}
      ${NSD_Check} $RadioLatest
    ${EndIf}
    StrCmp $NoUninstallerFlag "1" 0 +2
      ${NSD_Check} $CheckNoUninstaller
    StrCmp $NoShortcutFlag "1" 0 +2
      ${NSD_Check} $CheckNoShortcut

    ; Apply the restored visibility / enabled state.
    Call UpdateAdvancedUI
  ${EndIf}

  nsDialogs::Show
  variantDone:
  Goto variantEnd
variantSkip:
  Abort
variantEnd:
FunctionEnd

Function VariantSelectPageLeave
  ${NSD_GetState} $CheckCMS $0
  ${NSD_GetState} $CheckCMSCW $1
  ${If} $0 == 0
  ${AndIf} $1 == 0
    MessageBox MB_ICONEXCLAMATION "$(STR_VARIANT_ERROR)"
    Abort
  ${EndIf}
  StrCpy $InstallCMS $0
  StrCpy $InstallCMSCW $1

  ; --- Advanced CMS client options (Install mode only) ---
  ${If} $InstallMode == 1
    ${NSD_GetState} $CheckAdvOpt $0
    StrCpy $AdvOptFlag $0
    ${NSD_GetState} $CheckNoUninstaller $0
    StrCpy $NoUninstallerFlag $0
    ${NSD_GetState} $CheckNoShortcut $0
    StrCpy $NoShortcutFlag $0

    ; Version selection (only meaningful for a full CMS download). An explicit
    ; choice overrides the automatic (date-based) --build flag.
    ${NSD_GetState} $CheckSpecificVer $0
    ${If} $0 == 1
      StrCpy $SpecificVerFlag "1"
      ${NSD_GetState} $RadioSpecificBuild $1
      ${If} $1 == 1
        StrCpy $BuildChoiceFlag "1"
        ; Build number must be non-empty and digits only.
        ${NSD_GetText} $EditBuildNumber $BuildNumber
        Push $BuildNumber
        Call IsDigits
        Pop $2
        ${If} $2 == 0
          MessageBox MB_ICONEXCLAMATION "$(STR_VARIANT_BUILD_INVALID)"
          Abort
        ${EndIf}
        StrCpy $BuildFlag "--build $BuildNumber"
      ${Else}
        StrCpy $BuildChoiceFlag ""
        StrCpy $BuildNumber ""
        ; "Latest version": drop the automatic build flag.
        StrCpy $BuildFlag ""
      ${EndIf}
    ${Else}
      StrCpy $SpecificVerFlag ""
      StrCpy $BuildChoiceFlag ""
      StrCpy $BuildNumber ""
      ; Leave the automatic (date-based) build flag untouched.
    ${EndIf}
  ${Else}
    ; Update mode does not offer advanced options. Reset everything so a
    ; previous Install-mode selection cannot leak in, and restore the
    ; automatic build flag for any later full install.
    StrCpy $AdvOptFlag ""
    StrCpy $SpecificVerFlag ""
    StrCpy $BuildChoiceFlag ""
    StrCpy $NoUninstallerFlag ""
    StrCpy $NoShortcutFlag ""
    StrCpy $BuildNumber ""
    Call SetDefaultBuildFlag
  ${EndIf}
FunctionEnd

; Keep the last-entered build number across Back/Next navigation.
Function OnBuildTextChange
  ${NSD_GetText} $EditBuildNumber $BuildNumber
FunctionEnd

; Show/hide the advanced CMS client options on the variant page based on the
; current checkbox states. The "install specific version" checkbox reveals the
; radio group and build-number box; the build-number box is only enabled when
; the "specific build number" radio is selected.
Function UpdateAdvancedUI
  ; Visibility of the whole advanced group.
  ${NSD_GetState} $CheckAdvOpt $0
  ${If} $0 == 1
    System::Call "user32::ShowWindow(p $CheckSpecificVer, i ${SW_SHOW})"
    System::Call "user32::ShowWindow(p $CheckNoUninstaller, i ${SW_SHOW})"
    System::Call "user32::ShowWindow(p $CheckNoShortcut, i ${SW_SHOW})"

    ; The "install specific version" checkbox reveals the radio group + box.
    ${NSD_GetState} $CheckSpecificVer $1
    ${If} $1 == 1
      System::Call "user32::ShowWindow(p $RadioSpecificBuild, i ${SW_SHOW})"
      System::Call "user32::ShowWindow(p $EditBuildNumber, i ${SW_SHOW})"
      System::Call "user32::ShowWindow(p $RadioLatest, i ${SW_SHOW})"
    ${Else}
      System::Call "user32::ShowWindow(p $RadioSpecificBuild, i ${SW_HIDE})"
      System::Call "user32::ShowWindow(p $EditBuildNumber, i ${SW_HIDE})"
      System::Call "user32::ShowWindow(p $RadioLatest, i ${SW_HIDE})"
    ${EndIf}
  ${Else}
    System::Call "user32::ShowWindow(p $CheckSpecificVer, i ${SW_HIDE})"
    System::Call "user32::ShowWindow(p $RadioSpecificBuild, i ${SW_HIDE})"
    System::Call "user32::ShowWindow(p $EditBuildNumber, i ${SW_HIDE})"
    System::Call "user32::ShowWindow(p $RadioLatest, i ${SW_HIDE})"
    System::Call "user32::ShowWindow(p $CheckNoUninstaller, i ${SW_HIDE})"
    System::Call "user32::ShowWindow(p $CheckNoShortcut, i ${SW_HIDE})"
  ${EndIf}

  ; Persist the current checkbox states immediately so they survive a Back
  ; navigation (Back skips the Leave callback). Collapsing the group only
  ; hides the controls; their selections are preserved.
  ${NSD_GetState} $CheckAdvOpt $0
  StrCpy $AdvOptFlag $0
  ${NSD_GetState} $CheckSpecificVer $0
  StrCpy $SpecificVerFlag $0
  ${NSD_GetState} $CheckNoUninstaller $0
  StrCpy $NoUninstallerFlag $0
  ${NSD_GetState} $CheckNoShortcut $0
  StrCpy $NoShortcutFlag $0

  ; Enable the build-number box only for the "specific build number" radio.
  ${NSD_GetState} $RadioSpecificBuild $0
  ${If} $0 == 1
    System::Call "user32::EnableWindow(p $EditBuildNumber, i 1)"
  ${Else}
    System::Call "user32::EnableWindow(p $EditBuildNumber, i 0)"
  ${EndIf}
FunctionEnd

; Returns "1" (top of stack) when the value pushed on the stack is a non-empty
; string of ASCII digits, "0" otherwise.
Function IsDigits
  Exch $R0
  StrLen $R2 $R0
  IntCmp $R2 0 notDigits
  StrCpy $R1 0
digitLoop:
  IntCmp $R1 $R2 isDigits
  StrCpy $R3 $R0 1 $R1
  StrCmp $R3 "0" nextDigit
  StrCmp $R3 "1" nextDigit
  StrCmp $R3 "2" nextDigit
  StrCmp $R3 "3" nextDigit
  StrCmp $R3 "4" nextDigit
  StrCmp $R3 "5" nextDigit
  StrCmp $R3 "6" nextDigit
  StrCmp $R3 "7" nextDigit
  StrCmp $R3 "8" nextDigit
  StrCmp $R3 "9" nextDigit
  Goto notDigits
nextDigit:
  IntOp $R1 $R1 + 1
  Goto digitLoop
isDigits:
  StrCpy $R0 "1"
  Goto digitsDone
notDigits:
  StrCpy $R0 "0"
digitsDone:
  Exch $R0
FunctionEnd

Function OpenTroubleshootingLink
  ExecShell "open" "https://wiki.biligame.com/maplestory/CMSDL故障排除"
FunctionEnd

; Skip the directory page for MSVC mode - no install path is needed.
Function DirectoryPagePre
  StrCmp $InstallMode "4" 0 +2
    Abort
FunctionEnd

; ============================================================================
; Shared: write registry info and uninstaller
; ============================================================================

Function WriteRegInfo
  WriteRegStr HKCU "Software\${REG_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\${REG_KEY}" "Version" "${VERSION}"
  ; Store the localized product name so the uninstaller can locate shortcuts
  WriteRegStr HKCU "Software\${REG_KEY}" "ProductName" "$(STR_PRODUCT_NAME)"

  ; The Control Panel entry and Uninstall.exe are optional: skip both when
  ; "Do not create uninstaller" was chosen on the variant page.
  StrCmp $NoUninstallerFlag "1" noUninstallInfo

  ; Add uninstall information to Control Panel
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayName" "$(STR_PRODUCT_NAME)"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayIcon" "$INSTDIR\cmsdl.exe"

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  noUninstallInfo:
FunctionEnd

; ============================================================================
; 360 Total Security detection
; ============================================================================

Function CheckQihoo360
  ; Detect 360 Total Security / Qihoo 360 Safeguard tray processes.
  ; tasklist enumerates running processes; findstr exits with 0 when it
  ; matches 360Tray.exe or QHSafeTray.exe, and 1 when neither is running.
  checkQihooLoop:
  nsExec::ExecToStack 'cmd /C tasklist /FO CSV /NH | findstr /I /C:$\"360Tray.exe$\" /C:$\"QHSafeTray.exe$\"'
  Pop $R0   ; exit code: 0 = running, 1 = not running
  Pop $R1   ; stdout (discard)
  StrCmp $R0 "0" qihooRunning qihooDone

  qihooRunning:
  MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(STR_CLOSE_QIHOO_360_TOTAL_SECURITY)" IDRETRY checkQihooLoop
  ; The user cannot close it - abort the installation.
  Abort

  qihooDone:
FunctionEnd

; ============================================================================
; Install directory write-permission check
; ============================================================================

Function CheckInstallDirWritable
  ; The installer is not elevated (RequestExecutionLevel user), so a target
  ; such as C:\Program Files cannot be written to without Administrator
  ; rights. MSVC repair (4) doesn't use $INSTDIR.
  StrCmp $InstallMode "4" writableDone

  ; 1) The folder must exist and accept new files.
  ClearErrors
  CreateDirectory "$INSTDIR"
  FileOpen $0 "$INSTDIR\cmsdl_write_test" w
  IfErrors writableFail 0
  FileClose $0
  Delete "$INSTDIR\cmsdl_write_test"

  ; 2) An existing target file must be replaceable too. Near C:\Program Files
  ;    a folder often allows creating new files while denying changes to
  ;    existing ones, so probing a new file alone misses an earlier cmsdl.exe
  ;    that cannot be overwritten. Opening for append checks the write
  ;    permission without modifying the file.
  IfFileExists "$INSTDIR\cmsdl.exe" 0 writableDone
  ClearErrors
  FileOpen $0 "$INSTDIR\cmsdl.exe" a
  IfErrors writableFail 0
  FileClose $0
  Goto writableDone

  writableFail:
  MessageBox MB_ICONSTOP "$(STR_NO_WRITE_PERMISSION)"
  Abort

  writableDone:
FunctionEnd

; ============================================================================
; Upgrade path check and reinstallation (GUI update mode)
; ============================================================================

; Run `--upgrade-path-check latest` for the current client and decide how to
; proceed. $R0 selects the variant ("cms" or "cms_cw"); the outcome is stored
; in $UpgradeAction:
;   "patch"     - apply patches normally (also the fallback for errorlevel 0)
;   "reinstall" - the caller should reinstall the full client
;   "abort"     - stop the installation
; Errorlevel 100 keeps asking the user to retry until it succeeds or is cancelled.
; Build the "$UpgradeHeading" prefix (client name + newline) from $R0
; ("cms" or "cms_cw") so client-specific dialogs identify the client.
Function SetUpgradeHeading
  StrCmp $R0 "cms_cw" suhCW
  StrCpy $UpgradeHeading "$(STR_CLIENT_CMS)$\r$\n"
  Return
  suhCW:
  StrCpy $UpgradeHeading "$(STR_CLIENT_CMS_CW)$\r$\n"
FunctionEnd

Function CheckUpgradePath
  Call SetUpgradeHeading
  cupRetry:
    DetailPrint "$(STR_CHECKING_UPGRADE)"
    ; nsExec runs the check hidden (no console window) and logs its output to
    ; the details pane; the exit code is popped from the stack.
    nsExec::ExecToLog '"$INSTDIR\cmsdl.exe" $R0 --upgrade-path-check latest "$INSTDIR"'
    Pop $0
    StrCmp $0 "0" cupPatch 0
    StrCmp $0 "1" cupNoPatch 0
    StrCmp $0 "2" cupTooLarge 0
    StrCmp $0 "3" cupNoClient 0
    StrCmp $0 "100" cupNeedRetry 0
    ; Unknown exit code: keep the previous behaviour (just patch).
    Goto cupPatch

  cupPatch:
    StrCpy $UpgradeAction "patch"
    Return

  cupReinstall:
    StrCpy $UpgradeAction "reinstall"
    Return

  cupAbort:
    StrCpy $UpgradeAction "abort"
    Return

  cupNoPatch:
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "$UpgradeHeading$(STR_UPGRADE_NO_PATCH)" IDYES cupReinstall IDNO cupAbort
    Goto cupAbort

  cupNoClient:
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "$UpgradeHeading$(STR_UPGRADE_NO_CLIENT)" IDYES cupReinstall IDNO cupAbort
    Goto cupAbort

  cupTooLarge:
    MessageBox MB_YESNOCANCEL|MB_ICONQUESTION "$UpgradeHeading$(STR_UPGRADE_TOO_LARGE)" IDYES cupReinstall IDNO cupPatch
    Goto cupAbort

  cupNeedRetry:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$UpgradeHeading$(STR_UPGRADE_RETRY)" IDRETRY cupRetry
    Goto cupAbort
FunctionEnd

; Reinstall the variant named by $R0: remove its data directory, download the
; full client, then patch to the latest version. The specified build version
; ($BuildFlag / $BuildFlagCW) is passed to the download when present.
Function ReinstallVariant
  Call SetUpgradeHeading
  StrCmp $R0 "cms_cw" rvCW

  DetailPrint "$(STR_DOWNLOADING)"
  RMDir /r "$INSTDIR\mxd\Data"
  ExecWait '"$INSTDIR\cmsdl.exe" cms --download "$INSTDIR" $BuildFlag$NoGuiFlag$CloseFlag' $0
  StrCmp $0 "0" +3
    MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_FAILED)"
    Abort
  DetailPrint "$(STR_PATCHING)"
  ExecWait '"$INSTDIR\cmsdl.exe" cms --patch latest "$INSTDIR"$NoGuiFlag$CloseFlag' $0
  StrCmp $0 "0" +3
    MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_PATCH_FAILED)"
    Abort
  Return

  rvCW:
  DetailPrint "$(STR_DOWNLOADING_CMS_CW)"
  RMDir /r "$INSTDIR\mxdclassic\Maplestory_Classic_Data"
  ExecWait '"$INSTDIR\cmsdl.exe" cms_cw --download "$INSTDIR" $BuildFlagCW$NoGuiFlag$CloseFlag' $0
  StrCmp $0 "0" +3
    MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_CMS_CW_FAILED)"
    Abort
  DetailPrint "$(STR_PATCHING)"
  ExecWait '"$INSTDIR\cmsdl.exe" cms_cw --patch latest "$INSTDIR"$NoGuiFlag$CloseFlag' $0
  StrCmp $0 "0" +3
    MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_CMS_CW_FAILED)"
    Abort
  Return
FunctionEnd

; ============================================================================
; Installer Section
; ============================================================================

Section "Install"
  ; 360 Total Security is known to corrupt or quarantine game files while
  ; downloading or patching. Ask the user to close it first, and abort the
  ; installation if they cannot. Only install and update modes are affected.
  StrCmp $InstallMode "1" doQihooCheck
  StrCmp $InstallMode "2" doQihooCheck
  Goto qihooOk
  doQihooCheck:
    Call CheckQihoo360
  qihooOk:

  Call CheckInstallDirWritable

  SetOutPath "$INSTDIR"

  ; Fix SDOLogin mode: skip LR extraction, registry, and uninstaller.
  StrCmp $InstallMode "5" modeFixSDOLogin

  ; Extract LocaleRemulator files (only for non-Simplified-Chinese systems).
  ; The flag variable is empty on zh-CN systems, contains " --lrhook" otherwise.
  StrCmp $LrHookFlag "" skipLR
    SetOutPath "$INSTDIR\LocaleRemulator"
    File "LRConfig.xml"
    File "LRHookx32.dll"
    File "LRHookx64.dll"
    File "LRProc.exe"
    File "LRSubMenus.dll"
    SetOutPath "$INSTDIR"
  skipLR:

  ; Branch on operation mode
  StrCmp $InstallMode "2" modeUpdate
  StrCmp $InstallMode "3" modeUpdateCMSDL
  StrCmp $InstallMode "4" modeMSVC
  Goto modeInstall

  ; ----------------------------------------------------------------------
  ; UPDATE MODE
  ; ----------------------------------------------------------------------
  modeUpdate:
    ; Validate: at least one selected variant must have its data directory.
    StrCmp $InstallCMS "1" 0 updCheckCW
      IfFileExists "$INSTDIR\mxd\Data\Base\Base.wz" mxdReady checkBase
      Goto updCheckCW
    checkBase:
      IfFileExists "$INSTDIR\Data\Base\Base.wz" doMigrate updCheckCW
    Goto updCheckCW

    updCheckCW:
      StrCmp $InstallCMSCW "1" 0 updNeither
        IfFileExists "$INSTDIR\mxdclassic\*.*" mxdReady updNeither

    updNeither:
      MessageBox MB_ICONSTOP "$(STR_UPDATE_ABORT)"
      Abort

    doMigrate:
      ; Record all top-level entries (except ., .. and mxd) on the stack.
      StrCpy $R1 0
      FindFirst $R2 $R3 "$INSTDIR\*.*"
      mvCollect:
        StrCmp $R3 "" mvCollectDone
        StrCmp $R3 "." mvNext
        StrCmp $R3 ".." mvNext
        StrCmp $R3 "mxd" mvNext
          Push $R3
          IntOp $R1 $R1 + 1
        mvNext:
        FindNext $R2 $R3
        Goto mvCollect
      mvCollectDone:
      FindClose $R2

      ; Create the mxd subdirectory.
      CreateDirectory "$INSTDIR\mxd"

      ; Move each recorded entry into mxd; copy if it cannot be moved.
      mvMove:
        IntCmp $R1 0 mxdReady
        Pop $R3
        IntOp $R1 $R1 - 1
        ClearErrors
        Rename "$INSTDIR\$R3" "$INSTDIR\mxd\$R3"
        IfErrors 0 mvMove
          ; Move failed (e.g. locked / cross-volume) -> copy instead.
          CopyFiles /SILENT "$INSTDIR\$R3" "$INSTDIR\mxd"
        Goto mvMove

    mxdReady:
      ; Ensure cmsdl.ver exists under mxd with a baseline version.
      ; Not required anymore since v0.2.2
      ; IfFileExists "$INSTDIR\mxd\cmsdl.ver" verReady writeVer
      ; writeVer:
      ;   FileOpen $R0 "$INSTDIR\mxd\cmsdl.ver" w
      ;   FileWrite $R0 "0.0.0.14"
      ;   FileClose $R0
      ; verReady:

      ; Extract cmsdl.exe to the install directory.
      SetOutPath "$INSTDIR"
      File "..\target\release\cmsdl.exe"

      ; Extract LocaleRemulator files (only for non-Simplified-Chinese systems).
      StrCmp $LrHookFlag "" updSkipLR
        SetOutPath "$INSTDIR\LocaleRemulator"
        File "LRConfig.xml"
        File "LRHookx32.dll"
        File "LRHookx64.dll"
        File "LRProc.exe"
        File "LRSubMenus.dll"
        SetOutPath "$INSTDIR"
      updSkipLR:

      ; Registry + uninstaller.
      Call WriteRegInfo

      ; Warn if the connection is metered before starting the patch.
      ExecWait '"$INSTDIR\cmsdl.exe" is_metered' $0
      StrCmp $0 "1" 0 +3
        MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_METERED_WARNING)" IDYES +2
        Abort

      ; Warn if the target drive is a mechanical hard disk (HDD).
      ExecWait '"$INSTDIR\cmsdl.exe" is_hdd "$INSTDIR"' $0
      StrCmp $0 "1" 0 +3
        MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_IS_HDD_WARNING)" IDYES +2
        Abort

      ; Run the update for each selected variant. In GUI mode the upgrade path
      ; is checked first so an un-patchable or oversized update can offer a
      ; full reinstallation instead.
      StrCmp $InstallCMS "1" 0 patchCMS_CW
      StrCpy $R0 "cms"
      Call SetUpgradeHeading
      StrCmp $NoGuiFlag "" 0 upCMSPatch
      Call CheckUpgradePath
      StrCmp $UpgradeAction "abort" 0 upCMSNotAbort
        Abort
      upCMSNotAbort:
      StrCmp $UpgradeAction "reinstall" 0 upCMSPatch
        Call ReinstallVariant
        Goto patchCMS_CW
    upCMSPatch:
      DetailPrint "$(STR_PATCHING)"
      ExecWait '"$INSTDIR\cmsdl.exe" cms --patch latest "$INSTDIR" --purge-wz-files$NoGuiFlag$CloseFlag' $0
      StrCmp $0 "0" patchCMS_CW
        MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_PATCH_FAILED)"
        Abort

    patchCMS_CW:
      StrCmp $InstallCMSCW "1" 0 makeShortcuts
      StrCpy $R0 "cms_cw"
      Call SetUpgradeHeading
      StrCmp $NoGuiFlag "" 0 upCWPatch
      Call CheckUpgradePath
      StrCmp $UpgradeAction "abort" 0 upCWNotAbort
        Abort
      upCWNotAbort:
      StrCmp $UpgradeAction "reinstall" 0 upCWPatch
        Call ReinstallVariant
        Goto makeShortcuts
    upCWPatch:
      DetailPrint "$(STR_PATCHING)"
      ExecWait '"$INSTDIR\cmsdl.exe" cms_cw --patch latest "$INSTDIR"$NoGuiFlag$CloseFlag' $0
      StrCmp $0 "0" makeShortcuts
        MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_CMS_CW_FAILED)"
        Abort

  ; ----------------------------------------------------------------------
  ; INSTALL MODE
  ; ----------------------------------------------------------------------
  modeInstall:
    ; Extract cmsdl.exe
    File "..\target\release\cmsdl.exe"

    ; Registry + uninstaller - only when cms is selected.
    StrCmp $InstallCMS "1" 0 skipRegInfoInstall
      Call WriteRegInfo
    skipRegInfoInstall:

    ; Warn if the connection is metered before starting the download.
    ExecWait '"$INSTDIR\cmsdl.exe" is_metered' $0
    StrCmp $0 "1" 0 +3
      MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_METERED_WARNING)" IDYES +2
      Abort

    ; Warn if the target drive is a mechanical hard disk (HDD).
    ExecWait '"$INSTDIR\cmsdl.exe" is_hdd "$INSTDIR"' $0
    StrCmp $0 "1" 0 +3
      MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_IS_HDD_WARNING)" IDYES +2
      Abort

    ; Download CMS if selected.
    StrCmp $InstallCMS "1" 0 skipCMSDownload
      StrCpy $R0 "cms"
      Call SetUpgradeHeading
      DetailPrint "$(STR_DOWNLOADING)"
      ExecWait '"$INSTDIR\cmsdl.exe" cms --download "$INSTDIR" --purge-wz-files $BuildFlag$NoGuiFlag$CloseFlag' $0
      StrCmp $0 "0" skipCMSDownload
        MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_FAILED)"
        Abort
    skipCMSDownload:

    ; Download CMS_CW if selected.
    StrCmp $InstallCMSCW "1" 0 skipCMSCWDownload
      StrCpy $R0 "cms_cw"
      Call SetUpgradeHeading
      DetailPrint "$(STR_DOWNLOADING_CMS_CW)"
      ExecWait '"$INSTDIR\cmsdl.exe" cms_cw --download "$INSTDIR" $BuildFlagCW$NoGuiFlag$CloseFlag' $0
      StrCmp $0 "0" skipCMSCWDownload
        MessageBox MB_ICONSTOP "$UpgradeHeading$(STR_DOWNLOAD_CMS_CW_FAILED)"
        Abort
    skipCMSCWDownload:

    ; Skip shortcuts only if neither variant was installed.
    StrCmp $InstallCMS "1" makeShortcuts
    StrCmp $InstallCMSCW "1" makeShortcuts
    Goto sectionDone

  ; ----------------------------------------------------------------------
  ; UPDATE CMSDL MODE
  ; ----------------------------------------------------------------------
  modeUpdateCMSDL:
    ; Only replace cmsdl.exe. Do not update registry or write an uninstaller.
    File "..\target\release\cmsdl.exe"
    Goto sectionDone

  ; ----------------------------------------------------------------------
  ; MSVC MODE
  ; ----------------------------------------------------------------------
  modeMSVC:
    SetOutPath "$TEMP"
    File "..\nsis\get_msvc.ps1"
    ${DisableX64FSRedirection}
    ExecWait 'powershell.exe -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList @(\"-NoProfile\",\"-ExecutionPolicy\",\"Bypass\",\"-File\",\"$TEMP\get_msvc.ps1\")"' $0
    ${EnableX64FSRedirection}
    StrCmp $0 "0" sectionDone
      MessageBox MB_ICONSTOP "$(STR_MSVC_FAILED)"
      Abort

  ; ----------------------------------------------------------------------
  ; FIX SDOLOGIN MODE
  ; ----------------------------------------------------------------------
  modeFixSDOLogin:
    ; Verify the selected directory contains a game installation.
    IfFileExists "$INSTDIR\mxd\MapleStory.exe" sdoCheckOk
      MessageBox MB_ICONSTOP "$(STR_FIX_SDOLOGIN_NO_GAME)"
      Abort

    sdoCheckOk:
    ; Confirm the fix (clears existing account records).
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_FIX_SDOLOGIN_WARNING)" IDYES sdoProceed
      Abort

    sdoProceed:
    ; Extract cmsdl.exe to the game directory (skip registry and uninstaller).
    SetOutPath "$INSTDIR"
    File "..\target\release\cmsdl.exe"

    ; Remove the SDO directory entirely.
    RMDir /r "$INSTDIR\mxd\SDO"

    ; Run the filtered download to restore the SDO files.
    DetailPrint "Fixing SDOLogin error..."
    ExecWait '"$INSTDIR\cmsdl.exe" cms --download "$INSTDIR" --filter="SDO"$NoGuiFlag$CloseFlag' $0
    StrCmp $0 "0" sdoDone
      MessageBox MB_ICONSTOP "$(STR_FIX_SDOLOGIN_FAILED)"
      Abort

    sdoDone:
    ; Add firewall rules for game executables (requires elevation).
    ; If the user declines UAC, offer to retry.
    SetOutPath "$TEMP"
    File "..\nsis\add_firewall_rules.ps1"
    ${DisableX64FSRedirection}
    fwRetry:
    ExecWait 'powershell.exe -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList @(\"-NoProfile\",\"-ExecutionPolicy\",\"Bypass\",\"-File\",\"$TEMP\add_firewall_rules.ps1\",\"-InstallDir\",\"$INSTDIR\")"' $0
    ${EnableX64FSRedirection}
    StrCmp $0 "0" fwDone
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(STR_FIX_SDOLOGIN_UAC_RETRY)" IDRETRY fwRetry
    ; User cancelled - not a fatal error; proceed to finish.
    fwDone:
    Goto sectionDone

  ; ----------------------------------------------------------------------
  ; SHARED: shortcuts
  ; ----------------------------------------------------------------------
  makeShortcuts:
    ; "Do not create shortcut" skips both the CMS and CMS_CW shortcuts. Skip
    ; ahead to the optional official-launcher removal.
    StrCmp $NoShortcutFlag "1" checkOfficialLauncher
    ; Create CMS shortcut only when the regular CMS variant was installed.
    StrCmp $InstallCMS "1" 0 checkCMSCWShortcut
    nsExec::ExecToLog '"$INSTDIR\cmsdl.exe" cms --create-shortcut "$INSTDIR"$LrHookFlag$NoGuiFlag'
    Pop $0
    StrCmp $0 "0" checkCMSCWShortcut
      MessageBox MB_ICONSTOP "$(STR_SHORTCUT_FAILED)"
      Abort

  checkCMSCWShortcut:
    ; Create shortcuts for the CMS CW (classic) variant as well.
    StrCmp $InstallCMSCW "1" 0 checkOfficialLauncher
    nsExec::ExecToLog '"$INSTDIR\cmsdl.exe" cms_cw --create-shortcut "$INSTDIR"$NoGuiFlag'
    Pop $0
    StrCmp $0 "0" checkOfficialLauncher
      MessageBox MB_ICONSTOP "$(STR_SHORTCUT_FAILED)"
      Abort

  ; ----------------------------------------------------------------------
  ; OPTIONAL: Remove official launcher (MxdLauncher.exe)
  ; ----------------------------------------------------------------------
  checkOfficialLauncher:
    IfFileExists "$INSTDIR\MxdLauncher.exe" 0 sectionDone
    MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_REMOVE_OFFICIAL_LAUNCHER)" IDYES checkLauncherRunning
    Goto sectionDone

  checkLauncherRunning:
    ; Use tasklist + findstr to detect whether MxdLauncher.exe is running.
    ; findstr exits 0 if found (running), non-zero if not found (not running).
    nsExec::ExecToStack 'cmd /C tasklist /FI $\"IMAGENAME eq MxdLauncher.exe$\" /FO CSV /NH | findstr /I MxdLauncher.exe'
    Pop $R0   ; exit code: 0 = running, 1 = not running
    Pop $R1   ; stdout (discard)
    StrCmp $R0 "0" launcherStillRunning doRemoveLauncher

  launcherStillRunning:
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(STR_REMOVE_OFFICIAL_LAUNCHER_UAC)" IDRETRY checkLauncherRunning
    ; Cancel was clicked - skip removal and proceed to Finish page
    Goto sectionDone

  doRemoveLauncher:
    RMDir /r "$INSTDIR\LauncherSkin3.0"
    RMDir /r "$INSTDIR\Launcher3Update"
    RMDir /r "$INSTDIR\Launcher3SkinPre"
    RMDir /r "$INSTDIR\Launcher3Modules"
    RMDir /r "$INSTDIR\Launcher3Configs"
    RMDir /r "$INSTDIR\3rdParty"
    Delete "$INSTDIR\Uninst.exe"
    Delete "$INSTDIR\RepairClientV3.exe"
    Delete "$INSTDIR\MxdLauncher.exe"
    Delete "$INSTDIR\MovePath.bat"
    Delete "$INSTDIR\LocalVersion3.xml"
    Delete "$INSTDIR\installedFileList.txt"
    Delete "$COMMONDESKTOP\冒险岛.lnk"

  sectionDone:

SectionEnd

; ============================================================================
; Finish Page Customization
; ============================================================================

Function FinishPageShow
  ; Only customize the finish page when both cms and cms_cw are being installed.
  StrCmp $InstallMode "1" 0 done
  StrCmp $InstallCMS "1" 0 done
  StrCmp $InstallCMSCW "1" 0 done

  ${NSD_CreateRadioButton} 10u 60u 95% 12u "$(STR_FINISH_NO_LAUNCH)"
  Pop $RadioNoLaunch
  ${NSD_AddStyle} $RadioNoLaunch ${WS_GROUP}

  ${NSD_CreateRadioButton} 10u 76u 95% 12u "$(STR_FINISH_LAUNCH_CMS)"
  Pop $RadioLaunchCMS

  ${NSD_CreateRadioButton} 10u 92u 95% 12u "$(STR_FINISH_LAUNCH_CMS_CW)"
  Pop $RadioLaunchCMSCW

  ; Default: launch CMS.
  ${NSD_Check} $RadioLaunchCMS
  StrCpy $LaunchVariant "1"
done:
FunctionEnd

Function FinishPageLeave
  StrCmp $InstallMode "1" 0 done
  StrCmp $InstallCMS "1" 0 done
  StrCmp $InstallCMSCW "1" 0 done

  ${NSD_GetState} $RadioNoLaunch $0
  ${If} $0 == 1
    StrCpy $LaunchVariant "0"
    Goto done
  ${EndIf}

  ${NSD_GetState} $RadioLaunchCMS $0
  ${If} $0 == 1
    StrCpy $LaunchVariant "1"
    Goto done
  ${EndIf}

  ${NSD_GetState} $RadioLaunchCMSCW $0
  ${If} $0 == 1
    StrCpy $LaunchVariant "2"
  ${EndIf}
done:
FunctionEnd

; ============================================================================
; Launch Game Prompt
; ============================================================================

Function .onInstSuccess
  StrCmp $InstallMode "3" done
  StrCmp $InstallMode "4" done

  ; Dual-variant install: use the radio button choice from the finish page.
  StrCmp $InstallMode "1" 0 singleVariant
  StrCmp $InstallCMS "1" 0 singleVariant
  StrCmp $InstallCMSCW "1" 0 singleVariant

  ; Both variants were installed - honour the radio selection.
  StrCmp $LaunchVariant "0" done
  StrCmp $LaunchVariant "2" launchCMS_CW
  ; Launch CMS (default).
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching$LrHookFlag$NoGuiFlag"
  Goto done

launchCMS_CW:
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms_cw --patch latest $\"$INSTDIR$\" --launch-after-patching$LrHookFlag$NoGuiFlag"
  Goto done

singleVariant:
  ; Update and Fix SDOLogin modes - always launch CMS.
  StrCmp $InstallMode "2" launchOriginalCms
  StrCmp $InstallMode "5" launchOriginalCms
  ; Install mode with a single variant.
  StrCmp $InstallCMS "1" launchOriginalCms
  StrCmp $InstallCMSCW "1" launchOnlyCMSCW
  Goto done

launchOriginalCms:
  MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_LAUNCH_PROMPT)" /SD IDYES IDNO done
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching$LrHookFlag$NoGuiFlag"
  Goto done

launchOnlyCMSCW:
  MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_LAUNCH_PROMPT_CMS_CW)" /SD IDYES IDNO done
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms_cw --patch latest $\"$INSTDIR$\" --launch-after-patching$LrHookFlag$NoGuiFlag"

done:
FunctionEnd

; ============================================================================
; Uninstaller Section
; ============================================================================

Section "Uninstall"
  ; Read the localized product name stored at install time so we can find
  ; the shortcuts that were actually created.
  ReadRegStr $0 HKCU "Software\${REG_KEY}" "ProductName"
  StrCmp $0 "" 0 +2
    StrCpy $0 "${PRODUCT_NAME}"

  ; Remove shortcuts
  Delete "$DESKTOP\$0.lnk"
  RMDir /r "$SMPROGRAMS\$0"

  ; Remove entire installation directory
  RMDir /r "$INSTDIR"

  ; Remove registry entries
  DeleteRegKey HKCU "Software\${REG_KEY}"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}"

SectionEnd
