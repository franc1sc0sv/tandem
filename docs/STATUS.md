# Tandem — Estado del proyecto

> Corte de **v0.3**: 2026-11-18 · rama `develop` en `509727a` · el CI de `develop` pasa en Linux, Windows y macOS salvo pruebas inestables conocidas (T-160).
> Diseño en `DESIGN.md` (v0.3) y `DECISIONS.md`; definición de terminado en `ACCEPTANCE.md`; trabajo autónomo en `dev/night-run.md`; informes de aceptación en `reports/`.

## En una línea

**Todo lo que se puede construir y probar sin tu hardware está hecho; lo que falta es probarlo en tu Mac, tu PC y tus monitores reales** (la suite `scripts/hw/run.sh`, T-123, está lista para correrla) y cerrar las fallas que halló la aceptación de hoy.

| | |
|---|---|
| Tareas | **92 hechas de 111** (`T-001`…`T-165`; ver abajo las abiertas) |
| Pruebas | **1442 de Rust + 619 de la UI (vitest)**, 0 fallos, 11 ignoradas (`#[ignore = "hardware"]` y bancos de latencia) |
| Calidad | `fmt`, `clippy -D warnings`, `svelte-check` y `pnpm build` sin avisos |
| CI por PR | Linux · Windows · macOS · UI x3 · cobertura · `.dmg` · instalador de Windows |

## Qué hay

| Área | Estado |
|---|---|
| **Máquinas** | Dos máquinas simétricas, `Machine::{A, B}` (A1), con su sistema, nombre y color en la config (`MachineInfo`); no hay «Mac» y «PC» fijos (D2) |
| **Red** | QUIC cifrado con autenticación mutua; enlaces USB4 → Ethernet → Wi-Fi con failover e histéresis. **Los datos también pueden ir por Wi-Fi** (A2): túneles y archivos en streams de baja prioridad y con tope, para no meter latencia a la entrada. mDNS y emparejamiento con código de 6 dígitos |
| **Agente `tandemd`** | Orquestador de escenas, energía, entrada, portapapeles, túneles, archivos y roles; probado con dos agentes reales en `tandem-sim` |
| **Escenas** | Sustituyen a los modos (D6): una escena guarda qué muestra cada monitor y el acomodo, y se aplica como transacción con vuelta atrás |
| **Monitores** | Un solo estándar (D11): se controla el monitor que habla el protocolo del firmware propio; si no, aparece «no compatible». Lectura de EDID y cables por entrada (`[[cables]]`) |
| **Teclado y mouse** | Motor propio, sin dueño (D7): funcionan desde cualquier máquina y Tandem sigue a la que se usa (arbitraje por mensajes entre agentes). Cursor en vivo con suscripción (hasta 30 Hz) |
| **Portapapeles, archivos, túneles** | Texto, HTML e imagen sin eco; arrastre de archivos; túneles de puertos |
| **Energía** | Wake-on-LAN, apagado limpio, detección de corte de corriente |
| **UI** | Mockup funcionando (T-115) en modo real y demo, barra de menú, bandeja, atajos, capturas de paridad en `docs/screenshots/v03/` |
| **Pruebas en hardware** | Suite `crates/tandem-hw` + `scripts/hw/run.sh` (T-123); las pruebas que cambian la entrada de un monitor son opt-in (`--with-monitors`) y llevan vuelta automática (A8) |
| **Instalación** | `.dmg` de macOS con LaunchAgent; instalador de Windows; release en CI con un tag `v*` (borrador) |

## Tareas abiertas

| Tarea | Estado | Qué |
|---|---|---|
| T-027 | todo (`windows-hw`, `user`) | Servicio SYSTEM de Windows para UAC; necesita decidir certificado Authenticode |
| T-088, T-095, T-096 | blocked | Monitores ignorados, firmware propio del 34" y UI viva |
| T-124 | todo | Aceptación de punta a punta |
| T-140…T-147, T-160, T-161 | todo | Fallas de las partes A y B de la aceptación (demo, textos, capturas, D1) y pruebas inestables en CI |
| T-162…T-165 | done en este corte | Datos personales fuera del repo, documentación al día, criterios de tareas y comentarios `SAFETY` |

## Verificación en hardware pendiente

Cada tarea con hardware lleva su lista en `docs/tasks/T-xxx.md`, sección `## Verificación en hardware pendiente`. Tareas con ítems abiertos:

- **Teclado y mouse:** T-022, T-023, T-024, T-025, T-074, T-093 (captura e inyección en macOS/Windows, distribución ES, scroll, bloqueo).
- **Pantallas y monitores:** T-010, T-011, T-012, T-013, T-014, T-015, T-104, T-109, T-110, T-111, T-121 (CGS/CCD en la Mac, DDC/CI, entradas con vuelta automática).
- **Red y energía:** T-006, T-030, T-070, T-071, T-078, T-080, T-081, T-082 (mDNS, firewall, cables, Wake-on-LAN, instaladores).
- **Roles, portapapeles, UI:** T-028, T-029, T-087, T-089, T-090, T-092, T-094, T-105, T-108, T-122, T-131.
- **Suite completa:** T-123 (`scripts/hw/run.sh` de punta a punta y marcar los ítems cubiertos).

## Riesgos y limitaciones conocidas

- **Casi nada se probó aún en hardware real.** Todo pasa con dobles de prueba, el simulador del puesto y CI; lo físico (APIs de macOS, hooks de Windows, DDC/CI, distribución de teclado) solo se confirma en tus equipos.
- **Pruebas inestables en CI** (macOS y Ubuntu, T-160): no son regresiones, pero hacen rojo el CI al azar.
- **APIs privadas de macOS** (`SetsCursorInBackground`, `CGSConfigureDisplayEnabled`): pueden cambiar entre versiones.
- **Firma:** macOS con firma ad-hoc (los permisos TCC pueden volver a pedirse al reinstalar); Windows sin Authenticode (SmartScreen avisa).
- **Windows sin el servicio SYSTEM (T-027):** el mouse remoto no funciona en UAC ni en el inicio de sesión.
- **Corte de corriente durante un cambio que usa a la otra máquina:** espera hasta ~130 s antes de marcarla apagada (T-063).
- **Monitores:** el cambio de entrada depende del firmware propio del 34" (T-095, bloqueada); las pruebas que cambian entradas exigen vuelta automática porque el usuario puede quedarse sin ver una máquina.

## Qué falta para pasar a `main`

Cerrar las fallas abiertas de la aceptación, correr la suite de hardware (T-123) y revisar `develop`; el PR a `main` lo abre el usuario.
