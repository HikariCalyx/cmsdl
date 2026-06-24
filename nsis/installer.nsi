; MapleStory CN CMSDL Installer
; NSIS Installer Script

; ============================================================================
; Configuration
; ============================================================================

!include "MUI2.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "LogicLib.nsh"
!include "nsDialogs.nsh"

; Version
!define VERSION "4.226.1.0"

; Product Info (English)
!define PRODUCT_NAME "MapleStory CN"

; Product Info (Simplified Chinese) - Change this to your desired name
!define PRODUCT_NAME_ZH "冒险岛"

; Registry key (no spaces)
!define REG_KEY "MapleStoryCN"

!define PRODUCT_PUBLISHER "Hikari Calyx Tech"
!define PRODUCT_WEB_SITE "https://github.com/HikariCalyx/cmsdl"

; Installation Directory
!define INSTALL_DIR "$%SystemDrive%\MapleStoryCN"

; Required Space: 70,780,930,990 bytes (approximately 65.92 GB)
!define REQUIRED_SPACE "70780930990"

; Installer Icon - must be defined before MUI2 settings
!define MUI_ICON "icon.ico"
!define MUI_UNICON "icon.ico"

; ============================================================================
; Variables
; ============================================================================

; Operation mode: "1" = Install (full download), "2" = Update (patch)
Var InstallMode
Var Dialog
Var RadioInstall
Var RadioUpdate
Var ShortcutIcon

; ============================================================================
; MUI2 Settings
; ============================================================================

; Installer pages
!insertmacro MUI_PAGE_WELCOME
Page custom ModeSelectPage ModeSelectPageLeave
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
LangString STR_DOWNLOAD_FAILED ${LANG_ENGLISH} "Game file download failed with error code $0. Installation will continue."
LangString STR_PATCH_FAILED ${LANG_ENGLISH} "Game update failed with error code $0."
LangString STR_LAUNCH_PROMPT ${LANG_ENGLISH} "Installation completed. Would you like to launch the game now?"
LangString STR_UNSUPPORTED_OS ${LANG_ENGLISH} "This application requires Windows 10 or later on x64 architecture.$\nYour system does not meet the requirements."
LangString STR_UNSUPPORTED_ARCH ${LANG_ENGLISH} "This application requires x64 architecture.$\nYour system is not x64 compatible."
LangString STR_PRODUCT_NAME ${LANG_ENGLISH} "${PRODUCT_NAME}"
LangString STR_MODE_TITLE ${LANG_ENGLISH} "Choose Operation"
LangString STR_MODE_SUBTITLE ${LANG_ENGLISH} "Select whether to install or update the game."
LangString STR_MODE_INSTALL ${LANG_ENGLISH} "Install (download the full game)"
LangString STR_MODE_UPDATE ${LANG_ENGLISH} "Update (update an existing game installation)"
LangString STR_UPDATE_ABORT ${LANG_ENGLISH} "No existing game installation was found in the selected directory. Update cannot continue."

; ============================================================================
; Language Strings - Simplified Chinese
; ============================================================================

LangString STR_DOWNLOADING ${LANG_SIMPCHINESE} "正在下载游戏文件..."
LangString STR_PATCHING ${LANG_SIMPCHINESE} "正在更新游戏文件..."
LangString STR_DOWNLOAD_FAILED ${LANG_SIMPCHINESE} "游戏文件下载失败，错误代码：$0。安装将继续。"
LangString STR_PATCH_FAILED ${LANG_SIMPCHINESE} "游戏更新失败，错误代码：$0。"
LangString STR_LAUNCH_PROMPT ${LANG_SIMPCHINESE} "安装完成。您要立即启动游戏吗？"
LangString STR_UNSUPPORTED_OS ${LANG_SIMPCHINESE} "此应用程序需要 Windows 10 或更高版本（x64 架构）。$\n您的系统不符合要求。"
LangString STR_UNSUPPORTED_ARCH ${LANG_SIMPCHINESE} "此应用程序需要 x64 架构。$\n您的系统不兼容 x64。"
LangString STR_PRODUCT_NAME ${LANG_SIMPCHINESE} "${PRODUCT_NAME_ZH}"
LangString STR_MODE_TITLE ${LANG_SIMPCHINESE} "选择操作"
LangString STR_MODE_SUBTITLE ${LANG_SIMPCHINESE} "请选择是安装还是更新游戏。"
LangString STR_MODE_INSTALL ${LANG_SIMPCHINESE} "安装（下载完整游戏）"
LangString STR_MODE_UPDATE ${LANG_SIMPCHINESE} "更新（更新现有游戏）"
LangString STR_UPDATE_ABORT ${LANG_SIMPCHINESE} "在所选目录中未找到现有的游戏安装。无法继续更新。"

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

