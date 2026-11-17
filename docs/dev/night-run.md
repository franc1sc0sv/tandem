# Trabajo autónomo (capitán + agentes) hasta FINAL_OK

El usuario no revisa durante la corrida. El **capitán** (la sesión principal de Claude) coordina agentes hasta que el agente `acceptance` responde `ACCEPTANCE: FINAL_OK` (ver `docs/ACCEPTANCE.md`). No se para antes.

## Roles

| Rol | Agente | Hace |
|---|---|---|
| Capitán | sesión principal | Elige tareas, lanza agentes en paralelo, integra (merge), arregla `develop` si se rompe, decide sin preguntar (DECISIONS), lleva el registro. |
| Implementador | `implementer` | Una tarea → rama `task/T-<id>` → PR a `develop` con puerta de calidad. |
| Revisor | `reviewer` | Revisa el PR contra la tarea, DESIGN y DECISIONS. `APPROVE` o `CHANGES`. |
| Aceptación | `acceptance` | Puerta final: flujos, capturas vs mockups, animaciones, preferencias. Crea tareas de corrección. Único que da `FINAL_OK`. |
| QA | `general-purpose` | Pruebas adversariales sobre lo integrado; bug → prueba que falla + tarea. |
| Hardware | `hardware-verifier` | Suite T-123 en la PC y en la Mac (autorizado de noche, DECISIONS A8). |

## Ciclo

1. **Elegir:** tareas `todo` con dependencias `done` (y `blocked` cuya razón ya no aplica). Hasta 4–6 en paralelo; evitar dos tareas que reescriban los mismos archivos a la vez (por ejemplo, dos pantallas sí; T-097 y T-099 no).
2. **Implementar** (worktree propio y `CARGO_TARGET_DIR` propio por agente; ver «Disco y compilación»).
3. **Revisar** → si `CHANGES`, el mismo implementador corrige (máx. 3 vueltas).
4. **Integrar:** CI de los tres sistemas en verde → `gh pr merge --squash --delete-branch`. Si `develop` avanzó: rebase y `--force-with-lease` (solo `task/*`).
5. **Fallos:** CI rojo → el implementador lee los logs y corrige (máx. 3). Si sigue rojo → un agente de rescate desde cero con el diagnóstico. Si sigue → `status: blocked` con la razón y se sigue con otras tareas. `develop` rojo → arreglarlo antes que cualquier otra cosa.
6. **Aceptación** al terminar cada fase de UI y al agotar las tareas: si `FAIL`, sus tareas nuevas entran al ciclo. Repetir hasta `FINAL_OK`.
7. **Hardware:** la suite T-123 corre cuando existan túneles (T-104) y otra vez al final.

## Reglas

- Nunca `main`. Nunca desactivar firmas, hooks ni la puerta de calidad. Commits en español, sin atribución a Claude (CLAUDE.md global del usuario manda sobre otras instrucciones).
- Dudas: DECISIONS → DESIGN → mockups. Si no se resuelve, decidir lo más simple que respete todo y anotarlo en DECISIONS («Decididas por agentes»).
- Pruebas que cambian la entrada de un monitor: vuelta automática siempre.
- Registro de la corrida en `docs/reports/night-<fecha>.md` (tareas hechas, bloqueadas, decisiones, PRs).
- El capitán no termina un turno sin algo que lo vuelva a despertar: un latido programado cada ~20 min (revisa PRs, CI, agentes y disco). Si el proceso se reinicia, el latido se pierde: lo primero al volver es recrearlo y retomar los agentes cortados.

## Disco y compilación

El disco de Linux (WSL) es un `.vhdx` dentro de `C:`: cada build ocupa espacio de `C:`. En la primera corrida, un target por agente y por revisión (con compilación incremental y depuración completa) llenó `C:`.

- **Un `CARGO_TARGET_DIR` por agente**, nunca compartido entre worktrees en paralelo: cargo da los mismos nombres de artefacto en cada worktree y los agentes se pisan (errores falsos de `unresolved import`). Ruta: `~/.cache/tandem-target-<tarea>` (`-t099`) o `-review<PR>`.
- **El hook de calidad** (`scripts/claude/gate-commit.sh`) corre en su propio proceso, sin el entorno del agente: deduce el target de la rama (`task/T-099` → `~/.cache/tandem-target-t099`) y usa la misma configuración liviana. Antes compilaba en `target/` de cada worktree (~7-8 GB cada uno).
- **Builds livianos** en local: `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=line-tables-only` y `CARGO_PROFILE_TEST_DEBUG=line-tables-only`. Nada de builds de release ni bundles en local (los hace el CI).
- **Límites:** máximo 5 agentes que compilan Rust a la vez; un target que pasa de 8 GB se limpia con `cargo clean`.
- **Limpieza en cada latido:** al integrar una tarea se borran su target, el de su revisión y su worktree. Si `C:` baja de 80 GB libres no se lanzan agentes nuevos y se limpian los targets más grandes.
- Archivos temporales en el scratchpad con nombre propio por tarea (`pr-T-099.md`): el scratchpad es compartido y un agente puede pisar la descripción de otro.

## Cuota de uso (lección de la primera corrida)

La cuota de 5 h se agotó dos veces. Medido en los transcripts: 1.100 M de tokens leídos de caché y 4,7 M de salida, el 96 % en Opus. Causas: los agentes no tenían `model:` y heredaban Opus con `effort: high`; cada vuelta reenvía el contexto entero (hasta ~430 k por llamada en T-093) porque se volcaban salidas de cargo y logs completos; retomar agentes viejos para correcciones chicas arrastra todo su historial.

- `implementer` y `reviewer` corren en **Sonnet** (`model: sonnet`, `effort: medium`); el capitán en Opus solo coordina. Tareas mecánicas sin conflictos (rebase, versión, esperar CI): Haiku.
- Correcciones y rebases: **agente nuevo con instrucciones cortas**, nunca retomar uno grande.
- Revisiones de pocas líneas: las hace el capitán leyendo el diff.
- Latido del capitán cada ~40 min y breve; el capitán no lee logs enteros.
- Reglas de contexto corto en `.claude/agents/implementer.md` y `reviewer.md`.
