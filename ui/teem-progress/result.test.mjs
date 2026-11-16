import { test, expect } from 'bun:test'
import { readFile } from 'node:fs/promises'
import * as progress from './progress.mjs'

const result = { workflow_id: 'workflow_a', state: 'ready', title: 'Theme update', content: 'Checks passed. Candidate ready; not integrated.' }
const fixture = () => progress.parseProjection({ version: 1, updated_at: 100, sessions: {
  ses_owner: { todos: [], latest_result: result }, ses_other: { todos: [] },
} })

test('result notice survives empty active work and labels stale evidence honestly', () => {
  expect(typeof progress.resultNotice).toBe('function')
  expect(progress.resultNotice(undefined)).toBeUndefined()
  for (const [state, label] of [['ready', 'Teem result available'], ['blocked', 'Teem needs attention'], ['needs_attention', 'Teem needs attention']]) {
    const selected = progress.selectProgress(fixture(), 'ses_owner', 100)
    selected.latest_result.state = state
    expect(progress.resultNotice(selected)).toEqual({ label, title: 'Theme update', content: result.content })
    selected.stale = true
    expect(progress.resultNotice(selected).label).toBe('Teem last result · status unavailable')
  }
  expect(progress.resultNotice(progress.selectProgress(fixture(), 'ses_other', 100))).toBeUndefined()
})

test('result command reads only the selected chat and opens retained evidence without a model call', async () => {
  expect(typeof progress.installResultCommand).toBe('function')
  let sessionID = 'ses_owner'
  let command
  let layer
  const dialogs = []
  const selections = []
  const context = {
    ui: { router: { current: () => ({ type: 'session', sessionID }) }, dialog: { alert: async input => { dialogs.push(input) } } },
    keymap: { layer: factory => { layer = factory(); command = layer.commands[0] } },
  }
  progress.installResultCommand(context, id => {
    selections.push(id)
    return progress.selectProgress(fixture(), id, 100)
  })
  expect(layer.mode).toBe('global')
  expect(command).toMatchObject({ id: 'teem.result', palette: true, slash: { name: 'teem-result' } })
  await command.run()
  expect(dialogs.pop()).toEqual({ title: 'Teem result available · Theme update', message: result.content })
  sessionID = 'ses_other'
  await command.run()
  expect(dialogs.pop().message).toContain('No saved Teem result for this chat')
  sessionID = undefined
  await command.run()
  expect(dialogs.pop().message).toContain('Open the intended chat')
  expect(selections).toEqual(['ses_owner', 'ses_other'])
})

test('linked result command uses explicit read-only linkage and keeps stale status visible', async () => {
  expect(typeof progress.installResultCommand).toBe('function')
  let command
  let dialog
  progress.installResultCommand({
    ui: { router: { current: () => ({ type: 'session', sessionID: 'ses_visible' }) }, dialog: { alert: async value => { dialog = value } } },
    keymap: { layer: factory => { command = factory().commands[0] } },
  }, id => progress.selectProgress(fixture(), id, 200, { progressSources: { ses_visible: 'ses_owner' } }))
  await command.run()
  expect(dialog.title).toBe('Teem last result · status unavailable · Theme update')
  expect(dialog.message).toContain('Linked work · original owner retained')
  expect(dialog.message).toContain(result.content)
  expect(dialog.message).toContain('Last observation; current status unavailable')
})

test('completion notice is mounted outside the collapsible sidebar in the composer slot', async () => {
  const tui = await readFile(new URL('./tui.tsx', import.meta.url), 'utf8')
  expect(tui).toContain("append: 'session.composer.top'")
  const composer = tui.slice(tui.indexOf("append: 'session.composer.top'"), tui.indexOf("append: 'sidebar.content'"))
  expect(composer).toContain('resultHint(')
  expect(composer.match(/<text\b/g)).toHaveLength(1)
  expect(composer).not.toContain('result().title')
  expect(composer).not.toContain('open()')
})

test('composer hint stays short and keeps attention and stale status explicit', () => {
  expect(typeof progress.resultHint).toBe('function')
  expect(progress.resultHint(undefined)).toBeUndefined()
  const selected = progress.selectProgress(fixture(), 'ses_owner', 100)
  selected.latest_result.title = 'An extremely long task title '.repeat(10)
  expect(progress.resultHint(selected)).toBe('Teem result · /teem-result')
  for (const state of ['blocked', 'needs_attention']) {
    selected.latest_result.state = state
    expect(progress.resultHint(selected)).toBe('Teem needs attention · /teem-result')
  }
  selected.stale = true
  expect(progress.resultHint(selected)).toBe('Teem result stale · /teem-result')
  expect(progress.resultHint(selected).length).toBeLessThan(40)
})

