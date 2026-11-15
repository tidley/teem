# Qualification-only recovery (#20 infrastructure)

`foreman_recover_qualification` reserves a linked qualification-only workflow.
It preserves the retained candidate and parent history, and does not implement
parent #20 or import #19/#21/#22. No source integration, installation, service
operation, publication or additional spending authority is granted.

## Explicit admission and retries

Use the same authenticated native Foreman and configured project as the original
workflow. Native dispatch checks host-provided session/agent identity and the
current project policy for new admission. An existing recovery UUID retry is a
read: it still authenticates the caller and validates ownership and all immutable
arguments, even if execution policy has since been removed; it cannot relaunch.
Fixed MCP retains its existing exact Foreman binding and
same-user security limitations; that transport is not native caller authentication.

Read the exact candidate through bounded `foreman_status` detail first. Retain a
new UUID and the snapshot SHA-256, not its Git tree ID. Illustrative input only:

```json
{
  "recovery_id": "8e9c16c6-3a3c-40cf-aad1-2a8c20bf403b",
  "source_workflow_id": "b741a438-e336-48f5-9735-d6e4dfbbd0aa",
  "expected_snapshot": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}
```

Retry those **same three arguments** after a lost response: any existing attempt
is returned by reads, without repeating launch admission. Changed arguments under
that UUID reject. A different UUID for the same candidate rejects even after
findings; UUID churn cannot renew a correction budget. A setup-rejected recovery
attempt can itself be the source of another explicit recovery, provided its
positive non-launch proof and all ancestral bindings still validate.

SQLite immediate admission reserves the attempt before filesystem preparation,
subprocess admission or reviewer submission. The attempt records immutable request
arguments and digests of the full parent and its native task records. It inherits
the issue, scope, acceptance criteria, base and exact policy. Original native
identities, results, snapshot bytes and partial evidence directories are not
rewritten, reused or deleted.

Admission is deliberately narrow: an initial-round unintegrated setup-blocked
candidate, succeeded implementation provenance, exact snapshot bytes/tree/base
and scope, and verified synchronous non-launch evidence (a spawn-rejection receipt
or the narrow historical controller contract below).
Existing check/reviewer admissions must reconcile their own identities, not use
recovery to substitute a launch. Every lineage workflow must be terminal, with no
active/queued/uncertain native reservation, ambiguous check launch or changed
receipt. Any workflow pause or task-stop record rejects. The #2 paused/uncertain
handoff contract is unchanged. Consumed revision rounds require explicit scoped
reconciliation rather than resetting their budget through qualification recovery.

Source must remain clean at the original base and the complete current policy must
equal the admitted policy. These guards apply at admission, progression and queued
reviewer dispatch. Changed source/policy refuses qualification and requires
explicit reconciliation/requalification, never silently declaring a stale tree
integration-ready. Ordinary explicitly authorised reconciliation strips recovery
metadata and uses the existing implementation pipeline; it is not triggered by
recovery findings. Regression fixtures cover ready/needs_attention recovery
handoff both before and after an independent source advance.

## Explicit setup handoff onto advanced source

Qualification recovery still refuses advanced source or changed policy. When the
user authorises continuing the same retained implementation on current source,
`foreman_reconcile` has a distinct **setup handoff**, not the review-only
`blocked_snapshot` escape hatch:

```json
{
  "workflow_id": "79054c15-371b-4402-b368-cd513285cacf",
  "source_workflow_id": "b741a438-e336-48f5-9735-d6e4dfbbd0aa",
  "setup_snapshot": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}
```

These are the only three arguments for setup handoff. The source must already
have an issue; issue, bounded scope and acceptance criteria are inherited. It
requires the same retained snapshot, succeeded native implementation provenance,
positive modern/historical non-launch proof and quiescent lineage checks as
recovery. Pauses, any task-stop record, queued/active/uncertain work, ambiguous
launches and completed review admissions reject. Ordinary blocked error prose
or missing handles do not suffice. `blocked_snapshot` remains review-output-only.

