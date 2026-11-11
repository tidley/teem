# Foreman access and reinstall policy

## Trusted development

The checked-in `examples/pilot-foreman.json` is the reinstallable Foreman profile.
It uses trusted Build-equivalent tool access rather than a command allowlist:

```json
[
  { "action": "*", "resource": "*", "effect": "allow" },
  { "action": "browser", "resource": "*", "effect": "deny" }
]
```

Routine shell commands, local edits, external-directory reads, research tools and
read-only review agents do not require per-command permission prompts. Managed
implementation work still uses Teem workflows and their recorded check/review gates.
This profile changes only the Foreman. It does not replace worker or reviewer policies.

Routine inspection includes:

- Git status and remote refs (`git status`, `git ls-remote`).
- Repository metadata and open PRs (`gh repo view`, `gh pr list`).
- PR details, reviews and checks (`gh pr view`, `gh pr checks`).
- Commit history and whitespace checks (`git log`, `git diff --check`).
- Read-only CI/release inspection (`gh run list/view`, `gh release list/view`).
- Help/version inspection, including `ngit --help`.

Prefer ordinary read-only GitHub CLI commands over raw API calls. Do not mistake
these examples for safe wildcard grants to a restricted agent: Git inspection flags
can write files or invoke helpers. The trusted profile deliberately grants broad
host-user access instead of claiming to classify every possible command as safe.

## Approval still means approval

Pushes, PR/issue creation, merges, tags/releases, installation, deployment and
destructive cleanup require explicit user authorisation for their intended scope.
Approval for one push or PR is not standing publication authority. Do not recommend
permanent `git push *` or `gh pr create *` approval to replace that decision.

These are behavioural approval rules, not shell-enforced barriers. Commands can
act with the host user's filesystem, process and network authority. Protect private
data and credentials; preserve unrelated work. Do not claim an OS sandbox. Change
permissions or MCP configuration only when the user explicitly requests it. Native
permission cards remain human-only; tool access does not authorise answering them.

## Reinstalling

Keep both the permissions and the `system` instructions from the tracked profile.
Do not rely on this conversation, saved project grants or a machine-local permission
exception. On a fresh setup, use `examples/pilot-foreman.json` as the project profile.
When updating an existing profile, merge only `agents.teem_foreman.permissions` and
`agents.teem_foreman.system`; preserve its color, model, MCP command, exact Foreman
binding and other local settings. Do not copy one machine's session IDs or credentials.

OpenCode reloads configuration after a save. Check the effective `teem_foreman` agent
in the intended project location and inspect any session-specific permissions, which
can override the agent rules. No service restart or paid task is needed to load or
inspect this policy. Installing Teem does not silently overwrite an existing profile.

A reinstall from a remote source needs the commit containing this profile. Unpublished
local edits are not included in a GitHub installation. This policy is not permission
to publish those edits, merge them, or change worker/reviewer sessions.