test('result command presents bounded multiline evidence without flattening or executing worker text', async () => {
  const details = 'Reported changes (worker claim)\nFixed stale qualification.\n\nChecks\nmocha: passed\n\nDeployment: not checked\u001b'
  const projection = progress.parseProjection({ version: 1, updated_at: 100, sessions: {
    ses_owner: { todos: [], latest_result: { ...result, details } },
  } })
  let command
  let shown
  progress.installResultCommand({
    ui: { router: { current: () => ({ sessionID: 'ses_owner' }) }, dialog: { alert() { throw new Error('Expected scrollable presenter') } } },
    keymap: { layer: factory => { command = factory().commands[0] } },
  }, id => progress.selectProgress(projection, id, 100), value => { shown = value })
  await command.run()
  expect(shown.message).toContain('qualification.\n\nChecks\nmocha: passed')
  expect(shown.message).not.toContain('\u001b')
  expect(shown.title).toContain('Theme update')
  expect(() => progress.parseProjection({ version: 1, updated_at: 100, sessions: {
    ses_owner: { todos: [], latest_result: { ...result, details: 'x'.repeat(10001) } },
  } })).toThrow('Invalid Teem outcome')
})

test('structured results expose the actual review blocker before earlier worker claims', async () => {
  const sections = [
    { title: 'Review blocked', body: 'Mandatory policy copy unavailable.\nNext: the Foreman must repair prerequisites and requalify.' },
    { title: 'Checks', body: 'regressions: passed\nwhitespace: passed' },
    { title: 'Reviews', body: 'Correctness: blocked\nScope: pass' },
    { title: 'Worker report', body: 'Earlier worker claim: **pending** verification' },
  ]
  const fixture = progress.parseProjection({ version: 1, updated_at: 100, sessions: {
    ses_owner: { todos: [], latest_result: { ...result, state: 'blocked', sections } },
  } })
  const selected = progress.selectProgress(fixture, 'ses_owner', 100)
  expect(selected.latest_result.sections).toEqual(sections)
  expect(progress.resultHint(selected)).toBe('Teem review blocked · /teem-result')
  let command, shown
  progress.installResultCommand({ ui: { router: { current: () => ({ sessionID: 'ses_owner' }) } }, keymap: { layer: factory => { command = factory().commands[0] } } }, () => selected, value => { shown = value })
  await command.run()
  expect(shown.sections).toEqual(sections)
  expect(() => progress.parseProjection({ version: 1, updated_at: 100, sessions: {
    ses_owner: { todos: [], latest_result: { ...result, sections: [{ title: 'Too big', body: 'x'.repeat(10001) }] } },
  } })).toThrow('Invalid Teem outcome')
})

test('an archived current result cannot fall back to an older linked-chat blocker', () => {
  const projection = progress.parseProjection({ version: 1, updated_at: 100, sessions: {
    ses_owner: { todos: [], latest_result: { ...result, state: 'blocked' } },
    ses_current: { todos: [], result_notice_cleared: true },
  } })
  const selected = progress.selectProgress(projection,'ses_current',100,{progressSources:{ses_current:'ses_owner'}})
  expect(selected.latest_result).toBeUndefined()
  expect(selected.sourceSessionID).toBeUndefined()
  expect(progress.resultHint(selected)).toBeUndefined()
})

test('saved blocker metadata identifies the affected project, action and observation time', async () => {
  const blocker = { kind:'source_changes', project:'teem', source:'/code/teem', reason:'Source has unfinished changes; none discarded', action:'Finish or commit existing work; preserve unrelated changes, or archive the superseded notice.', current_source_checked:false }
  const projection = progress.parseProjection({version:1,updated_at:100,sessions:{ses_owner:{todos:[],latest_result:{...result,state:'blocked',historical:true,recorded_at:50,blocker}}}})
  const selected = progress.selectProgress(projection,'ses_owner',100)
  expect(selected.latest_result.blocker).toEqual(blocker)
  expect(progress.resultNotice(selected).label).toBe('Teem saved stopped attempt')
  expect(progress.resultNotice(selected).content).toContain('Next:')
  expect(progress.resultNotice(selected).content).toContain('/code/teem')
  let command, shown
  progress.installResultCommand({ui:{router:{current:()=>({sessionID:'ses_owner'})}},keymap:{layer:factory=>{command=factory().commands[0]}}},()=>selected,value=>{shown=value})
  await command.run()
  expect(shown.recorded_at).toBe(50)
  expect(shown.message).toContain('preserve unrelated changes')
})

test('unsubmitted saved plans are not labelled active executions', () => {
  const row = {content:'Old task — not submitted or unavailable',status:'pending'}
  const projection = progress.parseProjection({version:1,updated_at:100,sessions:{
    ses_saved:{todos:[row],items:[{...row,source:'supervisor',task_state:'unavailable'}]},
    ses_running:{todos:[row],items:[{...row,source:'supervisor',task_state:'running'}]},
  }})
  expect(progress.selectProgress(projection,'ses_saved',100).executionActive).toBe(false)
  expect(progress.selectProgress(projection,'ses_running',100).executionActive).toBe(true)
})

test('long valid checkout paths and malformed optional items preserve usable progress', () => {
  const source = '/' + 'nested/'.repeat(320)
  const row = {content:'Saved task',status:'pending'}
  const blocker = {kind:'source_changes',project:'teem',source,reason:'Unfinished changes',action:'Preserve work',current_source_checked:false}
  const projection = progress.parseProjection({version:1,updated_at:100,sessions:{ses_owner:{todos:[row],items:[null],latest_result:{...result,state:'blocked',blocker}}}})
  const selected = progress.selectProgress(projection,'ses_owner',100)
  expect(selected.todos).toEqual([row])
  expect(selected.latest_result.blocker.source).toBe(source)
  expect(selected.executionActive).toBe(false)
})
