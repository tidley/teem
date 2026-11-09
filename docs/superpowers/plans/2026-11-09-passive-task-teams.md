# Passive task teams implementation record

**Goal:** Keep Foreman available for chat while current worker/reviewer progress
updates without model turns; allocate up to two workers and two reviewers per task.

**Architecture:** Optional disjoint worker assignments share one workflow, not a
checkout. The supervisor validates and combines their patches, then checks and
reviews one fixed candidate. A local TUI plugin reads private atomic progress
snapshots; it does not write native tool history or wake Foreman.

**Constraints:** Preserve source, candidates, receipts, native inputs and immutable
requests. No commits, merges, publication, paid qualification runs, backend restart,
White Noise changes or broadened shell permissions.

## Implementation and verification

- [x] Add failing workflow tests for per-task concurrency, combined candidates,
  disjoint ownership, uncertain workers, current-only display and passive receipts.
- [x] Implement `workers` admission and per-workflow role limits in
  `foreman_workflows.py`, `foreman_supervisor.py`, and `foreman_plugin.py`.
- [x] Test combined-candidate revisions and independent reviewer teams. Detect the
  demonstrated ignore-rule interaction that could drop a teammate's new file.
- [x] Replace progress synthetic admissions with atomic mode-0600 `progress.json`
  in `foreman_progress.py`; preserve historical admission receipts.
- [x] Add `ui/teem-progress` using the public OpenCode TUI slot API, with native
  todo fallback for unbound sessions and a stale-observation warning.
- [x] Update profile and setup documentation to remove model-driven refreshes.
- [x] Run `python3 -B -m unittest discover -s tests -v` and
  `bun test ui/teem-progress/progress.test.mjs`.
- [x] Resolve both independent read-only reviews and verify again. The correctness
  review found a single explicit worker scope bypass; its failing regression now
  passes, and both reviewers cleared the fix. All 161 Python and 3 UI tests pass.
- [x] Back up pilot state and configuration; refresh only when idle without
  replaying tasks or deleting old native inputs.
- [x] Verify actual TUI load/rendering separately from pure helper tests. Record
  limits for live multi-worker qualification and independent phone viewing.

## Installed pilot evidence

Backup: `/home/tom/.local/state/teem/backups/passive-task-teams-20261109T181314Z`.
The existing Teem supervisor and pilot MCP connection were refreshed. OpenCode's
backend and White Noise were not restarted or changed. Effective Foreman agent
configuration contains the passive guidance.

Herdr's exact pilot pane (`w7:p1`, matched by native session identity) rendered the
new passive panel. One explicitly labelled UI-only verification step appeared and
was removed after the check. Foreman's assistant message IDs stayed unchanged,
its inbox stayed empty and it stayed idle. The original empty plan was restored.
All ten task records, three completed workflows and unintegrated candidates remain.
Old native todo tool calls remain visible in conversation history, not in the live
sidebar. No agents were replayed or new paid qualification workflows dispatched.

Multi-worker live execution, stale-warning live rendering and independent phone
projection are not qualified. The phone viewer does not receive the local TUI feed.
