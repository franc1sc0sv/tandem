export const meta = {
  name: 'next-tasks',
  description: 'Toma las siguientes tareas listas del backlog, las implementa en worktrees aislados, las revisa y deja PRs en borrador',
  whenToUse: 'Para avanzar el backlog de Tandem sin supervisión. Args: { n: número de tareas (por defecto 2), ids: ["T-002"] opcional }',
  phases: [
    { title: 'Elegir', detail: 'tareas todo, needs none, dependencias en main, sin PR abierto' },
    { title: 'Implementar', detail: 'un implementer por tarea, en su worktree' },
    { title: 'Revisar', detail: 'reviewer contra diseño y criterios' },
    { title: 'Corregir', detail: 'una ronda de correcciones si la revisión pide cambios' },
  ],
}

// Cada build de cargo en su worktree usa varios GB; con 24 GB en WSL, 2 a la vez es lo seguro.
const N = Math.min((args && args.n) || 2, 3)
const FORCED = (args && args.ids) || null
// Modo portable: para sesiones abiertas fuera de apps/tandem, donde los agentes del proyecto
// no están registrados. Usa el agente genérico y le pide seguir el archivo del agente.
const PORTABLE = !!(args && args.portable)
const REPO = '~/projects/personal-projects/apps/tandem'
function role(name, task) {
  if (!PORTABLE) return { agentType: name, isolation: name === 'implementer' ? 'worktree' : undefined, pre: '' }
  const wt = name === 'implementer'
    ? `Trabaja en un worktree propio: cd ${REPO} && git fetch origin && (git worktree add .claude/worktrees/${task.id} task/${task.id} 2>/dev/null || git worktree add -b task/${task.id} .claude/worktrees/${task.id} origin/main), y haz todo dentro de ${REPO}/.claude/worktrees/${task.id}. No toques el checkout principal. `
    : `Trabaja dentro de ${REPO}. `
  return { agentType: 'general-purpose', isolation: undefined, pre: `Lee ${REPO}/CLAUDE.md y ${REPO}/.claude/agents/${name}.md y sigue esas instrucciones como si fueras ese agente. ${wt}` }
}

const PICK = {
  type: 'object',
  required: ['tasks', 'skipped'],
  properties: {
    tasks: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'title'],
        properties: { id: { type: 'string' }, title: { type: 'string' } },
      },
    },
    skipped: { type: 'array', items: { type: 'string' }, description: 'id: razón, para tareas todo que no se tomaron' },
  },
}

const IMPL = {
  type: 'object',
  required: ['status', 'summary'],
  properties: {
    status: { type: 'string', enum: ['pr_opened', 'blocked', 'failed'] },
    pr_number: { type: 'integer' },
    pr_url: { type: 'string' },
    summary: { type: 'string' },
  },
}

const REVIEW = {
  type: 'object',
  required: ['verdict', 'findings'],
  properties: {
    verdict: { type: 'string', enum: ['APPROVE', 'CHANGES'] },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['where', 'problem', 'fix'],
        properties: { where: { type: 'string' }, problem: { type: 'string' }, fix: { type: 'string' } },
      },
    },
  },
}

phase('Elegir')
const pickPrompt = `Trabaja dentro de ${REPO}. ` + (FORCED
  ? `Verifica que estas tareas existen en docs/tasks/ en origin/main (haz git fetch origin primero) y devuélvelas con su título: ${FORCED.join(', ')}. No apliques filtros.`
  : `Haz git fetch origin. Lee los archivos docs/tasks/T-*.md tal como están en origin/main (git show origin/main:docs/tasks/<archivo>).
Una tarea está lista si: status es todo; needs es exactamente none; cada id en depends_on tiene status done en origin/main; y no hay un PR abierto para la rama task/<id> (gh pr list --state open --head task/<id>).
Devuelve hasta ${N} tareas listas, priorizando las de fase más temprana (F0 < F1 < F2 < F3 < F4 < F5) y luego el id menor.
No elijas dos tareas que vayan a crear o modificar el mismo crate a la vez; si chocan, deja la segunda en skipped.
En skipped, lista las tareas todo que no tomaste con la razón corta.`)
const picked = await agent(pickPrompt, { label: 'elegir tareas', phase: 'Elegir', schema: PICK, effort: 'low' })

if (!picked || !picked.tasks.length) {
  log('No hay tareas listas para trabajar sin hardware ni usuario.')
  return { done: [], skipped: picked ? picked.skipped : [] }
}
log(`Tareas: ${picked.tasks.map(t => t.id).join(', ')}`)

const results = await pipeline(
  picked.tasks,

  task => {
    const r = role('implementer', task)
    return agent(
      `${r.pre}Implementa la tarea ${task.id} ("${task.title}") de docs/tasks/${task.id}.md siguiendo tus instrucciones de agente. Al final reporta el número y URL del PR.`,
      { label: `implementar ${task.id}`, phase: 'Implementar', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
    )
  },

  (impl, task) => {
    if (!impl || impl.status !== 'pr_opened' || !impl.pr_number) return { task, impl, review: null }
    const r = role('reviewer', task)
    return agent(
      `${r.pre}Revisa el PR #${impl.pr_number} de la tarea ${task.id} siguiendo tus instrucciones de agente.`,
      { label: `revisar ${task.id}`, phase: 'Revisar', agentType: r.agentType, schema: REVIEW },
    ).then(review => ({ task, impl, review }))
  },

  r => {
    if (!r || !r.review || r.review.verdict !== 'CHANGES') return r
    const list = r.review.findings.map((f, i) => `${i + 1}. ${f.where}: ${f.problem} → ${f.fix}`).join('\n')
    const ro = role('implementer', r.task)
    return agent(
      `${ro.pre}El PR #${r.impl.pr_number} (tarea ${r.task.id}, rama task/${r.task.id}) recibió estos hallazgos de revisión:\n${list}\n\nHaz git fetch origin y git switch task/${r.task.id} (sigue la rama existente, no crees otra). Corrige cada hallazgo, pasa la puerta de calidad, haz commit y push a la misma rama. Si un hallazgo es incorrecto, explícalo con un comentario en el PR en vez de cambiar el código.`,
      { label: `corregir ${r.task.id}`, phase: 'Corregir', agentType: ro.agentType, isolation: ro.isolation, schema: IMPL },
    ).then(fix => ({ ...r, fix }))
  },
)

const summary = results.filter(Boolean).map(r => ({
  id: r.task.id,
  status: r.impl ? r.impl.status : 'failed',
  pr: r.impl && r.impl.pr_url,
  review: r.review ? r.review.verdict : null,
  fixed: r.fix ? r.fix.status : null,
  notes: r.fix ? r.fix.summary : r.impl && r.impl.summary,
}))
const dropped = picked.tasks.length - summary.length
if (dropped) log(`${dropped} tarea(s) se cayeron por un error del agente; revisa /workflows.`)
return { done: summary, skipped: picked.skipped }
