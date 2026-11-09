# Live Foreman checklist and inherited models

Scope: implement the agreed live checklist and model inheritance. Managed checkouts,
automatic command policies, and implementation/review orchestration remain separate
milestones. This delivery does not broaden execution approval or start model workers.

Foreman defines an ordered plan through a fixed-binding MCP tool. SQLite stores the
plan. Entries can link to retained task UUIDs; linked execution state is computed by
the supervisor, never supplied by the model. Unlinked planning entries retain their
explicit manual state. A linked successful run means execution ended, not tests or
review passed. Missing, failed, interrupted and uncertain tasks remain explicit.

OpenCode exposes native todos through actual `todowrite` tool calls, not a todo HTTP
write endpoint. The supervisor publishes bounded synthetic state-change events asking
Foreman to read authoritative status and refresh its native todos. The opted-in Foreman
profile enables the native tool. This uses the existing optional White Noise todo
reader without importing, configuring, starting, or restarting Codr. Native todo
rendering remains a live-client qualification, not a claim based on HTTP admission.

Progress snapshots are persisted, coalesced and fingerprinted. Notifications use
stable native IDs; uncertain sends are reconciled by reads, never replayed. Completion
notifications also request todo refresh, so a terminal result does not require a
second progress notification. Progress never acknowledges completion on Foreman's behalf.
Polling the same state is silent. Native progress events can consume Foreman tokens;
they do not dispatch implementation or review workers. No output prose is parsed into
test/review approval.

At first dispatch, the exact bound native Foreman session supplies provider, model and
variant. The selection is persisted with the task before session creation. Retries and
reconciliation use that recorded model, not a later Foreman or configuration choice.
Legacy records retain their configured-model behavior. Failed model reads reserve the
task without creating a replacement or repeating prompts.

Safety: preserve all prior database columns/rows and native bindings. No commits,
merges, publication, deployment, model worker runs, or global OpenCode changes. Local
activation requires an idle Foreman and no unexpected queued/live tasks. Back up state
and local profile before updating them. Verify local tests and two independent
read-only reviews before completion.
