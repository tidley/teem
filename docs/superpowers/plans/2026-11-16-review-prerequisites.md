# Review prerequisite recovery implementation plan

**Goal:** Prevent unavailable review prerequisites from consuming code-correction attempts, and qualify a retained candidate without new implementation work.

**Architecture:** Explicit project `review_prerequisites` names small regular files. Admission freezes canonical file paths and SHA-256 digests; before worker/reviewer launch Teem validates those files and supplies private read-only checkout-local copies plus preloaded guidance. A strict `blocked` review verdict is distinct from source findings. Explicit snapshot-bound qualification recovery reuses the existing no-worker recovery pipeline with a separate review-blocked proof, preserving the parent and validating source, policy, evidence and lineage.

**Tech stack:** Existing Rust, SQLite, JSON and native fake-runtime tests; no dependencies or live model calls.

## Execution

- [x] Add failing tests in `tests/review_prerequisites.rs` for file declarations, missing/changed files before launch, local read-only copies and no permission expansion. Implement `src/prerequisites.rs`, project normalisation and worker/reviewer preparation hooks. Preserve legacy policies and requests without declarations.
- [x] Add failing strict-verdict tests and workflow regressions for `blocked` with a bounded reason and no findings. Extend `src/workflows.rs` so blocked reviews retain their snapshot and results, terminate without another worker or incrementing the correction counter, and preserve genuine `changes_requested` behaviour.
- [x] Extend `foreman_recover_qualification` with explicit `review_snapshot` alternative to `expected_snapshot`. Add a separate review-prerequisite proof in `src/recovery.rs`: require valid non-unanimous completed reviews, passing verified check receipts, two distinct completed reviewers, quiescent unpaused lineage, no integration, immutable snapshot and unchanged clean source. Do not infer eligibility from historical prose. Allow only explicitly configured prerequisite repairs; keep execution scope/checks/base unchanged. Run fresh checks/two reviews and forbid all implementation/correction workers.
- [x] Test idempotent admission, policy/source/snapshot drift, uncertain work, duplicate recoveries, no parent changes and new blocked reviews. Retain existing stop/fresh-check failure coverage through the shared qualification pipeline. Test positive progression to ready without a worker for structured blocks and explicit historical requalification.
- [x] Replace ambiguous exhausted-budget reporting with the configured number of automatic code-correction attempts. Update `src/tools.json`, status/progress reporting, README and recovery documentation.
- [x] Run focused tests during each red/green step, then `cargo test --locked --quiet` (307 passed), `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, `bun test ui/teem-progress` (48 passed), and `git diff --check`. Obtain two independent read-only reviews in parallel and fix demonstrated findings.

Additional user steering: implemented formatted result sections with causal attention reasons and tested the production renderer at 44/100 columns. Separately reviewed and applied the explicit human review-waiver exception to live shared/PAVE policies, matching approved copies and both Teem profiles. No native service deployment, PAVE commit/push, workflow replay or state mutation occurred. Scope and authority remain separate from review qualification.

Implementation stays in the existing checkout. No commits, native deployment or PAVE workflow continuation are part of this task. The separately requested live instruction-policy exception is described above. Preserve unrelated maintenance and receipt changes. The existing legacy setup-recovery contract remains unchanged.
