import { readFile } from 'node:fs/promises'
import { isAbsolute } from 'node:path'
import { Plugin } from '@opencode/plugin/tui'
import { createMemo, createSignal, For, Show } from 'solid-js'
import { nativeTodos, parseProjection, selectProgress } from './progress.mjs'

export default Plugin.define({
  id: 'local.teem.progress',
  setup(context) {
    const file = context.options.stateFile
    if (typeof file !== 'string' || !isAbsolute(file)) throw new Error('Teem needs an absolute stateFile')
    const [projection, setProjection] = createSignal<ReturnType<typeof parseProjection>>()
    const [now, setNow] = createSignal(Date.now() / 1000)
    let stopped = false
    let reading = false
    async function refresh() {
      if (reading || stopped) return
      reading = true
      try {
        const text = await readFile(file, 'utf8')
        if (text.length > 4 * 1024 * 1024) throw new Error('Teem projection too large')
        const value = parseProjection(JSON.parse(text))
        if (!stopped) setProjection(value)
      } catch {
        // Preserve the last observation; its timestamp visibly becomes stale.
      } finally {
        reading = false
        if (!stopped) setNow(Date.now() / 1000)
      }
    }
    void refresh()
    const timer = setInterval(() => void refresh(), 1000)
    context.ui.slot({
      append: 'sidebar.content',
      render(props) {
        const [open, setOpen] = createSignal(true)
        const passive = createMemo(() => selectProgress(projection(), props.sessionID, now()))
        const list = createMemo(() => passive()?.todos ?? nativeTodos(
          context.data.session.message.list(props.sessionID),
          context.data.session.get(props.sessionID)?.revert?.messageID,
        ))
        const remaining = createMemo(() => list().filter(t => t.status === 'pending' || t.status === 'in_progress').length)
        const theme = context.theme
        return <Show when={passive() || list().length > 0}>
          <box>
            <text fg={theme.text.base} onMouseDown={() => setOpen(!open())}>
              {open() ? '▾ ' : '▸ '}{passive() ? 'Teem · current work' : 'Todo'} ({remaining()})
            </text>
            <Show when={passive()?.stale}>
              <text fg={theme.text.feedback.warning.base}>Progress unavailable · last observation</text>
            </Show>
            <Show when={open()}>
              <Show when={passive() && !passive()?.stale && list().length === 0}>
                <text fg={theme.text.muted}>No active tasks · results remain in status history</text>
              </Show>
              <For each={list()}>{todo => <box flexDirection="row" gap={0}>
                <text flexShrink={0} fg={todo.status === 'in_progress' ? theme.text.feedback.warning.base : theme.text.muted}>
                  {({ completed: '[✓]', in_progress: '[•]', cancelled: '[-]', pending: '[ ]' })[todo.status]}{' '}
                </text>
                <text flexGrow={1} wrapMode="word" fg={theme.text.muted}>{todo.content}</text>
              </box>}</For>
              <Show when={passive()?.latest_result}>{outcome => <box marginTop={1}>
                <text fg={theme.text.base}>Latest result · {outcome().title}</text>
                <text wrapMode="word" fg={outcome().state === 'ready' ? theme.text.muted : theme.text.feedback.warning.base}>
                  {outcome().content}
                </text>
              </box>}</Show>
            </Show>
          </box>
        </Show>
      },
    })
    return () => { stopped = true; clearInterval(timer) }
  },
})
