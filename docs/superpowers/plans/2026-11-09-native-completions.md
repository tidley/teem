# Teem independent completion delivery

**Goal:** Make Teem optional and independent of Codr/White Noise, with the display name `Teem`.

**Architecture:** The supervisor admits completion events directly into the exact Foreman OpenCode session.
Use the current synthetic-input API with a persisted native message ID, queued delivery, and explicit resume.
The ordinary conversation continues to work without Teem. Codr/White Noise may share it independently.

**Tech stack:** Existing Python standard library, SQLite, Herdr plugin v1, OpenCode current HTTP API.

## Tasks

1. Add failing runtime tests in `tests/test_runtime.py` for native synthetic admission,
   exact payload verification, and acceptance reconciliation from inbox or projected message.
2. Add `tests/test_notifications.py` for one-shot delivery, crash recovery, receipt backoff,
   independent Foremen, acknowledgement, and legacy-event preservation.
3. Replace Codr-specific tests in `tests/test_plugin.py` with no-Codr import and binding tests.
   Assert the `Teem` display name without changing machine IDs, tool names, or state paths.
4. Implement `notify` and `event_accepted` in `foreman_runtime.py`.
   Use `POST /api/session/{foreman}/synthetic`, `delivery: queue`, and `resume: true`.
   Reconcile by exact message ID and payload. Never retry an uncertain admission automatically.
5. Move code-based notification scheduling into `foreman_supervisor.py`.
   Add a transport marker to event records. Mark pre-migration rows as legacy, without replay or deletion.
   Retain old columns for data preservation, but do not use Codr bindings or receipts.
6. Remove the copied Codr adapter and its tests from this repository only.
   Preserve `/home/tom/code/codr-foreman` and historical source hashes.
7. Update `foreman_plugin.py`, manifest, role example, and documentation.
   Remove the notification-file option and Codr configuration instructions.
8. Verify with `python3 -B -m unittest discover -s tests -v` and `git diff --check`.
   Request two independent read-only reviews, fix demonstrated findings, and verify again.
9. Back up stopped pilot state before its additive schema update.
   Update only Teem-owned display/configuration metadata. Keep the supervisor disabled and stopped.

## Acceptance

- Teem modules import and operate without any Codr module, executable, configuration, or socket.
- Completion events reach the Foreman's native inbox directly in fake-transport tests.
- Delivery acknowledgement does not imply Foreman consumption. Only explicit event acknowledgement closes an event.
- Failed or uncertain admissions remain visible. No terminal-write fallback or automatic resend exists.
- Existing bindings, tasks, history, and legacy event records survive migration.
- Codr settings and services remain unchanged. No live model request, worker start, merge, or publication occurs.
