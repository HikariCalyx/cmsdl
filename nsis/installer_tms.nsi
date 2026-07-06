; MapleStory TW CMSDL Installer
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

; Version
!define VERSION "6.280.1.1"

; Product Info (English)
!define PRODUCT_NAME "MapleStory TW"

; Product Info (Traditional Chinese)
!define PRODUCT_NAME_ZH "新楓之谷"

; Registry key (no spaces)
!define REG_KEY "MapleStoryTW"

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

; Operation mode: "1" = Install (full download), "2" = Update CMSDL, "3" = MSVC
Var InstallMode
Var Dialog
Var RadioInstall
Var RadioUpdateCMSDL
Var RadioMSVC
Var CheckConsole
Var NoGuiFlag
Var CloseFlag

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

; Language - Traditional Chinese
!insertmacro MUI_LANGUAGE "TradChinese"

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
LangString STR_LINK_TROUBLESHOOTING ${LANG_ENGLISH} "Troubleshooting (Traditional Chinese only)"
LangString STR_USE_CONSOLE_TYPE ${LANG_ENGLISH} "Use the console-type CMSDL interface"
LangString STR_UPDATE_ABORT ${LANG_ENGLISH} "No existing game installation was found in the selected directory. Update cannot continue."
LangString STR_METERED_WARNING ${LANG_ENGLISH} "Your network connection is metered.$\nDownloading the game may incur additional costs.$\n$\nDo you want to continue?"

; ============================================================================
; Language Strings - Traditional Chinese
; ============================================================================

LangString STR_DOWNLOADING ${LANG_TRADCHINESE} "正在下載遊戲檔案..."
LangString STR_PATCHING ${LANG_TRADCHINESE} "正在更新遊戲檔案..."
LangString STR_DOWNLOAD_FAILED ${LANG_TRADCHINESE} "遊戲檔案下載失敗，錯誤代碼：$0。"
LangString STR_PATCH_FAILED ${LANG_TRADCHINESE} "遊戲更新失敗，錯誤代碼：$0。"
LangString STR_MSVC_FAILED ${LANG_TRADCHINESE} "MSVC 運行時安裝失敗，錯誤代碼：$0。"
LangString STR_SHORTCUT_FAILED ${LANG_TRADCHINESE} "創建捷徑失敗，錯誤代碼：$0。"
LangString STR_LAUNCH_PROMPT ${LANG_TRADCHINESE} "安裝完成。您要立即啟動遊戲嗎？"
LangString STR_UNSUPPORTED_OS ${LANG_TRADCHINESE} "此程式需要 Windows 10 或更高版本（64位元）。$\n請升級作業系統後再使用。"
LangString STR_UNSUPPORTED_ARCH ${LANG_TRADCHINESE} "此程式需要64位元。$\n您的系統不是64位元。"
LangString STR_PRODUCT_NAME ${LANG_TRADCHINESE} "${PRODUCT_NAME_ZH}"
LangString STR_MODE_TITLE ${LANG_TRADCHINESE} "選擇操作"
LangString STR_MODE_SUBTITLE ${LANG_TRADCHINESE} "請選擇是安裝還是更新遊戲。"
LangString STR_MODE_INSTALL ${LANG_TRADCHINESE} "安裝或修復（下載完整遊戲）"
LangString STR_MODE_UPDATE ${LANG_TRADCHINESE} "更新（更新現有遊戲）"
LangString STR_MODE_UPDATE_CMSDL ${LANG_TRADCHINESE} "升級 CMSDL"
LangString STR_MODE_MSVC ${LANG_TRADCHINESE} "修復運行時（VCRUNTIME140.dll 丟失等錯誤）"
LangString STR_LINK_TROUBLESHOOTING ${LANG_TRADCHINESE} "使用遇到問題了？點擊查看幫助"
LangString STR_USE_CONSOLE_TYPE ${LANG_TRADCHINESE} "使用指令樣式的CMSDL介面"
LangString STR_UPDATE_ABORT ${LANG_TRADCHINESE} "在所選目錄中未找到現有的遊戲安裝。無法繼續更新。"
LangString STR_METERED_WARNING ${LANG_TRADCHINESE} "您的網路連線為按流量計費的連線。$\n下載遊戲可能會產生額外費用。$\n$\n您是否要繼續？"

; ============================================================================
; Installer Attributes
; ============================================================================

