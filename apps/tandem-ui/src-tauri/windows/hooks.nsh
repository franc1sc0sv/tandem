; Tandem NSIS installer hooks (inserted by the Tauri bundler; see
; docs/dev/install-windows.md). tandemd.exe and tandemw.exe are copied to $INSTDIR as sidecars.
;
; - Before copying and before uninstalling: the agent is stopped, or it would leave the .exe files locked.
; - On install: `tandemd --install` registers tandemw.exe (a scheduled task with the highest
;   privileges if there is an administrator, otherwise HKCU\...\Run); then the agent is
;   started so as not to wait for the next login. No config is installed: the agent
;   generates it on startup with the monitors it detects (T-082).
; - With an administrator, `--install` also refreshes the secure desktop service (T-027) if it
;   is installed, and `--uninstall` removes it. The service lives in its own folder
;   (%ProgramFiles%\Tandem Service), so stopping the user's agent never touches it.
; - On uninstall (not on an update): `tandemd --uninstall` removes the autostart. The
;   user's data (%APPDATA%\Tandem) is deleted only if the delete-data checkbox is ticked; the
;   checkbox says that keeping the data keeps the pairing (T-345, windows/English.nsh and Spanish.nsh).

!macro TANDEM_STOP_AGENT
  ; The launcher first (it waits for the agent) and then the agent. Only this user's processes.
  nsis_tauri_utils::KillProcessCurrentUser "tandemw.exe"
  Pop $R0
  nsis_tauri_utils::KillProcessCurrentUser "tandemd.exe"
  Pop $R0
  Sleep 500
!macroend

!macro NSIS_HOOK_PREINSTALL
  !insertmacro TANDEM_STOP_AGENT
!macroend

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering tandemd to start at login"
  nsExec::ExecToLog '"$INSTDIR\tandemd.exe" --install'
  Pop $R0
  ${If} $R0 != 0
    DetailPrint "tandemd --install failed (code $R0)"
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK "Could not register tandemd to start automatically (code $R0). Run $\"$INSTDIR\tandemd.exe$\" --install to see the error."
    ${EndIf}
  ${Else}
    Exec '"$INSTDIR\tandemw.exe"'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro TANDEM_STOP_AGENT
  ; On an update the new installer registers the autostart again.
  ${If} $UpdateMode <> 1
    nsExec::ExecToLog '"$INSTDIR\tandemd.exe" --uninstall'
    Pop $R0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$APPDATA\Tandem"
  ${EndIf}
!macroend
