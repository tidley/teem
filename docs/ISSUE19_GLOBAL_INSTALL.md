# Shared global installation (issue #19)

## Supported entry point

`herdr plugin install tidley/teem` uses the manifest's existing Cargo build command
to run `install-plugin`. It now installs the Herdr executable **and** shared
OpenCode registration. Do not run it merely to test a build against your real HOME.
For a local checkout, use the same command before `herdr plugin link /absolute/checkout`.

New profiles use **`agents.teem`**, whose short ID the host input title-cases as
**Teem**. Agent data and the full system prompt come from the single shipped
`examples/pilot-teem.json` template. The historical `teem_foreman` key and pilot
root defaults stay unchanged. The installer does **not** copy `default_agent`,
`warming` or `experimental` globally, select a default agent/model, or change Build
or global permissions. The shipped agent's permissions apply only to that agent.
Existing custom `agents.teem` objects are preserved in full, not reset on upgrade.

Select Teem for a new chat in any repo without copying a repo profile or fixed MCP
binding. Repo overrides remain repo-owned and use normal host inheritance. The
installer does not edit or bypass them. Native tools retain the newer all-top-level
chat availability contract: `/teem enable` is optional for native tools and captures
the exact chat/model/Herdr identity. Missing project registration does not block
enablement or session-local bookkeeping. Delegation still requires exactly one
matching explicit repo policy with approved checks, execution approval and a
user-authorised issue. Installation creates none of those authorities. See
[managed project setup](HERDR_PLUGIN.md#managed-workflows-and-routine-checks).

## Paths and ownership

- Global document: `$XDG_CONFIG_HOME/opencode/opencode.json`, falling back to
  `$HOME/.config/opencode/opencode.json`. If only `opencode.jsonc` exists, extend
  it instead. Both existing documents together refuse with a consolidation error;
  the installer does not guess host precedence.
- Stable assets: `$XDG_DATA_HOME/teem/opencode/<content-sha256>/`, falling back to
  `$HOME/.local/share/teem/opencode/<content-sha256>/`. Each bundle contains
  `bin/teem` and the shipped `ui/teem-progress` package with `./server` and `./tui`
  exports. The digest covers executable and asset bytes. Global registration
  references this package, never a temporary build cwd. `bin/teem` is also
  installed in the build cwd for Herdr actions.
- Plugin `options.config`: the absolute `--config` path, or
  `$HERDR_PLUGIN_CONFIG_DIR/config.toml`; absent those, the existing CLI fallback
  `$XDG_CONFIG_HOME/teem/config.toml` or `$HOME/.config/teem/config.toml`.
  Confirm the intended Herdr path with `herdr plugin config-dir local.teem`.
  For local builds outside Herdr, supply `--config` explicitly if necessary.
  No supervisor config is created or rewritten.
- Installer-owned `teem-install.lock` beside the global document serializes
  cooperating document writers. Registration is one atomic replacement after
  validation/staging and a before-write comparison of global/CLI documents.
  Existing document mode and bytes outside registration edits are retained,
  including JSONC comments/trailing commas. New documents/assets are private;
  existing directory modes are unchanged. Avoid concurrent noncooperating editors.

The installer adds a missing `agents.teem` and one missing plugin entry. It
preserves all existing `agents.teem_foreman` settings, including visibility, and
other agents, plugins, MCPs, defaults, global settings, CLI and repo documents.
It generates no fixed MCP/session binding and rewrites no stored identities.

A package basename/path component `teem-progress`, package name
`local-teem-progress`, or path under the owned bundle directory identifies Teem.
More than one matching entry refuses rather than duplicating it. Custom package
paths, plugin options and extra fields remain user-owned; only missing
`executable`/`config` options are supplied. Owned bundle package/executable refs
advance on upgrade; custom executable/config and `stateFile` overrides do not.
Custom paths require operator asset upgrades. Existing profile customizations
are not automatically replaced with a newer prompt.

Teem entries/disables in `cli.json`/`cli.jsonc`, separate server/TUI file entries,
and disabled `local.teem.native`/`local.teem.progress` entries require explicit
consolidation. Installation refuses instead of guessing cross-document precedence,
bypassing a disable or loading duplicate IDs. It never edits CLI configuration.

Repeat installation verifies/reuses identical bundles and leaves an unchanged
global document byte-identical. Old bundles are not deleted: running plugins or
other settings may reference them. Failure can leave an unused complete bundle or
updated build-cwd executable, but cannot publish a partial global document. There
is no automatic cleanup or rollback of user settings.

Malformed JSON/JSONC, duplicate keys, non-object agents, non-array plugins,
unsupported Teem option types, duplicate/disabled entries, non-regular documents,
symlinks in destination/config or existing plugin paths, modified owned assets,
and owned executables without owner execute permission refuse without clobbering
the affected data. Resolve the named conflict explicitly and retry. Do not remove
a legacy agent as a repair. HOME/XDG/destination/config paths must be absolute
without parent traversal. Existing package paths (except the supported package
name), executable/config/stateFile overrides meet the same rule before ownership
checks or staging. Symlinked config homes require an explicit real XDG path.
These are local installer safeguards, not a sandbox against hostile same-user
filesystem races. There is no permission manager or machine-wide cleanup system.

## Discovery is not activation

`native-tools` serves the static managed-native catalog **before** loading supervisor
configuration, including with no config/model/service. `native-setup` returns a
null projection path while unconfigured; once configured it reads `state_dir` to
locate `progress.json`, without opening a database. Malformed supervisor config
leaves the catalog available but setup/calls return errors rather than inventing
policy. TUI projection failures retain the prior observation (eventually stale).
`/teem enable` errors stay visible in the originating chat. Explicit `stateFile`
overrides win. Fresh installations leave ordinary Todo rendering to the host;
older explicit-stateFile setups retain their fallback because they disabled the
host Todo panel. Set `nativeTodos: false` when restoring that panel; `true`
explicitly requests the fallback. Installation disables no sidebar feature.

There is no manifest auto-start hook. Installation/loading starts no supervisor,
service, worker, model or checks, and touches no runtime state, credentials,
enablement receipts, sessions, workflows or event history. Existing separately
enabled systemd policy is untouched. Follow
[explicit activation setup](HERDR_PLUGIN.md#installation-and-explicit-activation)
and manually approve repo paths/checks before delegation. Selecting the agent
is not paid-work authority.

## Legacy chats and acceptance limits

A chat bound to `teem_foreman` stays there by default. Removing/hiding that entry could cause
the host to fall back to Build; keep it available while such chats exist. Bound
chat migration by the installer is **out of scope**. Stored enablement/chat/session/workflow IDs
and existing project defaults are never rewritten. There is no host display-name
override or agent-selection hook implemented or claimed. This is Teem, not Flow.

### Explicit operator migration and legacy progress

After explicit user approval and changing the exact host chat to `teem`, an
operator can repair its saved enablement receipt with
`teem --config /absolute/config.toml migrate-chat-agent`, supplying JSON on stdin:

```json
{"sessionID":"ses_exact_chat","fromAgent":"teem_foreman","toAgent":"teem"}
```

This command supports only this legacy-to-canonical transition. It verifies the
top-level native chat twice, the saved project/directory and existing terminal
binding, and changes only the receipt's agent field. It preserves model capture,
tasks, workflows, stops and bindings; it starts no work. It is idempotent and is
not a tool exposed to native agents. Ordinary enablement still rejects identity
changes rather than silently migrating them.

Canonical profiles use `teem_*` native tool names derived from the shared
template. Existing custom prompts remain operator-owned. Remove a legacy fixed
MCP `--foreman` override only with approval and after preserving it; it routes all
callers to that owner, not the currently visible chat. Future submissions should
use authenticated native tools. Do not change the owners of existing workflows.

For explicitly approved observation of preserved work from another chat, TUI
options may contain `progressSources: {"ses_visible":"ses_original_owner"}`.
This is a read-only display fallback when the visible chat has no own progress;
it is labelled linked work, never changes submission identity, and never copies
or moves workflow records. A chat's own nonempty progress takes precedence. An
empty observation-only entry does not hide linked work. Without an
explicit mapping, other chats' progress is not selected.

Receipt migration and display linkage do **not** release uncertain reservations
or resubmit work. A missing runtime session or dispatch model alone is not
historical non-launch proof. Use supported recovery with the required evidence;
if it is unavailable, keep the affected workflow visibly uncertain.

Native Rust/Bun fixtures cover installer registration/preservation/refusal,
repeat/upgrade, canonical ID and unconfigured bootstrap contracts. Installer
children clear inherited environment and isolate HOME/XDG/Herdr paths before any
global writes. They never target the operator's manual registration or supervisor
state. Repo-preservation fixtures prove repo bytes stay unchanged, not that every
host version merges them correctly. The four configured checks and two independent
exact-candidate reviews are supervisor-owned and must run afresh for this candidate.

Host acceptance remains separate and has **not** run for this candidate. In an
isolated HOME with supported Herdr/current OpenCode, confirm build config-path
forwarding, loading both server/TUI exports, Teem selection in new chats across
two repos, repo override/default precedence, missing-config catalog startup and
visible setup errors without service activation. Then qualify exact native chat
enablement/delegation against an explicitly configured repo and approved checks.
Source fixtures are not proof of host selection, current-chat adoption, live
reload, service activation or bound-chat migration. No real installation is
claimed or authorised by this worker change.
