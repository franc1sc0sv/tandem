export const meta = {
  name: 'autopilot',
  description: 'Advances the Tandem backlog unsupervised: implement, review, CI and merge to develop, with QA rounds, until the tasks run out',
  whenToUse: 'When the user leaves and wants the backlog to advance on its own. Args: { rounds: max rounds (20), perRound: tasks per round (3), portable: true if the session is not opened in apps/tandem }',
  phases: [
    { title: 'Pick' },
    { title: 'Implement' },
    { title: 'Review' },
    { title: 'Fix' },
    { title: 'Integrate', detail: 'wait for CI, rebase if needed, merge to develop' },
    { title: 'QA', detail: 'adversarial tests on what was merged' },
    { title: 'Report' },
  ],
}

const ROUNDS = (args && args.rounds) || 20
const PER_ROUND = Math.min((args && args.perRound) || 3, 3)
const PORTABLE = !!(args && args.portable)
const REPO = '~/projects/personal-projects/apps/tandem'

// In portable mode the project's agents are not registered: the generic one is used
// and asked to follow the agent's file.
function role(name, branch) {
  if (!PORTABLE) return { agentType: name, isolation: name === 'implementer' ? 'worktree' : undefined, pre: '' }
  const wt = branch
    ? `Work in your own worktree: cd ${REPO} && git fetch origin && (git worktree add .claude/worktrees/${branch.replace('/', '-')} ${branch} 2>/dev/null || git worktree add -b ${branch} .claude/worktrees/${branch.replace('/', '-')} origin/develop), and do everything inside that directory. Do not touch the main checkout. `
    : `Work inside ${REPO}. `
  return { agentType: 'general-purpose', isolation: undefined, pre: `Read ${REPO}/CLAUDE.md and ${REPO}/.claude/agents/${name}.md and follow those instructions as if you were that agent. ${wt}` }
}

