# Teem

A local [Herdr](https://github.com/herdrdev/herdr) plugin with one visible Foreman and up to two workers plus two reviewers per managed task.
The supervisor is a native [Rust](https://github.com/rust-lang/rust) executable, not another agent.
The [OpenCode](https://github.com/anomalyco/opencode) TUI adapter uses the host's [TypeScript](https://github.com/microsoft/TypeScript) interface.

```text
Herdr ↔ Foreman ↔ Teem supervisor ↔ Workers/reviewers
           ↕
     Codr/White Noise — optional
```

Teem and [Codr](https://github.com/tidley/codr)/[White Noise](https://github.com/marmot-protocol/whitenoise-android) are independent, optional integrations.
Teem does not import, call, configure, start, or restart [Codr](https://github.com/tidley/codr).
Without Teem, an ordinary agent conversation works as before.
With Teem, an explicitly bound Foreman can delegate and receive native completion events.
The optional `/teem enable` command binds the exact current Herdr/OpenCode chat
without starting work or changing permissions. See
[native enablement setup and qualification limits](docs/ISSUE1_ENABLEMENT.md).

## Status

Early local-first MVP, developed independently from [Codr](https://github.com/tidley/codr).
Source: [github.com/tidley/teem](https://github.com/tidley/teem).
No paid worker run is part of repository setup.

## Components

- `herdr-plugin.toml`: Teem actions, startup hook, and transient status popup.
- `src/plugin.rs`: [Herdr](https://github.com/herdrdev/herdr) commands, fixed-binding [MCP](https://github.com/modelcontextprotocol/modelcontextprotocol) tools, and service unit output.
- `src/enable.rs`: explicit native chat receipts, per-call authority and model capture.
- `src/operator.rs`: explicit candidate integration, verified commit receipts and bounded Foreman color updates.
- `src/control.rs`: durable user stop requests, native interruption and preserved handoff state.
- `src/store.rs`: [SQLite](https://sqlite.org/src/) tasks, per-task 2+2 limits, and exclusive checkout ownership.
- `src/status.rs`: bounded readable status, authoritative counts and paginated evidence retrieval.
- `src/runtime.rs`: loopback-only [OpenCode](https://github.com/anomalyco/opencode) HTTP, completion admission and permission rules.
- `src/progress.rs`: plans, atomic private progress snapshots and native todo observations.
- `ui/teem-progress`: passive [OpenCode](https://github.com/anomalyco/opencode) TUI checklist; retains native todos for other conversations.
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

The manifest builds and installs `bin/teem` with [Cargo](https://github.com/rust-lang/cargo).
[Rust](https://github.com/rust-lang/rust) 1.85+ and a native C toolchain are needed
at installation; Rust and Cargo are not needed by the resulting executable.
[Git](https://github.com/git/git), [Linux](https://github.com/torvalds/linux)/[systemd](https://github.com/systemd/systemd),
[Herdr](https://github.com/herdrdev/herdr) and [OpenCode](https://github.com/anomalyco/opencode) remain required.
For a local checkout run `cargo run --locked --release -- install-plugin` before
`herdr plugin link /absolute/path/to/teem`. See the plugin guide for explicit setup.
Existing installations must explicitly replace their service and MCP executable
paths. Stop and back up the old supervisor before an approved cutover; never run
two supervisors against the same state directory. Installation alone does not
change those machine-local settings or replay tasks.

## Verify

From this directory (UI helper tests use [Bun](https://github.com/oven-sh/bun)):

```sh
cargo test --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
bun test ui/teem-progress/progress.test.mjs
```

Tests use temporary repositories and fake/local transports, not live models or
credentials.

## Setup and limits

See [the plugin guide](docs/HERDR_PLUGIN.md) and [the local pilot](docs/PILOT.md).
The legacy task API uses separate authorised checkouts. Workers may be sparse; legacy reviewers must be complete and pinned to a commit.
Managed workflows choose their own base and prepare separate complete clones from a configured source project.
They run approved checks and review the exact uncommitted patch/tree snapshot without creating source commits.
The supervisor never automatically integrates, merges, pushes, deploys or replays uncertain tasks.
After user approval, `foreman_integrate` can apply an exact verified candidate to
its original clean source, or verify and record its integration commit. It never
commits or publishes. Shared status and the sidebar show integration receipts.
The Foreman profile uses trusted Build-equivalent tool access without routine
shell or directory prompts. Publication, integration and destructive operations
still need user authorisation through instructions, not tool-level barriers.
See [Foreman access and reinstall policy](docs/FOREMAN_ACCESS.md). `foreman_color`
changes only its local accent without needing shell access.
The Foreman assigns a separate workflow and at least one dedicated worker to
each distinct task or issue; independent tasks may run in parallel. After user
approval, `foreman_stop` pauses a workflow and stops its selected native agents
without discarding their changes or automatically launching replacements.
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
`foreman_status` defaults to a bounded summary, not full prompts/history. It reports
worker/reviewer state counts, candidate check/review evidence and later integration
receipts. Explicit detail chunks retain access to full history and verified check
output without granting access to an external tool-output directory. See
[status modes and recovery](docs/HERDR_PLUGIN.md#bounded-status-and-evidence-retrieval).
Never acknowledge an outcome whose relevant evidence is inaccessible or unread.
Managed workflows provide project-approved checks and bounded review/revision loops.
Their `ready` state qualifies the exact candidate under its recorded policy, not an
integrated project or release. Untrusted check scripts require stronger isolation
than this same-user local MVP supplies. Paid end-to-end runs still need qualification.

Plugin ID: `local.teem`. Optional user service: `teem.service`.
Internal `foreman_*` tool names and native task identifiers remain stable from the prototype.
