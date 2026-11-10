# Teem Herdr plugin

Local MVP, version 0.1.0. This is original code, not a copy of the private PAVE agent system.
Repository setup does not authorise model tasks. See `docs/PILOT.md` for the local pilot and its activation gate.
Herdr 0.9.1 accepted the original manifest in a temporary, isolated plugin registry.

## What it provides

- One explicitly registered Foreman conversation in Herdr, with optional independent Codr/White Noise phone access.
- A native Rust supervisor with persistent SQLite task and event records.
- Two implementation slots and two reviewer slots per managed task. Legacy tasks share two slots per role.
- Headless OpenCode sessions. Workers do not create Herdr panes or additional Codr chats.
- Structured MCP delegation tools bound to one registered Foreman session.
- Caller-retained task UUIDs. Duplicate admission returns the original task instead of repeating work.
- Separate queue acceptance, native execution, terminal outcome, notification acceptance, and event acknowledgement.
- Existing sparse checkout support for legacy workers. Managed workflows prepare complete clones and review exact candidate trees without commits.

Native `succeeded` means the execution ended with a text result. It is not an engineering PASS.
The plugin does not parse review prose into an approval or merge code. Managed
reviews require a strict snapshot-bound JSON verdict and independently checked gates.

## Requirements

- Linux, Git, and a systemd user manager. Building requires Rust 1.85+ and a C toolchain.
- Herdr 0.9.1+ for the plugin interface.
- OpenCode's current `/api/session` protocol, as inspected in the local 2.1.4 source.
  This is not the older `/session` SDK protocol. Other OpenCode versions need contract qualification.
- A configured OpenCode background service and an explicitly selected model.
- Existing authorised checkout roots for manual tasks, or an explicit managed-project policy.

The plugin never launches, elects, replaces, or restarts OpenCode's background service.
The runtime and tests are native Rust and do not invoke an interpreter. Historical
wire/database expectations are inert JSON/SQL fixtures, not executable reference
code. Tests do not use live models or services.
Teem does not require Codr, White Noise, a bridge account, or a Codr local control socket.

## Explicit local setup

These commands change local configuration. They are operator instructions, not steps already performed.

1. Review `herdr-plugin.toml` and the Rust modules under `src/`.
   Install with `herdr plugin install tidley/teem`, or build a local checkout with
   `cargo run --locked --release -- install-plugin` before linking it.
2. Link the local plugin:

   ```sh
   herdr plugin link /home/tom/code/teem
   herdr plugin config-dir local.teem
   ```

3. Copy `examples/foreman.toml` to `config.toml` inside the printed plugin configuration directory.
4. Set a private `state_dir`, local service `registry`, explicit `model`, and `allowed_checkouts` in that file.
5. Leave `enabled = false` until you authorise model execution.
6. Focus the intended Foreman OpenCode pane. Invoke the `local.teem.bind` plugin action.
   This records its exact native session ID and terminal ID. A display name alone cannot bind it.
7. Generate a service unit with the absolute configuration path:

   ```sh
    bin/teem --config /absolute/plugin/config.toml service-unit
   ```

8. Review and save that output as `~/.config/systemd/user/teem.service`.
   Run `systemctl --user daemon-reload`. This does not start the supervisor.
9. After authorisation, set `enabled = true` and invoke the plugin's start action.
   To start after login without opening Herdr, explicitly enable the user service.
   This MVP does not change login, linger, or boot policy.

The startup hook starts only an already configured service. Missing configuration is a no-op.
An enabled configuration without an installed unit produces a visible service error.
Herdr startup hooks are not daemon supervisors. systemd owns the supervisor process.

## Foreman tools

