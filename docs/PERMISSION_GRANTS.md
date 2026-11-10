# Native CLI grants

Teem does not own a permission-grant store. OpenCode evaluates saved grants in
the native project of the executing session. A parent's applicable **Always
allow** grant can therefore cover a worker only when native project identity,
action and resource match, and configured policy does not deny the operation.
Being a child task or using the same model is not permission inheritance.

## Managed checkout identity

Managed checkouts are independent clones, not linked worktrees.
`checkouts::prepare` transfers objects from the local source, without fetching
from a network. An ordinary local clone gives `origin` the source's filesystem
path. That can change OpenCode discovery: the source can identify by a network
remote while the clone falls back to a cached identity or first root commit.
It is **not** correct to say every clone necessarily has a new project ID.

For a source with exactly one remote named `origin`, containing exactly one
credential-free HTTPS, SSH (`git` or empty username), or `git@host:path` URL,
new managed clones retain that exact URL as `origin`. Objects still come only
from the local source. No other source configuration, identity file, permission
row or revocation snapshot is copied. OpenCode continues to own normalization
and project-ID generation. Repeated worker/reviewer clones retain the remote;
unrelated remotes are not replaced with the parent's project identity.

Ambiguous/multiple remotes or URLs, repository-local URL rewrites/includes or
worktree-specific configuration, filesystem remotes, unsupported transports,
credential-bearing URLs, queries and fragments retain the existing local-clone
behavior. Cached-only identities and local-only projects are not transferred.
Existing checkouts and sessions are not rewritten. A missing applicable grant
still requires a human decision; Teem never changes project identity merely to
make a grant apply.

## Scope, revocation and cards

- Saved rules are read fresh by OpenCode for the executing native project.
  Teem never converts saved rules into session `allow` rules. Removing a saved
  grant affects subsequent native evaluations; this does not retroactively
  cancel an already admitted command or answer an existing pending request.
- Configured denial wins before saved-grant evaluation. Worker shell remains
  `ask`, reviewer shell remains denied, and edit scopes/default denies are
  unchanged. Agent/project policy can still deny a command.
- Resource patterns remain native and exact; Teem does not trim, widen or
  translate them. A grant for a command/path in one checkout may not match a
  different command/path in another checkout even with equal project IDs.
- **Allow once**, pending, rejected and expired requests are not persisted or
  promoted by Teem. Relayed worker cards retain only **Approve once** and
  **Reject worker batch**. No automatic reply or permanent card option is added.

## Evidence and limits

The operator supplied the inspected host contract: OpenCode
`packages/core/src/permission.ts:135-165,290-295` reads saved rules by project,
checks configured denial first and persists `request.save` only for `always`.
`packages/core/src/project.ts:279-350` prefers normalized remote identity, then
the common-directory `opencode` cache, then first root commit; linked worktrees
use their canonical main worktree/common directory. These are supplied source
observations, not a live permission test performed by this worker.

`tests/issue2_cli_grants.rs` contains a local-origin mismatch regression using
temporary Git repositories, retained remote metadata and conservative fallback.
It also covers a linked source's common-directory remote/cache and distinct
remotes with identical root commits; no source identity cache is transferred.
Its narrow fake transport documents fresh project-scoped exact-resource grant
evaluation, denial and revocation across sessions. It does not execute OpenCode's
hash, wildcard matcher or agent/project policy resolver. Existing approval
regressions cover once-only forwarding. Actual host discovery/evaluation still
needs supervisor qualification; no paid model, real grants or cards are needed
for the local fixtures. This is a limited metadata fix, not general cross-project
grant inheritance. Read-only `permission.saved.list` exists, but listing grants
cannot safely transfer their authority to a different native project.
