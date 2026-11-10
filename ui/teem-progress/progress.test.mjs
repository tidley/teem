import { test, expect } from 'bun:test'
import { parseProjection, selectProgress, nativeTodos, sidebarGroups } from './progress.mjs'

const todo = { content: 'Implement labels', status: 'in_progress' }
const data = { version: 1, updated_at: 100, sessions: { ses_foreman: { todos: [todo] } } }

// Match progress::workflow_rows, including its full evidence-compatible labels.
const heading = (id = 'aaaaaaaa-0000', title = 'Compact sidebar', project = 'teem') => ({
  id: `workflow_${id}`, workflow_id: id, workflow_state: 'checking', source: 'supervisor',
  label: `${project} · ${title}`, content: `${project} · ${title} — running approved checks`,
  status: 'in_progress', sidebar: { title, project },
})
const row = (head, id, label, status, detail, kind, extra = {}) => ({
  id, workflow_id: head.workflow_id, source: 'supervisor', label: `${head.label} · ${label}`,
  content: `${head.label} · ${label} — ${detail}`, status,
  sidebar: { kind, label }, ...extra,
})
const selectedItems = (items, extra = {}) => selectProgress(parseProjection({ ...data,
  sessions: { ses_foreman: { todos: items.map(({ content, status }) => ({ content, status })), items, ...extra } },
}), 'ses_foreman', 101)

test('compact rows show a workflow title/project once and short check/review labels', () => {
  const h = heading()
  const items = [h,
    row(h, 'worker', 'Implementation', 'completed', 'execution finished', 'implementation', { task_id: 'task', task_state: 'succeeded' }),
    row(h, 'tests', 'Tests', 'completed', 'passed (exit 0)', 'check'),
    row(h, 'fmt', 'Format', 'pending', 'check batch active; individual result pending', 'check'),
    row(h, 'clippy', 'Clippy', 'cancelled', 'failed; see recorded check evidence', 'check'),
    row(h, 'review', 'Review · correctness', 'cancelled', 'changes requested; see recorded findings', 'review'),
  ]
  const before = structuredClone(items)
  const group = selectedItems(items).groups[0]
  expect(group.title).toBe('Compact sidebar')
  expect(group.description).toContain('teem')
  expect(group.rows.map(r => r.content)).toEqual([
    'Tests · passed', 'Format · pending (batch)', 'Clippy · failed', 'Review · correctness · changes requested',
  ])
  expect(group.rows[1].status).toBe('pending')
  expect(group.rows.every(r => !r.content.includes(h.label))).toBe(true)
  expect(items).toEqual(before)
  expect(selectedItems(items).todos).toHaveLength(items.length)
})

