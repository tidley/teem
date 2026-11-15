# Stopped unseeded handoff implementation plan

**Goal:** Preserve and reuse an earlier real candidate when an explicitly stopped initial reconciliation made no edits after its patch failed to seed.

**Architecture:** Keep `foreman_reconcile` and the managed confirmed-stop contract. Add a narrow fallback for one interrupted implementation worker, no snapshot/check/review admission, unchanged input/base tree, and verified retained reconciliation artifacts. Select the immediate parent's actual snapshot; never invent a snapshot or unpause/replay the stopped run. Validate the whole lineage and pin the stopped workflow/task evidence for seed-time revalidation. Setup-blocked ancestors must still pass the existing positive non-launch/provenance checks; qualification-only recovery is not relaxed.

**Non-goals:** No live task/state edits, new paid workers, installation, restart, publication, budget renewal or general recovery refactor. A modified, successfully seeded, multi-worker or snapshot-less ancestor is rejected rather than discarded.

## Steps

- [x] Add native regressions to `tests/native.rs`: real failed-seed reconciliation, managed stop before output, parent patch preserved into a new worker, full fresh check/two-review pipeline, immutable source evidence, read-idempotent retry, unsafe cases and seed-time drift rejection.
- [x] Run `cargo test --locked --test native stopped_unseeded` and observe the missing-snapshot failure.
- [x] Add `src/stopped_input.rs` for stopped-input/lineage/artifact validation and a narrow internal source-candidate validator in `src/recovery.rs`. Connect to `src/reconcile.rs` without changing the public arguments. Pin source ownership and stopped workflow/task digests; carry both identities in worker notes.
- [x] Document the exact stopped-input exception and rejected cases in `docs/HERDR_PLUGIN.md` and the reconcile tool schema description.
- [x] Run targeted tests, root Rust tests, formatting, strict all-target Clippy, Bun helper tests and whitespace checks using central build storage. Do not change configured workflow timeouts.
- [x] Request two independent read-only reviews in parallel, fix demonstrated findings and verify again. Leave code uncommitted/uninstalled unless separately authorised and report that boundary.

## Verification

244 Rust tests and 38 Bun helper tests passed; formatting, strict all-target
Clippy and whitespace checks passed. Both independent reviewers reported no
remaining findings after the staged-only-edit regression was reproduced and
fixed. Seven targeted tests include admission and seed rejection of staged-only
changes, preserving the real index bytes. No live state, service or task changed.
