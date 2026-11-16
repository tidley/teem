# Teem tool names implementation plan

**Goal:** Advertise `teem_*` tools and use Teem in current UI/instructions, without
rewriting native identities, stored requests, old notifications or configuration keys.

**Architecture:** Keep legacy dispatch identifiers internally. Publish only the
Teem catalog and normalize both prefixes at authenticated call boundaries. Preserve
the exact earlier completion payload as an accepted receipt before changing new text.
Use the existing native bridge and hot-reload path; add no dependency or state migration.

**Verification:** Rust native/contract tests, Bun UI tests, formatting, strict Clippy,
staged whitespace checks and two independent read-only reviews. Preserve maintenance
changes outside the build/commit/deployment candidate.

## Steps

- [ ] Add failing catalog and dispatch-alias tests in `tests/native.rs`; verify
  `teem_status` and legacy `foreman_status` have the same result and native
  `teem_submit` cannot bypass the managed-only gate.
- [ ] Update `src/plugin.rs` catalog and call normalization and `src/enable.rs`
  filtering/guard normalization. Keep all schemas and task ownership unchanged.
- [ ] Update current completion instructions and new worker titles in
  `src/runtime.rs`; test exact acceptance of pre-rename receipts without resending.
- [ ] Update the TUI status hint, current profile/tool guidance and active docs;
  retain historical fixtures and legacy configuration/path names.
- [ ] Run full verification and two read-only reviews against a maintenance-free
  candidate; fix demonstrated findings and verify again.
- [ ] Deploy only the approved Teem update, without restarting shared OpenCode or
  interrupting tracked agents. Existing transcript tool names stay historical.
