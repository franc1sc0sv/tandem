---
paths:
  - "crates/**/windows/**"
  - "crates/**/macos/**"
  - "crates/**/windows.rs"
  - "crates/**/macos.rs"
---
Platform code:
- Everything under `#[cfg(...)]`; as much logic as possible in portable code with tests on Linux.
- FFI in small functions with `// SAFETY:`.
- Windows: check with `cargo check --workspace --target x86_64-pc-windows-gnu`. macOS: CI checks it (`macos-latest`).
- Tests with devices: `#[ignore = "hardware"]`.
- References allowed for reading: `docs/research/input-engine.md`, `docs/research/ddc-34-LC34G55T.md`. Do not copy GPL code.
