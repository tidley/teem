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
  const dialogs = []
  const selections = []
  const context = {
    ui: { router: { current: () => ({ type: 'session', sessionID }) }, dialog: { alert: async input => { dialogs.push(input) } } },
    keymap: { layer: factory => { command = factory().commands[0] } },
  }
  progress.installResultCommand(context, id => {
    selections.push(id)
    return progress.selectProgress(fixture(), id, 100)
  })
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
  expect(composer).toContain('resultNotice(')
  expect(composer).toContain('/teem-result')
  expect(composer).not.toContain('open()')
})
