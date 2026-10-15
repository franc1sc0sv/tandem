---
paths:
  - "crates/**/windows/**"
  - "crates/**/macos/**"
  - "crates/**/windows.rs"
  - "crates/**/macos.rs"
---
Código de plataforma:
- Todo bajo `#[cfg(...)]`; la lógica que se pueda, en código portable con pruebas en Linux.
- FFI en funciones pequeñas con `// SAFETY:`.
- Windows: verifica con `cargo check --workspace --target x86_64-pc-windows-gnu`. macOS: lo verifica el CI (`macos-latest`).
- Pruebas con dispositivos: `#[ignore = "hardware"]`.
- Referencias permitidas para leer: `docs/research/input-engine.md`, `docs/research/ddc-34-LC34G55T.md`. No copiar código GPL.
