# Issue #5: recorded routine-check guidance

This change supplies managed implementation workers with the exact routine checks
already recorded in their workflow's approved project policy. It does not approve
worker shell commands, change native permissions, or install/configure a service.
It is separate from native project-grant reuse (issue #2).

## Contract

- Every initial worker, including disjoint assignments, and every revision worker
  receives the recorded checks' exact `name`, `cwd` and `argv`. No unrelated project's
  checks or implicit inspection commands are added.
- The worker defers these checks until after its response. The supervisor executes
  them once through the existing fixed-candidate runner, in policy order, stopping
  on failure. Workers report extra verification needs without claiming deferred
  commands ran. Guidance itself is not test evidence.
- Check results and both independent read-only review verdicts must still qualify
  the exact candidate. A successful worker/reviewer execution is not review approval.
- Existing policy compatibility, revocation, isolation, uncertainty and once-only
  native permission safeguards remain unchanged. Changed checks/source or removed
  policy stop progression; scheduling-budget changes retain the existing exception.
  Policy changes do not cancel an already-running check or native session.
- The three-revision default survives: one initial implementation plus up to three
  corrective rounds, each requiring new checks and two exact-candidate reviews.
  Exhausted or uncertain workflows are not reopened or replayed by this change.

## Prompt-budget correction

The check normalizer permits eight checks, each with up to 64 arguments of up to
4096 UTF-8 bytes. Appending all of that data to the prompt could exceed
`Store::submit`'s 30,000-byte limit after checkout creation and block valid work.

The supervisor constructs the complete exact check list first. If the full guidance
and canonical JSON fit, it appends them inline. Otherwise it writes the full list as
pretty JSON to `.git/teem-routine-checks.json` in the new worker checkout, mode 0400.
The short replacement guidance directs the worker to read the complete file using
read pagination, without truncating commands. The replacement text is itself checked
against the final prompt budget, including assignment, revision, reconciliation and
native-todo text. This checkout-local file is policy evidence, not new shell authority
or a Teem runtime database edit. It is Git metadata, not a source change to integrate
or publish.

Revision feedback is capped at 12,000 bytes on a valid UTF-8 boundary, rather than
12,000 characters, so multibyte review diagnostics cannot silently expand the prompt
beyond its budget. Full feedback and prior candidates remain in workflow history.

## Exact optional whitespace inspection

An operator can opt into this **additional** inspection alongside a required test:

```toml
[[projects.example.checks]]
name = "whitespace"
kind = "inspect"
argv = ["git", "diff", "--check", "HEAD", "--"]
cwd = "."
timeout = 30
output_limit = 1048576
```

`HEAD` covers both staged and unstaged tracked changes, unlike plain
`git diff --check`. The supervisor stages captured new files in the candidate before
running checks. This is an exact argv/cwd opt-in, not permission for `git diff *` or
generic shell execution. The example configuration remains commented and disabled;
no live policy is edited. The same-user runner is not a sandbox: approve only trusted
commands, test code and build inputs.

## Qualification and honest limits

`tests/issue5_routine_checks.rs` reconstructs focused native Rust fixtures; it is not
a byte-identical transplant of an older candidate. Fixtures use temporary Git repos,
temporary stores and a fake local runtime, with no Python or live model/service calls.
They cover initial/disjoint and revision guidance, exact-policy isolation, changed
and revoked policies, no repeated worker admission while waiting, staged/unstaged
whitespace, maximum-size policy arguments plus 8000-byte assignments, oversized-policy
failed-check revisions with later checks skipped, and multibyte revision findings.
Existing approval-card tests retain once-only human decisions.

Required supervisor checks against the fixed final candidate are:

```sh
cargo test --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
bun test ui/teem-progress/progress.test.mjs
```

These fixtures and source inspection do not prove that a model will always comply
with guidance or never ask for shell access. Unmatched commands remain human-controlled;
workers must never answer permission cards, edit policy or bypass native approval.
No persistent project-grant mechanism is added. Live paid-model tests, installation,
restart, integration, commits and publication are outside this change. Check execution
and both independent exact-candidate verdicts must be reported separately by the
supervisor; this document does not claim they have passed.
