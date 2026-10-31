; SmartLife & Tuya Desktop installer (NSIS 3.x)
; Per-user install — no administrator rights required.
; Installs to %LOCALAPPDATA%\SmartLifeTuyaDesktop and creates an
; Add/Remove Programs entry.

!define APP_NAME "SmartLife & Tuya Desktop"
!define APP_VERSION "1.6.1"
!define APP_PUBLISHER "Shawn Rodis"
!define APP_URL "https://github.com/shawnrodis/SmartLifeTuyaDesktop"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "SmartLifeTuyaDesktop-Setup-v${APP_VERSION}.exe"
InstallDir "$LOCALAPPDATA\SmartLifeTuyaDesktop"
RequestExecutionLevel user
ShowInstDetails show

!include "MUI2.nsh"
!define MUI_ABORTWARNING
!define MUI_ICON "..\assets\SmartLifeTuyaDesktop.ico"
!define MUI_UNICON "..\assets\SmartLifeTuyaDesktop.ico"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\SmartLifeTuyaDesktop.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch SmartLife & Tuya Desktop"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "SmartLife & Tuya Desktop" SecApp
  SectionIn RO  ; required — cannot be unchecked
  SetOutPath "$INSTDIR"
  File "SmartLifeTuyaDesktop.exe"
  ; Bundled FFmpeg 7.1.1 (gyan.dev essentials build, GPL v3 — see the
  ; licenses folder): installed NEXT TO the app exe, which is where
  ; the app looks for it first, so live camera previews just work.
  File "..\assets\ffmpeg.exe"
  SetOutPath "$INSTDIR\licenses"
  File "..\licenses\FFmpeg-LICENSE.txt"
  File "..\LICENSE"
  SetOutPath "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  ; Add/Remove Programs entry (per-user).
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "DisplayIcon" "$INSTDIR\SmartLifeTuyaDesktop.exe"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop" "EstimatedSize" 104923

  ; Start Menu shortcuts (per-user).
  CreateDirectory "$SMPROGRAMS\SmartLife & Tuya Desktop"
  CreateShortcut "$SMPROGRAMS\SmartLife & Tuya Desktop\SmartLife & Tuya Desktop.lnk" "$INSTDIR\SmartLifeTuyaDesktop.exe"
  CreateShortcut "$SMPROGRAMS\SmartLife & Tuya Desktop\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Desktop Icon" SecDesktop
  CreateShortcut "$DESKTOP\SmartLife & Tuya Desktop.lnk" "$INSTDIR\SmartLifeTuyaDesktop.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "Installs the application and adds Start Menu shortcuts."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Adds a SmartLife & Tuya Desktop icon to your desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\SmartLifeTuyaDesktop.exe"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\licenses\FFmpeg-LICENSE.txt"
  Delete "$INSTDIR\licenses\LICENSE"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\SmartLifeTuyaDesktop"

  Delete "$SMPROGRAMS\SmartLife & Tuya Desktop\SmartLife & Tuya Desktop.lnk"
  Delete "$SMPROGRAMS\SmartLife & Tuya Desktop\Uninstall.lnk"
  RMDir "$SMPROGRAMS\SmartLife & Tuya Desktop"

  Delete "$DESKTOP\SmartLife & Tuya Desktop.lnk"
SectionEnd
