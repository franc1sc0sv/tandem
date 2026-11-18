# Pruebas en hardware

## PC Windows (esta máquina)

- Desde WSL: `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. El sandbox bloquea el puente WSL→Windows, así que cada uso requiere aprobación (es intencional).
- Binarios de Windows: `cargo build --target x86_64-pc-windows-gnu` (requiere `sudo apt install -y mingw-w64`), copiarlos a `C:\Users\PC\AppData\Local\Temp\tandem\` y ejecutarlos por interop.
- **Monitores:** Tandem sí los controla por DDC/CI (DESIGN §8, D11). Las pruebas que cambian entradas (T-110, T-123) son opt-in con `--with-monitors` y llevan vuelta automática: un vigilante independiente restaura la entrada aunque la prueba falle (A8). Nunca se corren sin esa vuelta, porque el usuario puede quedarse sin ver la PC.

## Mac

- `ssh mac-runner` (definido en `~/.ssh/config` con `HostName <ip-de-la-mac>` y `User <usuario>`; esos datos no se versionan), con la llave de WSL autorizada (T-051, 2026-11-16). Por Wi-Fi.
- MacBook Pro Apple Silicon, macOS 27.0.1. Herramientas: Xcode CLT; instalar rustup en el usuario si falta (`curl https://sh.rustup.rs -sSf | sh -s -- -y`).
- Si deja de responder: revisar que **Sesión remota** siga activa (`sudo systemsetup -getremotelogin`) y la IP (`ipconfig getifaddr en0`).

## Suite automática

`scripts/hw/run.sh` (T-123) corre las pruebas de las dos máquinas y deja un informe; ver `docs/dev/hardware-suite.md`. Los monitores solo se tocan con `--with-monitors` y con vuelta automática.
