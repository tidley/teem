// Pure display selection. These observations never qualify checks or reviews.
const statuses = new Set(['pending', 'in_progress', 'completed', 'cancelled'])

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
      typeof value.content !== 'string' || !value.content || value.content.length > 2000) {
    throw new Error('Invalid Teem outcome')
  }
  const clean = text => text.replace(/[\x00-\x1f\x7f]/g, ' ')
  return { latest_result: { workflow_id: value.workflow_id, state: value.state,
    title: clean(value.title), content: clean(value.content) } }
}

export function parseProjection(value) {
  if (!value || value.version !== 1 || !Number.isFinite(value.updated_at) ||
      !value.sessions || typeof value.sessions !== 'object' || Array.isArray(value.sessions)) {
    throw new Error('Invalid Teem projection')
  }
  return { updated_at: value.updated_at, sessions: Object.fromEntries(Object.entries(value.sessions).map(
    ([id, progress]) => [id, { todos: todos(progress?.todos), ...latestResult(progress?.latest_result) }],
  )) }
}

export function selectProgress(projection, sessionID, now = Date.now() / 1000) {
  const session = projection?.sessions[sessionID]
  if (!session) return undefined
  return { todos: session.todos, ...latestResult(session.latest_result), passive: true,
    stale: now - projection.updated_at > 15 || projection.updated_at > now + 5 }
}

// Same public message contract as OpenCode's native Todo panel. No synthetic
// messages or tool calls are created. Non-Teem sessions keep their own todos.
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
