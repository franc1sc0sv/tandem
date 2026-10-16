; Ganchos del instalador NSIS de Tandem (los inserta el bundler de Tauri; ver
; docs/dev/install-windows.md). tandemd.exe y tandemw.exe se copian a $INSTDIR como sidecars.
;
; - Antes de copiar y de desinstalar: se detiene el agente, que si no deja los .exe bloqueados.
; - Al instalar: `tandemd --install` registra tandemw.exe en HKCU\...\Run; despues se
;   arranca el agente para no esperar al proximo inicio de sesion. No se instala ninguna
;   config: el agente la genera al arrancar con los monitores que detecta (T-082).
; - Al desinstalar (no en una actualizacion): `tandemd --uninstall` quita el arranque. Los
;   datos del usuario (%APPDATA%\Tandem) se borran solo si se marca la casilla de borrar datos.

!macro TANDEM_STOP_AGENT
  ; Primero el lanzador (espera al agente) y despues el agente. Solo procesos de este usuario.
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
  DetailPrint "Registrando tandemd para arrancar con la sesion"
  nsExec::ExecToLog '"$INSTDIR\tandemd.exe" --install'
  Pop $R0
  ${If} $R0 != 0
    DetailPrint "tandemd --install fallo (codigo $R0)"
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK "No se pudo registrar el arranque automatico de tandemd (codigo $R0). Ejecuta $\"$INSTDIR\tandemd.exe$\" --install para ver el error."
    ${EndIf}
  ${Else}
    Exec '"$INSTDIR\tandemw.exe"'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro TANDEM_STOP_AGENT
  ; En una actualizacion el instalador nuevo vuelve a registrar el arranque.
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