Immediate transactional admission pins full parent/native-task digests and the
derived proof under `reconcile_setup_handoff`. A single successor UUID is retained
per source: identical retries return its current state by reads, including after
restart or uncertainty; changed arguments and alternative IDs reject. Admission
and seed both revalidate identity, snapshot, scope and proof. Seed excludes only
its own queued/preparing workflow with no native tasks from lineage admission,
not other reservations. Changed evidence fails closed before worker admission.

Unlike recovery, this explicit operation uses current **clean source and current
approved policy**. It carries the old patch/findings into one implementation
successor, strips qualification-only/recovery admission metadata, and preserves
the original workflow, base/tree/snapshot, history and native identities. Cleanly
applicable patches seed only the isolated checkout. Conflicts leave current code
intact and pass the retained patch to the scoped correction worker; main is never
reset or force-applied. All current checks and two independent exact-candidate
read-only reviews must run afresh before `ready`; no old evidence qualifies it.

This supports the advanced-source lifecycle shape reported for #19/#21; it does
not inspect, recover, replay or dispatch their real records. Failed recovery
admission IDs are not successor workflow IDs. Tool adoption, separately authorised
#19/#21 successor dispatch and combined qualification remain Foreman-owned gates.

## Fresh evidence, never a correction worker (qualification recovery)

The supervisor prepares a fresh complete snapshot-bound check checkout, manifest,
evidence directory, helper log, command logs and receipt under the recovery UUID.
It uses the existing check runner and digest validation for all configured checks
(four under the pilot policy). The runner stops at the first failed check; later
checks are unrun, not passing evidence. Only a complete passing batch admits
exactly two fresh independent reviewers, each in a separate complete exact-tree
checkout, using existing read-only permissions and strict snapshot-bound verdicts:

1. Correctness, contracts, errors, state and security.
2. Tests, regressions, scope and complexity.

Failed checks or either review requesting changes preserve findings and stop
`needs_attention`. Setup errors/malformed verdicts stop `blocked`. No implementation
or correction worker is dispatched, including on failure. `ready` means only the
fresh checks and both reviews passed. Worker claims, old checks/reviews and helper
probes never qualify a candidate. Integration still needs its separate authority
and existing exact-candidate gates.

Interrupted preparation or the launch-to-handle crash window becomes `uncertain`,
without replay. Saved check handles and native reviewer identities reconcile by
reads. Missing/late receipts do not authorise replacement. Retrying the same UUID
returns the retained uncertainty; another UUID cannot bypass it.

## Historical synchronous ENOENT admission

The old runner did not write `launch-failure.json` or `check_setup_failure`.
The operation also supports that history, without writing a retroactive receipt
or taking a new assertion from the caller. The trusted evidence is the retained
controller terminal outcome **combined with its write-order contract**:

- The initial round is `blocked`, unintegrated, with succeeded implementation
  provenance, no check handle/result/reviewer admission and no `resume_phase`.
- The controller's native terminal event belongs to that exact workflow/Foreman,
  and the round has its controller-generated completion timestamp. The exact
  synchronous outcome is `No such file or directory (os error 2)`.
- The private evidence directory contains **only** a valid complete `spec.json`
  and zero-byte regular `helper.log`. The spec's UUID token, directory, checkout,
  exact approved checks and snapshot digest validate against the retained round.
  Both retained checkouts, patch bytes, tree, base and scope must still validate.

Why this is different from trusting error prose: `Runner::start` has no fallible
operation after successful spawn; `worker_finished` puts the returned handle into
the in-memory round before the next fallible database write. The controller's
error handler saves that same round when recording `blocked`. A successful spawn
followed by a database error therefore retains the handle. A process crash before
that save leaves `snapshotting`, which becomes `uncertain`, never this synchronous
terminal outcome. A policy error on restart can produce `blocked`, but has a
different fixed outcome and is rejected. Missing files before spec/log completion
cannot satisfy the artifact checks. This relies on intact supervisor-owned state,
as do the existing workflow and structured receipt contracts; an operator-edited
database is not an alternate admission mechanism.

