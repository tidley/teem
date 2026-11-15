# Blocked review handoff implementation plan

**Goal:** Break the finished-agent/rejected-review deadlock without granting qualification or fabricating stop receipts.

**Architecture:** Extend explicit same-task reconciliation with `blocked_snapshot`, an exact retained digest. Admit only unpaused, unintegrated blocked candidates with completed, verified check receipts and two finished reviewers. Check the entire lineage for active/uncertain agents, pending stops and unresolved check launches. Preserve rejected outputs as unqualified worker notes. A successor uses ordinary implementation/check/review stages; recovery-only request metadata must not leak into it.

**Scope:** This is the bootstrap handoff needed to correct the retained recovery implementation. It does not itself make historical #19/#21 setup failures eligible for check-only recovery and does not install or activate a service. No live state edits, candidate imports, commits or paid agents.

## Execution

- [x] Add isolated Rust regressions in `tests/native.rs` for a genuine overlong-review failure, explicit digest admission, immutable parent/results, fresh full pipeline, removal of recovery-only markers, and rejected unsafe handoffs.
- [x] Run `cargo test --locked --test native blocked_review` and confirm the missing API fails before changing production code.
- [x] Extend `src/reconcile.rs` and `src/tools.json`; validate lineage, receipts, reviewers and digest at admission and again before seed. Include rejected review outputs in `.git/teem-reconciliation.json` as unqualified evidence.
- [x] Improve `src/workflows.rs` validation diagnostics for overlong findings without relaxing limits; make the reviewer prompt request shorter messages. Test rejection rather than accepting malformed evidence.
- [x] Document the supported handoff and historical limits in `docs/HERDR_PLUGIN.md`.
- [x] Run root Rust tests, formatting, strict all-target Clippy, Bun helper tests and whitespace checks. Request two independent read-only reviews in parallel and fix demonstrated findings.
- [ ] Report verified source separately from installation/runtime status; a coordinated adoption still needs service/installation authority.
