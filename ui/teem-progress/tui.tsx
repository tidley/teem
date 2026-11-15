import { readFile } from 'node:fs/promises'
import { Plugin } from '@opencode/plugin/tui'
import { createMemo, createSignal, For, Show } from 'solid-js'
import { installResultCommand, legacyTodoFallback, nativeTodos, parseProjection, resultNotice, selectProgress } from './progress.mjs'
import { installEnableCommand, projectionFile } from './enable.mjs'

export default Plugin.define({
  id: 'local.teem.progress',
  setup(context) {
    const [projection, setProjection] = createSignal<ReturnType<typeof parseProjection>>()
    const [now, setNow] = createSignal(Date.now() / 1000)
    const [feedback, setFeedback] = createSignal<Record<string, string>>({})
    installResultCommand(context, (sessionID: string) => selectProgress(projection(), sessionID, now(), context.options))
    installEnableCommand(context, (sessionID: string, message: string) => {
      if (sessionID) setFeedback(previous => ({ ...previous, [sessionID]: message }))
    })
    let stopped = false
    let reading = false
    async function refresh() {
      if (reading || stopped) return
      reading = true
      try {
        const file = await projectionFile(context.options)
        if (!file) return
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
      append: 'session.composer.top',
      render(props) {
        const notice = createMemo(() => resultNotice(selectProgress(projection(), props.sessionID, now(), context.options)))
        return <Show when={notice()}>{result => <box flexShrink={0}>
          <text wrapMode="word" fg={context.theme.text.base}>{result().label} · {result().title}</text>
          <text wrapMode="word" fg={context.theme.text.muted}>Read saved result: /teem-result</text>
        </box>}</Show>
      },
    })
    context.ui.slot({
      append: 'sidebar.content',
      render(props) {
        const [open, setOpen] = createSignal(true)
        const passive = createMemo(() => selectProgress(projection(), props.sessionID, now(), context.options))
        // Fresh installations leave ordinary Todo rendering to the host.
        // Older explicit-stateFile setups disabled that panel; retain their fallback.
        const list = createMemo(() => passive()?.todos ?? (legacyTodoFallback(context.options) ? nativeTodos(
          context.data.session.message.list(props.sessionID),
          context.data.session.get(props.sessionID)?.revert?.messageID,
        ) : []))
        const groups = createMemo(() => passive()?.groups ?? [{ id: 'native', rows: list() }])
        const remaining = createMemo(() => groups().flatMap(g => g.rows).filter(t => t.status === 'pending' || t.status === 'in_progress').length)
        const reported = createMemo(() => groups().some(g => g.rows.some(t => t.reported)))
        const theme = context.theme
        return <>
          <Show when={feedback()[props.sessionID]}>{message =>
            <text wrapMode="word" fg={theme.text.base}>{message()}</text>
          }</Show>
          <Show when={passive() || list().length > 0}>
          <box>
            <Show when={passive()?.sourceSessionID}>
              <text wrapMode="word" fg={theme.text.muted}>Linked work · original owner retained</text>
            </Show>
            <text fg={theme.text.base} onMouseDown={() => setOpen(!open())}>
              {open() ? '▾ ' : '▸ '}{passive() ? 'Teem · current work' : 'Todo'} ({remaining()})
            </text>
            <Show when={passive()?.stale}>
              <text fg={theme.text.feedback.warning.base}>Progress unavailable · last observation</text>
            </Show>
            <Show when={open()}>
              <Show when={passive() && !passive()?.stale && groups().every(g => !g.title && g.rows.length === 0)}>
                <text fg={theme.text.muted}>No active tasks · results remain in status history</text>
              </Show>
              <Show when={reported()}>
                <text wrapMode="word" fg={theme.text.muted}>Reported = agent claim, not evidence</text>
              </Show>
              <For each={groups()}>{group => <box>
                <Show when={group.title}>
                  <text wrapMode="word" fg={theme.text.base}>{group.title}</text>
                  <text wrapMode="word" fg={theme.text.muted}>{group.description} · {group.state}</text>
                </Show>
                <For each={group.rows}>{todo => <box flexDirection="row" gap={0}>
                  <text flexShrink={0} fg={todo.status === 'in_progress' ? theme.text.feedback.warning.base : theme.text.muted}>
                    {({ completed: '[✓]', in_progress: '[•]', cancelled: '[-]', pending: '[ ]' })[todo.status]}{' '}
                  </text>
                  <text flexGrow={1} wrapMode="word" fg={theme.text.muted}>{todo.content}</text>
                </box>}</For>
              </box>}</For>
              <Show when={passive()}>
                <text fg={theme.text.muted}>Details/history: foreman_status</text>
              </Show>
              <Show when={passive()?.latest_result}>{outcome => <box marginTop={1}>
                <text fg={theme.text.base}>Latest result · {outcome().title}</text>
                <text wrapMode="word" fg={outcome().state === 'ready' ? theme.text.muted : theme.text.feedback.warning.base}>
                  {outcome().content}
                </text>
              </box>}</Show>
            </Show>
          </box>
          </Show>
        </>
      },
    })
    return () => { stopped = true; clearInterval(timer) }
  },
})
