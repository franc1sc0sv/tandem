# Release

El workflow `Release` (`.github/workflows/release.yml`) construye los dos instaladores, sus
checksums y las notas, y crea un release **en borrador** en GitHub. Nada se publica solo:
publicar el borrador es siempre a mano.

| Disparo | Qué hace |
|---|---|
| Empujar un tag `v<versión>` | Todo, y crea el release en borrador de ese tag |
| A mano (*Actions → Release → Run workflow*), con «Modo prueba» marcado (por omisión) | Instaladores, checksums y notas como artefactos del workflow; **ni release ni tag** |
| A mano con «Modo prueba» desmarcado | Como un tag, para `v<versión>` en el commit elegido; el tag lo crea GitHub al publicar el borrador |
| PR que toca `release.yml` o `scripts/release/` | Siempre en modo prueba |

`workflow_dispatch` solo aparece en *Actions* cuando el workflow está en la rama por omisión
(`main`); hasta entonces, el modo prueba se ve en los PRs que lo tocan.

## Jobs

1. **preparar** (Ubuntu): corre `scripts/release/test.sh`, decide la versión y el modo y
   genera `notas.md` (también en el resumen del job).
2. **dmg (macos-latest)**: `scripts/macos/bundle.sh`, igual que el CI (ver
   [install-macos.md](install-macos.md)). Sube `instalador-macos`.
3. **instalador (windows)**: `tools/windows/installer/build-installer.ps1` y la prueba de
   instalar, reinstalar y desinstalar (`test-installer.ps1`): no sale un instalador que no se
   probó (ver [install-windows.md](install-windows.md)). Sube `instalador-windows`.
4. **checksums** (Ubuntu): junta los dos, exige exactamente un `Tandem_*.dmg` y un
   `Tandem_*-setup.exe`, escribe `SHA256SUMS.txt` y sube el artefacto **`tandem-<versión>`**
   con los dos instaladores, `SHA256SUMS.txt` y `notas.md`: lo que se publicaría.
5. **release (borrador)**: solo fuera del modo prueba. Es el único job con permiso de
   escritura (`contents: write`). Crea el borrador con los instaladores y los checksums.
   Si ya existe un release con ese tag, falla: borra el borrador y vuelve a correrlo.

## Versión

La versión vive en tres lugares que tienen que coincidir: `version` de `[workspace.package]`
en `Cargo.toml`, `apps/tandem-ui/src-tauri/tauri.conf.json` y `apps/tandem-ui/package.json`.
`scripts/release/version.sh [v<versión>]` lo comprueba y, con un tag, exige que sea
exactamente `v` + esa versión. El workflow lo corre siempre, así que un tag que no coincide
con la versión falla antes de compilar nada.

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
Las notas son un punto de partida: se pueden editar en el borrador antes de publicarlo.

## Sacar un release

1. En `develop`, subir la versión en los tres archivos (y `Cargo.lock` con `cargo check`) y
   integrarla como cualquier cambio.
2. Correr el workflow a mano en modo prueba sobre esa rama y revisar el artefacto
   `tandem-<versión>` (instalar los dos en el equipo real).
3. Llevar el cambio a `main` (lo hace el usuario) y empujar el tag:
   ```sh
   git switch main && git pull
   git tag -s v0.1.0 -m "Tandem 0.1.0"
   git push origin v0.1.0
   ```
4. Revisar el borrador en *Releases* (notas, instaladores, `SHA256SUMS.txt`) y publicarlo.

Para comprobar una descarga: `sha256sum -c SHA256SUMS.txt --ignore-missing` (en macOS,
`shasum -a 256 -c SHA256SUMS.txt`; en Windows, `Get-FileHash` y comparar).

## Firma

Hoy los instaladores del release salen como los del CI: el `.dmg` con firma ad-hoc y el
instalador de Windows sin Authenticode. Las dos firmas reales se activan con variables de
entorno (Apple: `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`...; Windows:
`TANDEM_WINDOWS_SIGN_COMMAND` o `TANDEM_WINDOWS_CERT_THUMBPRINT`), descritas en
[install-macos.md](install-macos.md#firma) e [install-windows.md](install-windows.md). Para
usarlas en el release: guardarlas como secretos del repo y pasarlas como `env:` al paso de
construcción de cada job. No se pasan vacías a propósito: el bundler de Tauri podría tomar
una `APPLE_SIGNING_IDENTITY` vacía como identidad en vez de usar la ad-hoc.

## Pendiente

- Binario universal de macOS (hoy `aarch64`, la arquitectura del runner `macos-latest`).
- Firma Developer ID / Authenticode en el release (decisión del usuario, ver arriba).
