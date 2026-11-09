# Live Progress Implementation Plan

**Goal:** supervisor-backed native Foreman checklists and inherited dispatch models.

**Architecture:** additive SQLite plan/model/progress records; fixed-bound MCP plan
tool; native synthetic notifications; actual native `todowrite` rendering by Foreman.

**Tech stack:** Python 3.11 standard library, SQLite, current OpenCode HTTP API.

Work inline in this checkout; no commits or implementation subagents.

1. Add failing tests in `tests/test_supervisor.py` and `tests/test_runtime.py` for
   recorded dispatch models, immutable snapshots, malformed references, and restarts.
   Add `runtime_model` to tasks, persist once before create, and use it in native
   identity validation. Keep legacy null models on configured fallback.
2. Add `tests/test_progress.py` for durable ordered plans and task-driven todo states.
   Add fixed-bound `foreman_plan` with strict schemas and validation. Reject duplicate
   IDs, cross-Foreman links, and model attempts to override linked task states.
3. Add local/fake transport tests for progress notifications: stable message IDs,
   same-state silence, coalescing, uncertain read-only reconciliation, completion
   coverage, and per-Foreman isolation. Implement `foreman_progress.py` and native
   progress payload/receipt methods; reuse existing event validation contracts.
4. Update `foreman_plugin.py` service/status, example profile and docs. Enable native
   todos only in the opted-in project profile. Require reading status before todo
   updates and describe execution vs engineering evidence accurately.
5. Run `python3 -B -m unittest discover -s tests -v` and `git diff --check`. Request two
   independent read-only reviews in parallel; fix demonstrated findings and verify.
6. Inspect/back up pilot state and profile. Refresh only Teem/MCP and opted-in Foreman
   configuration if idle and safe. Do not send a live progress event or start paid
   work just to test connectivity. Report source, running integration and live-client
   qualification separately.
