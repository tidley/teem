# Native regression tests

Run `cargo test --locked` from the repository root. Tests use temporary Git
repositories, local fake HTTP and native subprocess fixtures. No interpreter,
live model, live service or real credentials are required.

The removed prototype tests are covered by these native groups. Some cases are
table-driven; test counts are not a one-to-one translation of the old suite.

| Contract | Native coverage |
| --- | --- |
| Admission, exact roots/scopes/revisions, role/team limits and no replay | `native.rs` state, checkout, independent-team and restart tests |
| Exact native sessions, parent model/variant, input/final outcome and permissions | `native.rs` runtime route tests and frozen wire fixtures |
| Permission cards, lost create/reply responses, stale/foreign requests, batch rejection and backoff | `regressions.rs` approval tests |
| Native notification admission vs consumption, acknowledgement, receipt backoff and legacy preservation | `regressions.rs` notification tests |
| Private plans, independent Foremen, execution vs verification and silent projections | `regressions.rs` progress tests |
| Agent-reported todos, throttling, failed reads, explicit empty reports and terminal reads | `regressions.rs` agent-report tests |
| Complete clones, binary/new/deleted files, scope validation, source preservation and snapshot drift | `native.rs` checkout and workflow tests |
| Async checks, exact command/receipt binding, bounds, literal arguments and descendant cleanup | `native.rs` check tests; `support/subprocess.rs` |
| Combined workers, both snapshot-bound reviews, bounded revisions and failed-check diagnostics | `native.rs` workflow tests |
| Current work, readable titles, pending verdicts, latest results and no backfilled events | `native.rs` workflow-progress tests |
| CLI installation, service unit, disabled startup, MCP protocols and ownership | `rust_contracts.rs`, `regressions.rs` plugin tests |
| Legacy database and canonical digest compatibility | inert `fixtures/*.json` and `fixtures/legacy-state.sql` |

The numeric fixture contains 1,999 finite legacy values and their expected
encoding. Its expectations are data, not generated at test time by an old
implementation. The subprocess fixture is a separate Cargo test target; the
plugin's `install-plugin` command copies only the real `teem` executable.

The OpenCode adapter still uses its required TypeScript host API. Run its four
helper tests with `bun test ui/teem-progress/progress.test.mjs`.
