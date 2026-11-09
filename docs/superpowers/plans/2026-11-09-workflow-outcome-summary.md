# Workflow outcome summary implementation plan

**Goal:** Make workflow completion distinct from cancellation without restoring
model-driven todo updates or per-agent notification turns.

**Architecture:** Keep active todos unchanged. Export one separate latest result
for the passive TUI. Record one outcome event atomically with each newly terminal
workflow using the existing native notifier's durable admission and read-only
reconciliation. Foreman produces a concise final result, not a checklist refresh.

**Constraints:** No worker replay, historical event backfill, source integration,
publication, backend restart, phone-viewer changes or new paid qualification run.

- [x] Add failing workflow, runtime and UI-helper regression tests.
- [x] Add an optional `events.workflow_id` column without changing old event payloads.
- [x] Record one finish timestamp and uniquely keyed event in `WorkflowEngine.save`.
- [x] Use native synthetic input only for a final evidence-based workflow summary.
- [x] Export `progress.latest_result`; retain active-only todos and history.
- [x] Render a distinct Latest result block and validate its terminal state/text.
- [x] Update Foreman profile, MCP description, README and plugin guide.
- [x] Run full Python/UI verification and two independent read-only reviews.
  All 167 Python tests and 4 UI helper tests pass. Both reviews found no demonstrated
  issues. A concurrent full-suite run hit its tool timeout; the subsequent full run
  completed successfully in 40 seconds.
- [x] Back up live state and refresh only when safe; never backfill old notifications.
- [x] Confirm the existing completed pilot result renders without new agent input.

Verification commands: `python3 -B -m unittest discover -s tests -v`,
`bun test ui/teem-progress/progress.test.mjs`, and `git diff --check`.
Notification tests use fake/local transports, including uncertain response/restart
reconciliation. A new paid live workflow is not required to install this fix.

## Installed evidence

Backup: `/home/tom/.local/state/teem/backups/workflow-outcomes-20261109T183235Z`.
The Teem supervisor and pilot MCP were refreshed with Foreman idle and inbox empty.
Effective native Foreman configuration contains the final-summary guidance.

The exact pilot pane displays `Latest result · pilot · Compare label collections`
with `Completed · approved checks and both reviews passed; not integrated`, separate
from `current work (0)`. Foreman remained idle. All original rows across tasks,
workflows, agent reports, approvals, events, plans and progress event history match
the pre-refresh backup. Candidate patch SHA-256 digests still match. No historical
outcome notifications were added; no workers or reviews were replayed.

Future native final-summary admission/consumption and phone presentation remain
unqualified live. Local tests cover native payload validation and uncertain-admission
reconciliation. No OpenCode backend or optional White Noise service was changed.
