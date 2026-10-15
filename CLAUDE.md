# Tandem

Mac + PC Windows como un solo puesto: monitores (DDC/CI), teclado y mouse con motor propio, energía y enlace cifrado propio. Todo en Rust (+ Tauri para la UI).

- **Diseño (fuente de verdad):** `docs/DESIGN.md`. Léelo antes de cualquier cambio no trivial.
- **Investigación:** `docs/research/` (DDC del 34", Deskflow como referencia, motor de entrada).
- **Backlog:** `docs/tasks/T-*.md` (protocolo abajo).
- **Mockup de la UI:** `mockups/main.html`.

Si el código, la tarea y el diseño no coinciden, **detente y repórtalo**; no improvises arquitectura.
El usuario escribe en español: documentación, mensajes de commit y PRs en español.

## Protocolo del backlog

- Cada tarea: `docs/tasks/T-<id>.md` con frontmatter `status: todo | in-progress | blocked | done`, `depends_on`, `needs`.
- `needs`:
  - `none`: se hace sin hardware.
  - `windows-hw` / `mac-hw`: **también se implementa**, con código completo, pruebas con backends simulados y CI en las tres plataformas. Lo que solo se puede comprobar en el equipo real va en una sección `## Verificación en hardware pendiente` (lista con casillas) del archivo de la tarea; la tarea cuenta como `done` aunque esa lista quede pendiente.
  - `user` (requiere una acción física del usuario): no se toma.
- **Rama de integración: `develop`.** Las tareas salen de `origin/develop` y sus PRs apuntan a `develop`. `main` solo la toca el usuario: nunca commit, push, PR ni merge a `main`.
- Una tarea = una rama `task/T-<id>` = un PR hacia `develop`.
- **Merge autónomo a `develop`** (autorizado por el usuario): `gh pr merge <n> --squash --delete-branch`, solo si los tres jobs de CI están en verde y el agente reviewer aprobó. Si `develop` avanzó, rebase sobre `origin/develop` y `git push --force-with-lease origin task/T-<id>` (solo ramas `task/*`).
- En tu PR, pon `status: done` en el archivo de la tarea y marca los criterios cumplidos.
- Si no puedes cumplir un criterio que no sea de hardware, `status: blocked` con la razón en `## Notas`.
- Solo edita el archivo de tu tarea, no los de otras.

## Calidad (obligatorio antes de commit o PR; un hook lo exige)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Para código de Windows, el CI de `windows-latest` es la verificación. `cargo check --target x86_64-pc-windows-gnu` en WSL solo funciona si está instalado `mingw-w64` (crates con C como `ring`); si falta, no es un bloqueo.

## Pruebas (maximizar; ver `docs/TESTING.md`)

- Toda lógica nueva lleva pruebas unitarias. Los invariantes llevan pruebas de propiedades (`proptest`).
- Todo código de plataforma va detrás de un trait con un doble de prueba, para que la lógica se pruebe en Linux.
- Las rutas entre las dos máquinas se prueban con dos agentes dentro del mismo proceso, por loopback (`tandem-sim`).
- Un bug encontrado = primero una prueba que falla, después el arreglo.

## Convenciones de Rust

- Edición 2024. `thiserror` en librerías, `anyhow` solo en binarios. Sin `unwrap()`/`expect()` fuera de pruebas.
- Código de plataforma bajo `#[cfg(windows)]` / `#[cfg(target_os = "macos")]`, detrás de traits definidos en código portable; la lógica se prueba en Linux con implementaciones simuladas.
- Dependencias: declaradas en `[workspace.dependencies]` del `Cargo.toml` raíz.
- `unsafe` solo para FFI, en funciones pequeñas con comentario `// SAFETY:`.
- Comentarios y documentación en español, densidad baja: explica el porqué, no el qué.
- **Prohibido copiar código de lan-mouse o Deskflow (GPL).** Se pueden leer como referencia.

## Hardware

- Pruebas que tocan dispositivos: `#[ignore = "hardware"]`. Nunca en CI ni en tareas rutinarias.
- PC Windows: se accede desde WSL con `powershell.exe` (interop). Mac: `ssh mac-runner` (ver `docs/dev/hardware.md`).
- Cualquier prueba que cambie la entrada de un monitor **debe traer vuelta automática**: el usuario puede quedarse sin ver la PC.

## Git

- Commits firmados automáticamente con SSH. No uses `--no-gpg-sign` ni toques `commit.gpgsign`.
- Mensajes en español, en imperativo, una línea de resumen + cuerpo si hace falta.
