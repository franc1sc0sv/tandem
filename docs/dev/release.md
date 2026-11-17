# Release

El workflow `Release` (`.github/workflows/release.yml`) construye los dos instaladores, los
paquetes firmados del updater, los checksums, las notas y el `latest.json`, y crea el release
en GitHub (DECISIONS D19, DESIGN §13). Un release se crea siempre en borrador con todo subido;
una versión final se publica al terminar y una prerelease (`-rc.N`) queda en borrador.

| Disparo | Qué hace |
|---|---|
| Empujar un tag `v<versión>` | Todo; crea el release de ese tag. `v0.3.0` se publica; `v0.3.0-rc.1` queda en borrador como prerelease |
| A mano (*Actions → Release → Run workflow*), con «Modo prueba» marcado (por omisión) | Todo como artefactos del workflow, firmado con una llave desechable; **ni release ni tag** |
| A mano con «Modo prueba» desmarcado | Como un tag, para `v<versión>` en el commit elegido; el tag lo crea GitHub al publicar |
| PR que toca `release.yml`, `scripts/release/` o `tauri.updater.json` | Siempre en modo prueba |

`workflow_dispatch` solo aparece en *Actions* cuando el workflow está en la rama por omisión
(`main`); hasta entonces, el modo prueba se ve en los PRs que lo tocan.

## Jobs

1. **preparar** (Ubuntu): corre `scripts/release/test.sh`, decide la versión, el modo y si es
   prerelease, y genera `notas.md`. Si va a crear un release y falta el secreto
   `TAURI_SIGNING_PRIVATE_KEY`, falla aquí con el motivo (antes de compilar nada).
2. **dmg (macos-latest)**: `scripts/macos/bundle.sh --config src-tauri/tauri.updater.json`,
   igual que el CI (ver [install-macos.md](install-macos.md)) más el paquete del updater:
   `Tandem_<versión>_<arch>.app.tar.gz` y su firma `.sig`. Sube `instalador-macos`.
3. **instalador (windows)**: `tools/windows/installer/build-installer.ps1` con
   `TANDEM_UPDATER_ARTIFACTS=1` (deja `Tandem_<versión>_x64-setup.exe.sig`; el updater baja
   el mismo instalador) y la prueba de instalar, reinstalar y desinstalar
   (`test-installer.ps1`): no sale un instalador que no se probó (ver
   [install-windows.md](install-windows.md)). Sube `instalador-windows`.
4. **checksums y latest.json** (Ubuntu): exige exactamente un `.dmg`, un `-setup.exe` y un
   `.app.tar.gz`, escribe `SHA256SUMS.txt` y arma `latest.json` con
   `scripts/release/latest-json.sh`. Sube el artefacto **`tandem-<versión>`**: lo que se
   publicaría.
5. **release**: solo fuera del modo prueba; el único job con `contents: write`. Crea el
   release en borrador con todo, y si no es prerelease lo publica (`--latest`). Si ya existe
   un release con ese tag, falla: bórralo y vuelve a correrlo.

## Actualización automática

La app trae `tauri-plugin-updater` (código en `apps/tandem-ui/src-tauri/src/update.rs`, que es
portable y tiene las pruebas, y `updater.rs`, que lo conecta con Tauri):

- Lee `https://github.com/franc1sc0sv/tandem/releases/latest/download/latest.json`, es decir
  el último release **publicado** que no es prerelease. Los borradores y las `-rc` no llegan a
  nadie solos.