; Product name resolves at runtime based on selected language
Name "$(STR_PRODUCT_NAME) ${VERSION}"
OutFile "MapleStoryTW-${VERSION}-installer.exe"
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

  ; Select language based on OS language (Traditional Chinese = 0404).
  ; Set this first so the requirement-check message boxes are localized.
  StrCpy $LANGUAGE ${LANG_ENGLISH}
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Control\Nls\Language" "Default"
  StrCmp $0 "0404" 0 +2
    StrCpy $LANGUAGE ${LANG_TRADCHINESE}

  StrCmp $0 "0804" 0 +2
    StrCpy $LANGUAGE ${LANG_TRADCHINESE}

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
FunctionEnd

; ============================================================================
; Mode Selection Page (Install vs Update)
; ============================================================================

Function ModeSelectPage
  !insertmacro MUI_HEADER_TEXT "$(STR_MODE_TITLE)" "$(STR_MODE_SUBTITLE)"

  nsDialogs::Create 1018
  Pop $Dialog
  StrCmp $Dialog "error" modeDone

  ${NSD_CreateRadioButton} 10u 20u 95% 12u "$(STR_MODE_INSTALL)"
  Pop $RadioInstall
  ${NSD_CreateRadioButton} 10u 40u 95% 12u "$(STR_MODE_UPDATE_CMSDL)"
  Pop $RadioUpdateCMSDL
  ${NSD_CreateRadioButton} 10u 60u 95% 12u "$(STR_MODE_MSVC)"
  Pop $RadioMSVC

  ; Console-mode opt-in checkbox (always available). When checked, the created
  ; shortcut and the post-install launch pass --no-gui so the patcher runs in
  ; the console instead of the graphical window.
  ${NSD_CreateCheckbox} 10u 112u 95% 12u "$(STR_USE_CONSOLE_TYPE)"
  Pop $CheckConsole
  ; Restore previous state if the user went back.
  StrCmp $NoGuiFlag " --no-gui" 0 +2
    ${NSD_Check} $CheckConsole

  ; Restore previous selection
  StrCmp $InstallMode "2" selUpdateCMSDL
  StrCmp $InstallMode "3" selMSVC
    ${NSD_Check} $RadioInstall
    Goto modeShow
  selUpdateCMSDL:
    ${NSD_Check} $RadioUpdateCMSDL
    Goto modeShow
  selMSVC:
    ${NSD_Check} $RadioMSVC

  modeShow:
  nsDialogs::Show
  modeDone:
FunctionEnd

Function ModeSelectPageLeave
  ${NSD_GetState} $RadioUpdateCMSDL $0
  StrCmp $0 "1" setUpdateCMSDL
  ${NSD_GetState} $RadioMSVC $0
  StrCmp $0 "1" setMSVC
    StrCpy $InstallMode "1"
    Goto leaveDone
  setUpdateCMSDL:
    StrCpy $InstallMode "2"
    Goto leaveDone
  setMSVC:
    StrCpy $InstallMode "3"
  leaveDone:
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

  ; Branch on operation mode
  StrCmp $InstallMode "2" modeUpdateCMSDL
  StrCmp $InstallMode "3" modeMSVC
  Goto modeInstall

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
    ExecWait '"$INSTDIR\cmsdl.exe" tms --download "$INSTDIR" --purge-wz-files$NoGuiFlag$CloseFlag' $0
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
  ; SHARED: shortcuts
  ; ----------------------------------------------------------------------
  makeShortcuts:
    ; Choose the shortcut icon: prefer the game's MapleStory.exe (first icon)
    ; if it exists under mxd, otherwise fall back to cmsdl.exe.

    ; Create Start Menu folder
    CreateDirectory "$SMPROGRAMS\$(STR_PRODUCT_NAME)"

    ; Create Desktop shortcut
    CreateShortcut "$DESKTOP\$(STR_PRODUCT_NAME).lnk" "$INSTDIR\MapleStory.exe" 0

    ; Create Start Menu shortcuts
    CreateShortcut "$SMPROGRAMS\$(STR_PRODUCT_NAME)\$(STR_PRODUCT_NAME).lnk" "$INSTDIR\MapleStory.exe" 0
    CreateShortcut "$SMPROGRAMS\$(STR_PRODUCT_NAME)\Uninstall.lnk" "$INSTDIR\Uninstall.exe"

  sectionDone:

SectionEnd

; ============================================================================
; Launch Game Prompt
; ============================================================================

Function .onInstSuccess
  StrCmp $InstallMode "2" done
  StrCmp $InstallMode "3" done
  MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_LAUNCH_PROMPT)" /SD IDYES IDNO done
  ExecShell "open" "$INSTDIR\MapleStory.exe"
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
