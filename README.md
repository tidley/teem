# Teem

A local Herdr plugin with one visible Foreman and up to two workers plus two reviewers per managed task.
The supervisor is a native Rust executable, not another agent. Python is not
required at runtime. The OpenCode TUI adapter uses the host's TypeScript interface.

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
Source: [github.com/tidley/teem](https://github.com/tidley/teem).
No paid worker run is part of repository setup.

## Components

- `herdr-plugin.toml`: Teem actions, startup hook, and transient status popup.
- `src/plugin.rs`: Herdr commands, fixed-binding MCP tools, and service unit output.
- `src/operator.rs`: explicit candidate integration, verified commit receipts and bounded Foreman color updates.
- `src/store.rs`: SQLite tasks, per-task 2+2 limits, and exclusive checkout ownership.
- `src/runtime.rs`: loopback-only OpenCode HTTP, completion admission and permission rules.
- `src/progress.rs`: plans, atomic private progress snapshots and native todo observations.
- `ui/teem-progress`: passive OpenCode TUI checklist; retains native todos for other conversations.
- `src/checkouts.rs`: private complete clones and hashed uncommitted candidate snapshots.
- `src/checks.rs`: asynchronous approved checks and exact-snapshot receipts.
- `src/workflows.rs`: bounded implementation/check/two-reviewer/revision progression.
- `src/approvals.rs`: human-answerable native permission cards; never automatic approval.
- `tests/`: native Rust integration/regression tests and inert legacy JSON/SQL fixtures.
- `tests/support/subprocess.rs`: native child processes for timeout/output/cleanup tests;
  this fixture is never installed by the plugin.

## Install

```sh
herdr plugin install tidley/teem
```

The manifest builds and installs `bin/teem` with Cargo. Rust 1.85+ and a native C
toolchain are needed at installation; Rust, Python and Cargo are not needed by
the resulting executable. Git, Linux/systemd, Herdr and OpenCode remain required.
For a local checkout run `cargo run --locked --release -- install-plugin` before
`herdr plugin link /absolute/path/to/teem`. See the plugin guide for explicit setup.
Existing installations must explicitly replace their service and MCP executable
paths. Stop and back up the old supervisor before an approved cutover; never run
two supervisors against the same state directory. Installation alone does not
change those machine-local settings or replay tasks.

## Verify

From this directory:

```sh
cargo test --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
bun test ui/teem-progress/progress.test.mjs
```

Tests use temporary repositories and fake/local transports, not live models or
credentials. Neither the runtime nor the tests require a Python interpreter.

## Setup and limits

See [the plugin guide](docs/HERDR_PLUGIN.md) and [the local pilot](docs/PILOT.md).
The legacy task API uses separate authorised checkouts. Workers may be sparse; legacy reviewers must be complete and pinned to a commit.
Managed workflows choose their own base and prepare separate complete clones from a configured source project.
They run approved checks and review the exact uncommitted patch/tree snapshot without creating source commits.
The supervisor never automatically integrates, merges, pushes, deploys or replays uncertain tasks.
After user approval, `foreman_integrate` can apply an exact verified candidate to
its original clean source, or verify and record its integration commit. It never
commits or publishes. Shared status and the sidebar show integration receipts.
The Foreman profile permits routine local edits, asks before shell commands and
integration, and protects its own permissions/MCP configuration. `foreman_color`
changes only its local accent without needing shell access.
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
