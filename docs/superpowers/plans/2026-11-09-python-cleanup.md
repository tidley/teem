# Python-free Teem cleanup

## Outcome

Keep the Rust supervisor and the TypeScript OpenCode adapter. Remove the old
implementation, compatibility links, Python test suite and interpreter-backed
test helpers. Do not use language-statistic exclusions or rewrite Git history.

## Execution

- [x] Freeze legacy wire/float/database expectations as inert JSON/SQL fixtures.
- [x] Use a Rust subprocess fixture for bounded-check and process-cleanup tests.
- [x] Replace remaining approval, notification, state, progress, plugin, runtime
  and workflow regression coverage with Rust tests before deleting the old suite.
- [x] Verify Rust tests without an interpreter, formatting, strict Clippy and TUI
  tests; obtain two independent read-only reviews and resolve demonstrated issues.
- [x] Back up the stopped Teem state, source, service unit and Foreman MCP config
  privately. The live preflight found 22 successful tasks, six ready workflows,
  no dispatchable work, no pending native notifications and no pending approvals.
- [x] Install the verified native executable, change only Teem's service/MCP
  command paths, restart Teem, and verify original identities and task/event rows.
  Do not restart OpenCode, Herdr, Codr or messaging; do not launch paid test tasks.
- [x] Remove Python source/tests/links and stale active setup instructions. Keep
  prototype history available in Git and in the private cutover backup.

The configured pilot project's approved checks belong to that separate project;
Teem may supervise projects in any language. Do not port or alter unrelated pilot
source to change Teem's language statistics. New builds use the central fastbuild
adapter; persistent state and backups remain outside the build mount.

## Final evidence

- 115 Rust tests passed, including the no-interpreter-source hygiene gate. The
  complete suite also passed with Python command names shadowed by a failing
  native sentinel. Four TUI helper tests, formatting, strict all-target Clippy
  and whitespace checks passed.
- Both independent read-only reviews cleared the final source. Their coverage
  findings were addressed with native combined-candidate preservation,
  ignore-rule interference and fail-closed model-dispatch regressions.
- The installed service and MCP process both run the verified native executable.
  The service has zero automatic restarts. The existing OpenCode parent remains
  unchanged; no OpenCode, Herdr, Codr or messaging restart was requested.
- Complete SQLite dumps before and after the cutover match byte-for-byte. All 22
  successful tasks and six ready workflows, bindings, events and approvals remain
  unchanged. The supervisor configuration is identical; only the MCP executable
  command changed in the project profile. No tasks or notifications were replayed.
- Private backup and verification receipts remain outside the repository. No
  paid end-to-end test or unrelated pilot-project migration was performed.
