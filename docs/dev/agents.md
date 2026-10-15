# Cómo trabajan los agentes en Tandem

## Piezas

| Pieza | Archivo | Qué hace |
|---|---|---|
| Reglas del proyecto | `CLAUDE.md`, `.claude/rules/` | Se cargan en cada sesión y en cada agente |
| Backlog | `docs/tasks/T-*.md` | Una tarea por archivo, con `status`, `depends_on`, `needs` |
| implementer | `.claude/agents/implementer.md` | Implementa una tarea en su propio worktree y abre un PR en borrador |
| reviewer | `.claude/agents/reviewer.md` | Revisa el PR (solo lectura) y comenta |
| hardware-verifier | `.claude/agents/hardware-verifier.md` | Pruebas con la PC o la Mac; solo si el usuario lo pide |
| Workflow | `.claude/workflows/next-tasks.js` | Elige las tareas listas → implementar → revisar → corregir |
| Puerta de calidad | `scripts/claude/gate-commit.sh` + hook en `.claude/settings.json` | Bloquea `git commit` y `gh pr create` si fallan fmt, clippy o las pruebas |
| CI | `.github/workflows/ci.yml` | Lo mismo en Linux, Windows y macOS para cada PR |

## Ciclo

1. Abre Claude Code **en esta carpeta** (`apps/tandem`) para que se carguen los agentes, el workflow y los permisos.
2. Pide: `corre /next-tasks` (o `/next-tasks` con `{ "n": 3 }`).
3. Cada tarea termina en un **PR en borrador** con la revisión como comentario.
4. Tú revisas y haces merge. **El merge es lo que desbloquea** las tareas que dependen de esa.
5. Repite. Para que siga solo mientras no estás: `claude --bg "corre /next-tasks"` desde una terminal en esta carpeta, o `/loop corre /next-tasks` en una sesión abierta.

## Qué no hacen solos

- Tareas con `needs` distinto de `none` (hardware o usuario): se hacen en una sesión contigo.
- Merge a `main`, force-push, desactivar la firma de commits: prohibido por permisos.

## Paralelismo

Máximo 3 tareas a la vez (por defecto 2). Cada worktree compila su propio `target/` y WSL tiene 24 GB.
