# TEEM Herdr plugin

Local MVP, version 0.1.0. This is original code, not a copy of the private PAVE agent system.
Repository setup does not authorise model tasks. See `docs/PILOT.md` for the local pilot and its activation gate.
Herdr 0.9.1 accepted the original manifest in a temporary, isolated plugin registry.

## What it provides

- One explicitly registered Foreman conversation, visible through Herdr and Codr's existing White Noise mapping.
- A Python supervisor with persistent SQLite task and event records.
- Two implementation slots and two reviewer slots across the entire supervisor, not per conversation.
- Headless OpenCode sessions. Workers do not create Herdr panes or additional Codr chats.
- Structured MCP delegation tools bound to one registered Foreman session.
- Caller-retained task UUIDs. Duplicate admission returns the original task instead of repeating work.
- Separate queue acceptance, native execution, terminal outcome, notification acceptance, and event acknowledgement.
- Existing sparse checkout support for workers. Review requires a clean, complete checkout at a fixed commit.

Native `succeeded` means the execution ended with a text result. It is not an engineering PASS.
The plugin does not parse review prose into an approval or merge code.

## Requirements

- Linux, Python 3.11+, Git, and a systemd user manager.
- Herdr 0.9.1+ for the plugin interface.
- OpenCode's current `/api/session` protocol, as inspected in the local 2.1.4 source.
  This is not the older `/session` SDK protocol. Other OpenCode versions need contract qualification.
- A configured OpenCode background service and an explicitly selected model.
- Existing authorised Git checkout roots. Do not assign a checkout with unrelated changes.
- Codr's optional local queue interface for automatic Foreman completion wakes.

The plugin never launches, elects, replaces, or restarts OpenCode's background service.
Python uses only its standard library. Tests do not use live models or services.

## Explicit local setup

These commands change local configuration. They are operator instructions, not steps already performed.

1. Review `herdr-plugin.toml`, `foreman_plugin.py`, `foreman_supervisor.py`, and `foreman_runtime.py`.
2. Link the local plugin:

   ```sh
   herdr plugin link /home/tom/code/teem
   herdr plugin config-dir local.teem
   ```

3. Copy `examples/foreman.toml` to `config.toml` inside the printed plugin configuration directory.
4. Set a private `state_dir`, local service `registry`, explicit `model`, and `allowed_checkouts` in that file.
5. Leave `enabled = false` until you authorise model execution.
6. Focus the intended Foreman OpenCode pane. Invoke the `local.teem.bind` plugin action.
   This records its exact native session ID and terminal ID. A display name alone cannot bind it.
7. Generate a service unit with the absolute configuration path:

   ```sh
   python3 -B foreman_plugin.py --config /absolute/plugin/config.toml service-unit
   ```

8. Review and save that output as `~/.config/systemd/user/teem.service`.
   Run `systemctl --user daemon-reload`. This does not start the supervisor.
9. After authorisation, set `enabled = true` and invoke the plugin's start action.
   To start after login without opening Herdr, explicitly enable the user service.
   This MVP does not change login, linger, or boot policy.

The startup hook starts only an already configured service. Missing configuration is a no-op.
An enabled configuration without an installed unit produces a visible service error.
Herdr startup hooks are not daemon supervisors. systemd owns the supervisor process.

## Foreman tools

Configure a local MCP server for the intended Foreman runtime. Merge this example into existing configuration.
Do not overwrite an existing OpenCode configuration file.

```json
{
  "mcp": {
    "servers": {
      "foreman": {
        "type": "local",
        "command": [
          "python3", "-B", "/home/tom/code/teem/foreman_plugin.py",
          "--config", "/absolute/plugin/config.toml",
          "mcp", "--foreman", "ses_EXACT_REGISTERED_SESSION"
        ]
      }
    }
  }
}
```

The paths and native session ID in this example must match your explicit local binding.
Check the model runtime's configuration reload requirements before use.
The MCP tools appear with their own schemas and descriptions. They do not depend on an `AGENTS.md` role instruction.

- `foreman_submit`: provide a retained UUID, role, checkout, full base commit, relative file scope, and assignment.
- `foreman_status`: read tasks, exact native session IDs, results, and unacknowledged events.
- `foreman_ack`: acknowledge one event after consuming its result. This does not approve code.

Submission returns after durable queue admission. The supervisor performs execution separately.
A Foreman should finish its dispatch response rather than wait in a synchronous polling loop.
The runtime still serialises that Foreman's own conversation. This is not simultaneous model execution in one conversation.

The fixed MCP binding prevents tool arguments from selecting another Foreman.
It does not authenticate the calling model session. Configure it only where intended.
Binding does not remove the visible Foreman's existing implementation tools.
This MVP supplies delegation tools, not mandatory routing of every conversational message.
This is a same-user local integration, not protection against arbitrary local processes.
Background sessions deny MCP tools and delegation through their runtime permission rules.

