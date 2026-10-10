; Linkco PDF Editor — Windows NSIS Installer (LinkcoPDFEditorSetup.exe)
; Publisher: Al Rawabet Commercial Services & Contracting Company W.L.L.

Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

!ifndef VERSION
  !define VERSION "0.5.0"
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
!define MUI_FINISHPAGE_RUN "$INSTDIR\LinkcoPDFEditor.exe"
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
  File /nonfatal "${BIN_DIR}\LinkcoPdfPreviewHandler.dll"
  CopyFiles /SILENT "$INSTDIR\pdfcraft.exe" "$INSTDIR\LinkcoPDFEditor.exe"

  WriteRegStr HKLM "Software\Linkco\Linkco PDF Editor" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\LinkcoPDFEditor.exe" "" "$INSTDIR\LinkcoPDFEditor.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe" "" "$INSTDIR\pdfcraft.exe"

  ; Start Menu shortcut
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\LinkcoPDFEditor.exe" "" "$INSTDIR\LinkcoPDFEditor.exe" 0

  ; PDF Open With registration (never overwrites user's default handler)
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document" "" "PDF Document"
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document\DefaultIcon" "" "$INSTDIR\LinkcoPDFEditor.exe,0"
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document\shell\open\command" "" '"$INSTDIR\LinkcoPDFEditor.exe" "%1"'
  WriteRegStr HKLM "Software\Classes\.pdf\OpenWithProgids" "LinkcoPDFEditor.Document" ""
  WriteRegStr HKLM "Software\Classes\Applications\LinkcoPDFEditor.exe" "FriendlyAppName" "${APP_NAME}"
  WriteRegStr HKLM "Software\Classes\Applications\pdfcraft.exe" "FriendlyAppName" "${APP_NAME}"

  ; Windows File Explorer PDF Preview Handler (IPreviewHandler {8895b1c6-b41f-4c1c-a562-0d564250836f})
  ReadRegStr $0 HKLM "Software\Classes\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}" ""
  StrCmp $0 "" +3
  StrCmp $0 "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" +2
  WriteRegStr HKLM "Software\Linkco\Linkco PDF Editor" "PreviousPdfPreviewHandler" $0
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" "" "Linkco PDF Preview Handler"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" "DisplayName" "Linkco PDF Preview Handler"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" "AppID" "{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}"
  WriteRegDWORD HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" "DisableLowILProcessIsolation" 1
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "" "mscoree.dll"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "ThreadingModel" "STA"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "Class" "LinkcoPdfPreview.LinkcoPdfPreviewHandler"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "Assembly" "LinkcoPdfPreviewHandler, Version=0.5.0.0, Culture=neutral, PublicKeyToken=null"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "RuntimeVersion" "v4.0.30319"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\InprocServer32" "CodeBase" "file:///$INSTDIR\LinkcoPdfPreviewHandler.dll"
  WriteRegStr HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}\ProgId" "" "LinkcoPDFEditor.PreviewHandler"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\PreviewHandlers" "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}" "Linkco PDF Preview Handler"
  WriteRegStr HKLM "Software\Classes\LinkcoPDFEditor.Document\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}" "" "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}"
  WriteRegStr HKLM "Software\Classes\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}" "" "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}"
  WriteRegStr HKLM "Software\Classes\SystemFileAssociations\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}" "" "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}"

  ; Uninstaller & Windows Installed Apps entry
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\LinkcoPDFEditor.exe,0"
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
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\LinkcoPDFEditor.exe" "" "$INSTDIR\LinkcoPDFEditor.exe" 0
SectionEnd

Section "Uninstall"
  SetRegView 64
  SetShellVarContext all

  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"

  Delete "$INSTDIR\LinkcoPDFEditor.exe"
  Delete "$INSTDIR\LinkcoPDFEditor.exe.lnk"
  Delete "$INSTDIR\pdfcraft.exe"
  Delete "$INSTDIR\pdfcraft-cli.exe"
  Delete "$INSTDIR\LinkcoPdfPreviewHandler.dll"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  RMDir "$PROGRAMFILES64\Linkco"

  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\PreviewHandlers" "{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}"
  DeleteRegKey HKLM "Software\Classes\CLSID\{D4E7B6A2-4C91-4E3A-9B12-7A8F5C3E1D20}"
  ReadRegStr $0 HKLM "Software\Linkco\Linkco PDF Editor" "PreviousPdfPreviewHandler"
  StrCmp $0 "" 0 +3
  DeleteRegKey HKLM "Software\Classes\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}"
  Goto +2
  WriteRegStr HKLM "Software\Classes\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}" "" $0
  DeleteRegKey HKLM "Software\Classes\SystemFileAssociations\.pdf\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}"
  DeleteRegValue HKLM "Software\Classes\.pdf\OpenWithProgids" "LinkcoPDFEditor.Document"
  DeleteRegKey HKLM "Software\Classes\LinkcoPDFEditor.Document"
  DeleteRegKey HKLM "Software\Classes\Applications\LinkcoPDFEditor.exe"
  DeleteRegKey HKLM "Software\Classes\Applications\pdfcraft.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\LinkcoPDFEditor.exe"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\App Paths\pdfcraft.exe"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
  DeleteRegKey HKLM "Software\Linkco\Linkco PDF Editor"
SectionEnd
