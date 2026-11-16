# Teem Herdr plugin

Local MVP, version 0.1.0. This is original code, not a copy of the private PAVE agent system.
Repository setup does not authorise model tasks. See `docs/PILOT.md` for the local pilot and its activation gate.
Herdr 0.9.1 accepted the original manifest in a temporary, isolated plugin registry.

## What it provides

- Native Teem tools in every top-level chat where the server plugin is loaded, with optional independent Codr/White Noise phone access.
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

## Installation and explicit activation

These commands change local configuration. They are operator instructions, not steps already performed.

1. Review `herdr-plugin.toml` and the Rust modules under `src/`.
   Install with `herdr plugin install tidley/teem`. The manifest build installs
   the executable, shared global `agents.teem` profile and native server/TUI
   package. New selections use `teem` (host input label **Teem**), not
   `teem_foreman`. No repo-local OpenCode configuration is required. Existing
   defaults and legacy entries/bound identities stay intact.
   See [paths, conflicts, upgrade and host limits](ISSUE19_GLOBAL_INSTALL.md).
2. For a local checkout only, build/install before linking. These commands now
   also register global OpenCode configuration:

   ```sh
   cargo run --locked --release -- install-plugin
   herdr plugin link /absolute/path/to/teem
   herdr plugin config-dir local.teem
   ```

3. Discovery/selection works without supervisor configuration or activation.
   For delegation, copy `examples/foreman.toml` to `config.toml` inside the printed
   plugin configuration directory. Confirm it matches the registered native
   plugin's `options.config`; preserve intentional custom overrides.
   `HERDR_PLUGIN_CONFIG_DIR` selects this path at installation; absent it, the
   CLI fallback is `$XDG_CONFIG_HOME/teem/config.toml` or
   `$HOME/.config/teem/config.toml`. Local builds can explicitly use
   `--config /absolute/plugin/config.toml install-plugin`.
4. Set a private `state_dir`, local service `registry`, explicit `model`, and `allowed_checkouts` in that file.
5. Leave `enabled = false` until you authorise model execution.
6. Select Teem in the intended top-level OpenCode chat. Native tools authenticate
   that chat without manual binding. Use `/teem enable` only for optional exact
   Herdr/model capture. The `local.teem.bind` action is for legacy fixed-MCP setup;
   a display name alone cannot bind a chat.
7. Generate a service unit with the absolute configuration path:

   ```sh
    bin/teem --config /absolute/plugin/config.toml service-unit
   ```

8. Review and save that output as `~/.config/systemd/user/teem.service`.
   Run `systemctl --user daemon-reload`. This does not start the supervisor.
9. After authorisation, set `enabled = true` and invoke the plugin's start action.
   To start after login without opening Herdr, explicitly enable the user service.
   This MVP does not change login, linger, or boot policy.

There is no automatic startup hook: installation/loading must not activate even
an already configured supervisor. The explicit start action starts only an enabled,
configured service; missing configuration is a no-op. An enabled configuration
without an installed unit produces a visible service error. Existing operator-enabled
systemd/login policy is unchanged. systemd owns the supervisor process.

## Enable the current native chat

Loading the native server plugin makes Teem available in all top-level chats in
that location. Load it globally for all projects. Native tool calls authenticate
the session and agent from host-provided context; they do not require a Herdr pane,
prior `/teem enable`, or a registered project. Status, retained evidence, and
session-local checklists work before an execution policy is configured. The first
call creates only a bookkeeping binding marked `native-session:SESSION_ID`, not a
Herdr terminal identity. Existing real terminal bindings are preserved.

