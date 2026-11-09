# Teem

A local Herdr plugin with one visible Foreman and up to two workers plus two reviewers per managed task.
The supervisor is code, not another agent. Python 3.11 standard library only.

```text
Herdr ↔ Foreman ↔ Teem supervisor ↔ Workers/reviewers
           ↕
     Codr/White Noise — optional
```

Teem and Codr/White Noise are independent, optional integrations.
Teem does not import, call, configure, start, or restart Codr.
Without Teem, an ordinary agent conversation works as before.
With Teem, an explicitly bound Foreman can delegate and receive native completion events.

## Status

Early local-first MVP, developed independently from Codr.
`docs/SOURCE_SNAPSHOT.json` records the copied files and their original SHA-256 hashes.
Source: [github.com/tidley/teem](https://github.com/tidley/teem).
No paid worker run is part of repository setup.

## Components

- `herdr-plugin.toml`: Teem actions, startup hook, and transient status popup.
- `foreman_plugin.py`: plugin commands, fixed-binding MCP delegation tools, and optional service unit output.
- `foreman_supervisor.py`: SQLite tasks, per-task 2+2 limits, exclusive checkout ownership, and completion events.
- `foreman_runtime.py`: headless OpenCode transport, native completion admission, and permission rules.
- `foreman_progress.py`: persistent plans and atomic private progress snapshots; no model-driven UI refresh.
- `ui/teem-progress`: passive OpenCode TUI checklist; retains native todos for other conversations.
- `foreman_checkouts.py`: private complete clones and hashed uncommitted candidate snapshots.
- `foreman_checks.py`: asynchronous project-approved checks and bound output receipts.
- `foreman_workflows.py`: bounded implementation/check/two-reviewer/revision progression.
- `foreman_workflow_progress.py` and `foreman_agent_progress.py`: readable phase/agent checklists and read-only native todo observations.

## Verify

From this directory:

```sh
python3 -B -m unittest discover -s tests -v
bun test ui/teem-progress/progress.test.mjs
```

Tests use temporary repositories and fake/local transports, not live models or credentials.

## Setup and limits

See [the plugin guide](docs/HERDR_PLUGIN.md) and [the local pilot](docs/PILOT.md).
The legacy task API uses separate authorised checkouts. Workers may be sparse; legacy reviewers must be complete and pinned to a commit.
Managed workflows choose their own base and prepare separate complete clones from a configured source project.
They run approved checks and review the exact uncommitted patch/tree snapshot without creating source commits.
The supervisor does not grant blanket shell access, integrate changes, merge, push, deploy, or replay uncertain tasks.
Code permissions are not an operating-system sandbox. Existing phone admission and desktop draft races remain unqualified.
The pilot Foreman uses a project-local runtime profile. This does not impose that profile on other conversations.
Completion admission is not Foreman consumption. Uncertain delivery is reconciled by exact native message ID, never automatic resend.
Older notification records remain marked `legacy-codr` for manual inspection, without replay.
New workers and reviewers inherit Foreman's selected model at dispatch; the selection
is recorded per run. The passive TUI reads supervisor progress and agent-reported todos
without waking Foreman. It shows current work; completed results remain in status history.
Individual agent completion receipts do not trigger model turns. One final workflow
outcome asks Foreman for a concise result summary. The sidebar retains the latest
result separately from active todos, so completion cannot look like cancellation.
Managed workflows provide project-approved checks and bounded review/revision loops.
Their `ready` state qualifies the exact candidate under its recorded policy, not an
integrated project or release. Untrusted check scripts require stronger isolation
than this same-user local MVP supplies. Paid end-to-end runs still need qualification.

Plugin ID: `local.teem`. Optional user service: `teem.service`.
Internal `foreman_*` tool names and native task identifiers remain stable from the prototype.
