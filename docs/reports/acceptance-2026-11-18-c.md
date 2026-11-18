# Aceptación de Tandem v0.3, parte C: calidad base, decisiones y backlog

Fecha: 2026-11-18 · base: `origin/develop` 04f2967 (T-131) · worktree detached `acceptance-c` · rama `qa/acceptance-2026-11-18-c`.
Alcance: calidad base, reglas G1-G11 no visuales, coherencia documental, plan v0.3 (T-093 a T-131), ítems de hardware y seguridad. La paridad visual (parte A) y los flujos con UI (parte B) no se tocaron aquí.

**Veredicto de la parte C: FALLA (6 fallas, tareas T-160 a T-165).**

## 1. Calidad base (G11)

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all -- --check` | PASA |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASA |
| `cargo test --workspace`, corrida 1 | PASA: 1442 pruebas, 0 fallos, 11 ignoradas |
| `cargo test --workspace`, corrida 2 | PASA: 1442 pruebas, 0 fallos, 11 ignoradas |
| Pruebas inestables en local | Ninguna entre las dos corridas |
| `pnpm test` (vitest) | PASA: 46 archivos, 619 pruebas |
| `pnpm check` (svelte-check `--fail-on-warnings`) | PASA: 513 archivos, 0 errores, 0 avisos |
| `pnpm build` | PASA |
| CI de `develop`, últimas 30 corridas | 23 verdes y 5 rojas al empezar, más 36c4907 que terminó en rojo (6 rojas); 04f2967 terminó en verde |

Las ignoradas son las `#[ignore = "hardware"]` y los bancos de latencia.

**CI: FALLA (T-160).** Todos los fallos son pruebas inestables, no regresiones; las tres pasan en local:

| Prueba | Job | Corridas rojas |
|---|---|---|
| `tandem-input/tests/session.rs::la_latencia_incluye_la_espera_con_el_driver_ocupado` | macos | 72ca3bc, 5703559 |
| `tandemd/tests/pairing.rs::pairing_hot_opens_the_input_session` | macos | 59dd296, f327049 |
| `tandem-cli/tests/e2e.rs::un_cliente_mcp_abre_un_tunel_ejecuta_lee_copia_y_cierra` | ubuntu, macos, cobertura | e551446, 36c4907 |

La corrida de 04f2967 (el commit evaluado) terminó en verde en Linux, Windows y macOS, además de UI x3, cobertura, `.dmg` e instalador. Pero la corrida del commit anterior (36c4907) quedó roja por la prueba del MCP, y con tres pruebas que fallan al azar en CI el verde de un commit depende de la suerte, así que G11 no se da por cumplida hasta T-160.

## 2. Reglas G1-G11 no visuales y coherencia documental

| Regla | Resultado | Evidencia |
|---|---|---|
| G1 dos máquinas | FALLA | `states.ts` (estado «Primera vez») dibuja el botón «Agregar máquina» y el texto «Agrega otra máquina»; D1 los excluye. T-161 |
| G2 nada fijo a Mac/PC | PASA | `Machine::{A,B}`; pruebas con pares del mismo sistema en `tandem-sim/tests/power.rs`, `tandemd/tests/input.rs`, `tandem-env`; en la UI el ícono sale del sistema (`machine.test.ts`) |
| G3 barra lateral | no evaluada aquí (visual, parte A); `shell.test.ts` la prueba |
| G4 botones, chips | parte A |
| G5 sin relleno ni «Teclado aquí» / «requiere cable» | FALLA solo por el texto de G1; `git grep` no halla «Teclado aquí» ni «requiere cable» en `apps/` ni `crates/` |
| G6 todo hace algo real | parte B |
| G7, G8 animaciones | parte A; `prefers-reduced-motion` aparece en 17 archivos de la UI y en `styles/global.css` |
| G9 sin errores de consola | parte B (el arnés `shots.ts` comprueba errores de consola) |
| G10 configuración compartida y «Deshacer» | cubierta en el agente por `tandemd/tests/network.rs` (`UndoDesk`, `undo_to`); la parte con UI es de la parte B |
| G11 calidad | FALLA por la CI inestable (T-160) |

Coherencia documental (DECISIONS, DESIGN, ACCEPTANCE y tareas):

