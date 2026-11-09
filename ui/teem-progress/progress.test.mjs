import { test, expect } from 'bun:test'
import { parseProjection, selectProgress, nativeTodos } from './progress.mjs'

const todo = { content: 'Implement labels', status: 'in_progress' }
const data = { version: 1, updated_at: 100, sessions: { ses_foreman: { todos: [todo] } } }

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
