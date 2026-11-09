# Foreman operator completion

## Design

Keep one Foreman conversation for coordination and routine local editing.
Do not give it unrestricted shell access or access to worker approval APIs.
Compared with a read-only coordinator (incomplete handoffs) or unrestricted
shell (unnecessary authority), use two small native MCP operations:

- `foreman_integrate`: apply a ready, verified candidate to its original clean
  source at the exact base, or record an existing exact-candidate commit.
  Recheck patch, checks and both reviewer outcomes. Keep workflow history intact.
  Store a separate integration receipt; expose it in status and passive progress.
  Never commit, push, merge, deploy, rerun agents or overwrite dirty source.
  Persist an attempting marker before applying. Interrupted attempts require
  reconciliation by exact source content, never an automatic repeated apply.
- `foreman_color`: change only the Foreman color in its bound project JSON
  profile. Preserve permissions and MCP bindings; reject symlinks and bad colors.

Allow normal local edits, but not Git metadata or the Foreman's own permission
configuration. Shell commands require human approval. Integration also requires
native tool approval. Color changes do not need shell access. Existing worker
permission cards remain human-only. Routine status reads include integration
receipts and distinguish them from old candidate-readiness descriptions.

## Execution and verification

1. Add failing native integration, receipt, ownership and color tests.
2. Add the two tools, durable receipts and status/progress presentation.
3. Update the shareable and local profiles and the plugin guide.
4. Run locked Rust tests, formatting, strict all-target Clippy and Bun tests.
   Request two independent read-only reviews and fix demonstrated findings.
5. Build only after checking existing artifacts. Back up idle local state;
   replace the installed binary and restart only Teem. Reconnect its MCP server.
   Record the already integrated test commit through the new verified tool.
   Verify preserved task identities/history, live tools and updated chat state.

No automatic publication, new model workflow, destructive operation or enforced
wall-clock deadline is added by this change. Publication remains a separate gate.

## Verification evidence

124 Rust tests and seven Bun helper tests passed, together with formatting,
strict all-target Clippy and whitespace checks. Both independent reviews cleared
the final change after explicit root-path permission denies were added. Tests
with the actual local OpenCode wildcard matcher confirmed root/nested protection,
routine edit access, shell/integration approval and denied ordinary delegation.

The installed plugin and service use the same rebuilt native executable. Only
Teem was restarted and its project MCP reconnected. All original database rows
and managed candidate files matched the private stopped-state backup afterward;
the only new runtime record is the verified prior integration commit receipt.
The live Foreman successfully invoked its color tool with the unchanged current
accent and read the integration receipt. No implementation task was dispatched.
The source fix is saved locally; publication requires a separate request.
