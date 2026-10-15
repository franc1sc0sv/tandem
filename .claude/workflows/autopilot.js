export const meta = {
  name: 'autopilot',
  description: 'Avanza el backlog de Tandem sin supervisión: implementar, revisar, CI y merge a develop, con rondas de QA, hasta agotar las tareas',
  whenToUse: 'Cuando el usuario se va y quiere que el backlog avance solo. Args: { rounds: máx. rondas (20), perRound: tareas por ronda (3), portable: true si la sesión no está abierta en apps/tandem }',
  phases: [
    { title: 'Elegir' },
    { title: 'Implementar' },
    { title: 'Revisar' },
    { title: 'Corregir' },
    { title: 'Integrar', detail: 'esperar CI, rebase si hace falta, merge a develop' },
    { title: 'QA', detail: 'pruebas adversariales sobre lo integrado' },
    { title: 'Reporte' },
  ],
}

const ROUNDS = (args && args.rounds) || 20
const PER_ROUND = Math.min((args && args.perRound) || 3, 3)
const PORTABLE = !!(args && args.portable)
const REPO = '~/projects/personal-projects/apps/tandem'

// En modo portable los agentes del proyecto no están registrados: se usa el genérico
// y se le pide seguir el archivo del agente.
function role(name, branch) {
  if (!PORTABLE) return { agentType: name, isolation: name === 'implementer' ? 'worktree' : undefined, pre: '' }
  const wt = branch
    ? `Trabaja en un worktree propio: cd ${REPO} && git fetch origin && (git worktree add .claude/worktrees/${branch.replace('/', '-')} ${branch} 2>/dev/null || git worktree add -b ${branch} .claude/worktrees/${branch.replace('/', '-')} origin/develop), y haz todo dentro de ese directorio. No toques el checkout principal. `
    : `Trabaja dentro de ${REPO}. `
  return { agentType: 'general-purpose', isolation: undefined, pre: `Lee ${REPO}/CLAUDE.md y ${REPO}/.claude/agents/${name}.md y sigue esas instrucciones como si fueras ese agente. ${wt}` }
}

const PICK = {
  type: 'object',
  required: ['tasks', 'open_prs', 'remaining'],
  properties: {
    tasks: { type: 'array', items: { type: 'object', required: ['id', 'title'], properties: { id: { type: 'string' }, title: { type: 'string' } } } },
    open_prs: { type: 'array', description: 'PRs abiertos hacia develop de ramas task/*', items: { type: 'object', required: ['id', 'pr'], properties: { id: { type: 'string' }, pr: { type: 'integer' } } } },
    remaining: { type: 'array', items: { type: 'string' }, description: 'id: razón, de las tareas todo/blocked que no se tomaron' },
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
    findings: { type: 'array', items: { type: 'object', required: ['where', 'problem', 'fix'], properties: { where: { type: 'string' }, problem: { type: 'string' }, fix: { type: 'string' } } } },
  },
}
const MERGE = {
  type: 'object',
  required: ['merged', 'summary'],
  properties: { merged: { type: 'boolean' }, summary: { type: 'string' } },
}

