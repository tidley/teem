# Completion visibility implementation plan

**Goal:** Keep the saved Teem outcome discoverable with the sidebar hidden, without another agent request.

**Evidence:** Backend delivery and the automatic summary exist. The current `tui.tsx` renders results only inside `sidebar.content` and its `open()` gate. This demonstrates a hidden-result path, not the exact cause of the historical report. No upstream transport failure is established.

**Design:** Preserve the sidebar. Add a short, wrapping notice in the supported `session.composer.top` slot. Use neutral “result available” rather than claiming a ready candidate is installed or integrated. Stale projections say status unavailable. Add a keyboard-accessible `/teem-result` palette command showing the retained bounded outcome in an explicitly requested dialog. Reuse the existing exact-session selection and explicit legacy read-only linkage. No alerts, sound, desktop notifications, unread fabrication, task replay, extra messages or model calls.

**Alternatives:** A toast disappears and does not fix retained visibility. Enabling desktop notifications would change user preferences. A persistent composer notice plus local retrieval addresses the demonstrated path without either behavior.

**Files:** `ui/teem-progress/progress.mjs` owns pure notice selection and command registration; `tui.tsx` owns themed rendering; `result.test.mjs` covers isolation, stale outcomes, empty work and slot placement. Update `docs/HERDR_PLUGIN.md` with the command and evidence limits.

- [ ] Add failing Bun regressions for notice selection, current-chat retrieval, linked ownership, stale evidence and sidebar-independent placement.
- [ ] Implement `resultNotice(progress)`, `installResultCommand(context, select)` and the composer slot using existing theme tokens and no dependencies.
- [ ] Run all Bun tests, full locked Rust tests, formatting, strict Clippy and diff checks. The Rust suite covers embedded installer assets.
- [ ] Request two independent read-only reviews in parallel and fix demonstrated findings.
- [ ] Record verified source and remaining live-rendering limits in #26. Local adoption must preserve existing configs and history; publication is not authorised.
