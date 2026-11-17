# Native chat access and optional enablement

When the native server plugin is loaded, every top-level chat can use Teem tools
without first running `/teem enable` or registering a managed project. Status,
retained evidence and checklists are scoped to the caller's native session. Native
tool context supplies session and agent identity; children and mismatched agents
are rejected. Load the server plugin globally to cover all project locations.

First use records native-chat provenance and creates a session bookkeeping binding
(`native-session:SESSION_ID`) if none exists. This marker is not a fabricated Herdr terminal. Real terminal
bindings and explicit model receipts are preserved. Initial enablement can replace
that marker with the exact verified Herdr terminal. A stale real terminal needs
the guarded explicit refresh described below; ordinary tool calls never repair it.

In the intended top-level OpenCode chat inside Herdr, select the model and run
`/teem enable`. The command records that exact native chat, agent, project ID,
directory, Herdr terminal and, when configured, the managed-project alias whose
canonical path matches. It does not change the chat's agent or permissions.

## Restored panes and explicit refresh

Herdr restoration can give the same native chat a new terminal ID. Selecting Teem
can also leave a receipt captured earlier under `build` or the legacy agent name.
In that exact chat, `/teem enable` can refresh the receipt when:

- Native session ID, top-level status, project ID, directory and project policy
  still match. The previous receipt and stored terminal binding agree.
- A fresh trusted Herdr roster shows exactly one pane for this native chat,
  matching the current pane and terminal. A replaced terminal is absent from the
  roster; a still-live old pane requires an explicit handoff instead.
- The agent is unchanged, or the user has selected `teem` from `build` or the
  retained legacy profile. Other agent changes remain rejected.
- Herdr identity stays stable across roster inspection and the native identity
  and model are rechecked before writing.

The old and new receipts are retained in `chat_enablement_refreshes`. Updates are
atomic and repeating a successful enable creates no additional refresh record.
This changes no permissions, task/session/prompt IDs, workflow ownership, policy,
review history or execution state; it starts no work and replays no input.

The agent can instead call native `teem_refresh_enablement` with no arguments.
This tool uses host-authenticated session and caller identity, not a supplied
target or the server's focused-pane environment. It requires an existing receipt,
retains its effective model selection, resolves the unique exact chat from `herdr pane list`,
rechecks that pane with `herdr agent get`, and uses the same guards above. This
same-chat repair needs no repeated user approval. It cannot enable another chat,
change permissions, take over workflows or operate through fixed-owner MCP.

For an explicitly requested repair from another operator chat, the CLI provides
`teem --config CONFIG refresh-chat EXACT_SESSION_ID`. It reads identity from the
native runtime, retains the effective model selection, and applies the same roster
and receipt guards. It does not impersonate a native tool caller or inject a
prompt into the affected chat. This administrative command is not a targetable
model tool; repair authority must come from the operator's request.

For native/operator repair only, if no execution policy now matches the unchanged
home directory, retain the old project association rather than inventing a new
one. Status and other session-local tools can recover; execution admission still
requires a matching current home policy. A different matching project alias is
not adopted by refresh. The TUI's explicit model-capture command keeps its existing
policy-change guard.

Changing project/directory, inconsistent bindings, unavailable rosters and multiple
live views fail closed. An agent or stale-terminal mismatch is not a code-check failure.

## Operator prerequisites

- Configure the private Teem state directory, loopback native service registry,
  legacy fallback model as described in [the plugin guide](HERDR_PLUGIN.md).
  An approved managed-project policy is needed for delegation, not tool access or
  optional model capture. Duplicate aliases for the same directory remain ambiguous.
- Standard Herdr installation registers this package's TUI and native **server**
  entries globally with stable absolute `executable` and `config` options. Manual
  registration must supply those options. The TUI discovers its projection path
  from supervisor configuration unless an explicit absolute `stateFile` wins.
  Missing supervisor configuration leaves discovery safely unconfigured and does
  not prevent static tool catalog registration. Actual calls still require the
  prerequisites above. Preserve other plugins/settings.
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
chats through the wrong Teem coordinator. Existing fixed MCP remains independently bound
legacy compatibility, not authenticated native chat context.

## Behavior and boundaries

Enablement performs an exact `GET /api/session/ID`, requiring a top-level session
(`parentID` omitted or null), native agent, project ID and absolute directory. A
non-null parent still rejects child/worker chats. The TUI's
identity must agree with that read, and Herdr must identify the same session and
a nonempty terminal. Missing identity, model or runtime fails visibly. Repeating
enablement preserves the binding and updates captured models. A chat enabled
without a project policy can later capture its approved alias. Changed identity,
terminal or an already captured alias rejects rather than silently rebinding.
Both success and error feedback belong to the originating session, including
navigation while awaiting session sync or the bridge response.

Native tools receive session/agent from the trusted ToolContext, never from model
arguments. Each call rereads native identity and preserves any explicit receipt. The
native catalog omits `teem_submit`, and native calls reject it. New managed
workflows require exactly one home policy matching the current chat directory.
The explicitly authorised target may be another configured project alias; its
policy controls source, checks and access. Missing or ambiguous home policy blocks
delegation with a setup error, not a request to repeat `/teem enable` or open another
chat. Reconciliation retains the source workflow's project and verifies its owner;
it cannot switch issues or borrow another Teem coordinator's candidate. Rejected calls
create no workflows/tasks. Harmless retained-evidence inspection is not restricted
to the home project. Existing issue, stop, activity, candidate, budget, check and
review gates are unchanged.

New enabled workers/reviewers use the captured UI model/variant unless the native
session model has changed since capture; a changed native model takes precedence.
Authority is revalidated at dispatch. Automatically bound native chats use their
current native model; optional explicit receipts retain UI model capture. Legacy
fixed-MCP dispatch remains unchanged.
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
