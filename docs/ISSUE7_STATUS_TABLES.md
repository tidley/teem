# Issue #7: request-driven status tables

For a user-requested issue/task overview, the tracked Foreman example profile
generally asks for a concise Markdown table. This is reporting guidance, not a
renderer, UI redesign, new state/schema, or a guarantee of model behavior.
Installing the profile has not been performed as part of this change; existing
sessions, permissions, MCP configuration and runtime behavior are unchanged.

## Example overview

Illustrative evidence only, not the live status of these tasks:

| Issue/task | Current status | Agent working | Complete | Blocked |
| --- | --- | --- | --- | --- |
| Task A — docs | Implementing | Worker running | No | No |
| Task B — tests | Waiting for worker slot | Worker queued | No | No — queued |
| Task C — fix | Qualified ready candidate; checks and both exact-candidate reviews passed; not integrated | None | No — readiness is not completion | No |
| Task D — source-only fix | Verified local commit recorded; requested source-only delivery complete; installed/runtime validation and publication unknown | None | Yes — source-only scope | No |

No user input needed for this illustrative snapshot, whose evidence establishes
that none of Tasks A–D requires a user decision. The all-empty **Awaiting
user input** column is omitted. A queue wait is scheduling, not by itself a
user blocker. Use **Unknown** instead of this assurance when input needs cannot
be established.

## Pending-input example

| Issue/task | Current status | Agent working | Complete | Blocked | Awaiting user input |
| --- | --- | --- | --- | --- | --- |
| Task E — scoped check | Pending native shell permission | Unknown — permission pending, not proof of running | No | User — shell permission | Task E: choose Approve once or Reject worker batch on native form FORM_E for permission PERM_E; inspect its exact command/input |
| Task F — integration | Qualified ready candidate; not integrated | None | No | User — integration decision required | Task F: approve or decline integration of the exact qualified candidate; local commit only with separate applicable authority |
| Task G — checks | Configured test failed | None | No | Other — test failure | None — authorised bounded correction needs no new decision |
| Task H — legacy task | Current evidence unavailable | Unknown | Unknown | Unknown | Unknown — input requirements unavailable |

Form/request IDs above are placeholders. Use actual IDs when available and name
the affected task plus specific decision, approval or permission. Existing pending
native forms remain human-only: model text is not consent. Reporting an approval
need does not answer a card, grant permissions, or authorise integration.

## Evidence and boundaries

- Read authoritative current `foreman_status` evidence. Prefer later integration
  receipts over stale candidate outcomes; retain immutable candidates/history.
  Report a local commit only when recorded/verified, not merely worker-claimed.
- **Agent working** distinguishes running, queued, none and unknown, with current
  worker/reviewer roles when known. Starting, accepted, uncertain and `active`
  reserved slots are not proof of running. Report mixed running/queued roles
  separately when relevant. Agent-reported todos are not execution evidence.
- **Complete** concerns the user's requested scope. Native execution `succeeded`
  is not check/review approval. A qualified `ready` candidate passed configured
  checks and both exact-candidate reviews; it is not integrated. Applied is not
  committed; a local commit is not installed, runtime-validated or published.
  Status does not verify publication/runtime; use separate authoritative evidence
  or **Unknown**. Do not require publication for a source-only request, or claim
  delivery complete when its requested installation/publication remains unverified.
- **Blocked** gives the reason, distinguishing a user decision from technical,
  check, review, source-drift or evidence-access blockers. **Unknown** is not **No**
  for any field. If no input is known to be required, omit the all-empty input
  column and state that briefly; retain it with **Unknown** when evidence is absent.
- Preserve [bounded status/detail retrieval](HERDR_PLUGIN.md#bounded-status-and-evidence-retrieval):
  follow returned `next_offset` pages for the requested scope, use exact workflow/
  task IDs and detail pointers/chunks for clipped evidence, and restart chunk reads
  if hashes change. Label partial coverage; do not describe one page as all issues.
  Read relevant failures, findings and limitations before summarising/acknowledging.
  An unreadable result remains a reported blocker, not an acknowledged success.
- These tables are request-driven. Completion notifications still require one
  concise outcome summary and acknowledgement after evidence consumption. They
  must not trigger polling, repeated tables, checklist maintenance or work dispatch.
  Passive progress and existing native outcome/ack behavior are unchanged.

## Local regression coverage and limits

`tests/issue7_status_tables.rs` parses the tracked profile as JSON and checks
focused reporting and existing safety/permission instructions without whole-prompt
equality. These native tests check the tracked guidance, not model compliance or
live profile installation. Existing status and approval tests cover the underlying
evidence/permission contracts; no live model, service, or pending form is exercised.
