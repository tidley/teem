// Pure display selection. These observations never qualify checks or reviews.
const statuses = new Set(['pending', 'in_progress', 'completed', 'cancelled'])
const clean = text => text.replace(/[\x00-\x1f\x7f]/g, ' ')
const short = (text, limit) => {
  const chars = Array.from(clean(text))
  return chars.length > limit ? chars.slice(0, limit - 1).join('') + '…' : chars.join('')
}
const bounded = (value, limit = 2000) => typeof value === 'string' && value.length > 0 && value.length <= limit
// Only the supervisor's structured association is used, never title/prompt text.
const issueReference = value => typeof value === 'string' && /^[a-z0-9](?:[a-z0-9-]{0,37}[a-z0-9])?\/[a-z0-9._-]{1,100}#[1-9][0-9]{0,19}$/.test(value) &&
  !value.split('/')[0].includes('--') && !['.', '..'].includes(value.split('/')[1].split('#')[0]) &&
  BigInt(value.split('#')[1]) <= 18446744073709551615n ? value : undefined
const withoutIssue = (issue, text) => {
  while (issue && text.startsWith(issue + ' ')) text = text.slice(issue.length + 1)
  return text === issue ? '' : text
}
const withIssue = (issue, text) => issue ? `${issue} ${withoutIssue(issue, text)}` : text
const workflowStates = new Map(Object.entries({ queued: 'queued', preparing: 'preparing', working: 'implementation',
  snapshotting: 'snapshotting', checking: 'Running checks', review_preparing: 'preparing reviews',
  reviewing: 'reviews', uncertain: 'blocked: uncertain' }))
const checkNames = new Map(Object.entries({ test: 'Tests', tests: 'Tests', 'rust-tests': 'Tests',
  fmt: 'Format', format: 'Format', 'rust-format': 'Format', clippy: 'Clippy', 'rust-clippy': 'Clippy', 'tui-tests': 'UI tests' }))

