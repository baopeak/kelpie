; Kelpie installer hooks (bundle.windows.nsis.installerHooks).
;
; Windows will not overwrite a running program. The session daemon runs from its own copy
; outside the files the installer replaces (daemon_runtime.rs), so installing over a running
; Kelpie closes only the app: every session keeps running and the updated app reconnects to it.
; stop-daemon.ps1 handles what still runs from the install folder itself (a daemon started by
; Kelpie 0.1.x) and, on a real uninstall, everything.

!define KELPIE_HOOKS_DIR "${__FILEDIR__}"

!macro KELPIE_STOP_DAEMON mode
  ; The app first (Tauri's own check, normally run after this hook): while it runs it would
  ; start a daemon again the moment one is stopped.
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  InitPluginsDir
  File "/oname=$PLUGINSDIR\kelpie-stop-daemon.ps1" "${KELPIE_HOOKS_DIR}\stop-daemon.ps1"
  nsExec::Exec 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\kelpie-stop-daemon.ps1" -InstallDir "$INSTDIR" -DataDir "$LOCALAPPDATA\Kelpie" -Mode ${mode}'
  Pop $R9
  ${If} $R9 == 3
    MessageBox MB_ICONSTOP|MB_OK "Run this installer from Explorer or the Start menu, not from a Kelpie terminal: it has to stop the session daemon that terminal runs on." /SD IDOK
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro KELPIE_STOP_DAEMON install
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; An installer replacing this version ("uninstall before installing", the default when
  ; upgrading by hand) runs this uninstaller in place, from the install folder; a real uninstall
  ; runs it from a temporary copy. Only a real uninstall stops the sessions.
  ${If} $EXEDIR == $INSTDIR
    !insertmacro KELPIE_STOP_DAEMON install
  ${Else}
    !insertmacro KELPIE_STOP_DAEMON uninstall
  ${EndIf}
!macroend