; ============================================================================
; Initialize
; ============================================================================

Function .onInit
  ; Default operation mode is Install
  StrCpy $InstallMode "1"

  ; Select language based on OS language (Simplified Chinese = 0804).
  ; Set this first so the requirement-check message boxes are localized.
  StrCpy $LANGUAGE ${LANG_ENGLISH}
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Control\Nls\Language" "Default"
  StrCmp $0 "0804" 0 +2
    StrCpy $LANGUAGE ${LANG_SIMPCHINESE}

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
  ${NSD_CreateRadioButton} 10u 40u 95% 12u "$(STR_MODE_UPDATE)"
  Pop $RadioUpdate

  ; Restore previous selection
  StrCmp $InstallMode "2" selUpdate
    ${NSD_Check} $RadioInstall
    Goto modeShow
  selUpdate:
    ${NSD_Check} $RadioUpdate

  modeShow:
  nsDialogs::Show
  modeDone:
FunctionEnd

Function ModeSelectPageLeave
  ${NSD_GetState} $RadioUpdate $0
  StrCmp $0 "1" setUpdate
    StrCpy $InstallMode "1"
    Goto leaveDone
  setUpdate:
    StrCpy $InstallMode "2"
  leaveDone:
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
  StrCmp $InstallMode "2" modeUpdate modeInstall

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
      IfFileExists "$INSTDIR\mxd\cmsdl.ver" verReady writeVer
      writeVer:
        FileOpen $R0 "$INSTDIR\mxd\cmsdl.ver" w
        FileWrite $R0 "0.0.0.14"
        FileClose $R0
      verReady:

      ; Extract cmsdl.exe to the install directory.
      SetOutPath "$INSTDIR"
      File "..\target\release\cmsdl.exe"

      ; Registry + uninstaller.
      Call WriteRegInfo

      ; Run the patch. ExecWait gives cmsdl.exe a real console for its
      ; indicatif progress bars.
      DetailPrint "$(STR_PATCHING)"
      ExecWait '"$INSTDIR\cmsdl.exe" cms --patch latest "$INSTDIR"' $0
      StrCmp $0 "0" makeShortcuts
        MessageBox MB_ICONEXCLAMATION "$(STR_PATCH_FAILED)"
      Goto makeShortcuts

  ; ----------------------------------------------------------------------
  ; INSTALL MODE
  ; ----------------------------------------------------------------------
  modeInstall:
    ; Extract cmsdl.exe
    File "..\target\release\cmsdl.exe"

    ; Registry + uninstaller.
    Call WriteRegInfo

    ; Execute download command. ExecWait gives cmsdl.exe a real console
    ; window where its indicatif progress bars can render.
    DetailPrint "$(STR_DOWNLOADING)"
    ExecWait '"$INSTDIR\cmsdl.exe" cms --download "$INSTDIR" --skip-create-shortcut' $0
    StrCmp $0 "0" makeShortcuts
      MessageBox MB_ICONEXCLAMATION "$(STR_DOWNLOAD_FAILED)"

  ; ----------------------------------------------------------------------
  ; SHARED: shortcuts
  ; ----------------------------------------------------------------------
  makeShortcuts:
    ; Choose the shortcut icon: prefer the game's MapleStory.exe (first icon)
    ; if it exists under mxd, otherwise fall back to cmsdl.exe.
    StrCpy $ShortcutIcon "$INSTDIR\cmsdl.exe"
    IfFileExists "$INSTDIR\mxd\MapleStory.exe" 0 +2
      StrCpy $ShortcutIcon "$INSTDIR\mxd\MapleStory.exe"

    ; Create Start Menu folder
    CreateDirectory "$SMPROGRAMS\$(STR_PRODUCT_NAME)"

    ; Create Desktop shortcut
    CreateShortcut "$DESKTOP\$(STR_PRODUCT_NAME).lnk" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching" "$ShortcutIcon" 0

    ; Create Start Menu shortcuts
    CreateShortcut "$SMPROGRAMS\$(STR_PRODUCT_NAME)\$(STR_PRODUCT_NAME).lnk" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching" "$ShortcutIcon" 0
    CreateShortcut "$SMPROGRAMS\$(STR_PRODUCT_NAME)\Uninstall.lnk" "$INSTDIR\Uninstall.exe"

SectionEnd

; ============================================================================
; Launch Game Prompt
; ============================================================================

Function .onInstSuccess
  MessageBox MB_YESNO|MB_ICONQUESTION "$(STR_LAUNCH_PROMPT)" /SD IDYES IDNO done
  ExecShell "open" "$INSTDIR\cmsdl.exe" "cms --patch latest $\"$INSTDIR$\" --launch-after-patching"
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
