# Issue #4: compact passive sidebar

The existing `sidebar.content` slot now groups current Teem work by **workflow
ID**, not title or project. No host renderer is patched. A workflow title appears
once (up to 32 Unicode code points, with an ellipsis); its project appears once
unless already present in the title. A short workflow-ID prefix extends when
necessary to distinguish concurrent workflows. Revision and phase appear beside
that identity. Rows use Implementation, Tests, Format, Clippy, Review · correctness
and Review · tests/scope. Configured `rust-tests`, `rust-format`, `rust-clippy`
and `tui-tests` names display as Tests, Format, Clippy and UI tests; other
configured check names remain recognizable.

Example intended for an approximately 35-column sidebar (host wrapping applies):

```text
Teem · current work (4)
Compact sidebar
teem · #a12bc345 · check batch
[✓] Tests · passed
[ ] Format · pending (batch)
[ ] Clippy · pending (batch)
[ ] Review · correctness · pending
[ ] Review · tests/scope · pending
Details/history: foreman_status
```

The count is displayed pending/active rows, **not running processes or remaining
workflow gates**. Successful implementation rows and completed agent claims are
hidden; claims from ended tasks are hidden as well. Failed/interrupted execution,
failed checks, checks not reached after a batch stops, requested review changes,
pending verdicts, and approval/uncertainty blockers remain visible. Passed check
and review rows retain short explicit labels. Batch activity never marks every
individual check running: until its result exists, each check is pending.

Agent rows start with `Reported:` and share one `Reported = agent claim, not
evidence` disclaimer. They never become supervisor check/review results. Compact
claim text is bounded to 56 code points and phase/check names to 28. Textual
states accompany icons, so color is not the only status signal. These bounds are
not terminal-cell measurements; wide glyphs and long fallback/manual rows wrap.

## Evidence, compatibility and details

- Rust adds optional `sidebar` display metadata to the existing full `items`.
  Original IDs, full labels/content, native-compatible `todos`, stored requests,
  results and history remain available. Individual unreported checks now have
  `pending` status even while their batch is active.
- `foreman_status` provides the full IDs and bounded summaries. Request
  `{"mode":"detail","section":"progress","workflow_id":"EXACT_UUID"}`
  for full projected rows, or detail `overview` with that ID for recorded rounds,
  checks/reviews and integration receipts. Follow returned chunk pagination.
  UI truncation/hiding is not evidence deletion or permission to infer PASS.
- Manual/legacy entries are neither deduplicated nor hidden. Old projections with
  `todos` but no `items` retain their previous rendering. Old workflow items without
  display metadata use their own workflow's label prefix, never title identity.
- Unknown display fields are ignored. Bounded recognized metadata is selected,
  not blindly copied. Malformed items retain their corresponding compatible todo;
  malformed/oversized or count-mismatched item arrays show an explicit compact-view warning and the
  original todos. Invalid core projections still retain the last observation via
  the existing reader error path; they do not silently clear it.
- The passive one-second file timer, 15-second stale threshold and five-second
  future-clock tolerance are unchanged. Stale groups and latest results stay
  visible; a fresh projection replaces them. No model polling, synthetic refresh,
  tool-history edits or new runtime permissions are introduced.
- Latest results keep the supervisor's original receipt text and precedence;
  ready is not integrated. Empty bound sessions remain explicitly empty. Other
  sessions still use real native todos and the original revert boundary.

## Qualification boundary

Deterministic cases are added to the approved
`ui/teem-progress/progress.test.mjs` entry point and `tests/native.rs`: short/long/
multibyte titles, same-title concurrent workflows/projects, claims vs evidence,
pending batches, failed checks/reviews, pending verdicts, manual/native/legacy
rows, malformed/unknown additions, empty work, stale recovery and receipt retention.
These helper/source fixtures do **not** demonstrate actual host rendering,
keyboard/screen-reader accessibility, terminal-cell sizing or visual behavior.

The retained issue #4 patch is reconciled onto source
`fc426f4b06c17c453578c2af07ee11f5b4ff19c1`. Its earlier check/review records do not
qualify this candidate. The reconciliation adds a deterministic configured-name
fixture before the display aliases; execution is reserved for the supervisor.

The managed supervisor owns the fixed snapshot and must run all configured checks
(`cargo test --locked`, `cargo fmt --all --check`, strict all-target Clippy and
`bun test ui/teem-progress/progress.test.mjs`) followed by two independent read-only
exact-snapshot reviews. One bounded revision is available under the assigned
workflow. Worker execution success is not review approval. Live host validation
and installation require separate authorization; this candidate remains local,
uncommitted and unintegrated.