Configure a local MCP server for the intended Foreman runtime. Merge this example into existing configuration.
Do not overwrite an existing OpenCode configuration file.

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "teem": {
        "type": "local",
        "command": [
          "/absolute/path/to/teem/bin/teem",
          "--config", "/absolute/plugin/config.toml",
          "mcp", "--foreman", "ses_EXACT_REGISTERED_SESSION"
        ],
        "codemode": false
      }
    }
  }
}
```

The paths and native session ID in this example must match your explicit local binding.
Check the model runtime's configuration reload requirements before use.
The MCP tools appear with their own schemas and descriptions. They do not depend on an `AGENTS.md` role instruction.

- `foreman_submit`: provide a retained UUID, role, checkout, full base commit, relative file scope, and assignment.
- `foreman_status`: bounded task/workflow summaries, exact native session IDs, authoritative counts, check/review outcomes, unacknowledged events, and explicit paginated detail retrieval. See [status modes](#bounded-status-and-evidence-retrieval).
- `foreman_ack`: acknowledge one event after consuming its result. This does not approve code.
- `foreman_plan`: define this conversation's ordered checklist, linking execution items to retained task UUIDs.
- `foreman_workflow`: supply a configured project alias, retained workflow UUID, scope and acceptance criteria. No checkout paths or base hashes are needed.
- `foreman_integrate`: after approval, apply a ready exact candidate to its original clean source. Supply a full `commit` instead to verify and record an already integrated exact-candidate commit.
- `foreman_color`: set this bound Foreman's local accent using `project` and a six-digit hex `color`. No shell or restart is needed.

The current example profile supports ordinary local edits. Shell commands and
candidate integration require native approval; it cannot edit its own OpenCode
configuration or Git metadata. Use the bounded color tool for appearance changes.
Commits, publication, destructive actions and deployment still require their own
user authorisation. Never use shell/API access to answer worker permission cards
or to change Teem state. Edit permissions are not an OS sandbox.

Integration rechecks the patch digest, approved check receipts and both reviewers
against the same candidate before writing source. Source must be clean and at
the original base; source drift blocks application instead of forcing a merge.
An attempting marker survives crashes. Repeated calls only reconcile matching
source by reads; they never repeat an uncertain apply. Workflow history stays
unchanged. The separate `integration` receipt and current `detail` are included
in status and passive progress; `candidate_detail` retains the original outcome.
Commit recording verifies the exact candidate tree and its ancestry in source.
It does not infer remote publication or installation. When source is committed
outside the tool, record the verified commit before reporting integration status.

Status also lists approval card IDs, worker permission IDs, and relay states. There is no
model-callable approval tool. The Foreman must not answer cards on the human's behalf
or treat a conversational “yes” as a native permission decision.

Submission returns after durable queue admission. The supervisor performs execution separately.
A Foreman should finish its dispatch response rather than wait in a synchronous polling loop.
The runtime still serialises that Foreman's own conversation. This is not simultaneous model execution in one conversation.

The fixed MCP binding prevents tool arguments from selecting another Foreman.
It does not authenticate the calling model session. Configure it only where intended.
Binding does not remove the visible Foreman's existing implementation tools.
This MVP supplies delegation tools, not mandatory routing of every conversational message.
This is a same-user local integration, not protection against arbitrary local processes.
Background sessions deny MCP tools and delegation through their runtime permission rules.

## Managed workflows and routine checks

Add a machine-local project policy to the plugin configuration, not to worker-editable
source. Only explicitly configured projects can run managed work:

```toml
[projects.pilot]
path = "/absolute/project/root"
max_revisions = 1

