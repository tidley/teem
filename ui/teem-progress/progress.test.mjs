import { test, expect } from 'bun:test'
import { parseProjection, selectProgress, nativeTodos, sidebarGroups } from './progress.mjs'
import { enableCurrentChat, installEnableCommand, installNativeTools, nativeBridge } from './enable.mjs'

function enableContext() {
  let command
  const session = { id: 'ses_enabled', agent: 'build', projectID: 'project_a', location: { directory: '/project/a' } }
  const selected = { providerID: 'provider', modelID: 'selected', variant: 'high' }
  const context = {
    options: { executable: '/absolute/teem', config: '/private/config.toml' },
    ui: { router: { current: () => ({ type: 'session', sessionID: session.id }) }, model: { current: () => selected } },
    data: { session: { sync: async id => { expect(id).toBe(session.id) }, get: id => id === session.id ? session : undefined } },
    // Match the host SDK: a pure factory, owned cleanup, and no return value.
    keymap: { layer(factory) { command = factory().commands[0] } },
  }
  return { context, session, command: () => command }
}
const enabled = input => ({ enabled: true, sessionID: input.sessionID, project: 'a', started: false })

test('/teem enable sends exact current native identity and UI-selected model without starting work', async () => {
  const f = enableContext()
  const calls = []
  const invoke = async (...args) => { calls.push(args); return enabled(args[2]) }
  const first = await enableCurrentChat(f.context, ' enable ', invoke)
  expect(await enableCurrentChat(f.context, 'enable', invoke)).toEqual(first)
  expect(calls).toHaveLength(2)
  expect(calls[0]).toEqual([f.context.options, 'enable-chat', {
    sessionID: 'ses_enabled', agent: 'build', projectID: 'project_a', directory: '/project/a',
    model: { providerID: 'provider', id: 'selected', variant: 'high' },
  }])
  delete f.context.ui.model.current().variant
  await enableCurrentChat(f.context, 'enable', invoke)
  expect(calls[2][2].model).toEqual({ providerID: 'provider', id: 'selected' })
})

test('enable refuses unsupported commands, missing identity/model and unconfirmed bridge success', async () => {
  const f = enableContext()
  let called = false
  const invoke = async () => { called = true; return {} }
  for (const raw of ['', 'enabled', 'disable', 'enable extra', null]) {
    await expect(enableCurrentChat(f.context, raw, invoke)).rejects.toThrow('Use /teem enable')
  }
  f.context.ui.router.current = () => ({ type: 'home' })
  await expect(enableCurrentChat(f.context, 'enable', invoke)).rejects.toThrow('Open the intended chat')
  f.context.ui.router.current = () => ({ type: 'session', sessionID: f.session.id })
  delete f.session.agent
  await expect(enableCurrentChat(f.context, 'enable', invoke)).rejects.toThrow('identity unavailable')
  f.session.agent = 'build'
  f.context.ui.model.current = () => undefined
  await expect(enableCurrentChat(f.context, 'enable', invoke)).rejects.toThrow('Select a provider')
  expect(called).toBe(false)
  f.context.ui.model.current = () => ({ providerID: 'p', modelID: 'm' })
  await expect(enableCurrentChat(f.context, 'enable', invoke)).rejects.toThrow('did not confirm')
  await expect(enableCurrentChat(f.context, 'enable', async () => ({ enabled: true, sessionID: 'ses_other', started: false }))).rejects.toThrow('did not confirm')
})