The optional TUI command `/teem enable` binds the exact current top-level chat,
Herdr terminal, native agent/project/directory and selected model. A managed-project
alias is captured when configured; missing registration does not block enablement.
Repeating it is idempotent; it starts no work or service and
does not change permissions. Standard installation registers the TUI and native
server bridge with stable absolute `executable` and `config` options. Missing
supervisor configuration does not prevent static tool catalog registration.
Setup discovery reads only the configured projection path, without opening state.
The server uses trusted native tool
context rather than rebinding location-global MCP. See
[issue #1 setup, restrictions and qualification limits](ISSUE1_ENABLEMENT.md).

Native delegation is managed-only: `foreman_submit` is omitted/rejected, and both
workflow admission and reconciliation require exactly one configured home policy
matching the chat directory. The authorised target may be another configured
project; it retains its own source, checks and access policy. Reconciliation also
verifies candidate ownership and preserves its project and issue. Teem does not
guess test commands, register execution policies automatically, or treat loading
as permission for paid work. Evidence inspection stays available even if a policy
is removed. Existing fixed MCP remains
independently bound legacy compatibility as documented below.

## Foreman tools

This fixed-session MCP setup is **legacy compatibility**, not required by the
standard native installation. Configure it only for an intentionally fixed
Foreman runtime. Merge this example into existing configuration.
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
- `foreman_handoff`: after explicit human approval, move management of one named workflow and its explicit same-task descendants to this chat. Supply its exact current manager and a reason. Original native owners, requests, policy, snapshots, events and uncertainty remain unchanged. The prior manager retains read access, not mutation authority. New final notifications route to the new manager; old notifications are not replayed. New task scopes, budgets, integration, publication and device authority are not granted. Named status/task/event inspection follows the handoff; unscoped tools do not borrow another chat's authority.
- `foreman_repair_prerequisites`: explicitly resume an old prerequisite-only policy stop during implementation, observing the same existing workers. It rejects changed source/check/authority policies, failed/uncertain/stopped workers, partial snapshots, or admitted checks/reviews. It preserves the original admission policy and prior event, records pin/repair history, and emits a new final notification after completion. No worker replay or correction-budget renewal.
- `foreman_result_notice`: archive or restore a saved terminal result notice with a reason. This changes presentation only, never task state, review approval, integration, event acknowledgement, or retained evidence. Use after a requested dismissal or verified delivery elsewhere. Active or uncertain work cannot be archived. An explicit same-task successor clears the parent's notice automatically; unrelated pushes do not. Archived notices do not cause older failed results to reappear. Saved plans remain separate and are not proof of active execution.
  Inspect the latest 50 notice changes through `foreman_status` detail with the workflow UUID and pointer `/result_notice_history`.
- `foreman_plan`: define this conversation's ordered checklist, linking execution items to retained task UUIDs.
- `foreman_workflow`: supply an explicit authorised GitHub `issue`, configured project alias, retained workflow UUID, scope and acceptance criteria. No checkout paths or base hashes are needed.
- `foreman_recover_qualification`: explicitly qualify an eligible retained setup-blocked candidate with fresh checks and two reviews, never an implementation/correction worker. Retain a recovery UUID and expected snapshot digest. Supports historical synchronous ENOENT through validated controller/artifact provenance, not missing handles or error prose alone; see [admission, idempotency and historical proof](ISSUE20_RECOVERY.md).
  A further explicit review requalification requires a strict blocked verdict and a demonstrated new prerequisite/reference-delivery repair. The full original implementation, snapshot, check and review provenance is revalidated; unchanged blocked attempts cannot be renewed.
- `foreman_reconcile`: explicitly continue the same task on current clean source/current policy. For a positive-non-launch setup-blocked retained candidate, use the distinct `setup_snapshot` digest with a retained new workflow UUID and source workflow ID. This carries patch/findings into one implementation successor; it does not qualify the old snapshot or replay native work. Issue/scope are inherited, provenance is revalidated at seed, and all checks/two reviews run afresh. See [setup handoff](ISSUE20_RECOVERY.md#explicit-setup-handoff-onto-advanced-source). Existing ready/exhausted/confirmed-stop and review-output `blocked_snapshot` contracts remain separate.
- `foreman_integrate`: after approval, apply a ready exact candidate to its original clean source. Supply a full `commit` instead to verify and record an already integrated exact-candidate commit.
- `foreman_color`: set this bound Foreman's local accent using `project` and a six-digit hex `color`. No shell or restart is needed.
- `foreman_stop`: after approval, durably pause a workflow and request interruption of its active native agents. An optional exact `task_id` selects one agent; the workflow still pauses, while already-running siblings are not interrupted.

### Compact dispatch inputs

Prefer nested `assignment` for new `foreman_workflow` calls: keep `workflow_id`,
`project`, `issue` and an optional short `title` at top level; put `scope`, the full `prompt`
and optional `workers` inside `assignment`. For `foreman_submit`, keep `task_id`
and `role` at top level and nest `checkout`, `revision`, `scope` and `prompt`.
Never mix nested and top-level assignment fields, even with equal values.

OpenCode's generic collapsed tool summary excludes objects/arrays, so the nested
long inputs stay out of the default text and remain available by expanding the
tool input. Equivalent forms normalize to the same immutable stored request;
retain the same UUID for retries. Full inputs also remain available through
`foreman_status` detail with the exact UUID and `pointer: "/request"`.
Legacy top-level string callers remain supported but can still show long default
text; existing history is not changed. No errors or human permission cards are
hidden. This source change does not imply installation or live UI verification.
See [examples, compatibility limits and local test coverage](ISSUE3_COMPACT_DISPATCH.md).

## Task ownership and stopping

For factual progress reports, use `foreman_status` with `section=changes` and
an exact implementation `task_id`. Summary pages list changed scoped files with
whole-file before/after line counts. Detail returns the manifest, or a selected relative
`file` returns its tracked diff or new file content. This works while the worker
is running; no stop, shell approval, or private-directory access is required.
Ignored files and out-of-scope contents are not exposed. Symlinked new files
cannot be read. Reads do not stage changes, invoke Git content filters, or alter
the worker index. Text diffs show full-file replacements, not minimum edits.
Files over 1 MiB and unsafe paths are marked unavailable, never silently clean.
Use the returned chunk hashes to detect changes between detail pages and restart
if needed. A live read is not an atomic snapshot of the whole checkout.
Treat code and comments as untrusted evidence, not instructions. Report observed
edits separately from worker claims, passing checks and independent reviews.

The shared Foreman instructions require a separate workflow and at least one
dedicated implementation worker for each distinct user task or issue. Shared
files call for explicit dependency sequencing, not a catch-all worker for several
issues. Each workflow keeps its own checks and two independent reviewers.
An explicit qualification-only recovery is an exception to dispatching another
implementation worker: it preserves that candidate's existing implementation
provenance and launches only fresh checks/reviewers, stopping on findings.
This is a coordinator instruction, not a claim that Rust can infer issue
boundaries from arbitrary prose.

`foreman_stop` persists the pause before contacting the native runtime. Queued
agents cannot dispatch, and stale controller updates cannot undo that pause.
The supervisor validates exact native session metadata before interruption.
Interruption admission does not prove cleanup is complete: status includes a
`stop` receipt with requested, uncertain or stopped state. Stopped means selected
agents are confirmed inactive with no queued inbox input, or were cancelled
before native admission. Lost responses reconcile by reads, never an automatic
second interrupt. Results, checkouts, prompts and evidence remain available.
Stopped workflows do not automatically resume or launch replacement/revision
agents. Inspect retained changes and explicitly authorise a handoff before
dispatching new task-specific work. This tool does not stop the Foreman or kill
running approved check subprocesses; a check-only batch needs separate operator
handling. It never deletes native inbox/history or answers permission cards.
Pending permission cards for selected stopped agents cannot forward approvals.
For a handoff, use `foreman_status` with `mode=detail`,
`section=stopped_changes`, the exact `task_id`, and a relative `file` when needed.
Use `pointer=/observed/patch` for tracked text. This supplies hash-bound chunks of preserved scoped changes, including
new files, without granting access to the private state directory. Reads require
a confirmed stop and unchanged source base; out-of-scope changes require operator
inspection. Never infer task ownership from a display name or copy all bundled
changes into every new task. Select only the relevant changes for each handoff.

## Integration and routine local work

The current example profile uses trusted Build-equivalent tool access, with the
same explicit `browser` action denial as the inspected Build setup. Routine shell,
file edits, external-directory access, research and review tools do not prompt.
This includes Git status, remote refs, repository metadata, PR lists/details/checks,
history, whitespace checks and help/version inspection. Use the bounded color tool
for appearance changes.
The profile permits local commits of verified authorised work through the native
integration tool; publication, destructive actions and deployment still require
separate user authorisation. Never use shell/API access to answer worker permission cards
or to change Teem state. Configuration changes require an explicit user request.
These are behavioural approval rules, not shell-enforced barriers or an OS sandbox.
See [the durable Foreman policy and reinstall steps](FOREMAN_ACCESS.md).

Worker shell access is separate. The opt-in `trusted_worker_shell` setting applies
only to new dispatches and defaults to false; it does not change reviewers or
existing sessions. It grants host-user shell authority, including writes outside
built-in edit scopes. See [trusted worker mode and its limits](TRUSTED_WORKERS.md).

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

### Explicit issue tracking

Every **new** managed workflow requires a structured top-level `issue` string,
for example `"issue":"tidley/teem#8"`. Both compact `assignment` and legacy
top-level assignment forms use this same field and admission gate. Use an
explicit user-authorised association; titles and prompts never establish ownership.
The syntax is GitHub `owner/repo#positive-number`: ASCII owner/repository names,
owner up to 39 bytes, repository up to 100, positive unsigned 64-bit issue number.
Owner/repository case and leading number zeroes normalize to one lowercase reference.
URLs, bare `#8`, zero, negative/fractional numbers and extra path segments reject
with a corrective example. This validates syntax only, not GitHub existence or
user authority. It makes no network calls, needs no credentials and never publishes
issues. Having an issue never authorises paid work, expanded scope or deployment.

The canonical association is part of the immutable request. Retain the UUID and
same inputs on retries; changing the issue or scope under that UUID rejects.
Workers, reviewers and corrective rounds inherit it. `foreman_reconcile` inherits
the issue, scope and acceptance criteria and rejects an issue switch. An eligible
unlinked historical source needs an explicit `issue` on its **new** continuation;
retain that field on retries. Its parent request, native sessions and evidence
remain unchanged. Existing candidate, stop, activity, budget and qualification
gates still apply; this is not recovery permission or a legacy bypass.

Pre-upgrade unlinked workflows keep progressing and exact immutable retries remain
usable without backfilling an issue. Historical records are not rewritten or replayed.
The schema leaves `issue` optional solely to admit those old exact retries; a new
UUID without it rejects at admission. The legacy fixed-checkout `foreman_submit`
API, native todos and unrelated manual plans remain compatible, not a new alternative
for avoiding managed checks. Foreman guidance requires authorised issues for new
delegated tasks; use managed workflows for their checks and independent reviews.

Verified associations prefix the workflow heading, all generated worker/check/review
and agent-reported todo details, and manual plan items linked to those managed tasks.
Already-prefixed labels are not doubled. Compact sidebar groups keep short stage
labels with the same issue prefix, rather than repeating full workflow titles.
Unlinked/native/manual text is not interpreted as an association. Agent reports
remain claims, not qualification evidence. Bounded status, approval and native task
IDs are unchanged. These are source contracts, not proof of installed/live UI behavior;
the fixed local transport and adapter tests need supervisor execution. Independent
table/user-input reporting guidance is documented under [user-requested status tables](#user-requested-status-tables).

### Continuing authorised work

Routine in-scope engineering decisions are autonomous by default. The Foreman
chooses the smallest complete solution and handles ordinary local blockers, then
briefs the user on decisions, evidence and limitations at completion or a genuine
blocker. A plan or design checkpoint is not an extra approval gate for clear scope.
See [engineering autonomy and its boundaries](FOREMAN_ACCESS.md#engineering-decisions-by-default).

An authenticated native chat with a valid home policy can dispatch explicitly
authorised work to another configured project alias without moving or rebinding
the chat. Admission and queued progression validate the home identity; the target
policy controls source, checks and worker/reviewer access. Fixed MCP keeps its
exact-directory execution contract. Existing sessions and saved prompts are not
rewritten. New completion notifications allow eligible bounded same-task
continuations within existing user authority and remaining budget; they do not
grant new authority. Exact older notification receipts remain valid without replay.

The Foreman can continue routine implementation and bounded corrections within an
authorised issue without requesting the same approval again. Integration approval
also covers necessary same-task reconciliation and its qualified successor. It
does not authorise another issue, endless retries, publication, installation or
deployment. Exhausted and stopped native workflows stay immutable; uncertain or
active work must not be replaced. Integration and stop still require user approval;
the trusted Foreman profile does not enforce that requirement with native prompts.

Use `foreman_reconcile` with a new retained `workflow_id` and the
`source_workflow_id` to carry a ready, exhausted or managed confirmed-stopped
candidate into a fresh managed workflow. For stopped work, explicitly authorise
the handoff first. The source must remain `blocked`, with `workflow_paused=true`
and a `stopped` receipt, and retain a valid exact candidate snapshot and checkout.

A narrow additional case is an unpaused next-round preparation blocked by a
recorded managed Git timeout. The advanced round must have no recorded round
entry and neither deterministic worker identity may have any task reservation,
even terminal. Task reservation precedes native dispatch, so this establishes
that this round never dispatched. The previous real snapshot/checkout must still
verify. Explicit reconciliation uses a new workflow and checkout, preserves the
partial failed directory and rechecks this no-launch guard before seeding. It
does not retry the parent, qualify its failed-review candidate or manufacture a
stop receipt. Other blocked states remain ineligible. Fresh checks and two new
reviews are required under the unchanged same-task acceptance criteria.

Queued, active or uncertain native tasks and unconfirmed task stops anywhere in
the original task's continuation lineage reject the handoff, including siblings.
A partial stop does not release running siblings. Pending/attempting/waiting or
uncertain stops and arbitrary blocked/failed workflows are not eligible. A narrow
exception handles an initial reconciliation stopped before capturing a snapshot:
one confirmed-stopped implementation worker must have made no changes, its input
tree must equal its base tree, and its retained notes must record `seeded=false`.
The pinned patch and immediate parent's real snapshot must match exactly. That
earlier source must satisfy the normal candidate contract (including positive
non-launch/provenance proof for setup failures). The new worker receives that
earlier patch for scoped conflict resolution; no snapshot is invented for the
stopped attempt. Both source identities appear in its notes. Admission and seed
revalidate the stopped workflow, worker, artifacts and quiescent lineage. Edited,
successfully seeded, multi-worker or unproven snapshot-less inputs are rejected.
No task is replayed, no revision budget is silently renewed, and no previous
check/review qualifies the successor. The parent remains paused and unchanged;
this does not resume its sessions, replay requests or forge a ready state.
The project identity, task scope and acceptance criteria are inherited;
the current project policy and clean source base are used. Active, uncertain,
foreign and duplicate active continuations are rejected. Parent snapshots and
review/check evidence remain unchanged. Each continuation has a dedicated worker
and must pass fresh checks and two independent reviews, even if the old patch
applies cleanly. The worker can read retained patch/findings in
`.git/teem-reconciliation.patch` and `.git/teem-reconciliation.json`. If application
conflicts, current source is retained and the worker reconciles within scope;
the tool never force-applies a patch to source. A new continuation is not automatic
permission to renew exhausted budgets forever.

Revision budgets are scheduling limits, not integration requirements. A change
only to `max_revisions` does not revoke an existing candidate's qualification or
alter its captured allowance. Changes to source identity or approved checks still
block old qualification. Advanced source needs reconciliation and new evidence.
MCP reloads machine-local configuration for each tool call; invalid configuration
fails closed rather than silently using a stale policy.
Native session creation and prompt admission allow up to 60 seconds for a busy
OpenCode server to respond. Observations and control calls retain a ten-second
deadline. This does not limit model execution time or enable automatic retries:
an unknown admission outcome still reserves its exact native identity until
reconciled or explicitly stopped. Never resend an uncertain prompt to recover it.

With authorised local-commit authority, `foreman_integrate` accepts
`commit_local=true`: it applies and commits only the exact qualified candidate,
records the receipt, and leaves source clean for serial integration. It never
pushes or publishes. Do not combine it with an existing `commit` argument.
A durable committing marker prevents automatic retries after an uncertain commit;
only a matching commit can be reconciled by reads. Dirty or unrelated source work
is never automatically committed. Check the returned receipt before reporting
integration, and distinguish local commits from installation or publication.

Before applying a candidate for `commit_local`, Teem checks that Git can resolve
the operator's author and committer identity. It imports only configured identity
and signing settings from the operator's normal system/global/repository config;
repository-local settings retain precedence. It does not guess an author, change
Git configuration, inherit ambient `GIT_*` overrides, or silently disable signing.
The preflight snapshot is reused for the commit. A read-only comparison after
apply rejects changed or newly added identity/signing settings, including local
includes; it never adopts those new values. Other managed Git configuration isolation and the existing disabled-hook policy
are unchanged. Imported values are passed through Git's config environment, not
command arguments or diagnostics. Missing identity fails before source/index
changes; a signing/commit failure retains the committing receipt and still must
be reconciled by reads or an exact existing commit, never a blind second commit.

Add a machine-local project policy to the plugin configuration, not to worker-editable
source. Only explicitly configured projects can run managed work:

```toml
[projects.pilot]
path = "/absolute/project/root"
max_revisions = 3

[[projects.pilot.checks]]
name = "tests"
kind = "test"
argv = ["cargo", "test", "--locked"]
cwd = "."
timeout = 300
output_limit = 1048576
```

The operator must approve these commands and trust the project's test/build scripts.
An optional project-local `reference_paths` list approves external references for
new managed implementation workers only. It does not widen edit scope or change
shell/reviewer permissions. Paths must be existing absolute literals; canonical
targets and file/directory types are captured in the policy. Omitted or empty lists
preserve the old policy and permissions. Exact-file native tool semantics still
need qualification; no parent-directory fallback is granted. See
[reference configuration and qualification limits](ISSUE12_REFERENCES.md).

Managed checkout creation uses Git's local clone mode so successive workers and
reviewers do not repack the source history for every isolated checkout. Immutable
objects can be hard-linked on the same filesystem; indexes, refs, configuration
and working files remain separate. Inherited alternate object stores are
dissociated so a clone does not depend on a later source prune. Git refuses
symlinked local object storage. Failed or timed-out partial checkouts remain
preserved; they are not reused or deleted by a retry.

Routine-check instructions defer to captured native shell permissions. An allowed
scoped inspection or focused regression command needs no extra human approval;
an `ask` decision still belongs to the human. The supervisor still runs the
recorded checks against the exact candidate. Trusted shell does not authorise
publication, device operations, expanded scope or answering permission cards.

If omitted, `max_revisions` defaults to three. It permits one initial implementation
plus three corrective implementation rounds, not three total attempts. Each new
candidate must pass its approved checks and two exact-candidate reviews. An explicit
value from zero to three overrides the default. Budgets are captured at admission;
changing the project setting does not reopen exhausted workflows or replay uncertain,
failed or interrupted native sessions. Preserved work needs an authorised follow-up.
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

Initial and revision workers receive the recorded policy's exact check names, argv
and candidate-relative cwd as data. Their guidance says **not to run or request those
checks**: the supervisor runs them against the fixed candidate after the worker
responds, including on revisions. Workers report additional verification needs, not
claims that they ran deferred checks. Small lists are inline JSON. When the full list
would exceed the 30,000-byte task prompt budget, the supervisor writes the complete
list to `.git/teem-routine-checks.json` in that worker's checkout, mode 0400, and asks
the worker to read all pages. Commands are never shortened to fit; the fallback text
must also fit. Revision diagnostics are bounded at a UTF-8 byte boundary.

This is guidance, not an automatic approval path or a guarantee that a model will
never request shell access. Unmatched commands still require native human permission;
workers must not answer cards or change policy. It does not implement persistent
project grant inheritance. An operator may explicitly add the exact optional
`["git", "diff", "--check", "HEAD", "--"]` inspection with `cwd = "."` alongside
the required test check. Unlike plain `git diff --check`, it covers staged as well as
unstaged tracked changes; new untracked files enter the supervisor's staged candidate
before checks. Never save a wildcard `git diff *` approval or grant generic shell
access for this purpose. The commented example is inert, not live configuration.
See [issue #5 qualification](ISSUE5_ROUTINE_CHECKS.md) for scope and verification limits.

For an authorised task, Foreman calls `foreman_workflow` with issue reference, project alias, scope and
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

Standard installation registers the package's server/TUI exports in global
`opencode.json`/`opencode.jsonc`. It discovers `stateFile` from supervisor
configuration; an explicit existing option wins. The following older CLI-only
setup is optional compatibility, not a second installation step. Avoid duplicate
package registrations. To use it, merge these entries into `~/.config/opencode/cli.json` in this order,
preserving other settings/plugins (use your actual absolute paths):

```json
{
  "plugins": [
    "-opencode.sidebar.todo",
    { "package": "/absolute/teem/ui/teem-progress", "options": {
       "stateFile": "/absolute/private/state_dir/progress.json",
       "executable": "/absolute/teem/bin/teem",
       "config": "/absolute/private/config.toml"
    } }
  ]
}
```

The older example disables the native Todo panel. Its explicit `stateFile` retains
Teem's native Todo fallback for unbound chats, including their revert boundary.
Fresh global installations leave ordinary Todo to the host and add only subscribed
Teem progress. When restoring the host panel in an older setup, also set plugin
option `nativeTodos: false` to avoid duplicates. `nativeTodos: true` explicitly
requests the fallback; absent an override it is enabled only with an explicit
`stateFile`. Installation never edits `cli.json` or disables host sidebar features.
Other sidebar features are unchanged. Valid CLI changes reload without restarting
OpenCode's backend. The plugin adds no server, credentials or Python dependency.

Progress changes and individual managed-agent completions do not wake Foreman. Their
event receipts remain inspectable and unacknowledged (`passive` is not consumed or
approved). When a workflow reaches `ready`, `blocked`, or `needs_attention`, the
supervisor atomically records its final state and one native outcome event. Foreman
reads that workflow's evidence and acknowledges only after consumption. It continues
eligible bounded corrections or follow-on actions only within existing explicit
task authority and their normal gates; the event grants none. It reports decisions,
verified results and limitations at task completion or a genuine blocker. A ready
candidate need not be the end of an already-authorised integration task. Progress
rendering and unrelated work are not requested. The notification can queue behind
a user turn; individual stages still cost no Foreman turns.
An uncertain notification is reconciled by exact native ID reads, never resent.

Summary `foreman_status.progress.latest_result` also keeps one terminal result outside the
active checklist. The TUI shows its readable title and completed/blocked/needs-attention
summary, including “not integrated” for ready candidates. This remains visible when
active todos clear. New outcomes are ordered by recorded finish time. Older finished
workflows without that timestamp use admission time as a historical fallback; they
are displayed without backfilled notifications or any task replay. No historical
request, candidate, check receipt or reviewer verdict is rewritten.

A saved result also has a short notice above the typing box, outside the sidebar
and its collapse control. `/teem-result` (or **Teem: show latest result** in the
command palette) opens that chat's retained summary locally. It starts no model
call and does not resend or acknowledge a completion event. Explicit linked-work
views retain their original owner and label the link in the result dialog.
Stale observations say that current status is unavailable; “result available”
does not claim the candidate is integrated, installed or published. The dialog
shows the recorded outcome content, including any integration receipt summary.
The notice does not enable sounds, desktop alerts or force focus to another tab.
It becomes visible when that chat is viewed, even with the sidebar hidden.
This addresses a reproducible hidden-result path, not proof of why a particular
historical completion summary was missed.

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

Workers can edit only their declared relative scope through built-in edit tools.
Shell commands require human approval by default. Explicit captured
`trusted_worker_shell` policy allows worker shell without per-command approval;
see [its host-user authority and limits](TRUSTED_WORKERS.md). It does not change
task scope or authorise publication, device operations or state edits.
New worker sessions allow external-directory access within `/mnt/fastbuild/`, including
the required storage instructions and nested build-output directories. This exception
does not itself allow other external directories or unscoped edits. Explicit
project reference paths are separate read-context grants, not edit authority.
Use the central storage adapters; this permission does not authorise changing or deleting
other projects' build outputs. Reviewer permissions and global OpenCode defaults are unchanged.
Existing native sessions are not updated or replayed when this worker template changes.
Reviewers can use read, glob, and grep. They cannot edit, run shell commands, delegate, or use MCP tools.
These restrictions are runtime permissions, not an operating-system filesystem sandbox.
Human shell approval or trusted-worker mode can permit actions outside those tool
restrictions. Native permission cards remain human-only. The supervisor never
auto-approves permissions, merges, pushes, deploys or accesses hardware. The native
integration tool can create only an explicitly authorised exact-candidate local
commit; it cannot publish it.

## Status and recovery

### Finished reviewers with rejected output

A workflow can be blocked after both reviewers finish because a verdict fails
the snapshot-bound JSON contract (for example, a message exceeds 500 characters).
Worker success does not make such a verdict valid. The stop tool cannot stop an
already finished agent and must not fabricate a stopped receipt.

After explicit user approval for the same-task correction, use
`foreman_reconcile` with a new `workflow_id`, the original `source_workflow_id`,
and `blocked_snapshot` equal to the exact retained snapshot SHA-256. The source
must be unpaused, unintegrated and blocked, with passing completed checks and two
distinct finished reviewers, at least one invalid verdict. Check receipts and
output hashes must verify throughout the lineage; active/uncertain tasks,
unconfirmed stops, changed snapshots, unresolved helper launches or dirty source
prevent the handoff. The new correction workflow gets the retained patch and
rejected raw outputs as **unqualified evidence** and runs implementation, checks
and two independent reviews afresh. The parent, results and failed verdicts stay
unchanged. Recovery-only metadata does not carry into an implementation successor.

This is not qualification-only recovery of historical setup failures. An old
`spec.json`, empty helper log and missing handle do not prove non-launch. The
historical #19/#21 recovery contract still needs correction in the retained
recovery candidate. Do not bypass that distinction or edit runtime state to
manufacture eligibility. This API change requires separately authorised adoption
before the running supervisor/native bridge can use it.

Open the plugin's status popup on demand. It is not a persistent worker pane.
The popup is a snapshot, not a live dashboard. Close and reopen it to refresh.
Use the plugin's status action or run `herdr plugin pane open --plugin local.teem --entrypoint status`.
For machine-readable status:

```sh
bin/teem --config /absolute/plugin/config.toml \
  status --foreman ses_EXACT_REGISTERED_SESSION
```

### User-requested status tables

The tracked `examples/pilot-foreman.json` reporting guidance generally asks Foreman
to answer user-requested issue/task overviews with a concise table: **Issue/task**,
**Current status**, **Agent working**, **Complete**, **Blocked**, and **Awaiting user
input** when pertinent. Use authoritative current status and integration receipts;
unknown evidence is **Unknown**, not **No**. Identify the affected task and exact
decision/approval/permission for user blockers, separately from other blockers.
Omit an all-empty input column and briefly say no user input is needed only when
the evidence supports that statement. A ready candidate is not completed delivery.

This is guidance, not guaranteed model behavior, a new renderer, or a status schema
change. No live profile installation or runtime validation is implied. Completion
notifications retain their one concise outcome/ack contract, not polling, repeated
tables, checklist maintenance or dispatch. See [examples, evidence rules and
limits](ISSUE7_STATUS_TABLES.md). Bounded pagination/detail rules below still apply.

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