[[projects.pilot.checks]]
name = "tests"
kind = "test"
argv = ["cargo", "test", "--locked"]
cwd = "."
timeout = 300
output_limit = 1048576
```

The operator must approve these commands and trust the project's test/build scripts.
Commands use exact argv, not a shell. No model argument can choose a command, cwd,
timeout or revision budget. Shell launchers and obvious publication/destructive
executables are rejected. This does not make arbitrary test code safe: approved code
runs as the same user and can have side effects. The runner is not an OS sandbox.
Do not configure production deployment, paid services, secrets, hardware actions or
untrusted fork scripts as routine checks. Environment forwarding is restricted, but
the user's filesystem is not isolated.

Each project requires at least one test check; up to eight test/build/inspection checks
run in order, stopping on failure. Configure build commands only after their central
fastbuild adapter, target requirements and resource use have been qualified. The runner
reads storage policy and refuses an unavailable required mount. It does not invent
cache mappings or relocate existing build directories. Check logs/receipts are private
persistent evidence, not regenerable build caches. Test exit zero is check evidence,
not a guarantee of coverage or correctness.

For an authorised task, Foreman calls `foreman_workflow` with project alias, scope and
acceptance criteria, then remains available for chat. The exact native Foreman project
must match the configured source. The supervisor chooses clean source HEAD and creates
separate private local clones beneath its state directory. It copies local root
instructions when available but never copies an excluded Foreman `opencode.json`.
It does not use worktrees, rewrite source history, create source commits, discard
dirty checkouts, fetch a network remote, or clean up old work automatically.
Dirty sources and submodules are explicit blockers. Large-repository and platform
qualification remain project-specific.

Workflow progression is:

1. Queue one or two implementation workers with inherited Foreman model. Optional
   `workers` assignments each contain `scope` and `prompt`. Their file scopes must
   be disjoint and inside the overall task scope. Use one worker if the work cannot
   be split safely; two workers never edit the same checkout.
2. Freeze its actual scoped changes, including new/binary/deleted files, into a binary
   patch and Git tree with a SHA-256 manifest. Git tree objects are not commits.
3. For a two-worker task, validate each contribution against its own scope, then
   apply both verified patches to a new combined checkout. An apply conflict blocks
   the task without discarding either contribution. Freeze that combined candidate.
   Materialise a separate complete candidate and launch the approved checks
   asynchronously. Workers need not request shell permission for those checks.
4. Verify the candidate is unchanged and bind check receipts to that snapshot/command.
5. Queue two independent read-only reviews in separate complete candidate checkouts.
   Reviewers read the exact diff at `.git/teem-review.patch` and the current candidate,
   not only base HEAD. Their trees are
   validated before/after execution. Their JSON verdict must name the exact snapshot.
6. On failed checks or demonstrated findings, start a fresh revision worker from the
   prior snapshot, including bounded hash-verified failure diagnostics in its prompt,
   then rerun checks and both reviews. The configured zero-to-three
    revision budget bounds this. Revisions use one worker on the combined candidate
    so cross-part fixes remain consistent. One revision permits at most three worker
    executions (two initial, one revision) and four reviewer executions per task.
7. Report `ready` only when the exact candidate passes all configured checks and both
   reviews. The patch remains local and uncommitted. No integration, merge, publication,
    deployment or release is performed by progression. Separate explicit integration
    uses the operator tool described above. Multiple workflows are not automatically combined.

Each managed task has independent two-worker/two-reviewer caps. Additional work can
run its own team rather than waiting for another task's agent slots. Uncertain agents
retain their task slots. Legacy tasks keep a shared two-per-role pool. Up to two workflows can run checks
at once. Project policy is recorded at admission; changing/removing it stops subsequent
progression and queued managed launches after configuration reload. It does not cancel
already running native agents or approved checks. Do not treat a new task UUID as
permission to bypass a stopped workflow or user budget.

Checks clean up their owned process group on success as well as failure; they do not
terminate unrelated processes. This is still not containment for untrusted programs
that escape their process group or access other same-user files.

Check launch and preparation markers are durable. Unknown side-effect windows retain
all paths/evidence without automatic replay. A missing/late receipt or uncertain native
task can reconcile by reads; no replacement runs are launched merely to recover it.
Dismissed approvals and interrupted filesystem preparation may need manual inspection.
Summary status includes the current round's snapshot, check evidence, both reviewer
IDs/verdicts and specific blocker. Explicit detail includes every round's paths and
full evidence. Review findings and receipts are preserved even after revision.
Readiness is policy-specific and not a substitute for combined integration verification.

The workflow itself appears automatically in supervisor-backed progress. It shows
implementation, checks, reviews, revision/uncertainty and final candidate qualification.
Manual checklist entries are preserved. Finished workflows leave the current-work
display; their results and blockers remain in full status history.
Supply an optional short `title` at admission for the user-facing checklist. Existing
workflows get a readable title derived from their acceptance prompt without changing
the immutable request. Workflow UUIDs remain in status metadata, not generated labels.
Active workflows show only the current round's workers, approved checks, correctness
reviewer, and tests/scope reviewer separately. A finished reviewer stays verdict-pending
until the controller records its parsed snapshot-bound result.

New managed agents can also report their own native todos. Where no project config
would be overwritten, Teem creates a private ignored OpenCode config enabling only
native todos and disabling warming. It never copies Foreman MCP bindings. Config-edit
tasks and existing unsupported project configs use phase-only progress instead.
The benign `todowrite` capability is recorded in the new task's immutable request;
old native sessions are not rewritten. Edit scope and shell/reviewer restrictions
remain unchanged.

Teem reads completed native `todowrite` calls from the exact reserved session, at most
once per five seconds, and collects a final report after termination. It does not
parse prose into a plan. Read failures retain the last observed report rather than
inventing completion. The checklist shows up to eight reported detail entries per
current-round agent, explicitly labelled as agent-reported. These checkboxes are not
test, review or workflow gate evidence. Native progress polling and todo refresh
remain separate from the actual model execution state.

## Live checklist

Foreman registers an ordered list through `foreman_plan`. Each item has a stable short
`id` and a single-line `content` label. An execution item also has its retained
`task_id`; its status is supplied by the supervisor. An unlinked planning item has an
explicit `status`: `pending`, `in_progress`, `completed`, or `cancelled`.

For example, a linked implementation item can be followed by separate unlinked test
verification and review-evidence items. Linked task UUIDs can be registered before
submission. Missing tasks show “not submitted or unavailable.” Cross-Foreman links
and model-supplied statuses on linked items are rejected. Replacing a plan must
preserve unrelated planning items. An empty list explicitly clears the managed plan.
Existing conversations are not subscribed automatically and their todos are untouched.

The authoritative checklist snapshot is `foreman_status` detail section `progress`.
Summary `progress.items` is paginated (not a full native todo replacement). Its items distinguish
`supervisor` execution from manual `foreman-plan` state. Its `todos` contain native
todo-compatible content and statuses. Worker success produces “execution ended;
tests/review not implied.” Failed/interrupted work is not shown as completed.
Foreman must assess actual evidence before updating manual verification items.

This OpenCode version has no external todo-write endpoint. The passive TUI plugin
reads an atomic, mode-0600 `progress.json` under the configured private state directory.
The supervisor writes it each loop; the TUI reads it once per second. No model call,
synthetic input or fabricated tool history is needed. A stale observation is labelled
after 15 seconds. This local file integration needs the TUI on the same host as Teem.

To install, merge these entries into `~/.config/opencode/cli.json` in this order,
preserving other settings/plugins (use your actual absolute paths):

```json
{
  "plugins": [
    "-opencode.sidebar.todo",
    { "package": "/absolute/teem/ui/teem-progress", "options": {
      "stateFile": "/absolute/private/state_dir/progress.json"
    } }
  ]
}
```

The native Todo feature is disabled in this TUI configuration to prevent a second,
stale checklist. The replacement uses real native todos for all unbound conversations,
including their revert boundary; only subscribed Teem conversations use passive rows.
Other sidebar features are unchanged. Valid CLI changes reload without restarting
OpenCode's backend. The plugin adds no server, credentials or Python dependency.

Progress changes and individual managed-agent completions do not wake Foreman. Their
event receipts remain inspectable and unacknowledged (`passive` is not consumed or
approved). When a workflow reaches `ready`, `blocked`, or `needs_attention`, the
supervisor atomically records its final state and one native outcome event. Foreman
reads that workflow's evidence, gives one concise result summary, and acknowledges
the outcome event. No progress rendering or new worker run is requested. This final
summary can queue behind a user turn; individual stages still cost no Foreman turns.
An uncertain notification is reconciled by exact native ID reads, never resent.

Summary `foreman_status.progress.latest_result` also keeps one terminal result outside the
active checklist. The TUI shows its readable title and completed/blocked/needs-attention
summary, including “not integrated” for ready candidates. This remains visible when
active todos clear. New outcomes are ordered by recorded finish time. Older finished
workflows without that timestamp use admission time as a historical fallback; they
are displayed without backfilled notifications or any task replay. No historical
request, candidate, check receipt or reviewer verdict is rewritten.

Legacy direct task completions still notify Foreman, without a new todo-refresh request.
Unsent old progress inputs are superseded, not replayed. Already admitted or uncertain
inputs remain history; old payload bytes and receipts are preserved. The updated
Foreman profile says not to follow obsolete refresh instructions. No existing inbox
inputs or conversation messages are deleted. `progress_delivery` is legacy history.

This TUI display is not a native todo tool call. It is not automatically visible in
White Noise; that independent optional viewer is not contacted or changed.

## Inherited dispatch model

New worker and reviewer sessions inherit the exact bound Foreman's selected provider,
model, and optional variant at dispatch. The supervisor records `runtime_model` before
session creation. Reconciliation uses that recorded selection even if Foreman switches
models later. A missing or invalid selection leaves a reservation uncertain without
creating a worker or repeating a prompt. There is no model override in the task tool.

The configured `model` remains the fallback for pre-upgrade task records without a
recorded selection. It is not a separate model choice for new workers. Do not change
that legacy fallback while reconciling old uncertain/live tasks. Existing native
sessions are not rewritten. Model inheritance does not broaden task or spending
authority, checkout authorisation, concurrency, shell permissions, or review gates.

## Native completion events

The supervisor sends completion events directly to the exact Foreman's OpenCode session through `/api/session/{id}/synthetic`.
It uses queued delivery and explicit resume. It does not type into a terminal or replace a desktop draft.
The runtime queues the event without steering an active model step.
Each event has one persisted request UUID and a deterministic native message ID.
The event contains task/event identifiers, not raw worker output. Foreman retrieves the recorded result through its tools.
Native admission means the runtime accepted input. It does not prove the Foreman executed or consumed that input.
Only `foreman_ack` closes an event. Uncertain delivery triggers exact message/inbox reads, never automatic resend.
Native events can resume the Foreman and use model tokens when supervision is enabled.

Existing pre-upgrade event records receive the `legacy-codr` transport marker.
Their original states, acknowledgements, and IDs remain intact. The supervisor never replays them through the native path.
Old database columns remain for preservation, but Teem no longer reads Codr notification configuration.
Inspect and acknowledge historical results manually. The obsolete `--notification-file` option is removed.

## Independent optional integrations

```text
Herdr ↔ Foreman ↔ Teem supervisor ↔ Workers/reviewers
           ↕
     Codr/White Noise — optional
