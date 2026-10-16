# Instalador de Windows

Tandem se instala en la PC con un instalador NSIS que arma el bundler de Tauri
(`Tandem_<versión>_x64-setup.exe`). Trae la app, el agente `tandemd.exe` y
su lanzador sin consola `tandemw.exe`, y deja el agente arrancando con la sesión del usuario.

## Instalar

1. Descarga el artefacto `tandem-windows-installer` del CI (job `instalador (windows)`) o
   constrúyelo (ver abajo).
2. Ejecuta `Tandem_<versión>_x64-setup.exe`. No pide administrador: se instala para el
   usuario actual en `%LOCALAPPDATA%\Tandem`.
3. Al terminar, `tandemd` ya está corriendo; no hace falta cerrar sesión.

Instalación desatendida: `Tandem_<versión>_x64-setup.exe /S`.

Qué deja el instalador:

| Qué | Dónde |
|---|---|
| Programa (la app, `tandemd.exe`, `tandemw.exe`, `uninstall.exe`) | `%LOCALAPPDATA%\Tandem` |
| Configuración (`tandem.toml`) | `%APPDATA%\Tandem\tandem.toml` |
| Estado y logs del agente | `%APPDATA%\Tandem` (`state.json`, `logs\`) |
| Arranque con la sesión | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, valor `tandemd` |
| Accesos directos | Menú Inicio (y escritorio en la instalación desatendida) |

### Configuración inicial

Si no hay `%APPDATA%\Tandem\tandem.toml`, el instalador crea uno con `machine = "pc"` y los dos
monitores del puesto (plantilla en `crates/tandemd/templates/tandem.pc.toml`). **Nunca
sobrescribe** uno existente: reinstalar o actualizar conserva tus cambios. Después de editarlo:

```
"%LOCALAPPDATA%\Tandem\tandemd.exe" --check-config "%APPDATA%\Tandem\tandem.toml"
```

y reinicia el agente (cerrar sesión, o terminar `tandemd.exe` y ejecutar `tandemw.exe`).

## Arranque automático

Al terminar de copiar, el instalador ejecuta `tandemd --install`, que:

1. crea la configuración inicial si falta;
2. registra en la clave `Run` de `HKCU` el valor `tandemd` = `"…\Tandem\tandemw.exe"`.

`tandemw.exe` es un lanzador de subsistema gráfico que arranca `tandemd.exe` con
`CREATE_NO_WINDOW`: `tandemd` es una aplicación de consola (su CLI escribe en la terminal) y
lanzado directamente desde `Run` abriría una ventana negra en cada inicio de sesión. Los
argumentos pasan tal cual y el código de salida es el de `tandemd`; los errores van al log en
`%APPDATA%\Tandem\logs`.

Se eligió la clave `Run` de `HKCU` (y no una tarea programada) porque no pide administrador,
solo afecta al usuario que instala y es lo que Windows muestra en *Administrador de tareas →
Aplicaciones de inicio*, donde se puede desactivar. El modo servicio SYSTEM con escritorio
seguro (UAC, pantalla de inicio de sesión) es T-027 y queda como mejora posterior.

Para registrarlo o quitarlo a mano:

```
tandemd --install     # idempotente; acepta --config / --data-dir y los pasa a la orden de Run
tandemd --uninstall   # quita el valor de Run; deja la configuración
```

En macOS estas órdenes no existen: ahí el arranque es un LaunchAgent que instala la app (T-070).

## Desinstalar

Desde *Configuración → Aplicaciones → Tandem → Desinstalar* (o `uninstall.exe`, `/S` para
desatendido). El desinstalador:

1. detiene `tandemw.exe` y `tandemd.exe` del usuario;
2. ejecuta `tandemd --uninstall` (quita el valor `tandemd` de `Run`);
3. borra los binarios, los accesos directos y la entrada de *Aplicaciones*.

La configuración, el estado y los logs (`%APPDATA%\Tandem`) se conservan, salvo que marques la
casilla *Borrar los datos de la aplicación* en el desinstalador. Ojo: ahí vive también la
identidad del agente, así que borrarlos obliga a emparejar otra vez.

En una actualización (instalar una versión encima de otra) el instalador detiene el agente
antes de copiar, reemplaza los binarios, vuelve a registrar el arranque y lo arranca de nuevo.

## Construir el instalador

En Windows, con Rust (MSVC), Node 22 y pnpm:

```
cd apps\tandem-ui
pnpm install --frozen-lockfile
cd ..\..
pwsh tools\windows\installer\build-installer.ps1
```

El script compila `tandemd` y `tandemw` en release, los copia como sidecars a
`apps/tandem-ui/src-tauri/binaries/` (con el triple del destino en el nombre, como pide Tauri)
y ejecuta `pnpm tauri build --features app --config src-tauri/windows/installer.conf.json`. El
instalador queda en `target\release\bundle\nsis\`. Tauri descarga NSIS la primera vez.

Piezas:

- `apps/tandem-ui/src-tauri/windows/installer.conf.json`: destino NSIS, sidecars, modo
  `currentUser`, idioma español. Va aparte de `tauri.conf.json` para que el `cargo build` normal
  de la UI no exija los sidecars.
- `apps/tandem-ui/src-tauri/windows/hooks.nsh`: los ganchos de instalar y desinstalar.
- `crates/tandemd/src/install/`: `--install` / `--uninstall` (la lógica se prueba con un doble del
  registro; el registro real, en el CI de Windows).
- `tools/windows/installer/test-installer.ps1`: prueba de punta a punta que corre el CI.

### Prueba en el CI

El job `instalador (windows)` construye el instalador, lo sube como artefacto
`tandem-windows-installer` y corre `test-installer.ps1`: instala en silencio, comprueba los
archivos, el valor de `Run`, que la config inicial sea válida y que `tandemd` esté corriendo;
reinstala encima y comprueba que la config del usuario no cambió; desinstala y comprueba que
el agente se detuvo, que el valor de `Run` y los binarios desaparecieron y que la config se
conservó. **No lo corras en la PC de uso diario**: instala y desinstala de verdad.

## Firma Authenticode

Por omisión el instalador **no se firma** y Windows SmartScreen avisa de un editor
desconocido la primera vez («Más información → Ejecutar de todas formas»). Firmar es decisión
del usuario (cuesta un certificado de firma de código); el script lo activa con variables de
entorno, y Tauri firma entonces los binarios, los sidecars y el instalador:

| Variable | Uso |
|---|---|
| `TANDEM_WINDOWS_CERT_THUMBPRINT` | Huella SHA-1 de un certificado instalado en el almacén del usuario (`Cert:\CurrentUser\My`). Firma con `signtool`, SHA-256. |
| `TANDEM_WINDOWS_TIMESTAMP_URL` | Servidor de sellado de tiempo. Por omisión `http://timestamp.digicert.com`. |
| `TANDEM_WINDOWS_SIGN_COMMAND` | Orden propia de firma, con `%1` en lugar del archivo (por ejemplo, Azure Trusted Signing o un token HSM). Si está, gana sobre la huella. |

Ejemplos:

```
$env:TANDEM_WINDOWS_CERT_THUMBPRINT = 'A1B2…'
pwsh tools\windows\installer\build-installer.ps1

$env:TANDEM_WINDOWS_SIGN_COMMAND = 'trusted-signing-cli -e https://eus.codesigning.azure.net -a cuenta -c perfil %1'
pwsh tools\windows\installer\build-installer.ps1
```

En el CI se firmaría guardando el certificado en un secreto del repositorio, importándolo al
almacén del runner antes del build y pasando la huella; no está activado porque no hay
certificado. El servicio SYSTEM de T-027 sí tendrá requisitos de firma propios (se documentan
en esa tarea).