test('claims are not gate evidence; ended and completed claim clutter is hidden, failures retained', () => {
  const h = heading()
  const items = [h,
    row(h, 'live', 'Implementation', 'in_progress', 'running', 'implementation', { task_id: 'live', task_state: 'running' }),
    row(h, 'claim', 'Tests passed', 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported', task_id: 'live' }),
    row(h, 'done', 'Old claim', 'completed', 'agent-reported progress only', 'reported', { source: 'agent-reported', task_id: 'live' }),
    row(h, 'ended-claim', 'Late claim', 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported', task_id: 'ended' }),
    row(h, 'ended', 'Implementation 2', 'cancelled', 'execution failed', 'implementation', { task_id: 'ended', task_state: 'failed' }),
    row(h, 'tests', 'Tests', 'pending', 'not run yet', 'check'),
  ]
  const rows = selectedItems(items).groups[0].rows
  expect(rows.map(r => r.content)).toEqual(['Implementation · running', 'Reported: Tests passed', 'Implementation 2 · failed', 'Tests · pending'])
  expect(rows[1].reported).toBe(true)
})

test('same-title workflows and multiple projects retain distinct IDs and visible identity', () => {
  const heads = [heading('aaaaaaaa-0000', 'Same', 'one'), heading('aaaaaaaa-1111', 'Same', 'one'), heading('bbbbbbbb-0000', 'Same', 'two')]
  const groups = selectedItems(heads).groups
  expect(new Set(groups.map(g => g.id)).size).toBe(3)
  expect(new Set(groups.map(g => g.description)).size).toBe(3)
  expect(groups[0].description).toContain('one')
  expect(groups[2].description).toContain('two')
})

test('configured Rust and adapter checks use compact labels without implying batch execution', () => {
  const h = heading()
  const names = ['rust-tests', 'rust-format', 'rust-clippy', 'tui-tests', 'custom-inspection']
  const items = [h, ...names.map((name, index) => row(h, `check-${index}`, name,
    'pending', 'check batch active; individual result pending', 'check'))]
  const group = selectedItems(items).groups[0]
  expect(group.rows.map(r => r.content)).toEqual([
    'Tests · pending (batch)', 'Format · pending (batch)', 'Clippy · pending (batch)',
    'UI tests · pending (batch)', 'custom-inspection · pending (batch)',
  ])
  expect(group.rows.every(r => r.status === 'pending')).toBe(true)
  expect(selectedItems(items).todos.map(t => t.content)).toEqual(items.map(i => i.content))
})

test('long and multibyte titles/claims are bounded without splitting surrogate pairs', () => {
  for (const title of ['Narrow', 'Very long title '.repeat(30), '🦄界'.repeat(120)]) {
    const h = heading('workflow', title)
    const groups = sidebarGroups({ items: [h, row(h, 'claim', '🦄'.repeat(300), 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported' })] })
    expect([...groups[0].title].length).toBeLessThanOrEqual(32)
    expect([...groups[0].rows[0].content].length).toBeLessThanOrEqual(56)
    expect(groups[0].title).not.toMatch(/[\uD800-\uDBFF]$/)
  }
})

test('manual entries stay separate and unchanged; legacy items use prefix only within their workflow', () => {
  const h = heading()
  delete h.sidebar
  const worker = row(h, 'worker', 'Worker', 'in_progress', 'waiting for human approval or answer', 'implementation')
  delete worker.sidebar
  const manual = { id: 'manual', source: 'foreman-plan', content: 'teem · Keep manual text', status: 'completed' }
  const groups = selectedItems([manual, h, worker, { ...manual, id: 'manual2' }]).groups
  expect(groups[0].rows[0]).toMatchObject({ content: manual.content, status: 'completed' })
  expect(groups[1].rows[0].content).toBe('Implementation · blocked: approval')
  expect(groups[2].rows[0].content).toBe(manual.content)
})

test('optional malformed/unknown metadata falls back visibly without erasing usable rows', () => {
  const h = heading()
  const good = row(h, 'tests', 'Tests', 'pending', 'not run yet', 'check')
  const broken = { ...good, id: 'bad', sidebar: { kind: 'future', label: {} } }
  const selected = selectedItems([h, good, broken, { content: 'Manual row', status: 'pending', workflow_id: 7 }])
  expect(selected.groups.flatMap(g => g.rows).some(r => r.content === 'Tests · pending')).toBe(true)
  expect(selected.groups.flatMap(g => g.rows).some(r => r.content === 'Manual row')).toBe(true)
  for (const items of [null, {}, Array(2001).fill(good)]) {
    const value = structuredClone(data)
    value.sessions.ses_foreman.items = items
    const result = selectProgress(parseProjection(value), 'ses_foreman', 101)
    expect(result.todos).toEqual([todo])
    expect(result.groups[0].rows[0].content).toContain('compact view unavailable')
    expect(result.groups[1].rows).toEqual([todo])
  }
})

test('grouped stale observations and integration receipts survive and recover unchanged', () => {
  const h = heading()
  const receipt = { workflow_id: 'old', state: 'ready', title: 'Earlier work', content: 'Committed · exact candidate; publication not checked' }
  const value = { ...data, sessions: { ses_foreman: { items: [h], todos: [{ content: h.content, status: h.status }], latest_result: receipt } } }
  const old = parseProjection(value)
  expect(selectProgress(old, 'ses_foreman', 121)).toMatchObject({ stale: true, latest_result: receipt, groups: old.sessions.ses_foreman.groups })
  const fresh = parseProjection({ ...value, updated_at: 121 })
  expect(selectProgress(fresh, 'ses_foreman', 122)).toMatchObject({ stale: false, latest_result: receipt })
  expect(selectedItems([], { latest_result: receipt }).groups).toEqual([])
})

test('pending verdict, stopped batch and unknown future states do not imply a pass', () => {
  const h = heading()
  h.workflow_state = 'future-state'
  const group = selectedItems([h,
    row(h, 'review', 'Review · tests/scope', 'in_progress', 'execution finished; verdict pending', 'review', { task_state: 'succeeded' }),
    row(h, 'check', 'Tests', 'cancelled', 'not run; check batch stopped', 'check'),
    row(h, 'uncertain', 'Implementation', 'in_progress', 'uncertain; no replacement started', 'implementation'),
  ]).groups[0]
  expect(group.state).toBe('unknown state')
  expect(group.rows.map(r => r.content)).toEqual([
    'Review · tests/scope · verdict pending', 'Tests · not run', 'Implementation · blocked: uncertain',
  ])
  expect(group.rows.every(r => r.status !== 'completed')).toBe(true)
})

test('invalid individual item fields retain the corresponding native-compatible row', () => {
  const value = structuredClone(data)
  value.sessions.ses_foreman.items = [{ content: {}, status: 'pending', workflow_id: {}, sidebar: { title: [] } }]
  expect(selectProgress(parseProjection(value), 'ses_foreman', 101).groups[0].rows).toEqual([todo])
  const h = heading()
  h.sidebar = { title: [], project: 'x'.repeat(301), revision: '1', extra: { ignored: true } }
  expect(selectedItems([h]).groups[0]).toMatchObject({ title: h.label, description: '#aaaaaaaa' })
})

test('unknown prototype-like strings are inert labels, not mapping entries', () => {
  for (const name of ['constructor', '__proto__', 'toString']) {
    const h = heading()
    h.workflow_state = name
    const group = selectedItems([h, row(h, 'check', name, 'pending', 'not run yet', 'check'),
      row(h, 'claim', 'Claim', 'pending', 'agent-reported progress only', 'reported', { source: 'agent-reported', task_id: { toString: 'invalid' } }),
    ]).groups[0]
    expect(group.state).toBe('unknown state')
    expect(group.rows[0].content).toBe(`${name} · pending`)
    expect(group.rows[1].content).toBe('Reported: Claim')
  }
})

test('mismatched optional item counts cannot erase compatible todos', () => {
  const value = structuredClone(data)
  value.sessions.ses_foreman.items = []
  const selected = selectProgress(parseProjection(value), 'ses_foreman', 101)
  expect(selected.groups[1].rows).toEqual([todo])
  expect(selected.groups[0].rows[0].content).toContain('compact view unavailable')
})

test('bound sessions show only supervisor rows, even an empty current list', () => {
  expect(selectProgress(parseProjection(data), 'ses_foreman', 101)).toEqual({ todos: [todo], passive: true, stale: false })
  const empty = structuredClone(data)
  empty.sessions.ses_foreman.todos = []
  expect(selectProgress(parseProjection(empty), 'ses_foreman', 101).todos).toEqual([])
  expect(selectProgress(parseProjection(data), 'ses_other', 101)).toBeUndefined()
})

test('stale observations cannot look live; invalid status/content is rejected', () => {
  expect(selectProgress(parseProjection(data), 'ses_foreman', 121).stale).toBe(true)
  for (const value of [null, {}, { ...data, version: 2 }, { ...data, updated_at: 'now' },
    { ...data, sessions: { ses_foreman: { todos: [{ content: 'Done', status: 'passed' }] } } }]) {
    expect(() => parseProjection(value)).toThrow()
  }
})

test('staleness starts strictly beyond the age and future timestamp thresholds', () => {
  const projection = parseProjection(data)
  for (const [now, stale] of [[115, false], [115.001, true], [95, false], [94.999, true]]) {
    expect(selectProgress(projection, 'ses_foreman', now)).toEqual({ todos: [todo], passive: true, stale })
    expect(selectProgress(projection, 'ses_other', now)).toBeUndefined()
  }
})

test('stale selection preserves current todos and latest result without mutating its input', () => {
  const value = structuredClone(data)
  const result = { workflow_id: 'workflow-id', state: 'ready', title: 'Compare labels',
    content: 'Completed · checks and both reviews passed; not integrated' }
  value.sessions.ses_foreman.latest_result = result
  const projection = parseProjection(value)
  const before = structuredClone(projection)
  for (const now of [115.001, 94.999]) {
    expect(selectProgress(projection, 'ses_foreman', now)).toEqual({
      todos: [todo], latest_result: result, passive: true, stale: true,
    })
    expect(projection).toEqual(before)
  }
})

test('a newer projection replaces stale progress with fresh current rows', () => {
  const older = parseProjection(data)
  const before = structuredClone(older)
  const now = 121
  expect(selectProgress(older, 'ses_foreman', now).stale).toBe(true)
  const value = structuredClone(data)
  value.updated_at = 120
  value.sessions.ses_foreman.todos = [{ content: 'Review labels', status: 'pending' }]
  const newer = parseProjection(value)
  expect(selectProgress(newer, 'ses_foreman', now)).toEqual({
    todos: value.sessions.ses_foreman.todos, passive: true, stale: false,
  })
  expect(selectProgress(newer, 'ses_other', now)).toBeUndefined()
  expect(older).toEqual(before)
  expect(selectProgress(older, 'ses_foreman', now).stale).toBe(true)
})

test('completed result remains visible separately from the empty current checklist', () => {
  const value = structuredClone(data)
  value.sessions.ses_foreman.todos = []
  const result = { workflow_id: 'workflow-id', state: 'ready', title: 'Compare labels',
    content: 'Completed · checks and both reviews passed; not integrated' }
  value.sessions.ses_foreman.latest_result = result
  const selected = selectProgress(parseProjection(value), 'ses_foreman', 101)
  expect(selected.todos).toEqual([])
  expect(selected.latest_result).toEqual(result)
  value.sessions.ses_foreman.latest_result.state = 'running'
  expect(() => parseProjection(value)).toThrow()
})

test('unbound sessions retain their real latest native todo call and revert boundary', () => {
  const call = (id, todos) => ({ id, type: 'assistant', content: [
    { type: 'tool', name: 'todowrite', state: { status: 'completed', input: { todos } } },
  ] })
  const messages = [call('msg_a', [todo]), call('msg_b', [])]
  expect(nativeTodos(messages)).toEqual([])
  expect(nativeTodos(messages, 'msg_b')).toEqual([todo])
  expect(nativeTodos(messages, 'missing_boundary')).toEqual([])
  expect(nativeTodos([call('later_sort_id', [todo]), call('earlier_sort_id', [])], 'earlier_sort_id')).toEqual([todo])
  expect(nativeTodos([{ ...messages[0], error: { message: 'Failed' } }])).toEqual([todo])
  expect(nativeTodos([call('msg_a', [{ content: 'Native fallback', status: 'unknown' }])])).toEqual([
    { content: 'Native fallback', status: 'pending' },
  ])
})
