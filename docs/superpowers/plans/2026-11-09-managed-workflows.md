# Managed Workflows Implementation Plan

**Goal:** delegate by project/acceptance criteria, automatically run approved checks,
and review/revise the exact uncommitted result.

**Architecture:** `foreman_checkouts.py` owns safe local clones and content snapshots;
`foreman_checks.py` owns asynchronous exact-argv checks and receipts;
`foreman_workflows.py` owns durable bounded progression. Existing Store, task runtime,
MCP and progress paths remain the transport and execution authority.

**Tech stack:** Python 3.11 stdlib, Git, SQLite, current OpenCode HTTP API.

Work inline in the assigned checkout. No commits or implementation subagents.

1. Add failing `tests/test_checkouts.py`: separate clones, dirty-source rejection,
   no source mutation, binary/new/deleted files, scope escapes, tampered snapshots,
   exact review trees, and preserved existing paths. Implement local Git operations
   with disabled hooks/global config, private alternate indexes and SHA-256 manifests.
2. Add failing `tests/test_checks.py`: strict argv/cwd policy, no shell evaluation,
   real success/failure/timeout/output-limit receipts, asynchronous launch and receipt
   validation. Implement a private helper, durable launch spec and bounded process
   execution. Never automatically repeat a launch after an uncertain response.
3. Add failing `tests/test_workflows.py`: automatic paths/base selection, immutable
   request/policy, two reviewers of exact candidate, strict verdicts, bounded revision,
   failed/uncertain execution, crash markers and policy revocation. Add SQLite tables,
   managed-path authorisation and a deterministic engine integrated before scheduling.
4. Add fixed-bound workflow admission/status MCP tools and native progress projection.
   Preserve all legacy task/request/event payloads. Update opted-in profile to use
   workflows and supervisor-run checks, not blanket shell access or manual Git plumbing.
5. Run `python3 -B -m unittest discover -s tests -v` and `git diff --check`; obtain two
   independent read-only reviews in parallel. Fix demonstrated findings and verify.
6. Back up local state/config/profile; inspect queue and native identity. Refresh only
   Teem and pilot MCP safely. Configure only the disposable pilot's existing unittest
   command as a supported policy; do not start model-backed work for qualification.
   Report implemented, local-tested, running, and live-qualified states separately.
