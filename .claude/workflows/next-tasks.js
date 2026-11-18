export const meta = {
  name: 'next-tasks',
  description: 'Takes the next ready tasks from the backlog, implements them in isolated worktrees, reviews them and leaves draft PRs',
  whenToUse: 'To advance the Tandem backlog unsupervised. Args: { n: number of tasks (default 2), ids: ["T-002"] optional }',
  phases: [
    { title: 'Pick', detail: 'todo tasks, needs none, dependencies on main, no open PR' },
    { title: 'Implement', detail: 'one implementer per task, in its own worktree' },
    { title: 'Review', detail: 'reviewer against the design and criteria' },
    { title: 'Fix', detail: 'one round of fixes if the review asks for changes' },
  ],
}

// Each cargo build in its worktree uses several GB; with 24 GB in WSL, 2 at a time is the safe limit.
const N = Math.min((args && args.n) || 2, 3)
const FORCED = (args && args.ids) || null
// Portable mode: for sessions opened outside apps/tandem, where the project's agents
// are not registered. It uses the generic agent and asks it to follow the agent's file.
const PORTABLE = !!(args && args.portable)
const REPO = '~/projects/personal-projects/apps/tandem'
function role(name, task) {
  if (!PORTABLE) return { agentType: name, isolation: name === 'implementer' ? 'worktree' : undefined, pre: '' }
  const wt = name === 'implementer'
    ? `Work in your own worktree: cd ${REPO} && git fetch origin && (git worktree add .claude/worktrees/${task.id} task/${task.id} 2>/dev/null || git worktree add -b task/${task.id} .claude/worktrees/${task.id} origin/main), and do everything inside ${REPO}/.claude/worktrees/${task.id}. Do not touch the main checkout. `
    : `Work inside ${REPO}. `
  return { agentType: 'general-purpose', isolation: undefined, pre: `Read ${REPO}/CLAUDE.md and ${REPO}/.claude/agents/${name}.md and follow those instructions as if you were that agent. ${wt}` }
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
    skipped: { type: 'array', items: { type: 'string' }, description: 'id: reason, for todo tasks that were not taken' },
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

phase('Pick')
const pickPrompt = `Work inside ${REPO}. ` + (FORCED
  ? `Verify that these tasks exist in docs/tasks/ on origin/main (run git fetch origin first) and return them with their title: ${FORCED.join(', ')}. Apply no filters.`
  : `Run git fetch origin. Read the docs/tasks/T-*.md files as they are on origin/main (git show origin/main:docs/tasks/<file>).
A task is ready if: status is todo; needs is exactly none; every id in depends_on has status done on origin/main; and there is no open PR for branch task/<id> (gh pr list --state open --head task/<id>).
Return up to ${N} ready tasks, prioritizing the earliest phase (F0 < F1 < F2 < F3 < F4 < F5) and then the lowest id.
Do not pick two tasks that will create or modify the same crate at once; if they clash, put the second in skipped.
In skipped, list the todo tasks you did not take with a short reason.`)
const picked = await agent(pickPrompt, { label: 'pick tasks', phase: 'Pick', schema: PICK, effort: 'low' })

if (!picked || !picked.tasks.length) {
  log('There are no ready tasks that need no hardware or user.')
  return { done: [], skipped: picked ? picked.skipped : [] }
}
log(`Tasks: ${picked.tasks.map(t => t.id).join(', ')}`)

const results = await pipeline(
  picked.tasks,

  task => {
    const r = role('implementer', task)
    return agent(
      `${r.pre}Implement task ${task.id} ("${task.title}") from docs/tasks/${task.id}.md following your agent instructions. At the end report the PR number and URL.`,
      { label: `implement ${task.id}`, phase: 'Implement', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
    )
  },

  (impl, task) => {
    if (!impl || impl.status !== 'pr_opened' || !impl.pr_number) return { task, impl, review: null }
    const r = role('reviewer', task)
    return agent(
      `${r.pre}Review PR #${impl.pr_number} of task ${task.id} following your agent instructions.`,
      { label: `review ${task.id}`, phase: 'Review', agentType: r.agentType, schema: REVIEW },
    ).then(review => ({ task, impl, review }))
  },

  r => {
    if (!r || !r.review || r.review.verdict !== 'CHANGES') return r
    const list = r.review.findings.map((f, i) => `${i + 1}. ${f.where}: ${f.problem} → ${f.fix}`).join('\n')
    const ro = role('implementer', r.task)
    return agent(
      `${ro.pre}PR #${r.impl.pr_number} (task ${r.task.id}, branch task/${r.task.id}) received these review findings:\n${list}\n\nRun git fetch origin and git switch task/${r.task.id} (follow the existing branch, do not create another). Fix each finding, pass the quality gate, commit and push to the same branch. If a finding is wrong, explain it with a comment on the PR instead of changing the code.`,
      { label: `fix ${r.task.id}`, phase: 'Fix', agentType: ro.agentType, isolation: ro.isolation, schema: IMPL },
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
if (dropped) log(`${dropped} task(s) dropped because of an agent error; check /workflows.`)
return { done: summary, skipped: picked.skipped }