test('command success and repeated errors remain associated with origin across navigation and awaits', async () => {
  const f = enableContext()
  const messages = new Map()
  let resolve
  let fail = false
  const pending = new Promise(done => { resolve = done })
  const invoke = async (_, command, input) => {
    expect(command).toBe('enable-chat')
    await pending
    if (fail) throw new Error('runtime unavailable')
    return enabled(input)
  }
  expect(installEnableCommand(f.context, (sessionID, message) => messages.set(sessionID, message), invoke)).toBeUndefined()
  const command = f.command()
  expect(command).toMatchObject({ id: 'teem.enable', slash: { name: 'teem', arguments: true }, palette: true })
  const run = command.run('enable')
  f.context.ui.router.current = () => ({ type: 'session', sessionID: 'ses_other' })
  resolve()
  await run
  expect(messages.get('ses_enabled')).toContain('Teem enabled')
  expect(messages.has('ses_other')).toBe(false)
  f.context.ui.router.current = () => ({ type: 'session', sessionID: 'ses_enabled' })
  fail = true
  for (let i = 0; i < 2; i++) {
    const failed = command.run('enable')
    f.context.ui.router.current = () => ({ type: 'session', sessionID: 'ses_other' })
    await failed
    expect(messages.get('ses_enabled')).toContain('runtime unavailable')
    expect(messages.has('ses_other')).toBe(false)
    f.context.ui.router.current = () => ({ type: 'session', sessionID: 'ses_enabled' })
  }
})

test('navigation while session sync awaits cannot substitute another chat model or error destination', async () => {
  for (const rejectSync of [false, true]) {
    const f = enableContext()
    const messages = new Map()
    let finish
    f.context.data.session.sync = () => new Promise((resolve, reject) => {
      finish = () => rejectSync ? reject(new Error('session sync failed')) : resolve()
    })
    let sent
    installEnableCommand(f.context, (id, message) => messages.set(id, message), async (_, __, input) => {
      sent = input
      return enabled(input)
    })
    const run = f.command().run('enable')
    f.context.ui.router.current = () => ({ type: 'session', sessionID: 'ses_other' })
    f.context.ui.model.current = () => ({ providerID: 'other', modelID: 'other' })
    finish()
    await run
    expect(messages.has('ses_other')).toBe(false)
    expect(messages.get('ses_enabled')).toContain(rejectSync ? 'session sync failed' : 'Teem enabled')
    if (!rejectSync) expect(sent.model).toEqual({ providerID: 'provider', id: 'selected', variant: 'high' })
  }
})

test('native tool registration uses trusted invocation context not tool input, and disposes', async () => {
  const f = enableContext()
  const definitions = [{ name: 'foreman_status', description: 'Inspect evidence', inputSchema: { type: 'object' } }]
  let tool
  let disposed = false
  const calls = []
  f.context.tool = { transform(edit) { edit({ add: value => { tool = value } }); return () => { disposed = true } } }
  const invoke = async (...args) => { calls.push(args); return args[1] === 'native-tools' ? definitions : { counts: {} } }
  const dispose = await installNativeTools(f.context, invoke)
  expect(tool.input).toEqual(definitions[0].inputSchema)
  const signal = new AbortController().signal
  const input = { sessionID: 'ses_forged', agent: 'forged' }
  expect(await tool.execute(input, { sessionID: 'ses_trusted', agent: 'build', signal })).toEqual({ content: '{\n  "counts": {}\n}' })
  expect(calls[1]).toEqual([f.context.options, 'native-call', { name: 'foreman_status', arguments: input, sessionID: 'ses_trusted', agent: 'build' }, signal])
  dispose()
  expect(disposed).toBe(true)
  await expect(installNativeTools(f.context, async () => ({}))).rejects.toThrow('catalog unavailable')
})

test('native bridge fails visibly for missing configuration, aborted requests, spawn, exit and JSON errors', async () => {
  await expect(nativeBridge({}, 'enable-chat', {})).rejects.toThrow('absolute executable and config')
  const options = { executable: '/nonexistent/teem-issue1', config: '/private/config.toml' }
  const abort = new AbortController()
  abort.abort()
  await expect(nativeBridge(options, 'enable-chat', {}, abort.signal)).rejects.toThrow('aborted')
  await expect(nativeBridge(options, 'enable-chat', {})).rejects.toThrow()
  await expect(nativeBridge({ ...options, executable: '/bin/false' }, 'native-tools')).rejects.toThrow('exited')
  await expect(nativeBridge({ ...options, executable: '/bin/echo' }, 'native-tools')).rejects.toThrow('invalid JSON')
})

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

