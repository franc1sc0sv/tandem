# Aceptación de Tandem v0.3, parte C, ronda 2: calidad base, decisiones y backlog

Fecha: 2026-11-18 · base: `origin/develop` b3841bc (T-152, T-155, T-159) · rama `qa/acceptance-2026-11-18-c2`.
Alcance: igual que la ronda 1 (`acceptance-2026-11-18-c.md`): calidad base, G1-G11 no visuales, coherencia documental, plan v0.3, hardware y seguridad.

**Veredicto de la parte C: FALLA (4 fallas nuevas, tareas T-190 a T-193).**

## Las 6 fallas de la ronda 1

| Tarea | Resultado |
|---|---|
| T-160 pruebas inestables | PARCIAL: `session.rs` y el e2e MCP no volvieron a fallar, pero `pairing_hot_opens_the_input_session` volvió a fallar en macOS en `develop` tras el arreglo (run 44368941401, 97442ab). Pasa a T-190 |
| T-161 «Agregar máquina» | ARREGLADA en el código (T-142: «Emparejar la otra máquina», `pair-peer`); el archivo `T-161.md` sigue `todo` (T-193) |
| T-162 datos personales | ARREGLADA: ya no hay IP ni usuario de la Mac en el árbol; `scripts/ci/privacidad.sh` corre en CI y pasa |
| T-163 STATUS, TESTING, hardware | PARCIAL: `TESTING.md` y `dev/hardware.md` al día; `STATUS.md` ya quedó viejo otra vez (T-192) |
| T-164 criterios sin marcar | ARREGLADA para las tareas citadas; queda T-150 (T-193) |
| T-165 comentarios SAFETY | ARREGLADA: `scripts/ci/safety.sh` pasa y corre en CI |

## 1. Calidad base (G11)

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all -- --check` | PASA |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASA |
| `cargo test --workspace`, corrida 1 | PASA: 1465 pruebas, 0 fallos, 11 ignoradas |
| `cargo test --workspace`, corrida 2 | PASA: igual |
| `cargo test -p tandemd --test pairing` x6 | PASA 6 de 6 |
| `pnpm test` | PASA: 47 archivos, 667 pruebas |
| `pnpm check` | PASA: 518 archivos, 0 errores, 0 avisos |
| `pnpm build` | PASA |
| `scripts/ci/privacidad.sh`, `scripts/ci/safety.sh` | PASAN |
| CI de `develop` en b3841bc (run 44372328096) | PASA en todos los jobs |
| CI de `develop`, historial desde T-160 | FALLA (T-190, T-191): 97442ab roja en macOS (`pairing_hot_opens_the_input_session`, timeout de 30 s); e86b9c8 roja por un test de la UI (`aceptacion-b.test.ts`, T-158) tras fusionar dos PRs, corregida por la siguiente fusión; el run 44345845261 (4751832) tiene el job `ubuntu-latest` colgado en `cargo test` más de 2 h 20 min y `ci.yml` solo tiene `timeout-minutes` en un job |

## 2. Reglas G1-G11 no visuales y coherencia documental

| Regla | Resultado |
|---|---|
| G1 dos máquinas | PASA: sin «Agregar máquina» ni «Agrega otra máquina» en `apps/` ni `crates/` fuera de comentarios y de las máscaras documentadas |
| G2 nada fijo a Mac/PC | PASA: `Machine::{A,B}`; sin `Machine::Mac/Pc` ni `set_mode` en docs ni código |
| G5 sin relleno | PASA: sin «Teclado aquí» ni «requiere cable» |
| G7, G8, G3, G4, G6, G9 | visuales o con UI: partes A y B |
| G10 | cubierta en el agente por `tandemd/tests/network.rs`; la parte con UI es de la parte B |
| G11 | FALLA por T-190 y T-191 |

Documentos: `DECISIONS`, `DESIGN` y `ACCEPTANCE` coherentes. FALLA T-192: `STATUS.md` dice que T-140…T-147 y T-160 están `todo` (están `done`), «92 hechas de 111» (hay 115 `done` de 121 archivos). FALLA T-193: T-161 `todo` aunque está arreglada; T-150 `done` con un criterio no marcado que no es de hardware.

## 3. Plan v0.3 (T-093 a T-131)

Sin cambios respecto a la ronda 1: 36 de 39 `done`; T-095 y T-096 `blocked` (reemplazadas), T-124 `todo` (esta aceptación). Las listas «Verificación en hardware pendiente» son concretas.

## 4. Hardware

H1 a H5: pendientes de hardware, cada uno con su comprobación concreta (T-123, T-093, T-094, T-104, T-108, T-109, T-110); no cuentan como falla. La suite T-123 aún no corrió en el equipo real.

## 5. Seguridad

Secretos: ninguno versionado. Datos del usuario: PASA (T-162). `unsafe` con `// SAFETY:`: PASA (`safety.sh`). Código GPL copiado: sin hallazgos.

## Tareas de corrección

| Tarea | Falla |
|---|---|
| [T-190](../tasks/T-190.md) | `pairing_hot_opens_the_input_session` sigue fallando en macOS en CI |
| [T-191](../tasks/T-191.md) | CI sin `timeout-minutes`; job de Ubuntu colgado más de 2 h |
| [T-192](../tasks/T-192.md) | `STATUS.md` desactualizado |
| [T-193](../tasks/T-193.md) | T-161 `todo` y T-150 `done` con un criterio no marcado |

`ACCEPTANCE: FAIL`