The recovery request pins a derived `historical-controller-enoent-v1` proof with
the complete spec digest and immutable terminal-event identity. Progression
revalidates it alongside the parent/task digests; notification acknowledgement
and delivery fields are deliberately not part of that immutable identity.
Any new log/result/handle, changed spec/event or missing provenance refuses
recovery. If either modern receipt exists, both must validate: no fallback from
partial/changed modern proof to historical admission is allowed. Error text,
process absence, deadline expiry and executable probes **alone** remain insufficient.

The same three-argument tool input above applies to historical candidates; no
operator SQL, fabricated receipt, permission change or implementation dispatch is
needed. The historical-shape fixtures exercise fresh four-check qualification,
both independent reviews, failing checks/findings, concurrent retries and restart.

This is the supported admission shape for retained historical setup failures,
**not a claim that any live records were inspected or recovered**. Actual runtime
identifiers belong in private operator evidence, not public documentation.
Admission must still validate all evidence and current
source/policy. Advanced source requires the explicit setup handoff above, not
qualification recovery. Any later combined reconciliation must preserve the original
snapshot SHA/tree/base, issue, scope, native identities and policy provenance.
Candidate-specific checks cannot qualify a combined tree or advanced source.
Neither candidate's code is imported here.

### Later combined reconciliation touchpoints

- `src/reconcile.rs` must continue removing `qualification_recovery` when creating
  an authorised implementation successor. Recovery itself must never call that
  implementation admission path to qualify a retained candidate.
- `src/enable.rs`, `src/plugin.rs` and `src/tools.json` supply native authority,
  dispatch and catalog contracts. #19's eventual installed executable/assets must
  expose the same catalog as the supervisor; the dynamic adapter needs no static
  recovery binding. Do not drop currently installed all-chat compatibility.
- `src/recovery.rs` pins the recorded project policy, base, issue and scope. #21's
  future onboarding/policy or optional-issue changes must explicitly reconcile
  those contracts rather than overwrite historical requests or reinterpret their
  provenance. A policy/source migration is not qualification of an old snapshot.
- #22's startup/reload work must preserve native reservations and uncertain launch
  outcomes; this operation neither reloads services nor grants adoption authority.

## Status, verification and adoption

Use the returned attempt ID as `workflow_id` for existing status/detail pagination
and `check_output`. Summary includes `qualification_recovery`, immutable arguments,
source ID, fresh checks/reviews and zero implementation workers. For example:

```json
{"workflow_id":"8e9c16c6-3a3c-40cf-aad1-2a8c20bf403b"}
{"mode":"detail","workflow_id":"8e9c16c6-3a3c-40cf-aad1-2a8c20bf403b","pointer":"/data/rounds/0"}
{"mode":"detail","workflow_id":"8e9c16c6-3a3c-40cf-aad1-2a8c20bf403b","pointer":"/request/qualification_recovery"}
```

The native adapter loads schemas dynamically; no per-tool TypeScript binding or
permission expansion is needed. Source qualification does not expose this tool
through an older installed executable or loaded catalog. Adoption requires a
separately approved coordinated executable/service update and tool reload,
preserving installed all-chat behavior, runtime state and rollback. None is
performed by this source task. Actual #19/#21 recovery, #22, durable service
adoption, combined qualification and live acceptance remain separate gates.

`tests/issue20_recovery.rs` contains native subprocess fixtures with isolated
HOME/XDG/config, four local Git checks, fake native outcomes, immutable retries,
concurrent IDs, restart/uncertainty and unsafe-source/evidence rejection matrices.
No live models are used. Coverage is not an execution result. The supervisor owns
all four configured checks and two independent exact-candidate reviews of this
feature; previous candidate checks/reviews are not transferable. Execution success
is not review approval.
