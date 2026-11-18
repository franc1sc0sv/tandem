# Glossary

The repository is entirely in English (see `CLAUDE.md`, "Language"). This file fixes the terms so the UI, the mockups, the code and the docs say the same thing. When translating older Spanish text, use these terms; for anything not listed, pick the plain, short English word a desktop app would use and stay consistent within your change.

## Domain terms

| Spanish (old) | English | Notes |
|---|---|---|
| puesto | desk | The pair of machines working as one. "Mi puesto" → "My desk". |
| máquina | machine | Never "host" or "computer" in the UI. |
| la otra (máquina) | the other machine | In code: `peer`. |
| acomodo | arrangement | Screen layout of both machines. Screen title: "Arrangement". |
| escena | scene | |
| túnel / túneles | tunnel / tunnels | "1 tunnel", "2 tunnels". |
| cómputo remoto | remote compute | |
| entorno | environment | WSL, zsh, PowerShell… |
| enlace | link | USB4, Ethernet, Wi-Fi. |
| red | network | |
| emparejar / emparejamiento | pair / pairing | "Pair the other machine". |
| código de emparejamiento | pairing code | |
| llave / credencial | key / credential | |
| salir del puesto | leave desk | "Leave desk". |
| cruce / cruzar | crossing / cross | The cursor moving to the other machine. |
| probar cruce / prueba de cruce | test crossing / crossing test | Button: "Test crossing". |
| borde | edge | |
| teclado y mouse | keyboard and mouse | |
| actividad detectada | activity detected | |
| bandeja | tray | |
| paleta (de comandos) | command palette | |
| asistente (de configuración) | setup | UI: "Setup"; code keeps `wizard`. |
| ajustes | settings | |
| energía | power | Power on / power off / Wake-on-LAN. |
| encender / apagar | power on / power off | |
| módulos | modules | |
| monitores | monitors | |
| entrada (de un monitor) | input | |
| portapapeles | clipboard | |
| archivos / arrastre | files / drag | |
| bloquear / bloqueado | lock / locked | |
| avisos | notices | |
| transacción / vuelta atrás | transaction / rollback | |
| estados / estados difíciles | states / trouble states | |
| permisos | permissions | |
| progreso | progress | |
| actualizaciones | updates | |
| atajos | shortcuts | |
| agente | agent | `tandemd`. |
| no disponible | unavailable | |
| apagado (módulo) | off | |

## Docs and tasks

| Spanish (old) | English |
|---|---|
| Contexto | Context |
| Criterios de aceptación | Acceptance criteria |
| Corrección esperada | Expected fix |
| Notas | Notes |
| Verificación en hardware pendiente | Pending hardware verification |
| Tareas abiertas | Open tasks |
| PASA / FALLA | PASS / FAIL |
| capturas | screenshots |
| similitud | similarity |
| peor cuadro | worst tile |
| máscara | mask |

## File and job renames

Renames happen in the task that owns the directory (see `docs/tasks/T-200.md`); every reference is updated in the same PR.

| Old | New |
|---|---|
| `src/screens/Acomodo.svelte`, `Ajustes`, `Escena`, `Estados`, `Maquina`, `Monitores`, `Progreso`, `Puesto`, `Red`, `Tuneles` | `Arrangement.svelte`, `Settings`, `Scene`, `States`, `Machine`, `Monitors`, `Progress`, `Desk`, `Network`, `Tunnels` |
| `src/lib/acomodo.ts`, `puesto.ts`, `atajos.ts` | `arrangement.ts`, `desk.ts`, `hotkeys.ts` (it must not clash with `shortcuts.ts`) |
| `mockups/v03/V3-Acomodo`, `V3-Asistente`, `V3-Bandeja`, `V3-Computo`, `V3-Estados`, `V3-Maquina`, `V3-Monitores`, `V3-Puesto`, `V3-Red` | `V3-Arrangement`, `V3-Setup`, `V3-Tray`, `V3-Compute`, `V3-States`, `V3-Machine`, `V3-Monitors`, `V3-Desk`, `V3-Network` |
| `mockups/v03/F-AgregarHost`, `F-Ajustes`, `F-Asis1…6`, `F-Avisos`, `F-MaquinaMac`, `F-NuevaEscena`, `F-Paleta`, `F-PruebaCruce`, `F-Transacciones` | `F-AddHost`, `F-Settings`, `F-Setup1…6`, `F-Notices`, `F-MachineMac`, `F-NewScene`, `F-Palette`, `F-CrossingTest`, `F-Transactions` (`F-ClaudeMCP` stays) |
| `docs/screenshots/v03/similitud.json` and Spanish screenshot names | `similarity.json` and English names |
| `scripts/ci/privacidad.sh`, `tareas.sh` | `privacy.sh`, `tasks.sh` |
| CI jobs `ui (capturas)`, `cobertura`, `instalador (windows)`, `preparar` | `ui (screenshots)`, `coverage`, `installer (windows)`, `prepare` |
