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
2. **Implementar** (worktree propio, `CARGO_TARGET_DIR` compartido).
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
