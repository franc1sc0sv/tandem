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
- `needs`: `none` (se puede hacer sin hardware), `windows-hw`, `mac-hw`, `user` (requiere al usuario). Las tareas con algo distinto de `none` no se toman en lotes automáticos.
- Una tarea = una rama `task/T-<id>` = un PR en borrador. Nunca commit a `main`, nunca merge, nunca force-push.
- En tu PR, pon `status: done` en el archivo de la tarea y marca los criterios cumplidos; el merge del usuario la cierra.
- Si no puedes cumplir un criterio, `status: blocked` con la razón en `## Notas`.
- Solo edita el archivo de tu tarea, no los de otras.

## Calidad (obligatorio antes de commit o PR; un hook lo exige)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Para código de Windows además: `cargo check --workspace --target x86_64-pc-windows-gnu`.

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