- DECISIONS y DESIGN no se contradicen entre sí en lo que revisé (A1, A2, A6, A7, D1-D20 y las notas de los agentes concuerdan; T-098 «solo teclado y portapapeles se apagan» fue ampliado por T-115 con el módulo `tunnels`, y T-098 lo documenta como de ese momento).
- FALLA (T-163): `docs/STATUS.md` es el corte de v0.2 y contradice A2, D7, D11 y D2; `docs/TESTING.md:50-55` usa `Machine::Mac/Pc` y `set_mode` (contra A1 y D6); `docs/dev/hardware.md:7` dice «ninguna prueba cambia entradas» (contra A8, T-110 y T-123).
- FALLA (T-164): tareas `done` con criterios sin marcar o fuera de protocolo: T-051, T-115, T-123 (dos criterios que son de hardware), T-128 (encabezado `## Verificación pendiente` y ítem no de hardware), T-012 y T-022.

## 3. Plan v0.3 (T-093 a T-131)

Los 39 archivos existen. 36 están `done`. Las demás:

| Tarea | Estado | Justificación |
|---|---|---|
| T-095 | blocked | Reemplazada por T-109, T-110 y T-111 (nota del 2026-11-16) |
| T-096 | blocked | Reemplazada por T-112 a T-120 |
| T-124 | todo | Es esta aceptación (partes A, B y C); se cierra con `FINAL_OK` |

T-088 (fuera del rango) también está reemplazada (T-110). Ninguna tarea `done` depende de una `blocked`. Salvo lo de T-164, no hay tarea `done` de T-093 a T-131 con criterios sin cumplir fuera de su lista de hardware.

Listas «Verificación en hardware pendiente»: las de T-093, T-094, T-096, T-104, T-105, T-108, T-109, T-110, T-111, T-121, T-122, T-123 y T-131 son concretas (qué comando, qué resultado, en qué máquina). La de T-128 no lo es (T-164).

## 4. Ítems de hardware

| Ítem | Estado | Dónde está su comprobación |
|---|---|---|
| H1 emparejar por Wi-Fi | pendiente de hardware | T-123, primer ítem (`scripts/hw/run.sh`, empareja con identidades de prueba); T-093 |
| H2 túneles PC→Mac y Mac→PC | pendiente de hardware | T-104 (WSL y PowerShell desde la Mac), T-108, T-123 |
| H3 34" con vuelta automática | pendiente de hardware | T-109, T-110 (matar la prueba a mitad), T-123 con `--with-monitors` |
| H4 cruce real con entrada sintética | pendiente de hardware | T-123 («`SendInput`/`CGEventPost` sintéticos crucen de verdad») y T-093 |
| H5 USB-C | pendiente de hardware | T-094 (spike S5 y cable TB4/USB4, A6) |

Cuentan como pendientes, no como falla: cada uno tiene su comprobación concreta. La suite T-123 todavía no corrió en el equipo real (solo `--sim`, 12/12); por eso ningún ítem `[hw]` está verificado con fecha.

## 5. Seguridad

| Comprobación | Resultado |
|---|---|
| Secretos, tokens y llaves | PASA: ninguno versionado; los `ghp_x` y `BEGIN OPENSSH PRIVATE KEY` son fixtures del filtro de credenciales (T-130); no hay `.env`, `.pem` ni identidades en `git ls-files` |
| Datos del usuario | FALLA (T-162): el repo es público y trae la IP de la Mac (`192.168.1.191`) y su usuario de macOS (`franciscohernandez`) en `docs/dev/hardware.md`, `docs/tasks/T-051.md` y `docs/dev/HANDOFF.md`; MAC y ruta `/Users/fran` en fixtures |
| `unsafe` con `// SAFETY:` | FALLA (T-165): de 162 usos, 155 lo llevan; faltan 7 (`extern` de IOKit, ApplicationServices y Carbon, `tap_callback`, `unsafe impl` de `drag_macos.rs`) |
| Código GPL copiado | PASA, con el límite de una revisión por búsqueda: no hay encabezados GPL ni «copiado de»; las únicas menciones a Deskflow son comentarios de comportamiento («como Deskflow») |

## Tareas de corrección

| Tarea | Falla |
|---|---|
| [T-160](../tasks/T-160.md) | Pruebas inestables en CI (macOS y Ubuntu) |
| [T-161](../tasks/T-161.md) | «Agregar máquina» en el estado «Primera vez» (D1, G1, G5) |
| [T-162](../tasks/T-162.md) | IP de la LAN y usuario de la Mac en el repo público |
| [T-163](../tasks/T-163.md) | STATUS, TESTING y dev/hardware desactualizados |
| [T-164](../tasks/T-164.md) | Criterios sin marcar y encabezados de hardware |
| [T-165](../tasks/T-165.md) | Comentarios SAFETY que faltan |

`ACCEPTANCE: FAIL 6 (tareas: T-160, T-161, T-162, T-163, T-164, T-165)`