function implement(task, label) {
  const r = role('implementer', `task/${task.id}`)
  return agent(
    `${r.pre}Implementa la tarea ${task.id} ("${task.title}") de docs/tasks/${task.id}.md siguiendo tus instrucciones de agente. El PR va hacia develop. Al final reporta el número y URL del PR.`,
    { label, phase: 'Implementar', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
  )
}

function review(id, pr) {
  const r = role('reviewer')
  return agent(`${r.pre}Revisa el PR #${pr} (tarea ${id}) siguiendo tus instrucciones de agente.`,
    { label: `revisar ${id}`, phase: 'Revisar', agentType: r.agentType, schema: REVIEW })
}

function fix(id, pr, findings, branch) {
  const r = role('implementer', branch)
  const list = findings.map((f, i) => `${i + 1}. ${f.where}: ${f.problem} → ${f.fix}`).join('\n')
  return agent(
    `${r.pre}El PR #${pr} (rama ${branch}) recibió estos hallazgos:\n${list}\n\nSigue la rama existente ${branch} (no crees otra). Corrige cada hallazgo con una prueba cuando aplique, pasa la puerta de calidad, commit y push a la misma rama. Si un hallazgo es incorrecto, explícalo con un comentario en el PR.`,
    { label: `corregir ${id}`, phase: 'Corregir', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
  )
}

function integrate(id, pr, branch) {
  const r = role('implementer', branch)
  return agent(
    `${r.pre}Integra el PR #${pr} (rama ${branch}) en develop. Pasos:
1. Si GitHub reporta conflictos o la rama está atrasada: git fetch origin, rebase de ${branch} sobre origin/develop, resuelve conflictos conservando ambas intenciones, pasa la puerta de calidad y git push --force-with-lease origin ${branch}.
2. Espera el CI: gh pr checks ${pr} --watch (los tres jobs: ubuntu, windows, macos).
3. Si un job falla: lee el log (gh run view --log-failed), corrige en la rama con una prueba si aplica, push, y vuelve al paso 2. Máximo 3 intentos.
4. Con los tres jobs en verde: gh pr ready ${pr} (si es borrador) y gh pr merge ${pr} --squash --delete-branch.
Nunca toques main. Reporta merged=true solo si el merge ocurrió.`,
    { label: `integrar ${id}`, phase: 'Integrar', agentType: r.agentType, isolation: r.isolation, schema: MERGE },
  )
}

async function carry(id, pr, title, branch = `task/${id}`) {
  let rev = await review(id, pr)
  if (rev && rev.verdict === 'CHANGES') {
    await fix(id, pr, rev.findings, branch)
    rev = await review(id, pr)
    if (rev && rev.verdict === 'CHANGES') {
      await fix(id, pr, rev.findings, branch)
    }
  }
  const m = await integrate(id, pr, branch)
  return { id, title, pr, review: rev ? rev.verdict : null, merged: !!(m && m.merged), notes: m ? m.summary : 'falló la integración' }
}

const history = []
let idle = 0
let sinceQa = 0

for (let round = 1; round <= ROUNDS; round++) {
  phase('Elegir')
  const pick = await agent(
    `Trabaja dentro de ${REPO}. Haz git fetch origin. Lee docs/tasks/T-*.md tal como están en origin/develop (git show origin/develop:docs/tasks/<archivo>).
Una tarea está lista si: status es todo; needs NO contiene "user"; cada id en depends_on tiene status done en origin/develop; y no hay un PR abierto para la rama task/<id> (gh pr list --state open --head task/<id>).
Devuelve en tasks hasta ${PER_ROUND} tareas listas, por fase (F0 < F1 < F2 < F3 < F4 < F5) y luego id. No elijas dos que creen o modifiquen el mismo crate (los crates dicen en su alcance qué tocan); deja la segunda fuera. No elijas T-062 (QA): la corre este workflow aparte.
En open_prs lista los PRs abiertos con base develop cuya rama empiece por task/ (gh pr list --state open --base develop --json number,headRefName).
En remaining, cada tarea todo o blocked no tomada con su razón corta.`,
    { label: `elegir (ronda ${round})`, phase: 'Elegir', schema: PICK, effort: 'low' },
  )
  if (!pick) break

  const adopted = pick.open_prs.slice(0, PER_ROUND)
  const fresh = pick.tasks.slice(0, Math.max(0, PER_ROUND - adopted.length))
  if (!fresh.length && !adopted.length) {
    idle++
    log(`Ronda ${round}: nada listo. Pendientes: ${pick.remaining.length}`)
    if (idle >= 1) break
    continue
  }
  idle = 0
  log(`Ronda ${round}: nuevas ${fresh.map(t => t.id).join(', ') || '—'} · retomadas ${adopted.map(p => p.id).join(', ') || '—'}`)

  const items = [
    ...adopted.map(p => ({ kind: 'adopt', id: p.id, title: p.id, pr: p.pr })),
    ...fresh.map(t => ({ kind: 'new', id: t.id, title: t.title })),
  ]
  const results = await pipeline(
    items,
    it => it.kind === 'adopt' ? { status: 'pr_opened', pr_number: it.pr, summary: 'retomado' } : implement(it, `implementar ${it.id}`),
    (impl, it) => {
      if (!impl || impl.status !== 'pr_opened' || !impl.pr_number) {
        return { id: it.id, title: it.title, pr: null, review: null, merged: false, notes: impl ? impl.summary : 'el agente falló' }
      }
      return carry(it.id, impl.pr_number, it.title)
    },
  )
  const ok = results.filter(Boolean)
  history.push(...ok.map(r => ({ round, ...r })))
  const merged = ok.filter(r => r.merged).length
  log(`Ronda ${round}: ${merged}/${items.length} integradas`)
  sinceQa += merged

  if (sinceQa >= 4) {
    sinceQa = 0
    const qa = await agent(
      `${role('implementer', `qa/round-${round}`).pre}Ronda de QA adversarial (ver docs/tasks/T-062.md y docs/TESTING.md) sobre lo integrado en develop en las últimas rondas: ${ok.filter(r => r.merged).map(r => r.id).join(', ')} y anteriores sin QA. Rama qa/round-${round} desde origin/develop. Busca casos sin cubrir (bordes, errores, concurrencia, invariantes con proptest) y agrega pruebas; si una falla, arregla el código en el mismo PR. No cambies el status de T-062. PR hacia develop con título "QA ronda ${round}" listando pruebas nuevas y bugs encontrados.`,
      { label: `QA ronda ${round}`, phase: 'QA', agentType: PORTABLE ? 'general-purpose' : 'implementer', isolation: PORTABLE ? undefined : 'worktree', schema: IMPL },
    )
    if (qa && qa.status === 'pr_opened' && qa.pr_number) {
      history.push({ round, ...(await carry(`qa-${round}`, qa.pr_number, `QA ronda ${round}`, `qa/round-${round}`)) })
    }
  }
}

phase('Reporte')
const report = await agent(
  `${role('implementer', 'status/autopilot').pre}Escribe docs/STATUS.md (en español, visual, para alguien que vuelve después de horas): qué quedó integrado en develop (tabla tarea → PR), qué falta y por qué (dependencias, needs user), la lista agregada de "Verificación en hardware pendiente" de todas las tareas, y los riesgos conocidos. Historial de esta corrida: ${JSON.stringify(history)}. Rama status/autopilot desde origin/develop, PR hacia develop, espera CI verde y merge con gh pr merge --squash --delete-branch.`,
  { label: 'reporte final', phase: 'Reporte', agentType: PORTABLE ? 'general-purpose' : 'implementer', isolation: PORTABLE ? undefined : 'worktree', schema: MERGE },
)
return { history, report }
