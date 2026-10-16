# Instalar Tandem en macOS

`Tandem.app` trae dentro el agente: `Contents/MacOS/tandemd`. La app instala un LaunchAgent
que arranca `tandemd` al iniciar sesión, y en el primer arranque guía los permisos que macOS
pide para capturar e inyectar teclado y mouse.

## Instalar

1. Abre `Tandem_<versión>_<arch>.dmg` (del artefacto `tandem-macos-dmg` del CI, o del que
   genera `scripts/macos/bundle.sh`) y arrastra **Tandem** a **Aplicaciones**.
2. La primera vez, como la firma es ad-hoc (ver [Firma](#firma)), Gatekeeper no la abre con
   doble clic: clic derecho → **Abrir** → **Abrir**. En macOS 15 o posterior, si no aparece
   la opción: **Ajustes del Sistema → Privacidad y seguridad → Abrir igualmente**. Desde una
   terminal también sirve quitar la cuarentena:
   ```sh
   xattr -dr com.apple.quarantine /Applications/Tandem.app
   ```
3. Ábrela **desde Aplicaciones**. Si se abre desde la imagen de disco o macOS la ejecuta
   desde una copia temporal (*App Translocation*, pasa con apps en cuarentena fuera de
   Aplicaciones), la app no instala el agente y lo dice en la guía: el LaunchAgent apuntaría
   a una ruta que desaparece.
4. Escribe `~/Library/Application Support/Tandem/tandem.toml` (formato en
   `crates/tandemd/src/config.rs`). Sin él, `tandemd` sale con error y launchd lo vuelve a
   intentar cada 30 s; arranca solo en cuanto el archivo es válido. Para comprobarlo:
   ```sh
   /Applications/Tandem.app/Contents/MacOS/tandemd --check-config ~/Library/Application\ Support/Tandem/tandem.toml
   ```

## El agente de inicio de sesión

Cada vez que arranca, la app deja al día `~/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist`
(`tandem_ui::launch_agent`):

- apunta al `tandemd` de la app que se está ejecutando y lleva su versión
  (`TANDEM_APP_VERSION`): actualizar la app cambia el plist, y la app saca el agente viejo
  de launchd (`launchctl bootout`) y carga el nuevo (`launchctl bootstrap gui/<uid>`);
- si el plist ya es el correcto y está cargado, no toca nada (abrir la app no reinicia
  `tandemd`); si está pero no cargado, lo vuelve a cargar;
- `RunAtLoad`, `KeepAlive` solo si sale con error, `ThrottleInterval` de 30 s, solo en la
  sesión gráfica (`LimitLoadToSessionType = Aqua`);
- la salida de `tandemd` antes de abrir su log va a `~/Library/Logs/Tandem/tandemd.log`; su
  log propio, en `~/Library/Application Support/Tandem/logs/`.

Cómo quedó se ve en la primera pantalla de la guía de permisos (botón **Permisos** de la
ventana). A mano:

```sh
launchctl print gui/$(id -u)/com.franc1sc0sv.tandem.tandemd   # estado, PID, último código de salida
launchctl kickstart -k gui/$(id -u)/com.franc1sc0sv.tandem.tandemd   # reiniciarlo
```

## Permisos

`tandemd` es quien captura e inyecta, así que los permisos son suyos. La guía del primer
arranque (`tandem_ui::setup`) muestra una pantalla por permiso con un botón que abre el panel
correspondiente de **Ajustes del Sistema → Privacidad y seguridad**; se vuelve a abrir con el
botón **Permisos**. Al pulsar **Terminar** queda marcada en
`~/Library/Application Support/Tandem/setup.done` y no vuelve a salir sola.

| Permiso | Para qué | Panel |
|---|---|---|
| Accesibilidad | Inyectar teclado y mouse que llegan de la PC; «Traer ventanas» | `Privacy_Accessibility` |
| Monitorización de entrada | Capturar teclado y mouse de la Mac para mandarlos a la PC | `Privacy_ListenEvent` |
| Red local | Descubrir la PC por mDNS (`_tandem._udp`) y hablar con ella por QUIC | `Privacy_LocalNetwork` |

En Accesibilidad y Monitorización de entrada, si `tandemd` no aparece en la lista, pulsa **+**
y elige `/Applications/Tandem.app/Contents/MacOS/tandemd` (⌘⇧G para escribir la ruta). La
red local la pregunta macOS la primera vez que `tandemd` busca la PC; el texto del diálogo
sale de `NSLocalNetworkUsageDescription` (`apps/tandem-ui/src-tauri/Info.plist`).

Con firma ad-hoc, macOS asocia el permiso a la firma exacta del binario: **cada versión nueva
de la app pide los permisos otra vez** (quita la entrada vieja de la lista y agrega la nueva).
Con Developer ID el permiso sobrevive a las actualizaciones.

Comandos útiles al probar:

```sh
tccutil reset Accessibility com.franc1sc0sv.tandem   # olvidar el permiso de la app
rm ~/Library/Application\ Support/Tandem/setup.done   # volver a ver la guía al abrir
```

## Construir el .dmg

En una Mac con Rust, Node 22 y pnpm:

```sh
pnpm --dir apps/tandem-ui install
scripts/macos/bundle.sh
```

El script compila `tandemd` en release para el target de la máquina (o `TANDEM_TARGET`),
lo deja como sidecar en `apps/tandem-ui/src-tauri/binaries/tandemd-<target>` (no se versiona),
corre `tauri build --bundles app,dmg` con `src-tauri/tauri.sidecar.json` (que agrega el
sidecar; fuera del script la app compila sin él) y comprueba que la app trae `tandemd`, que el
`Info.plist` tiene la descripción de la red local, la firma (`codesign --verify --deep --strict`)
y la imagen (`hdiutil verify`). Resultado en
`target/<target>/release/bundle/dmg/Tandem_<versión>_<arch>.dmg`.

El job `dmg (macos-latest)` del CI hace lo mismo en cada PR y sube el `.dmg` como artefacto
`tandem-macos-dmg` (Apple Silicon, firma ad-hoc).

## Firma

**Decisión del usuario:** por omisión la firma es **ad-hoc** (`"signingIdentity": "-"` en
`apps/tandem-ui/src-tauri/tauri.macos.conf.json`): no hace falta cuenta de desarrollador ni
secretos, la app corre en Apple Silicon y Gatekeeper pide confirmar la primera apertura. El
bundler de Tauri firma también el sidecar.

Para firmar con **Developer ID** y **notarizar**, basta con definir las variables que lee el
bundler de Tauri antes de `scripts/macos/bundle.sh` (en CI, como secretos del repo):

| Variable | Qué es |
|---|---|
| `APPLE_SIGNING_IDENTITY` | Identidad de firma, p. ej. `Developer ID Application: Nombre (TEAMID)`. Reemplaza la ad-hoc |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | El certificado `.p12` en base64 y su contraseña, si no está ya en el llavero (CI) |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Notarización con Apple ID y contraseña de app |
| `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH` | O notarización con una clave de App Store Connect |

Con identidad real y credenciales de notarización, `tauri build` firma con *hardened runtime*,
notariza y engrapa el ticket; Gatekeeper abre la app sin preguntar y los permisos de
privacidad sobreviven a las actualizaciones. Sin credenciales de notarización, firma pero no
notariza.

## Desinstalar

```sh
launchctl bootout gui/$(id -u)/com.franc1sc0sv.tandem.tandemd
rm ~/Library/LaunchAgents/com.franc1sc0sv.tandem.tandemd.plist
rm -rf /Applications/Tandem.app
# Opcional: configuración, estado, identidad y logs.
rm -rf ~/Library/Application\ Support/Tandem ~/Library/Logs/Tandem
```

Y quitar `tandemd` y Tandem de las listas de Accesibilidad, Monitorización de entrada y Red
local.

## Pendiente

- Binario universal (Intel + Apple Silicon): hoy el `.dmg` es de la arquitectura de la Mac
  que lo construye (`TANDEM_TARGET=x86_64-apple-darwin` para Intel).
- Que la guía muestre si cada permiso ya está concedido: lo sabe `tandemd`, no la app, y la
  IPC todavía no lo informa.
