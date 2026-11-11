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
(`parentID` omitted or null), native agent, project ID and absolute directory. A
non-null parent still rejects child/worker chats. The TUI's
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

The TUI adapter uses the host SDK's callable `router.current()` and
`model.current()` accessors. `keymap.layer` takes a pure factory and returns no
disposer; the host removes that layer when the plugin deactivates. Tests use
those exact API shapes, rather than property-shaped stand-ins. A setup failure
can leave `/teem enable` as plain chat text instead of running the TUI command;
check plugin errors before treating a model reply as an enablement receipt.

`tests/issue1_enable.rs` uses temporary directories/repositories, fake native
transports, a local HTTP server and the native Rust Herdr subprocess fixture.
The approved Bun entry `ui/teem-progress/progress.test.mjs` includes enable-command,
model/navigation, origin-bound repeated success/error, trusted tool context,
disposal and bridge-error tests. Run the four root checks in the plugin guide
against the exact candidate, followed by both independent reviews.

### Local acceptance evidence

On 2026-11-11 the operator verified active server and TUI plugin features, eight
native tools without legacy submission, and actual native enablement/status in
the intended Herdr chat. Enablement verified the native and Herdr identities,
returned `enabled=true` and `started=false`, and left task, workflow, event and
approval counts unchanged. The user subsequently confirmed the live TUI showed
`Teem enabled` and the current-work result. Private session and terminal IDs are
not needed in this public acceptance record.

Automated tests cover callable router/model accessors, the keymap factory and
host-owned cleanup, omitted/null top-level parent IDs, repeated enablement,
model inheritance, child rejection and project boundaries. The operator did not
inject terminal keystrokes to exercise the slash handler end to end; native
enablement/status were exercised directly. A visible enabled banner alone is
not proof that every slash-command path or a different host version works.