const PICK = {
  type: 'object',
  required: ['tasks', 'open_prs', 'remaining'],
  properties: {
    tasks: { type: 'array', items: { type: 'object', required: ['id', 'title'], properties: { id: { type: 'string' }, title: { type: 'string' } } } },
    open_prs: { type: 'array', description: 'open PRs to develop from task/* branches', items: { type: 'object', required: ['id', 'pr'], properties: { id: { type: 'string' }, pr: { type: 'integer' } } } },
    remaining: { type: 'array', items: { type: 'string' }, description: 'id: reason, of the todo/blocked tasks that were not taken' },
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
    `${r.pre}Implement task ${task.id} ("${task.title}") from docs/tasks/${task.id}.md following your agent instructions. The PR goes to develop. At the end report the PR number and URL.`,
    { label, phase: 'Implement', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
  )
}

function review(id, pr) {
  const r = role('reviewer')
  return agent(`${r.pre}Review PR #${pr} (task ${id}) following your agent instructions.`,
    { label: `review ${id}`, phase: 'Review', agentType: r.agentType, schema: REVIEW })
}

function fix(id, pr, findings, branch) {
  const r = role('implementer', branch)
  const list = findings.map((f, i) => `${i + 1}. ${f.where}: ${f.problem} → ${f.fix}`).join('\n')
  return agent(
    `${r.pre}PR #${pr} (branch ${branch}) received these findings:\n${list}\n\nFollow the existing branch ${branch} (do not create another). Fix each finding with a test where applicable, pass the quality gate, commit and push to the same branch. If a finding is wrong, explain it with a comment on the PR.`,
    { label: `fix ${id}`, phase: 'Fix', agentType: r.agentType, isolation: r.isolation, schema: IMPL },
  )
}

function integrate(id, pr, branch) {
  const r = role('implementer', branch)
  return agent(
    `${r.pre}Merge PR #${pr} (branch ${branch}) into develop. Steps:
1. If GitHub reports conflicts or the branch is behind: git fetch origin, rebase ${branch} onto origin/develop, resolve conflicts keeping both intentions, pass the quality gate and git push --force-with-lease origin ${branch}.
2. Wait for CI: gh pr checks ${pr} --watch (the three jobs: ubuntu, windows, macos).
3. If a job fails: read the log (gh run view --log-failed), fix on the branch with a test if applicable, push, and go back to step 2. At most 3 attempts.
4. With the three jobs green: gh pr ready ${pr} (if it is a draft) and gh pr merge ${pr} --squash --delete-branch.
Never touch main. Report merged=true only if the merge happened.`,
    { label: `integrate ${id}`, phase: 'Integrate', agentType: r.agentType, isolation: r.isolation, schema: MERGE },
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
  return { id, title, pr, review: rev ? rev.verdict : null, merged: !!(m && m.merged), notes: m ? m.summary : 'integration failed' }
}

const history = []
let idle = 0
let sinceQa = 0

for (let round = 1; round <= ROUNDS; round++) {
  phase('Pick')
  const pick = await agent(
    `Work inside ${REPO}. Run git fetch origin. Read docs/tasks/T-*.md as they are on origin/develop (git show origin/develop:docs/tasks/<file>).
A task is ready if: status is todo; needs does NOT contain "user"; every id in depends_on has status done on origin/develop; and there is no open PR for branch task/<id> (gh pr list --state open --head task/<id>).
Return in tasks up to ${PER_ROUND} ready tasks, by phase (F0 < F1 < F2 < F3 < F4 < F5) and then id. Do not pick two that create or modify the same crate (tasks state in their scope what they touch); leave the second out. Do not pick T-062 (QA): this workflow runs it separately.
In open_prs list the open PRs with base develop whose branch starts with task/ (gh pr list --state open --base develop --json number,headRefName).
In remaining, each todo or blocked task not taken with a short reason.`,
    { label: `pick (round ${round})`, phase: 'Pick', schema: PICK, effort: 'low' },
  )
  if (!pick) break

  const adopted = pick.open_prs.slice(0, PER_ROUND)
  const fresh = pick.tasks.slice(0, Math.max(0, PER_ROUND - adopted.length))
  if (!fresh.length && !adopted.length) {
    idle++
    log(`Round ${round}: nothing ready. Pending: ${pick.remaining.length}`)
    if (idle >= 1) break
    continue
  }
  idle = 0
  log(`Round ${round}: new ${fresh.map(t => t.id).join(', ') || '-'} · resumed ${adopted.map(p => p.id).join(', ') || '-'}`)

  const items = [
    ...adopted.map(p => ({ kind: 'adopt', id: p.id, title: p.id, pr: p.pr })),
    ...fresh.map(t => ({ kind: 'new', id: t.id, title: t.title })),
  ]
  const results = await pipeline(
    items,
    it => it.kind === 'adopt' ? { status: 'pr_opened', pr_number: it.pr, summary: 'resumed' } : implement(it, `implement ${it.id}`),
    (impl, it) => {
      if (!impl || impl.status !== 'pr_opened' || !impl.pr_number) {
        return { id: it.id, title: it.title, pr: null, review: null, merged: false, notes: impl ? impl.summary : 'the agent failed' }
      }
      return carry(it.id, impl.pr_number, it.title)
    },
  )
  const ok = results.filter(Boolean)
  history.push(...ok.map(r => ({ round, ...r })))
  const merged = ok.filter(r => r.merged).length
  log(`Round ${round}: ${merged}/${items.length} merged`)
  sinceQa += merged

  if (sinceQa >= 4) {
    sinceQa = 0
    const qa = await agent(
      `${role('implementer', `qa/round-${round}`).pre}Adversarial QA round (see docs/tasks/T-062.md and docs/TESTING.md) on what was merged into develop in the latest rounds: ${ok.filter(r => r.merged).map(r => r.id).join(', ')} and earlier ones without QA. Branch qa/round-${round} from origin/develop. Look for uncovered cases (edges, errors, concurrency, invariants with proptest) and add tests; if one fails, fix the code in the same PR. Do not change T-062's status. PR to develop titled "QA round ${round}" listing new tests and bugs found.`,
      { label: `QA round ${round}`, phase: 'QA', agentType: PORTABLE ? 'general-purpose' : 'implementer', isolation: PORTABLE ? undefined : 'worktree', schema: IMPL },
    )
    if (qa && qa.status === 'pr_opened' && qa.pr_number) {
      history.push({ round, ...(await carry(`qa-${round}`, qa.pr_number, `QA round ${round}`, `qa/round-${round}`)) })
    }
  }
}

phase('Report')
const report = await agent(
  `${role('implementer', 'status/autopilot').pre}Write docs/STATUS.md (in English, visual, for someone coming back after hours): what was merged into develop (task -> PR table), what is missing and why (dependencies, needs user), the aggregated list of "Pending hardware verification" from all tasks, and known risks. History of this run: ${JSON.stringify(history)}. Branch status/autopilot from origin/develop, PR to develop, wait for green CI and merge with gh pr merge --squash --delete-branch.`,
  { label: 'final report', phase: 'Report', agentType: PORTABLE ? 'general-purpose' : 'implementer', isolation: PORTABLE ? undefined : 'worktree', schema: MERGE },
)
return { history, report }