test('verified issue appears in the heading, not redundant prefixes on compact children', () => {
  const issue = 'tidley/teem#8'
  const h = { ...heading(), issue }
  h.label = `${issue} ${h.label}`
  h.content = `${issue} ${h.content}`
  const items = [h,
    row(h, 'worker', 'Implementation', 'in_progress', 'running', 'implementation'),
    row(h, 'check', 'tests', 'completed', 'passed (exit 0)', 'check'),
    row(h, 'correctness', 'Review · correctness', 'pending', 'not started', 'review'),
    row(h, 'scope', 'Review · tests/scope', 'in_progress', 'running', 'review'),
    row(h, 'claim', `${issue} Inspect changes`, 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported' }),
    row(h, 'unknown', 'Unknown stage', 'pending', 'not started', undefined),
  ]
  const before = structuredClone(items)
  const selected = selectedItems(items)
  expect(selected.groups).toHaveLength(1)
  const group = selected.groups[0]
  expect(group.title).toBe(`${issue} Compact sidebar`)
  expect(group.rows).toHaveLength(6)
  expect(group.rows.map(r => r.content)).toEqual([
    'Implementation · running', 'Tests · passed', 'Review · correctness · pending',
    'Review · tests/scope · running', 'Reported: Inspect changes',
    'teem · Compact sidebar · Unknown stage — not started',
  ])
  expect(group.rows.map(r => r.status)).toEqual(items.slice(1).map(r => r.status))
  expect(group.rows[4].reported).toBe(true)
  expect(selected.todos).toEqual(items.map(({ content, status }) => ({ content, status })))
  expect(items).toEqual(before)
})

test('repeated matching leading references are removed from every child kind only', () => {
  const issue = 'tidley/teem#13'
  const prefix = `${issue} ${issue} `
  const h = { ...heading('repeated', prefix + 'Cleanup'), issue }
  const items = [h,
    row(h, 'worker', prefix + 'Implementation', 'in_progress', 'running', 'implementation'),
    row(h, 'check', prefix + 'Tests', 'completed', 'passed (exit 0)', 'check'),
    row(h, 'review', prefix + 'Review', 'pending', 'verdict pending', 'review'),
    row(h, 'claim', prefix + 'Inspect changes', 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported' }),
    row(h, 'fallback', 'Unknown stage', 'cancelled', 'failed; inspect full evidence', undefined, {
      content: prefix + 'Worker attribution — failed; inspect full evidence',
    }),
  ]
  const group = selectedItems(items).groups[0]
  expect(group.title).toBe(`${issue} Cleanup`)
  expect(group.rows.map(r => r.content)).toEqual([
    'Implementation · running', 'Tests · passed', 'Review · Awaiting review',
    'Reported: Inspect changes', 'Worker attribution — failed; inspect full evidence',
  ])
})

test('multiple issue groups keep order and identity while preserving different and embedded references', () => {
  const first = { ...heading('aaaaaaaa-0000', 'Same'), issue: 'tidley/teem#13' }
  const second = { ...heading('aaaaaaaa-1111', 'Same'), issue: 'tidley/teem#12' }
  const items = [first,
    row(first, 'other', 'tidley/teem#12 Tests', 'pending', 'not started', 'check'),
    row(first, 'embedded', 'See tidley/teem#13', 'pending', 'not started', 'review'),
    row(first, 'claim', 'Compare tidley/teem#12', 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported' }),
    second, row(second, 'worker', 'Implementation', 'in_progress', 'running', 'implementation'),
  ]
  const receipt = { workflow_id: 'earlier', state: 'ready', title: 'tidley/teem#13 Earlier', content: 'tidley/teem#13 Not integrated' }
  const selected = selectedItems(items, { latest_result: receipt })
  expect(selected.groups.map(g => [g.id, g.title])).toEqual([
    [first.workflow_id, 'tidley/teem#13 Same'], [second.workflow_id, 'tidley/teem#12 Same'],
  ])
  expect(new Set(selected.groups.map(g => g.description)).size).toBe(2)
  expect(selected.groups[0].rows.map(r => [r.id, r.content])).toEqual([
    ['other', 'tidley/teem#12 Tests · pending'], ['embedded', 'See tidley/teem#13 · pending'],
    ['claim', 'Reported: Compare tidley/teem#12'],
  ])
  expect(selected.groups[1].rows[0].content).toBe('Implementation · running')
  expect(selected.latest_result).toEqual(receipt)
})

test('fallback rows retain attribution and issue-looking free text beyond the matching leading prefix', () => {
  const issue = 'tidley/teem#13'
  const h = { ...heading(), issue }
  const texts = [
    `${issue} ${issue} Worker attribution — see ${issue}`,
    'tidley/teem#12 Other attribution — pending',
    `Investigate ${issue} regression`,
    `${issue}: meaningful punctuation`,
    `${issue}0 Different issue`,
  ]
  const items = [h, ...texts.map((content, index) => ({
    id: `fallback-${index}`, workflow_id: h.workflow_id, content, status: 'pending',
  }))]
  expect(selectedItems(items).groups[0].rows.map(r => r.content)).toEqual([
    `Worker attribution — see ${issue}`, ...texts.slice(1),
  ])
  const manual = { content: `${issue} Manual attribution`, status: 'pending', source: 'foreman-plan' }
  const ungrouped = { ...manual, workflow_id: 'missing', source: 'supervisor' }
  expect(selectedItems([h, manual, ungrouped]).groups.slice(1).map(g => g.rows[0].content)).toEqual([
    manual.content, ungrouped.content,
  ])
})

test('issue-looking native/manual titles and invalid associations are never inferred', () => {
  const text = 'tidley/teem#8 User text'
  const h = heading('unknown', text)
  const legacy = selectedItems([h, row(h, 'check', 'tests', 'pending', 'not run yet', 'check')])
  expect(legacy.groups[0].rows[0].content).toBe('Tests · pending')
  for (const issue of ['#8', 'tidley/teem#0', 'tidley/teem#18446744073709551616', 'tidley/..#8', null]) {
    const selected = selectedItems([{ ...h, issue }, row(h, 'check', 'tests', 'pending', 'not run yet', 'check')])
    expect(selected.groups[0].rows[0].content).toBe('Tests · pending')
  }
  const manual = { content: text, status: 'pending' }
  expect(selectedItems([manual]).groups[0].rows[0]).toEqual(manual)
  expect(nativeTodos([{ type: 'assistant', content: [{ type: 'tool', name: 'todowrite',
    state: { status: 'completed', input: { todos: [manual] } } }] }])).toEqual([manual])
})

test('missing or invalid structured issues cannot strip issue-looking child text', () => {
  const text = 'tidley/teem#13 Inspect'
  for (const issue of [undefined, '#13', 'tidley/teem#0', 'Tidley/teem#13', null]) {
    const h = { ...heading(), issue }
    const group = selectedItems([h,
      row(h, 'worker', text, 'in_progress', 'running', 'implementation'),
      row(h, 'check', text, 'pending', 'not started', 'check'),
      row(h, 'review', text, 'pending', 'not started', 'review'),
      row(h, 'claim', text, 'in_progress', 'agent-reported progress only', 'reported', { source: 'agent-reported' }),
      { workflow_id: h.workflow_id, content: text, status: 'pending' },
    ]).groups[0]
    expect(group.title).toBe('Compact sidebar')
    expect(group.rows.map(r => r.content)).toEqual([
      `${text} · running`, `${text} · pending`, `${text} · pending`, `Reported: ${text}`, text,
    ])
  }
})

test('long canonical references remain complete while compact stage labels stay short', () => {
  const issue = `${'a'.repeat(39)}/${'b'.repeat(100)}#18446744073709551615`
  const h = { ...heading('long', '🦀'.repeat(100)), issue }
  h.label = `${issue} ${h.label}`
  h.content = `${issue} ${h.content}`
  const selected = selectedItems([h, row(h, 'check', 'tests', 'pending', 'not run yet', 'check')])
  expect(selected.groups[0].title.startsWith(issue + ' ')).toBe(true)
  expect(selected.groups[0].rows[0].content).toBe('Tests · pending')
})

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
    'Tests · passed', 'Format · Awaiting result', 'Clippy · failed', 'Review · correctness · changes requested',
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
  expect(group.state).toBe('Running checks')
  expect(group.rows.map(r => r.content)).toEqual([
    'Tests · Awaiting result', 'Format · Awaiting result', 'Clippy · Awaiting result',
    'UI tests · Awaiting result', 'custom-inspection · Awaiting result',
  ])
  expect(group.rows.every(r => r.status === 'pending')).toBe(true)
  expect(selectedItems(items).todos.map(t => t.content)).toEqual(items.map(i => i.content))
})

test('submission, execution and unavailable results remain distinct display observations', () => {
  const h = { ...heading(), issue: 'tidley/teem#14' }
  const items = [h,
    row(h, 'submitted', 'Implementation', 'in_progress', 'native input accepted', 'implementation', { task_id: 'submitted-task', task_state: 'accepted' }),
    row(h, 'running', 'Implementation 2', 'in_progress', 'running', 'implementation', { task_id: 'running-task', task_state: 'running' }),
    row(h, 'awaiting', 'Tests', 'in_progress', 'check batch active; individual result pending', 'check'),
    row(h, 'passed', 'Format', 'completed', 'passed (exit 0)', 'check'),
    row(h, 'review', 'Review · correctness', 'in_progress', 'execution finished; verdict pending', 'review', { task_id: 'review-task', task_state: 'succeeded' }),
  ]
  const before = structuredClone(items)
  const selected = selectedItems(items)
  const group = selected.groups[0]
  expect(group).toMatchObject({ id: h.workflow_id, title: 'tidley/teem#14 Compact sidebar', state: 'Running checks' })
  expect(group.rows.map(r => [r.id, r.content, r.status])).toEqual([
    ['submitted', 'Implementation · Submitted', 'in_progress'],
    ['running', 'Implementation 2 · running', 'in_progress'],
    ['awaiting', 'Tests · Awaiting result', 'pending'],
    ['passed', 'Format · passed', 'completed'],
    ['review', 'Review · correctness · Awaiting review', 'in_progress'],
  ])
  expect(selected.todos).toEqual(items.map(({ content, status }) => ({ content, status })))
  expect(items).toEqual(before)
})

test('ordinary pending, approval, uncertainty and ready receipt labels stay unchanged', () => {
  const h = { ...heading(), workflow_state: 'uncertain' }
  const items = [h,
    row(h, 'pending', 'Review', 'pending', 'not started', 'review'),
    row(h, 'approval', 'Implementation', 'in_progress', 'waiting for human approval or answer', 'implementation'),
    row(h, 'uncertain', 'Implementation 2', 'in_progress', 'uncertain; no replacement started', 'implementation'),
  ]
  const receipt = { workflow_id: 'earlier', state: 'ready', title: 'Earlier work', content: 'Ready · not integrated' }
  const before = structuredClone({ items, receipt })
  const selected = selectedItems(items, { latest_result: receipt })
  expect(selected.groups[0].state).toBe('blocked: uncertain')
  expect(selected.groups[0].rows.map(r => [r.content, r.status])).toEqual([
    ['Review · pending', 'pending'],
    ['Implementation · blocked: approval', 'in_progress'],
    ['Implementation 2 · blocked: uncertain', 'in_progress'],
  ])
  expect(selected.latest_result).toEqual(receipt)
  expect(selected.todos).toEqual(items.map(({ content, status }) => ({ content, status })))
  expect({ items, receipt }).toEqual(before)
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
    'Review · tests/scope · Awaiting review', 'Tests · not run', 'Implementation · blocked: uncertain',
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