- Cada paquete va firmado con la llave minisign del updater; la app solo instala lo que valida
  con la pública (`plugins.updater.pubkey` en `tauri.conf.json`). No es firma de código: los
  instaladores siguen sin Developer ID ni Authenticode (ver [Firma](#firma)).
- **Qué se actualiza:** el paquete de cada sistema trae la UI y `tandemd`. En macOS el updater
  reemplaza `Tandem.app` y la app se reinicia; al arrancar ve el plist del LaunchAgent con otra
  versión y relanza `tandemd` con el binario nuevo (`launch_agent.rs`). En Windows corre el
  instalador NSIS en modo pasivo: sus ganchos (`windows/hooks.nsh`) detienen `tandemw` y
  `tandemd` antes de copiar y los vuelven a arrancar al terminar. El arranque con la sesión de
  Windows es la clave `Run` de `HKCU` (T-071), no una tarea programada.
- **«Actualizar todas»** (Ajustes → Actualizaciones): primero la otra máquina, después esta.
  Esta le pide a la otra, por `tandemd` (`UiCommand::UpdatePeer` → `ControlMsg::Update`), que
  se instale; la UI de la otra baja su paquete y se reinicia. Esta espera hasta 10 min a que
  la otra vuelva anunciando la versión nueva (o hablando otra versión del protocolo, que
  también prueba que cambió) y recién entonces se instala ella. Nunca se reinician las dos a la
  vez, así el teclado y el mouse siguen en la que funciona. Si la otra no vuelve, **vuelta
  atrás**: esta no se toca y se avisa; al volver a intentarlo, el plan solo instala lo que
  falte.
- **«Actualizar solo»** (por omisión encendido, guardado en `update.toml` junto a
  `tandem.toml`): busca cada 6 h. Con par, guía solo la máquina `A` (la otra espera su pedido);
  sin par, cada una se actualiza sola.

### Llave del updater

Se genera una vez y se guarda como secreto del repo; la pública va en `tauri.conf.json`.

```sh
cd apps/tandem-ui
pnpm tauri signer generate -w ~/.config/tandem-release/updater.key   # pide una clave (opcional)
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.config/tandem-release/updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # solo si le pusiste clave
```

Después, el contenido de `~/.config/tandem-release/updater.key.pub` va en
`plugins.updater.pubkey` de `apps/tandem-ui/src-tauri/tauri.conf.json`. **La privada nunca
entra al repo.** Si se pierde, las apps instaladas ya no aceptan actualizaciones firmadas con
otra: habría que instalar la versión nueva a mano en las dos máquinas.

Sin el secreto, el modo prueba firma con una llave desechable (los paquetes no sirven para
actualizar a nadie, pero el camino entero queda probado) y un release falla en `preparar`.

## Versión

La versión vive en tres lugares que tienen que coincidir: `version` de `[workspace.package]`
en `Cargo.toml`, `apps/tandem-ui/src-tauri/tauri.conf.json` y `apps/tandem-ui/package.json`.
`scripts/release/version.sh [v<versión>]` lo comprueba y, con un tag, exige que sea
exactamente `v` + esa versión. El workflow lo corre siempre, así que un tag que no coincide
con la versión falla antes de compilar nada. El updater compara versiones con el orden de
SemVer (`0.3.0-rc.2` < `0.3.0` < `0.3.1`).

## Notas

`scripts/release/notes.sh [--version x.y.z] [--desde <ref>] [--hasta <ref>]` arma las notas
con las tareas integradas: cada PR de tarea entra a `develop` como un squash
`T-<id>: <título> (#<pr>)`. Por omisión toma desde el tag `v*` anterior (o desde el inicio).

- Agrupa las tareas por la `phase` de su `docs/tasks/T-<id>.md`, en el orden de las fases del
  diseño. Una tarea con varios commits sale una vez, con todos sus PRs.
- Lo que no es de una tarea (backlog, QA, diseño) va en «Otros cambios».
- Al final, las tareas con casillas sin marcar en `## Verificación en hardware pendiente`:
  lo que nadie comprobó todavía en el equipo real.

Para verlas en local: `scripts/release/notes.sh --version "$(scripts/release/version.sh)"`.
Las mismas notas van en `latest.json` (las ve el updater).

## Sacar un release

1. En `develop`, subir la versión en los tres archivos (y `Cargo.lock` con `cargo check`) y
   integrarla como cualquier cambio.
2. Probar una prerelease: versión `x.y.z-rc.N`, tag `vx.y.z-rc.N` en ese commit. Queda en
   borrador; instalar los dos paquetes en el equipo real. (O correr el workflow a mano en modo
   prueba y revisar el artefacto `tandem-<versión>`.)
3. Llevar el cambio a `main` (lo hace el usuario) y empujar el tag de la versión final:
   ```sh
   git switch main && git pull
   git tag -s v0.3.0 -m "Tandem 0.3.0"
   git push origin v0.3.0
   ```
4. El workflow publica el release al terminar; desde ese momento las apps lo encuentran.

Para comprobar una descarga: `sha256sum -c SHA256SUMS.txt --ignore-missing` (en macOS,
`shasum -a 256 -c SHA256SUMS.txt`; en Windows, `Get-FileHash` y comparar).

## Firma

Hoy los instaladores del release salen como los del CI: el `.dmg` con firma ad-hoc y el
instalador de Windows sin Authenticode (D19: sin firmar). Las dos firmas reales se activan con
variables de entorno (Apple: `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`...; Windows:
`TANDEM_WINDOWS_SIGN_COMMAND` o `TANDEM_WINDOWS_CERT_THUMBPRINT`), descritas en
[install-macos.md](install-macos.md#firma) e [install-windows.md](install-windows.md). Para
usarlas en el release: guardarlas como secretos del repo y pasarlas como `env:` al paso de
construcción de cada job. No se pasan vacías a propósito: el bundler de Tauri podría tomar
una `APPLE_SIGNING_IDENTITY` vacía como identidad en vez de usar la ad-hoc.

La firma minisign del updater es otra cosa y sí está: ver [Llave del updater](#llave-del-updater).

## Pendiente

- Binario universal de macOS (hoy `aarch64`, la arquitectura del runner `macos-latest`; el
  `latest.json` solo trae `darwin-aarch64` y `windows-x86_64`).
- Firma Developer ID / Authenticode en el release (decisión del usuario, ver arriba).
