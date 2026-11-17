# Pruebas en hardware

## PC Windows (esta máquina)

- Desde WSL: `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. El sandbox bloquea el puente WSL→Windows, así que cada uso requiere aprobación (es intencional).
- Binarios de Windows: `cargo build --target x86_64-pc-windows-gnu` (requiere `sudo apt install -y mingw-w64`), copiarlos a `C:\Users\PC\AppData\Local\Temp\tandem\` y ejecutarlos por interop.
- **Monitores:** Tandem ya no los controla (DESIGN §8, T-092); ninguna prueba cambia entradas. El 34" es la única pantalla de la PC.

## Mac

- `ssh mac-runner` (en `~/.ssh/config`: `HostName 192.168.1.191`, `User franciscohernandez`), con la llave de WSL autorizada (T-051, 2026-11-16). Por Wi-Fi.
- MacBook Pro Apple Silicon, macOS 27.0.1. Herramientas: Xcode CLT; instalar rustup en el usuario si falta (`curl https://sh.rustup.rs -sSf | sh -s -- -y`).
- Si deja de responder: revisar que **Sesión remota** siga activa (`sudo systemsetup -getremotelogin`) y la IP (`ipconfig getifaddr en0`).
