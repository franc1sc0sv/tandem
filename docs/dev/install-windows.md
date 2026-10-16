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
| Configuración (`tandem.toml`, la genera el agente al arrancar) | `%APPDATA%\Tandem\tandem.toml` |
| Estado y logs del agente | `%APPDATA%\Tandem` (`state.json`, `logs\`) |
| Arranque con la sesión | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, valor `tandemd` |
| Accesos directos | Menú Inicio (y escritorio en la instalación desatendida) |

### Configuración automática

El instalador no trae configuración. La primera vez que arranca, `tandemd` crea
`%APPDATA%\Tandem\tandem.toml` con lo que ve (`tandemd::autoconfig`, T-082), igual que en la
Mac:

- `machine = "pc"`;
- un `[[monitors]]` por cada monitor con EDID que ven la topología de Windows y el bus DDC
  (dxva2), de izquierda a derecha: id = id PnP en minúsculas (`sam711a`, `sam7489`),
  `edid_model` = id PnP, etiqueta = tamaño (`34"`, `32"`) o modelo, y `ddc` con los códigos
  de la base de perfiles (`profiles/ddc`) si el modelo está en ella (los dos Samsung del
  puesto lo están: 34" HDMI 0x06 / DP 0x09, 32" HDMI 0x05 / DP 0x0F);
- cableado por omisión DisplayPort a la PC y HDMI a la Mac; se cambia con un clic en
  **Puesto** (botón de la ventana) y se guarda en `tandem.toml`;
- red, entrada y energía con sus valores por omisión, sin datos de ejemplo.

Reinstalar o actualizar **nunca** toca un `tandem.toml` existente. Se puede editar (formato en
`docs/examples/tandem.pc.toml`); para detectar los monitores de nuevo, bórralo y reinicia el
agente (cerrar sesión, o terminar `tandemd.exe` y ejecutar `tandemw.exe`). Para validar uno
editado a mano:

```
"%LOCALAPPDATA%\Tandem\tandemd.exe" --check-config "%APPDATA%\Tandem\tandem.toml"
```

### Puerto

El puerto por omisión es el UDP **52000**. En las PCs con WSL o Hyper-V, Windows reserva rangos
de puertos (en el puesto real, 47000–48999: el 47770 de antes fallaba con `os error 10048`). Si
el puerto configurado no se puede abrir, el agente prueba otros (52000, 52100, 53100, 54200,
55300, 56400, 45500, 44600) y anuncia el real por mDNS, así la Mac lo encuentra igual; el de
emparejamiento es el siguiente al que quedó. Si ninguno abre, el error del log dice cuáles
probó y cómo arreglarlo. Los rangos reservados se ven con:

```
netsh interface ipv4 show excludedportrange protocol=udp
```

## Arranque automático

Al terminar de copiar, el instalador ejecuta `tandemd --install`, que registra en la clave
`Run` de `HKCU` el valor `tandemd` = `"…\Tandem\tandemw.exe"`, y arranca el agente (que
genera la configuración si no hay).

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
archivos, el valor de `Run`, que `tandemd` esté corriendo y que haya generado una config de la
PC válida (el instalador no trae ninguna);
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
