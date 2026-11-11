import { spawn } from 'node:child_process'
import { isAbsolute } from 'node:path'

// Fixed argv and stdin JSON, never a shell or location-global MCP rebinding.
export function nativeBridge(options, command, input, signal) {
  return new Promise((resolve, reject) => {
    if (!['enable-chat', 'native-tools', 'native-call'].includes(command)
      || !isAbsolute(options?.executable ?? '') || !isAbsolute(options?.config ?? '')) {
      reject(new Error('Teem needs absolute executable and config options'))
      return
    }
    if (signal?.aborted) { reject(new Error('Teem request aborted')); return }
    let child
    try {
      child = spawn(options.executable, ['--config', options.config, command], { stdio: ['pipe', 'pipe', 'pipe'] })
    } catch (error) { reject(error); return }
    let done = false
    let size = 0
    const stdout = []
    const stderr = []
    const finish = (error, value) => {
      if (done) return
      done = true
      clearTimeout(timer)
      signal?.removeEventListener('abort', abort)
      if (error) { child.kill('SIGKILL'); reject(error) } else resolve(value)
    }
    const abort = () => finish(new Error('Teem request aborted'))
    const timer = setTimeout(() => finish(new Error('Teem bridge timed out; inspect status before retrying')), 15000)
    signal?.addEventListener('abort', abort, { once: true })
    const collect = target => chunk => {
      size += chunk.length
      if (size > 1024 * 1024) finish(new Error('Teem bridge output exceeds 1 MiB'))
      else target.push(chunk)
    }
    child.stdout.on('data', collect(stdout))
    child.stderr.on('data', collect(stderr))
    child.on('error', error => finish(error))
    child.stdin.on('error', error => finish(error))
    child.on('close', code => {
      if (done) return
      if (code !== 0) {
        finish(new Error(Buffer.concat(stderr).toString('utf8').trim() || `Teem bridge exited ${code}`))
        return
      }
      try { finish(null, JSON.parse(Buffer.concat(stdout).toString('utf8'))) }
      catch { finish(new Error('Teem bridge returned invalid JSON')) }
    })
    try { child.stdin.end(input === undefined ? '' : JSON.stringify(input)) }
    catch (error) { finish(error) }
  })
}

export async function enableCurrentChat(context, raw, invoke = nativeBridge) {
  if (typeof raw !== 'string' || raw.trim() !== 'enable') throw new Error('Use /teem enable')
  const sessionID = context.ui.router.current?.sessionID
  if (!sessionID) throw new Error('Open the intended chat before /teem enable')
  // Snapshot the selector before sync can yield to navigation to another chat.
  const current = context.ui.model.current
  if (!current?.providerID || !current.modelID) throw new Error('Select a provider and model before /teem enable')
  const model = { providerID: current.providerID, id: current.modelID }
  if (current.variant !== undefined) model.variant = current.variant
  await context.data.session.sync(sessionID)
  const session = context.data.session.get(sessionID)
  if (session?.id !== sessionID || !session.agent || !session.projectID || !session.location?.directory) {
    throw new Error('Current chat identity unavailable; refresh the chat before /teem enable')
  }
  const result = await invoke(context.options, 'enable-chat', {
    sessionID, agent: session.agent, projectID: session.projectID,
    directory: session.location.directory, model,
  })
  if (result?.enabled !== true || result.sessionID !== sessionID || result.started !== false) {
    throw new Error('Teem did not confirm enablement for this chat; inspect status')
  }
  return result
}

export function installEnableCommand(context, feedback, invoke = nativeBridge) {
  return context.keymap.layer({
    commands: [{
      id: 'teem.enable', title: 'Teem: enable this chat', group: 'Teem', palette: true,
      slash: { name: 'teem', arguments: true },
      async run(input) {
        // Both success and failure stay with the originating chat, across awaits.
        const sessionID = context.ui.router.current?.sessionID
        try {
          const result = await enableCurrentChat(context, input, invoke)
          feedback(sessionID, `Teem enabled · ${result.project} · no work started`)
        } catch (error) {
          feedback(sessionID, `Teem enable failed: ${error instanceof Error ? error.message : String(error)}`)
        }
      },
    }],
  })
}

export async function installNativeTools(context, invoke = nativeBridge) {
  const definitions = await invoke(context.options, 'native-tools')
  if (!Array.isArray(definitions) || definitions.some(d => !d?.name || !d.description || !d.inputSchema)) {
    throw new Error('Teem native tool catalog unavailable')
  }
  return context.tool.transform(editor => {
    for (const definition of definitions) editor.add({
      name: definition.name, description: definition.description, input: definition.inputSchema,
      execute: async (input, native) => ({
        content: JSON.stringify(await invoke(context.options, 'native-call', {
          name: definition.name, arguments: input, sessionID: native.sessionID, agent: native.agent,
        }, native.signal), null, 2),
      }),
    })
  })
}
