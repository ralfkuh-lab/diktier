; Diktier — Windows-Installer (Spec §11, Phase 5 Paket F).
;
; Per-User: kein Admin, Installation nach %LOCALAPPDATA%\Programs\Diktier,
; Deinstallationseintrag unter HKCU. Gebaut von scripts\release.ps1, das
; VERSION, SRCDIR (fertiges Bundle) und OUTFILE als /D-Defines übergibt:
;
;   makensis /DVERSION=0.1.0 /DSRCDIR=…\dist\diktier-0.1.0-win-x64 ^
;            /DOUTFILE=…\dist\Diktier_0.1.0_x64-setup.exe installer\diktier.nsi
;
; Unsigniert — SmartScreen meldet „Unbekannter Herausgeber" (siehe README).

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !error "VERSION fehlt — über /DVERSION=… aufrufen (scripts\release.ps1)"
!endif
!ifndef SRCDIR
  !error "SRCDIR fehlt — über /DSRCDIR=… aufrufen (scripts\release.ps1)"
!endif
!ifndef OUTFILE
  !define OUTFILE "Diktier_${VERSION}_x64-setup.exe"
!endif

!define APPNAME "Diktier"
!define PUBLISHER "Ralf Kuhlendahl"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Diktier"

Name "${APPNAME} ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Diktier"
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
SetCompressor /SOLID lzma
ShowInstDetails show
ShowUnInstDetails show

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}.0"
VIAddVersionKey "FileDescription" "${APPNAME} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "MIT-Lizenz"

; ------------------------------------------------------------------ Modern UI 2

!define MUI_ICON "${__FILEDIR__}\..\assets\diktier.ico"
!define MUI_UNICON "${__FILEDIR__}\..\assets\diktier.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\diktier.exe"
!define MUI_FINISHPAGE_RUN_TEXT "$(RunNow)"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${__FILEDIR__}\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; Erste Sprache = Default, wenn die Systemsprache keine der beiden ist.
!insertmacro MUI_LANGUAGE "German"
!insertmacro MUI_LANGUAGE "English"

LangString SecAppName    ${LANG_GERMAN}  "Diktier (erforderlich)"
LangString SecAppName    ${LANG_ENGLISH} "Diktier (required)"
LangString SecAutoName   ${LANG_GERMAN}  "Mit Windows starten"
LangString SecAutoName   ${LANG_ENGLISH} "Start with Windows"
LangString SecDeskName   ${LANG_GERMAN}  "Desktop-Verknüpfung"
LangString SecDeskName   ${LANG_ENGLISH} "Desktop shortcut"
LangString SecAppDesc    ${LANG_GERMAN}  "Programm, ONNX Runtime, Lizenzen und Startmenü-Eintrag."
LangString SecAppDesc    ${LANG_ENGLISH} "Program, ONNX Runtime, licenses and Start menu entry."
LangString SecAutoDesc   ${LANG_GERMAN}  "Legt einen Eintrag im Autostart-Ordner an (diktier --install-autostart)."
LangString SecAutoDesc   ${LANG_ENGLISH} "Adds an entry to the Startup folder (diktier --install-autostart)."
LangString SecDeskDesc   ${LANG_GERMAN}  "Legt eine Verknüpfung auf dem Desktop an."
LangString SecDeskDesc   ${LANG_ENGLISH} "Creates a shortcut on the desktop."
LangString RunNow        ${LANG_GERMAN}  "Diktier jetzt starten"
LangString RunNow        ${LANG_ENGLISH} "Run Diktier now"
LangString StopRunning   ${LANG_GERMAN}  "Beende ein laufendes Diktier …"
LangString StopRunning   ${LANG_ENGLISH} "Stopping a running Diktier ..."
LangString MsgPurge      ${LANG_GERMAN}  "Heruntergeladenes Sprachmodell und Einstellungen löschen?$\r$\n$\r$\n(~650 MB in %LOCALAPPDATA%\diktier und %APPDATA%\diktier)"
LangString MsgPurge      ${LANG_ENGLISH} "Delete the downloaded speech model and settings?$\r$\n$\r$\n(~650 MB in %LOCALAPPDATA%\diktier and %APPDATA%\diktier)"

; ------------------------------------------------------------------- Sektionen

; Vor dem Kopieren muss der Daemon weg — sonst ist diktier.exe gesperrt.
; Fehler werden ignoriert: „läuft nicht" ist der Normalfall.
!macro StopDaemon
  DetailPrint "$(StopRunning)"
  nsExec::ExecToLog 'taskkill /IM diktier.exe /F'
  Pop $0
  Sleep 800
!macroend

Section "$(SecAppName)" SEC_APP
  SectionIn RO
  !insertmacro StopDaemon

  SetOutPath "$INSTDIR"
  File "${SRCDIR}\diktier.exe"
  File "${SRCDIR}\README.md"
  File "${SRCDIR}\versions.toml"
  SetOutPath "$INSTDIR\lib"
  File "${SRCDIR}\lib\onnxruntime.dll"
  SetOutPath "$INSTDIR\LICENSES"
  File "${SRCDIR}\LICENSES\*"
  SetOutPath "$INSTDIR"

  ; Eine Verknüpfung, kein eigener Ordner im Startmenü.
  CreateShortCut "$SMPROGRAMS\Diktier.lnk" "$INSTDIR\diktier.exe" "" "$INSTDIR\diktier.exe" 0

  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\diktier.exe,0"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" "$0"
SectionEnd

Section "$(SecAutoName)" SEC_AUTOSTART
  ; §9: Das Programm kennt seinen Startup-Eintrag selbst — kein zweiter Weg.
  nsExec::ExecToLog '"$INSTDIR\diktier.exe" --install-autostart'
  Pop $0
SectionEnd

Section /o "$(SecDeskName)" SEC_DESKTOP
  CreateShortCut "$DESKTOP\Diktier.lnk" "$INSTDIR\diktier.exe" "" "$INSTDIR\diktier.exe" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_APP} "$(SecAppDesc)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_AUTOSTART} "$(SecAutoDesc)"
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_DESKTOP} "$(SecDeskDesc)"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------------------------------------------------------- Uninstaller

Section "Uninstall"
  ; Erst den Autostart abmelden, solange die Exe noch da ist — dann beenden.
  nsExec::ExecToLog '"$INSTDIR\diktier.exe" --remove-autostart'
  Pop $0
  !insertmacro StopDaemon

  Delete "$SMPROGRAMS\Diktier.lnk"
  Delete "$DESKTOP\Diktier.lnk"

  Delete "$INSTDIR\diktier.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\versions.toml"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\lib"
  RMDir /r "$INSTDIR\LICENSES"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINSTKEY}"

  ; Modell und Einstellungen gehören dem Nutzer, nicht dem Installer.
  MessageBox MB_YESNO|MB_ICONQUESTION "$(MsgPurge)" IDNO KeepUserData
    RMDir /r "$LOCALAPPDATA\diktier"
    RMDir /r "$APPDATA\diktier"
  KeepUserData:
SectionEnd
