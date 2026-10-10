; Linkco PDF Editor — Windows NSIS Installer (LinkcoPDFEditorSetup.exe)
; Publisher: Al Rawabet Commercial Services & Contracting Company W.L.L.

Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

!ifndef VERSION
  !define VERSION "0.3.0"
!endif
!ifndef BIN_DIR
  !define BIN_DIR "..\..\target\x86_64-pc-windows-msvc\release"
!endif
!ifndef ICON_PATH
  !define ICON_PATH "..\..\assets\app-icon\pdfcraft.ico"
!endif
!ifndef OUT_FILE
  !define OUT_FILE "..\..\dist\release\LinkcoPDFEditorSetup.exe"
!endif

!define APP_NAME "Linkco PDF Editor"
!define COMPANY_NAME "Al Rawabet Commercial Services & Contracting Company W.L.L."
!define COPYRIGHT "© Al Rawabet Commercial Services & Contracting Company W.L.L."
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Linkco PDF Editor"

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\Linkco\Linkco PDF Editor"
InstallDirRegKey HKLM "Software\Linkco\Linkco PDF Editor" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP_NAME}"
VIAddVersionKey "CompanyName" "${COMPANY_NAME}"
VIAddVersionKey "FileDescription" "${APP_NAME} Setup"
VIAddVersionKey "InternalName" "LinkcoPDFEditorSetup"
VIAddVersionKey "OriginalFilename" "LinkcoPDFEditorSetup.exe"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "${COPYRIGHT}"

!define MUI_ICON "${ICON_PATH}"
!define MUI_UNICON "${ICON_PATH}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\pdfcraft.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "${APP_NAME} (required)" SecCore
  SectionIn RO
  SetRegView 64
  SetShellVarContext all

  SetOutPath "$INSTDIR"
  File "${BIN_DIR}\pdfcraft.exe"
  File "${BIN_DIR}\pdfcraft-cli.exe"
  CreateShortcut "$INSTDIR\LinkcoPDFEditor.exe.lnk" "$INSTDIR\pdfcraft.exe" "" "$INSTDIR\pdfcraft.exe" 0

  WriteRegStr HKLM "Software\Linkco\Linkco PDF Editor" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe" "" "$INSTDIR\pdfcraft.exe"

  ; Start Menu shortcut
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\pdfcraft.exe" "" "$INSTDIR\pdfcraft.exe" 0

  ; PDF Open With registration (never overwrites user's default handler)
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document" "" "PDF Document"
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document\DefaultIcon" "" "$INSTDIR\pdfcraft.exe,0"
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document\shell\open\command" "" '"$INSTDIR\pdfcraft.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\.pdf\OpenWithProgids" "LinkcoPDFEditor.Document" ""
  WriteRegStr HKLM "Software\Classes\Applications\pdfcraft.exe" "FriendlyAppName" "${APP_NAME}"

  ; Uninstaller & Windows Installed Apps entry
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\pdfcraft.exe,0"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "URLInfoAbout" "https://www.linkco.com.qa"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "HelpLink" "https://www.linkco.com.qa/contact-us/"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Contact" "info@linkco.com.qa (+974 4437 2511)"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKLM "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Desktop Shortcut" SecDesktop
  SetShellVarContext all
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\pdfcraft.exe" "" "$INSTDIR\pdfcraft.exe" 0
SectionEnd

Section "Uninstall"
  SetRegView 64
  SetShellVarContext all

  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\LinkcoPDFEditor.exe.lnk"
  Delete "$INSTDIR\pdfcraft.exe"
  Delete "$INSTDIR\pdfcraft-cli.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  RMDir "$PROGRAMFILES64\Linkco"

  DeleteRegValue HKLM "Software\Classes\.pdf\OpenWithProgids" "LinkcoPDFEditor.Document"
  DeleteRegKey HKLM "Software\Classes\LinkcoPDFEditor.Document"
  DeleteRegKey HKLM "Software\Classes\Applications\pdfcraft.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
  DeleteRegKey HKLM "Software\Linkco\Linkco PDF Editor"
SectionEnd