```

Teem does not enable, configure, restart, or depend on Codr/White Noise.
Codr/White Noise can share the Foreman conversation through their existing integration, independently of Teem.
Without Teem, use the ordinary Build agent and existing Herdr/Codr behaviour.
To stop Teem delegation in the pilot, keep its supervisor stopped, disable its project-local MCP connection, and select Build.
Restore `default_agent` to `build` if you no longer want the pilot's opt-in Foreman profile as the default.
Stopping the supervisor does not cancel already running native agents.

The existing phone submission and desktop draft race remains outside this MVP.
The plugin shares the existing conversation. It does not provide a new atomic mobile/desktop input router.

## Sparse work and integration

Git worktrees share repository objects but normally materialise all tracked files.
Sparse checkout reduces the materialised file set. It does not make every dependency or test available.
The legacy task API accepts existing sparse checkouts. Managed workflows create new
complete clones; neither path alters or deletes existing user checkouts.

Record a narrow task scope and immutable base revision. Each live task owns its assigned checkout exclusively.
Sparse tasks report `sparse: true`. Do not classify missing-dependency test failures as code failures or as passing checks.
For a legacy manual review, produce a fixed commit through the project's authorised source workflow.
Assign reviewers complete, clean checkouts at that commit. Two concurrent reviewers need separate checkout roots in this MVP.
Run full verification against the combined result before integration. Do not merge sparse-task results merely because individual agents finished.

Workers can edit only their declared relative scope through built-in edit tools. Shell commands require human approval.
New worker sessions allow external-directory access within `/mnt/fastbuild/`, including
the required storage instructions and nested build-output directories. This exception
does not allow other external directories, unscoped edits, or unattended shell commands.
Use the central storage adapters; this permission does not authorise changing or deleting
other projects' build outputs. Reviewer permissions and global OpenCode defaults are unchanged.
Existing native sessions are not updated or replayed when this worker template changes.
Reviewers can use read, glob, and grep. They cannot edit, run shell commands, delegate, or use MCP tools.
These restrictions are runtime permissions, not an operating-system filesystem sandbox.
Human shell approval can permit actions outside those tool restrictions. Never treat shell approval as routine unattended consent.
The plugin never auto-approves permissions or performs commits, merges, pushes, deployments, or hardware operations.

## Status and recovery

Open the plugin's status popup on demand. It is not a persistent worker pane.
The popup is a snapshot, not a live dashboard. Close and reopen it to refresh.
Use the plugin's status action or run `herdr plugin pane open --plugin local.teem --entrypoint status`.
For machine-readable status:

```sh
bin/teem --config /absolute/plugin/config.toml \
  status --foreman ses_EXACT_REGISTERED_SESSION
