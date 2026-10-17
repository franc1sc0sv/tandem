# Traspaso — estado al 2026-10-16

Punto de partida para la próxima sesión. Léelo entero antes de tocar nada.

## Dónde estamos

- `develop` está en `T-085` (#57) + este traspaso. **50 tareas `done`**, CI verde. `main` sigue sin tocar: el paso develop → main lo hace el usuario.
- **Siguiente tarea: [T-087](../tasks/T-087.md)**, el asistente de configuración inicial con verificación: puesto vacío por defecto, monitores por selector (solo los conectados), eliminar un monitor borra todo lo suyo, seguridad de pantallas, teclado por pulsación y portapapeles en Windows. La especificación completa está en el archivo de la tarea. Se lanzó dos veces y se detuvo a pedido del usuario. No existe T-086 en `develop`: se reemplazó por T-087. Las ramas o worktrees locales `task/T-086` y `task/T-087`, si existen, son descartables.
- Pendientes que necesitan al usuario: **T-027** (servicio SYSTEM en Windows; requiere decidir la firma Authenticode), **T-051** (SSH a la Mac; bloqueado: la Mac es de trabajo y Remote Login queda en Off, probablemente por MDM; no forzarlo).
- Por decidir con el usuario: firmar y notarizar en macOS (Apple Developer, 99 USD al año). Sin eso, Gatekeeper bloquea la app y hay que usar *Abrir igualmente* o `xattr -dr com.apple.quarantine`.

## Hechos de hardware verificados (no volver a investigar)

| | 34" Samsung LC34G55T | 32" Samsung Odyssey G5 |
|---|---|---|
| EDID visto por la PC (DP) | `SAM711A` | `SAM7489` |
| EDID visto por la Mac (HDMI) | `SAM7119` | `SAM748A` |
| DDC → HDMI (Mac) | `0x06` | `0x05` |
| DDC → DP (PC) | `0x09` | `0x0F` |

- **El EDID cambia según la entrada**: el mismo monitor tiene un id PnP distinto en cada máquina. Por eso se duplicaron los monitores al sincronizar.
- Los dos monitores declaran en sus capabilities códigos que **no obedecen**. La lectura de `0x60` no es confiable: el 34" siempre lee `0x09`, y el 32" no se actualiza tras un cambio desde el menú. El timing report del 34" no distingue la entrada. **Pendiente:** probar el timing report del 32" con el usuario presente.
- Solo la PC tiene canal DDC con los monitores. La Mac no lo tiene: BetterDisplay da "DDC channel unavailable".
- **Windows reserva los puertos UDP 47000–48999** en esa PC (WSL/Hyper-V). Tandem usa 52000 (default desde T-082, con puertos de reserva).
- PC: Gigabyte X870, Realtek 2.5GbE, USB4. La Mac es una MacBook Pro M5 de trabajo (`192.168.1.191` por Wi-Fi).

## Estado de las máquinas del usuario

- **PC:** Tandem instalado (versión T-085) en `%LOCALAPPDATA%\Tandem`, con `tandemd` arrancando con la sesión. La config `%APPDATA%\Tandem\tandem.toml` tiene **el puesto duplicado (4 monitores)** que dejó la primera sincronización. Antes de probar T-087 hay que hacer «Restablecer puesto», o vaciar `[[monitors]]` a mano y borrar `state.json`. Respaldo de la primera config en `tandem.toml.original`. Está **emparejada** con la Mac (`trust.toml`).
- **Mac:** Tandem instalado desde el `.dmg` de CI. El usuario **detuvo el agente** con `launchctl bootout gui/$(id -u)/com.franc1sc0sv.tandem.tandemd` porque el 32" quedó en negro. Para volver a cargarlo: `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist`, o abrir la app. Su config la generó T-082 y también tiene el puesto duplicado.
- Herramienta de diagnóstico en la PC: `%LOCALAPPDATA%\Temp\tandem\tandem-ddc.exe` (`list`, `caps`, `profile`, `switch … --return-after`, `calibrate`).

## Cómo trabajamos (flujo probado)

- **Abre la sesión de Claude Code en `apps/tandem`** para que carguen `CLAUDE.md`, los agentes y el workflow. Si se abre en la carpeta padre, usa `portable: true`.
- Cada tarea se corre con el workflow `.claude/workflows/autopilot.js` en modo *pool*, una corrida por tarea:
  `{"rounds":1,"perRound":1,"portable":true,"hostMerge":true,"tasks":[{"id":"T-087","title":"…lee docs/tasks/T-087.md…"}],"auth":"<orden textual del usuario>"}`
  - `hostMerge: true` es obligatorio. El clasificador de auto mode **no deja hacer merge a los subagentes** (ni con la regla en settings): el merge lo hace la sesión principal.
  - `auth` lleva la orden textual del usuario. Los subagentes leen el último mensaje del usuario y se frenan si es solo una pregunta; si el usuario escribe en medio, que termine con "y sigue".
- **Integrar:** `scripts/claude/integrate-pr.sh <pr> <rama>`. Solo hace merge si el CI pasó **sobre una cabeza que contiene el `develop` actual**: si la rama está atrasada, hace `gh pr update-branch` y espera. Resuelve solo los conflictos de `Cargo.lock`; para cualquier otro conflicto se detiene.
- **Tareas listas:** `scripts/claude/ready-tasks.py [ids en curso…]`.
- **El repo es público** (minutos de CI ilimitados). El CI corre en cada PR y en `develop`. No intentes apagarlo ni hacer merge sin CI: el clasificador lo bloquea como "CI Bypass", y el usuario eligió mantenerlo.
- **Probar en la PC desde WSL:** `powershell.exe` por interop, que necesita salir del sandbox. Instalar con `Start-Process … /S -PassThru` y `WaitForExit`; lanzar el `.exe` directo desde WSL no instala. Descargar artefactos de CI con `allowed_domains: *.blob.core.windows.net`.
- En WSL, fuera del sandbox, la prueba `pairing::tests::nobody_confirming_times_out_without_saving` falla también en `develop` (`ConnectionLost(TimedOut)` en 0.06 s); en CI pasa. Está pendiente investigarlo.
- **Lecciones:**
  - Antes de cada merge, comprueba que el CI de `develop` siga verde después del anterior.
  - El sandbox local salta las pruebas de sockets Unix, así que el CI es la verdad.
  - Cualquier prueba de hardware que cambie entradas de monitor lleva vuelta automática.