## Codr notifications

Automatic completion wakes are optional. Without a Codr binding, completion events remain available through the status tool.
After inspection, prepare a private JSON file with these exact fields:

```json
{
  "codr": "/home/tom/.local/bin/herdr-white-noise-bridge",
  "config": "/home/tom/.config/herdr-white-noise-bridge/config.toml",
  "bridge_account": "EXACT_64_HEX_ACCOUNT_FROM_CODR",
  "session_key": "[\"herdr:opencode\",\"opencode\",\"id\",\"ses_EXACT_REGISTERED_SESSION\"]",
  "terminal_id": "EXACT_FOREMAN_TERMINAL_ID"
}
```

From the focused Foreman pane inside Herdr, run:

```sh
python3 -B /home/tom/code/teem/foreman_plugin.py \
  --config /absolute/plugin/config.toml bind \
  --notification-file /absolute/private/notification.json
```

The session key and terminal must match the native Foreman binding.
Use Codr's existing snapshot and queue setup to obtain these values. Do not infer them from names.
The supervisor sends one wake through `codr_foreman.Client`. It never falls back to terminal writes.
Wakes contain event/task IDs, not raw worker output. Foreman retrieves results through its tools.
Codr's forwarding/readiness controls still apply. Queue acceptance is not proof that Foreman consumed an event.
Only `foreman_ack` closes an event. An uncertain notification is reconciled by receipt reads, never automatic resend.

The existing phone submission and desktop draft race remains outside this MVP.
The plugin shares the existing conversation. It does not provide a new atomic mobile/desktop input router.

## Sparse work and integration

Git worktrees share repository objects but normally materialise all tracked files.
Sparse checkout reduces the materialised file set. It does not make every dependency or test available.
This plugin accepts existing sparse checkouts. It does not create, alter, clean up, or delete them.

Record a narrow task scope and immutable base revision. Each live task owns its assigned checkout exclusively.
Sparse tasks report `sparse: true`. Do not classify missing-dependency test failures as code failures or as passing checks.
Before review, produce a fixed commit through the project's authorised source workflow.
Assign reviewers complete, clean checkouts at that commit. Two concurrent reviewers need separate checkout roots in this MVP.
Run full verification against the combined result before integration. Do not merge sparse-task results merely because individual agents finished.

Workers can edit only their declared relative scope through built-in edit tools. Shell commands require human approval.
Reviewers can use read, glob, and grep. They cannot edit, run shell commands, delegate, or use MCP tools.
These restrictions are runtime permissions, not an operating-system filesystem sandbox.
Human shell approval can permit actions outside those tool restrictions. Never treat shell approval as routine unattended consent.
The plugin never auto-approves permissions or performs commits, merges, pushes, deployments, or hardware operations.

## Status and recovery

Open the plugin's status popup on demand. It is not a persistent worker pane.
The popup is a snapshot, not a live dashboard. Close and reopen it to refresh.
Use the plugin's status action or run `herdr plugin pane open --plugin local.teem --entrypoint status`.
For machine-readable status:

```sh
python3 -B foreman_plugin.py --config /absolute/plugin/config.toml \
  status --foreman ses_EXACT_REGISTERED_SESSION
```

`starting`, `accepted`, `running`, and `uncertain` retain a slot and exclusive checkout ownership.
A crash after native submission does not cause another prompt submission.
Reconciliation reads the exact session, prompt message, and inbox IDs.
An idle session without the expected native input is uncertain, not successful.
Changed native identity or permissions also retain the reservation for inspection.

If a task needs permission or an answer, status reports the block and native session ID.
Inspect that session with OpenCode before approving anything. There is no automatic permission approval tool.
Stopping the supervisor does not cancel backend-owned OpenCode execution.
Interrupt the exact native session through OpenCode when cancellation is required.
After a worker has no accepted input, an uncertain reservation needs manual investigation.
This MVP has no force-release or automatic replay command. Do not edit live state to bypass the reservation.

After reboot, the supervisor reconciles persisted task IDs when explicitly started or enabled as a user service.
It does not create replacement sessions for uncertain tasks.
Herdr and the underlying OpenCode runtime retain responsibility for their own restoration.

## Verification

```sh
python3 -B -m unittest discover -s tests -v
```

Tests use temporary state, temporary Git repositories, fake runtime transports, and a local fake HTTP server.
They do not contact live Codr, Herdr, OpenCode credentials, GitHub, relays, or models.
Live installation, concurrency, permission enforcement, desktop/mobile delivery, and reboot recovery still need an authorised pilot.