```

### Bounded status and evidence retrieval

This is the source contract; it does **not** mean an installed executable, MCP
connection, UI, or existing session has been upgraded. Installation/configuration
changes require separate authorisation. No directory permissions are expanded.

The default `foreman_status {}` is a summary of at most **24,000 bytes** when
pretty-printed as JSON. MCP returns multiline text, rather than one enormous JSON
line. Full requests, prompts, project command policies, prior rounds and agent todos
are not embedded in the routine summary. No stored history is shortened or removed.

- `counts` covers **all matching records**, before pagination. `foreman_counts`
  covers this bound Foreman's entire team even when filtered. Both distinguish
  workers/reviewers and queued, starting, accepted, running, uncertain, succeeded,
  failed and interrupted. `active` counts reserved slots: starting + accepted +
  running + uncertain. It is **not** the running count. Queued tasks do not occupy
  slots. Uncertain tasks still reserve theirs. Agent todos are not execution evidence.
- Workflow summaries include title/state, current snapshot, current-round worker
  result excerpts, all configured checks (up to the native policy maximum of eight),
  actual return codes/timeout/output-limit/error evidence, and both recorded review
  verdicts with snapshot binding. A null verdict is not an approval, even if reviewer
  execution succeeded. Checks not reached after a failure are labelled not run.
- Workflow `detail` follows the later integration receipt; `candidate_detail` retains
  the original candidate outcome when a receipt exists. `ready` is not integration;
  applied is not committed; committed is not published, installed, or running.
  Publication/runtime are explicitly not checked by status.
- Summary strings have a visible truncation suffix with their original byte count
  (normally a 160-byte excerpt, 120 for titles). At most two findings per reviewer
  are shown, with `findings_total` and `findings_truncated`. Missing failure text or
  worker limitations must be read through detail before claiming the outcome consumed.
  `rounds_total`, `workers_total`, `review_results_total`, check totals and page totals
  expose collection bounds. Earlier rounds remain evidence history, not new runs.

Arguments shared by MCP and CLI:

| MCP argument | CLI flag | Meaning |
| --- | --- | --- |
| `workflow_id` | `--workflow-id` | One workflow and its related tasks/events/approvals/progress |
| `task_id` | `--task-id` | One task, its related records and its parent workflow; mutually exclusive with workflow_id |
| `mode` | `--mode` | `summary` (default) or `detail` |
| `section` | `--section` | `overview` (default), `workflows`, `tasks`, `events`, `approvals`, `projects`, `progress`, `agent_progress`, `progress_delivery`, or detail-only `check_output` |
| `offset` | `--offset` | Summary row offset; detail UTF-8 byte offset in the JSON-encoded selected value |
| `limit` | `--limit` | Summary 1–5 rows per collection, default 2; detail 1–4096 bytes, default 2048 |
| `pointer` | `--pointer` | Detail-only JSON pointer; empty selects the whole record/section |

Summary `overview` includes workflows, tasks, unacknowledged events, approval card
identifiers/states, project aliases and progress. `projects` is now a paginated array
of aliases (`name`), check counts and revision limits, not full policy objects.
Use a specific section for more rows. Every returned collection has a `pages` entry
with `total`, `returned`, `offset`, `next_offset` and ordering. Pages are newest first
except projects/progress, which use stored order. Overview offset applies independently
to each collection. The byte budget may reduce the requested page size; always follow
the returned `next_offset`. A scoped request excludes unrelated agent reports, events,
approvals and progress (including legacy progress-delivery records that cannot be
attributed). Counts are captured with the evidence in one SQLite read snapshot.
Separate calls can observe new state; summary offsets are not a frozen export.

Examples (MCP argument objects):

```json
{"workflow_id":"WORKFLOW_UUID"}
{"section":"tasks","offset":2,"limit":5}
{"mode":"detail","workflow_id":"WORKFLOW_UUID","pointer":"/data/rounds/0"}
{"mode":"detail","task_id":"TASK_UUID","pointer":"/result","limit":1024}
{"mode":"detail","section":"events","limit":1024}
{"mode":"detail","section":"check_output","workflow_id":"WORKFLOW_UUID","pointer":"/data/rounds/0/check_result/results/0","limit":1024}
```

Detail `overview` requires one workflow/task ID. Other detail sections select the
whole collection (and support those filters). Detail events include acknowledged
history; progress-delivery detail includes all legacy rows, without replaying them.
`pointer` selects a value within that record/collection before chunking. To read an
earlier candidate, use its index in `/data/rounds/N`; the summary covers only the last
round. `/request` retrieves immutable requests only when explicitly requested.

Detail returns `chunk`, `total_bytes`, `sha256`, and `next_offset`. Concatenate chunks
as strings, then parse the resulting JSON. The pointer's selected value is JSON-encoded,
so a selected string includes JSON quotes/escapes; a chunk need not itself be valid
JSON. Repeat the same selection with the returned next_offset until null. Verify
sha256 stays unchanged; if it changes, restart rather than mixing snapshots. This
hash identifies the encoded value, not a candidate approval. `check_output` reads
only the selected recorded log beneath that workflow's private managed directory,
rejects symlink paths/digest changes, and returns its JSON-encoded text (or an
`encoding: bytes` object with the exact bytes for non-UTF-8 logs). It does not expose
arbitrary file reads. Use the recorded `output_sha256` for original log bytes.

CLI equivalent:

```sh
bin/teem --config /absolute/plugin/config.toml status --foreman ses_EXACT_REGISTERED_SESSION \
  --workflow-id WORKFLOW_UUID --mode detail --pointer /data/rounds/0 --limit 1024
