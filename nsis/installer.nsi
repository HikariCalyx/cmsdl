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

; Version
!define VERSION "4.226.4.3"

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

; ============================================================================
; MUI2 Settings
; ============================================================================

; Installer pages
!insertmacro MUI_PAGE_WELCOME
Page custom ModeSelectPage ModeSelectPageLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE DirectoryPagePre
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
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
LangString STR_DO_NOT_INCLUDE_LR ${LANG_ENGLISH} "Do not include Locale Remulator"
LangString STR_USE_CONSOLE_TYPE ${LANG_ENGLISH} "Use the console-type CMSDL interface"
LangString STR_REMOVE_OFFICIAL_LAUNCHER ${LANG_ENGLISH} "Would you like to remove the official game launcher? Removing it does not affect game launching."
LangString STR_REMOVE_OFFICIAL_LAUNCHER_UAC ${LANG_ENGLISH} "You're currently running official launcher, but you didn't close it. Once you finish closing, please click Retry."
LangString STR_METERED_WARNING ${LANG_ENGLISH} "Your network connection is metered.$\nDownloading the game may incur additional costs.$\n$\nDo you want to continue?"

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
LangString STR_DO_NOT_INCLUDE_LR ${LANG_SIMPCHINESE} "你不应该看到这个选项"
LangString STR_USE_CONSOLE_TYPE ${LANG_SIMPCHINESE} "使用命令行样式的CMSDL界面"
LangString STR_REMOVE_OFFICIAL_LAUNCHER ${LANG_SIMPCHINESE} "您想要移除官方游戏启动器吗？移除该启动器不会影响启动游戏。"
LangString STR_REMOVE_OFFICIAL_LAUNCHER_UAC ${LANG_SIMPCHINESE} "您当前正在运行官方启动器，但尚未关闭它。关闭后，请点击重试。"
LangString STR_METERED_WARNING ${LANG_SIMPCHINESE} "您的网络连接为按流量计费的连接。$\n下载游戏可能会产生额外费用。$\n$\n您是否要继续？"

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

  ; If the current date is on or before July 28, 2026, add --build 1036 to
  ; the download command (required for a specific game build rollout).
  ${GetTime} "" "L" $0 $1 $2 $3 $4 $5 $6
  ; $2 = year (4 digits), $1 = month, $0 = day of month
  StrCpy $BuildFlag ""
  IntCmp $2 2026 yearEq beforeCutoff afterCutoff
  yearEq:
    IntCmp $1 7 monthEq beforeCutoff afterCutoff
  monthEq:
    IntCmp $0 28 beforeCutoff beforeCutoff afterCutoff
  beforeCutoff:
    StrCpy $BuildFlag "--build 1036"
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

Function OpenTroubleshootingLink
  ExecShell "open" "https://wiki.biligame.com/maplestory/CMSDL故障排除"
FunctionEnd

; Skip the directory page for MSVC mode — no install path is needed.
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

  ; Add uninstall information to Control Panel
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayName" "$(STR_PRODUCT_NAME)"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "Publisher" "${PRODUCT_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${REG_KEY}" "DisplayIcon" "$INSTDIR\cmsdl.exe"

  ; Write uninstaller
  WriteUninstaller "$INSTDIR\Uninstall.exe"
FunctionEnd

; ============================================================================
; Installer Section
; ============================================================================

Section "Install"
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
    ; If an mxd directory already exists, migration was already done before.
    IfFileExists "$INSTDIR\mxd\Data\Base\Base.wz" mxdReady checkBase

    checkBase:
      ; mxd does not exist; require an existing Base.wz directory.
      IfFileExists "$INSTDIR\Data\Base\Base.wz" doMigrate abortUpdate

    abortUpdate:
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

      ; Run the patch. ExecWait gives cmsdl.exe a real console for its
      ; indicatif progress bars.
      DetailPrint "$(STR_PATCHING)"
      ; Always run the installer's own patch step in the console: it relies on
      ; the exit code and shows its own progress. In GUI mode a successful patch
      ; leaves the window open, which would block ExecWait until closed.
      ExecWait '"$INSTDIR\cmsdl.exe" cms --patch latest "$INSTDIR" --purge-wz-files$NoGuiFlag$CloseFlag' $0
      StrCmp $0 "0" makeShortcuts
        MessageBox MB_ICONSTOP "$(STR_PATCH_FAILED)"
        Abort

  ; ----------------------------------------------------------------------
  ; INSTALL MODE
  ; ----------------------------------------------------------------------
  modeInstall:
    ; Extract cmsdl.exe
    File "..\target\release\cmsdl.exe"

    ; Registry + uninstaller.
    Call WriteRegInfo

    ; Warn if the connection is metered before starting the download.
    ExecWait '"$INSTDIR\cmsdl.exe" is_metered' $0
    StrCmp $0 "1" 0 +3
      MessageBox MB_YESNO|MB_ICONEXCLAMATION "$(STR_METERED_WARNING)" IDYES +2
      Abort

    ; Execute download command. ExecWait gives cmsdl.exe a real console
    ; window where its indicatif progress bars can render.
    DetailPrint "$(STR_DOWNLOADING)"
    ExecWait '"$INSTDIR\cmsdl.exe" cms --download "$INSTDIR" --purge-wz-files $BuildFlag$NoGuiFlag$CloseFlag' $0
    StrCmp $0 "0" makeShortcuts
      MessageBox MB_ICONSTOP "$(STR_DOWNLOAD_FAILED)"
      Abort

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
    ExecWait 'powershell.exe -ExecutionPolicy Bypass -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList @(\"-NoProfile\",\"-ExecutionPolicy\",\"Bypass\",\"-File\",\"$TEMP\add_firewall_rules.ps1\",\"-InstallDir\",\"$INSTDIR\")'" $0
    ${EnableX64FSRedirection}
    StrCmp $0 "0" fwDone
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(STR_FIX_SDOLOGIN_UAC_RETRY)" IDRETRY fwRetry
    ; User cancelled — not a fatal error; proceed to finish.
    fwDone:
    Goto sectionDone

  ; ----------------------------------------------------------------------
  ; SHARED: shortcuts
  ; ----------------------------------------------------------------------
  makeShortcuts:
    nsExec::ExecToLog '"$INSTDIR\cmsdl.exe" cms --create-shortcut "$INSTDIR"$LrHookFlag$NoGuiFlag'
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
    ; Cancel was clicked — skip removal and proceed to Finish page
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
; Launch Game Prompt
; ============================================================================

Function .onInstSuccess
  StrCmp $InstallMode "3" done
  StrCmp $InstallMode "4" done
  MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_LAUNCH_PROMPT)" /SD IDYES IDNO done
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching$LrHookFlag$NoGuiFlag"
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