// Optional display metadata is not evidence. Old projections remain usable, and
// malformed additions fall back to their original rows rather than clearing work.
export function sidebarGroups(progress) {
  const fallback = () => [{ id: 'compact-unavailable', rows: [{
    content: 'Teem compact view unavailable · showing full rows', status: 'pending',
  }] }, { id: 'legacy', rows: progress.todos ?? [] }]
  if (!Array.isArray(progress.items) || progress.items.length > 2000) return fallback()
  if (progress.todos && progress.items.length !== progress.todos.length) return fallback()
  const groups = []
  const workflows = new Map()
  const ended = new Set()
  const valid = item => item && bounded(item.content) && statuses.has(item.status)
  const workflowID = item => bounded(item?.workflow_id, 128) && /^[a-zA-Z0-9_-]+$/.test(item.workflow_id) ? item.workflow_id : undefined
  for (const item of progress.items) {
    if (!valid(item)) continue
    const id = workflowID(item)
    if (id && bounded(item.task_id, 128) && ['succeeded', 'failed', 'interrupted'].includes(item.task_state)) {
      ended.add(`${id}/${item.task_id}`)
    }
    if (!id || !bounded(item.workflow_state, 80) || !bounded(item.label) || workflows.has(id)) continue
    const meta = item.sidebar
    const title = bounded(meta?.title) ? meta.title : item.label
    const issue = issueReference(item.issue)
    const project = bounded(meta?.project, 300) && title !== meta.project && !title.startsWith(meta.project + ' · ') ? meta.project : ''
    workflows.set(id, { id, title: withIssue(issue, short(withoutIssue(issue, title), 32)), issue, project, prefix: item.label + ' · ',
      heading: item, rows: [] })
  }
  // Extend colliding short IDs instead of conflating concurrent same-title work.
  const ids = [...workflows.keys()].sort()
  for (const [index, id] of ids.entries()) {
    const group = workflows.get(id)
    let length = 8
    for (const neighbor of [ids[index - 1], ids[index + 1]]) {
      if (!neighbor) continue
      let common = 0
      while (common < id.length && common < neighbor.length && id[common] === neighbor[common]) common++
      length = Math.max(length, common + 1)
    }
    group.description = `${group.project ? short(group.project, 20) + ' · ' : ''}#${group.id.slice(0, length)}`
    if (Number.isInteger(group.heading.sidebar?.revision) && group.heading.sidebar.revision > 0 && group.heading.sidebar.revision <= 3) {
      group.description += ` · revision ${group.heading.sidebar.revision}`
    }
  }
  const added = new Set()
  const manual = (item, index) => groups.push({ id: `manual-${index}`, rows: [item] })
  progress.items.forEach((item, index) => {
    if (!valid(item)) {
      manual(progress.todos?.[index] ?? { content: 'Progress row unavailable · inspect status', status: 'pending' }, index)
      return
    }
    const group = workflows.get(workflowID(item))
    if (!group || (item.workflow_state && item !== group.heading)) {
      manual({ content: clean(item.content), status: item.status }, index)
      return
    }
    if (!added.has(group.id)) {
      groups.push({ id: group.id, title: group.title, description: group.description,
        state: workflowStates.get(group.heading.workflow_state) ?? 'unknown state', rows: group.rows })
      added.add(group.id)
    }
    if (item === group.heading) return
    const reported = item.source === 'agent-reported'
    const label = bounded(item.label) ? item.label : ''
    const detail = label && item.content.startsWith(label + ' — ') ? item.content.slice(label.length + 3) : ''
    const meta = item.sidebar
    let kind = ['implementation', 'check', 'review', 'reported'].includes(meta?.kind) && bounded(meta.label) ? meta.kind : undefined
    let name = kind ? meta.label : label.startsWith(group.prefix) ? label.slice(group.prefix.length).replace(/^Revision \d+ · /, '') : ''
    if (!kind) {
      if (/^Worker(?: \d+)?$/.test(name)) { kind = 'implementation'; name = name.replace('Worker', 'Implementation') }
      else if (name.startsWith('Check: ')) { kind = 'check'; name = name.slice(7) }
      else if (name === 'Correctness review') { kind = 'review'; name = 'Review · correctness' }
      else if (name === 'Tests and scope review') { kind = 'review'; name = 'Review · tests/scope' }
    }
    if (reported) {
      if (item.status === 'completed' || (bounded(item.task_id, 128) && ended.has(`${group.id}/${item.task_id}`))) return
      if (kind !== 'reported') name = name.replace(/^.*? · reported: /, '')
      group.rows.push({ id: bounded(item.id, 256) ? item.id : `row-${index}`, content: short(`Reported: ${withoutIssue(group.issue, name || item.content)}`, 56), status: item.status, reported: true })
      return
    }
    if (kind === 'implementation' && item.status === 'completed') return
    if (!kind || kind === 'reported' || !detail) {
      group.rows.push({ id: bounded(item.id, 256) ? item.id : `row-${index}`, content: clean(withoutIssue(group.issue, item.content)), status: item.status })
      return
    }
    if (kind === 'check') name = checkNames.get(name.toLowerCase()) ?? name
    let state = ({ pending: 'pending', in_progress: 'active', completed: 'passed', cancelled: 'cancelled' })[item.status]
    let status = item.status
    if (detail.includes('individual result pending')) { state = 'Awaiting result'; status = 'pending' }
    else if (detail.includes('not run;')) state = 'not run'
    else if (detail.includes('verdict pending')) state = 'Awaiting review'
    else if (detail.includes('changes requested')) state = 'changes requested'
    else if (detail.includes('failed')) state = 'failed'
    else if (detail.includes('human approval')) state = 'blocked: approval'
    else if (detail.includes('uncertain')) state = 'blocked: uncertain'
    else if (detail === 'running') state = 'running'
    else if (detail === 'queued') state = 'queued'
    else if (detail === 'starting') state = 'starting'
    else if (detail === 'native input accepted') state = 'Submitted'
    else if (detail === 'interrupted') state = 'interrupted'
    group.rows.push({ id: bounded(item.id, 256) ? item.id : `row-${index}`, content: `${short(withoutIssue(group.issue, name), 28)} · ${state}`, status })
  })
  return groups
}

function todos(value) {
  if (!Array.isArray(value) || value.length > 2000) throw new Error('Invalid progress rows')
  return value.map((item) => {
    if (!item || typeof item.content !== 'string' || !item.content || item.content.length > 2000 ||
        !statuses.has(item.status)) throw new Error('Invalid progress row')
    return { content: item.content.replace(/[\x00-\x1f\x7f]/g, ' '), status: item.status }
  })
}

