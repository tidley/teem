# Explicit native chat enablement (issue #1)

In the intended top-level OpenCode chat inside Herdr, select the model and run
`/teem enable`. The command records that exact native chat, agent, project ID,
directory, Herdr terminal and the one configured managed-project alias whose
canonical path matches. It does not change the chat's agent or permissions.

## Operator prerequisites

- Configure the private Teem state directory, loopback native service registry,
  legacy fallback model and an approved managed-project policy as described in
  [the plugin guide](HERDR_PLUGIN.md). Duplicate aliases for the same directory
  are ambiguous and reject enablement.
- Load this package's TUI entry and native **server** entry using the host plugin
  loader. Both need absolute `executable` and `config` options. The TUI also needs
  the existing absolute `stateFile` option. Preserve other plugins/settings.
  The server entry is `ui/teem-progress/server.ts` (package export `./server`);
  the TUI entry remains `./tui`. Server registration requires the native
  `context.tool.transform` interface; TUI registration requires argument-bearing
  `context.keymap.layer` slash commands. Unsupported hosts must report a plugin
  error, not silently claim tool registration succeeded.
- Run the command from the Herdr pane owning the chat. The bridge inherits
  `HERDR_ENV=1`, `HERDR_PANE_ID` and optional `HERDR_BIN_PATH`; it inspects
  `herdr agent get PANE`. A display name is never sufficient identity.

These are setup instructions, not configuration changes performed by this patch.
No new location-global MCP binding should be generated: that would route other
chats through the wrong Foreman. Existing fixed MCP remains independently bound
legacy compatibility, not authenticated native chat context.

## Behavior and boundaries

Enablement performs an exact `GET /api/session/ID`, requiring a top-level session
(`parentID: null`), native agent, project ID and absolute directory. The TUI's
identity must agree with that read, and Herdr must identify the same session and
a nonempty terminal. Missing identity, model, runtime or project policy fails
visibly. Repeating enablement preserves the binding and updates only captured
models; changed identity or terminal rejects rather than silently rebinding.
Both success and error feedback belong to the originating session, including
navigation while awaiting session sync or the bridge response.

Native tools receive session/agent from the trusted ToolContext, never from model
arguments. Each call rereads native identity and the stored binding/policy. The
native catalog omits `foreman_submit`, and native calls reject it. New managed
workflows must use the enabled project alias. Reconciliation also checks the
**retained source workflow's** alias before admission, even when fixed MCP had
previously admitted that other project's workflow for the same Foreman. Rejected
calls create no workflows/tasks. Harmless retained-evidence inspection is not
restricted to the enabled project. Existing issue, stop, activity, candidate,
budget, check and review gates are unchanged.

New enabled workers/reviewers use the captured UI model/variant unless the native
session model has changed since capture; a changed native model takes precedence.
Authority is revalidated at dispatch. Legacy nonenabled dispatch remains unchanged.
The model is recorded per run, without copying parent permissions or rewriting
existing native sessions. Worker and read-only reviewer roles remain distinct.

**Enable starts no model, task, checks or service.** It does not enable supervision,
expand spending authority, answer approval cards, resume old sessions, replay
native input or modify machine-local configuration. Delegation still needs an
explicit authorised issue and the existing enabled supervisor/policy.

## Verification and qualification

`tests/issue1_enable.rs` uses temporary directories/repositories, fake native
transports, a local HTTP server and the native Rust Herdr subprocess fixture.
The approved Bun entry `ui/teem-progress/progress.test.mjs` includes enable-command,
model/navigation, origin-bound repeated success/error, trusted tool context,
disposal and bridge-error tests. Run the four root checks in the plugin guide
against the exact candidate, followed by both independent reviews.

These source fixtures do not prove live host compatibility. TUI/server loading,
actual slash argument delivery, real Herdr/native identity availability and model
selector behavior still require an authorised live qualification. No paid model
call, installation, service activation or host qualification is claimed here.
