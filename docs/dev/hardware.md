# Pruebas en hardware

## PC Windows (esta máquina)

- Desde WSL: `/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe`. El sandbox bloquea el puente WSL→Windows, así que cada uso requiere aprobación (es intencional).
- Binarios de Windows: `cargo build --target x86_64-pc-windows-gnu` (requiere `sudo apt install -y mingw-w64`), copiarlos a `C:\Users\PC\AppData\Local\Temp\tandem\` y ejecutarlos por interop.
- **Monitores:** toda prueba que cambie una entrada lleva vuelta automática; el 34" es la única pantalla de la PC.

## Mac (pendiente, tarea T-051)

1. En la Mac: Ajustes del Sistema → General → Compartir → **Sesión remota** activada.
2. Desde WSL: `ssh-copy-id <usuario>@<ip-de-la-mac>`.
3. En `~/.ssh/config`:
   ```
   Host mac-runner
     HostName <ip-de-la-mac>
     User <usuario>
   ```
4. Comprobar: `ssh mac-runner uname -a`.
