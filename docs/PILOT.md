# Local Teem pilot

## Directories

- Plugin source: `/home/tom/code/teem`.
- Pilot source and visible Foreman: `/home/tom/code/teem-pilot`.
- Sparse worker checkouts: `/home/tom/code/teem-pilot-worker-a` and `teem-pilot-worker-b`.
- Complete reviewer checkouts: `/home/tom/code/teem-pilot-review-a` and `teem-pilot-review-b`.

These paths describe the original local pilot. Teem's plugin source is published
at https://github.com/tidley/teem; the pilot project and its checkouts remain local.
The worker/reviewer clones use only the local pilot source as their origin.

## Safety defaults

The supervisor configuration is disabled. No task prompt is submitted during setup.
The configured model is only a candidate until Tom approves its use.
No global OpenCode configuration is changed.
`examples/pilot-foreman.json` provides a project-local coordinator profile with ordered runtime permission rules.
It denies direct edits, shell commands, ordinary subagents, and other mutation tools.
It permits source inspection and the three Teem delegation tools.
These runtime rules are not an operating-system sandbox and have not been tested with a live model.

The pilot's machine-local `opencode.json` starts with only the coordinator profile.
Setup adds an exact native Foreman ID to its disabled MCP connection after creating the empty session.
The connection stays disabled until the visible Herdr Foreman is explicitly bound.
The file remains locally excluded from Git, so worker clones do not inherit the Foreman binding.
Machine-local `AGENTS.md` files also remain locally excluded.

## Activation gate

Before the first model-backed run:

1. Confirm the exact Foreman session and terminal binding in local setup status.
2. Approve the worker model and one bounded assignment from the pilot README.
3. Enable the reviewed supervisor configuration and start its service deliberately.
4. Submit one worker task with a retained UUID, exact commit, and `src`/`tests` scope.
5. Check native acceptance, continued Foreman responsiveness, and the actual worker result.
6. Approve any required shell command yourself. Do not enable blanket approvals.
7. Inspect the changes before authorising a local commit and advancing a reviewer checkout.
8. Assign one reviewer against that fixed commit and verify result delivery.

Completion events return through the native OpenCode session API. No Codr control interface is required.
Codr/White Noise phone access is optional and independent. Its settings and services remain unchanged.
No native completion event is submitted during this code update because that could start a model request.

Automatic review scheduling, commits, merges, and publication remain disabled or unimplemented.
Existing Codr phone admission and desktop draft safety still need an explicit live test.

## Setup status

The machine-local setup record lives under `~/.local/state/teem/pilot-setup.json`.
It records the actual baseline commit, Foreman session, Herdr pane, configuration, and checkout paths.
It is not a credential file and must not contain service passwords or signer information.
Read that record rather than inventing terminal IDs or rerunning uncertain creation commands.

The status popup is available through the Teem plugin action after linking.
Use `herdr plugin pane open --plugin local.teem --entrypoint status` from inside Herdr.
No `/teem` slash command is installed by this setup.