```

If a client still truncates/spools output to an inaccessible path, **do not read it
by bypassing permissions, grant blanket external-directory access, infer PASS, or
acknowledge the event**. Retry the workflow/task's specific section with `limit: 1`,
or use smaller detail chunks/pointers. Read relevant failures, findings and limitations
before summarising and acknowledging. If no bounded selection is readable, report
that blocker and leave the event unacknowledged. An exceptionally large summary that
cannot fit even one row fails explicitly with narrowing instructions; it does not
silently remove failures to fit. Native execution success is never review approval.

`starting`, `accepted`, `running`, and `uncertain` retain a slot and exclusive checkout ownership.
A crash after native submission does not cause another prompt submission.
Reconciliation reads the exact session, prompt message, and inbox IDs.
An idle session without the expected native input is uncertain, not successful.
Changed native identity or permissions also retain the reservation for inspection.

### Worker approval cards

The supervisor relays pending worker tool permissions to the exact bound Foreman
conversation as native question forms. Each card shows the task, worker session,
checkout, original permission, and exact tool input. Choose **Approve once** or
**Reject worker batch** in the card. Approval permits only the selected request.
Native OpenCode rejection declines all pending permissions in that worker session
and can end its current step. Teem cancels the other stale cards on its next check.
There is no permanent approval option or default answer.
This does not send a model prompt, resume the Foreman, or require Codr/White Noise.
The ordinary Foreman chat stays available. Optional clients must support native forms.

Before forwarding a human answer, Teem rechecks the worker identity, original task
input, pending permission, and tool input. A changed or vanished request invalidates
its card. A duplicate poll or supervisor restart does not create another card.
An uncertain create response is reconciled with reads. If the native API confirms
that the card is absent, Teem can restore the same card only when its exact worker
permission and tool input remain pending and no permission reply was attempted.
This asks the human again; it does not reuse a lost answer or replay a task.
Recovery attempts have a 30-second backoff. A durable marker is written before any
permission reply. An uncertain permission reply is never retried, and its card is
not automatically restored. Old uncertain rows remain conservative on upgrade.

Check `approvals` in status and inspect the exact native worker request if reply
delivery is uncertain or a card is dismissed. Forms are runtime-owned: project
service eviction or backend restart can remove them. Cancelling a card does not
reject or cancel the worker's permission.

The form API is a same-user interface, not authenticated proof of a particular
person. Do not give the Foreman shell/API access that could answer its own cards.
Teem adds no model-callable reply tool and never interprets model text as consent.

Worker question forms and unsupported non-tool permissions still require inspection
of the native worker session. Status reports that block and the exact session ID.
Stopping the supervisor does not cancel backend-owned OpenCode execution.
Interrupt the exact native session through OpenCode when cancellation is required.
After a worker has no accepted input, an uncertain reservation needs manual investigation.
This MVP has no force-release or automatic replay command. Do not edit live state to bypass the reservation.

After reboot, the supervisor reconciles persisted task IDs when explicitly started or enabled as a user service.
It does not create replacement sessions for uncertain tasks.
Herdr and the underlying OpenCode runtime retain responsibility for their own restoration.

## Verification

```sh
cargo test --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
bun test ui/teem-progress/progress.test.mjs
```

Tests use temporary state, temporary Git repositories, fake runtime transports, and a local fake HTTP server.
They do not contact live Codr, Herdr, OpenCode credentials, GitHub, relays, or models.
Live installation, concurrency, permission enforcement, desktop/mobile delivery, and reboot recovery still need an authorised pilot.
