; Homestead installer (NSIS 3.x)
; Per-user install — no administrator rights required.
; Installs to %LOCALAPPDATA%\Homestead and creates an
; Add/Remove Programs entry.

!define APP_NAME "Homestead"
!define APP_VERSION "2.0.4"
!define APP_PUBLISHER "Shawn Rodis"
!define APP_URL "https://github.com/shawnrodis/Homestead"

Name "${APP_NAME} ${APP_VERSION}"
OutFile "Homestead-Setup-v${APP_VERSION}.exe"
InstallDir "$LOCALAPPDATA\Homestead"
RequestExecutionLevel user
ShowInstDetails show

!include "MUI2.nsh"
!define MUI_ABORTWARNING
!define MUI_ICON "..\assets\Homestead.ico"
!define MUI_UNICON "..\assets\Homestead.ico"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\Homestead.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Homestead"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Homestead" SecApp
  SectionIn RO  ; required — cannot be unchecked
  SetOutPath "$INSTDIR"
  File "Homestead.exe"
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
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "URLInfoAbout" "${APP_URL}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "DisplayIcon" "$INSTDIR\Homestead.exe"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead" "EstimatedSize" 104923

  ; Start Menu shortcuts (per-user).
  CreateDirectory "$SMPROGRAMS\Homestead"
  CreateShortcut "$SMPROGRAMS\Homestead\Homestead.lnk" "$INSTDIR\Homestead.exe"
  CreateShortcut "$SMPROGRAMS\Homestead\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Desktop Icon" SecDesktop
  CreateShortcut "$DESKTOP\Homestead.lnk" "$INSTDIR\Homestead.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "Installs the application and adds Start Menu shortcuts."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Adds a Homestead icon to your desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\Homestead.exe"
  Delete "$INSTDIR\ffmpeg.exe"
  Delete "$INSTDIR\licenses\FFmpeg-LICENSE.txt"
  Delete "$INSTDIR\licenses\LICENSE"
  RMDir "$INSTDIR\licenses"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Homestead"

  Delete "$SMPROGRAMS\Homestead\Homestead.lnk"
  Delete "$SMPROGRAMS\Homestead\Uninstall.lnk"
  RMDir "$SMPROGRAMS\Homestead"

  Delete "$DESKTOP\Homestead.lnk"
SectionEnd