function latestResult(value) {
  if (value === undefined || value === null) return {}
  if (!['ready', 'blocked', 'needs_attention'].includes(value.state) ||
      typeof value.workflow_id !== 'string' || !value.workflow_id ||
      typeof value.title !== 'string' || !value.title || value.title.length > 300 ||
      typeof value.content !== 'string' || !value.content || value.content.length > 2000 ||
      (value.details !== undefined && (typeof value.details !== 'string' || value.details.length > 10000))) {
    throw new Error('Invalid Teem outcome')
  }
  const clean = text => text.replace(/[\x00-\x1f\x7f]/g, ' ')
  const multiline = text => text.replace(/[\x00-\x09\x0b-\x1f\x7f]/g, ' ')
  let sections
  let blocker
  if (value.blocker != null) {
    if (!['kind', 'project', 'source', 'reason', 'action'].every(key => bounded(value.blocker[key], key === 'source' ? 8192 : 2000)) ||
        value.blocker.current_source_checked !== false) throw new Error('Invalid Teem blocker')
    blocker = Object.fromEntries(['kind', 'project', 'source', 'reason', 'action'].map(key => [key, clean(value.blocker[key])]))
    blocker.current_source_checked = false
  }
  if (value.recorded_at !== undefined && (!Number.isFinite(value.recorded_at) || value.recorded_at < 0 || value.recorded_at > 8640000000000)) throw new Error('Invalid Teem result time')
  if (value.sections !== undefined) {
    if (!Array.isArray(value.sections) || value.sections.length === 0 || value.sections.length > 8 ||
        value.sections.some(section => !section || typeof section.title !== 'string' || !section.title || section.title.length > 100 ||
          typeof section.body !== 'string' || section.body.length > 5000) ||
        value.sections.reduce((total, section) => total + section.title.length + section.body.length, 0) > 10000) throw new Error('Invalid Teem outcome')
    sections = value.sections.map(section => ({ title: clean(section.title), body: multiline(section.body) }))
  }
  return { latest_result: { workflow_id: value.workflow_id, state: value.state,
    title: clean(value.title), content: clean(value.content),
    ...(value.details === undefined ? {} : { details: multiline(value.details) }),
    ...(sections ? { sections } : {}),
    ...(blocker ? { blocker } : {}),
    ...(value.historical === true ? { historical: true } : {}),
    ...(value.recorded_at === undefined ? {} : { recorded_at: value.recorded_at }) } }
}

export function parseProjection(value) {
  if (!value || value.version !== 1 || !Number.isFinite(value.updated_at) ||
      !value.sessions || typeof value.sessions !== 'object' || Array.isArray(value.sessions)) {
    throw new Error('Invalid Teem projection')
  }
  return { updated_at: value.updated_at, sessions: Object.fromEntries(Object.entries(value.sessions).map(
    ([id, progress]) => {
      const rows = todos(progress?.todos)
      return [id, { todos: rows, ...latestResult(progress?.latest_result),
        ...(progress?.result_notice_cleared === true ? { result_notice_cleared: true } : {}),
        ...(Array.isArray(progress?.items) ? { executionActive: progress.items.some(item =>
          ['queued', 'starting', 'accepted', 'running', 'uncertain'].includes(item?.task_state) ||
          (typeof item?.workflow_state === 'string' && !['ready', 'blocked', 'needs_attention'].includes(item.workflow_state))) } : {}),
        ...(progress && Object.hasOwn(progress, 'items') ? { groups: sidebarGroups({ ...progress, todos: rows }) } : {}),
      }]
    },
  )) }
}

export function selectProgress(projection, sessionID, now = Date.now() / 1000, options) {
  // Explicit read-only fallback for preserved work owned by a legacy chat.
  // This never changes submission identity or overwrites a chat's own progress.
  const own = projection?.sessions[sessionID]
  const source = own && (own.todos.length || own.latest_result || own.groups?.length || own.result_notice_cleared) ? sessionID : options?.progressSources?.[sessionID] ?? sessionID
  const session = typeof source === 'string' ? projection?.sessions[source] : undefined
  if (!session) return undefined
  return { todos: session.todos, ...(source !== sessionID ? { sourceSessionID: source } : {}), ...(session.groups ? { groups: session.groups } : {}), ...(session.executionActive === undefined ? {} : { executionActive: session.executionActive }), ...latestResult(session.latest_result), passive: true,
    stale: now - projection.updated_at > 15 || projection.updated_at > now + 5 }
}

export function resultNotice(progress) {
  const result = progress?.latest_result
  if (!result) return undefined
  return { label: progress.stale ? 'Teem last result · status unavailable' :
    result.state === 'ready' ? 'Teem result available' : result.historical ? 'Teem saved stopped attempt' : 'Teem needs attention',
    title: result.title, content: result.blocker ? `${result.blocker.reason}\nProject: ${result.blocker.project}\nSource: ${result.blocker.source}\nNext: ${result.blocker.action}` : result.content }
}

export function resultHint(progress) {
  if (!progress?.latest_result) return undefined
  const reason = progress.latest_result.sections?.[0]?.title
  const label = progress.stale ? 'Teem result stale' :
    progress.latest_result.state === 'ready' ? 'Teem result' :
      progress.latest_result.blocker?.kind === 'source_changes' ? 'Teem saved source blocker' :
      reason === 'Review blocked' ? 'Teem review blocked' :
        reason === 'Review not approved' ? 'Teem review not approved' : 'Teem needs attention'
  return `${label} · /teem-result`
}

// Local retrieval only: never submit another prompt or acknowledge an event.
export function installResultCommand(context, select, present = value => context.ui.dialog.alert(value)) {
  context.keymap.layer(() => ({ mode: 'global', commands: [{
    id: 'teem.result', title: 'Teem: show latest result', group: 'Teem', palette: true,
    slash: { name: 'teem-result' },
    async run() {
      const sessionID = context.ui.router.current()?.sessionID
      const progress = sessionID ? select(sessionID) : undefined
      const notice = resultNotice(progress)
      const warning = [progress?.sourceSessionID ? 'Linked work · original owner retained' : '',
        progress?.stale ? 'Last observation; current status unavailable.' : '',
        Number.isFinite(progress?.latest_result?.recorded_at) ? `Attempt recorded: ${new Date(progress.latest_result.recorded_at * 1000).toISOString()}` : ''].filter(Boolean).join('\n\n')
      await present(notice ? {
        title: `${notice.label} · ${notice.title}`,
        message: [warning, progress.latest_result.details || notice.content].filter(Boolean).join('\n\n'),
        ...(progress.latest_result.recorded_at === undefined ? {} : { recorded_at: progress.latest_result.recorded_at }),
        ...(progress.latest_result.sections ? { sections: [ ...(warning ? [{ title: 'Observation', body: warning }] : []), ...progress.latest_result.sections ] } : {}),
      } : { title: 'Teem result', message: sessionID ?
        'No saved Teem result for this chat. Check teem_status for work in progress or older history.' :
        'Open the intended chat before /teem-result.' })
    },
  }] }))
}

// Same public message contract as OpenCode's native Todo panel. No synthetic
// messages or tool calls are created. Non-Teem sessions keep their own todos.
export function legacyTodoFallback(options) {
  return options?.nativeTodos === true ||
    (options?.nativeTodos !== false && typeof options?.stateFile === 'string')
}

export function nativeTodos(messages, revertID) {
  const boundary = revertID ? messages.findIndex(m => m.id === revertID) : -1
  const visible = boundary === -1 ? messages : messages.slice(0, boundary)
  for (let i = visible.length - 1; i >= 0; i--) {
    const message = visible[i]
    if (message.type !== 'assistant') continue
    for (let j = (message.content?.length ?? 0) - 1; j >= 0; j--) {
      const part = message.content[j]
      if (part.type !== 'tool' || part.name !== 'todowrite' || part.state?.status !== 'completed') continue
      const value = part.state.input?.todos
      if (!Array.isArray(value)) return []
      return value.flatMap(item => !item || typeof item.content !== 'string' || !item.content ? [] : [{
        content: item.content, status: statuses.has(item.status) ? item.status : 'pending',
      }])
    }
  }
  return []
}
