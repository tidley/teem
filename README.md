# TEEM

A local Herdr plugin with one visible Foreman and up to two headless workers plus two reviewers.
The supervisor is code, not another agent. Python 3.11 standard library only.

## Status

Local MVP. The original prototype remains at `/home/tom/code/codr-foreman`.
`docs/SOURCE_SNAPSHOT.json` records the copied files and their original SHA-256 hashes.
This repository has no public remote. No paid worker run is part of repository setup.

## Components

- `herdr-plugin.toml`: TEEM actions, startup hook, and transient status popup.
- `foreman_plugin.py`: plugin commands, fixed-binding MCP delegation tools, and optional service unit output.
- `foreman_supervisor.py`: SQLite tasks, global 2+2 limits, exclusive checkout ownership, and completion events.
- `foreman_runtime.py`: headless OpenCode current-API transport and permission rules.
- `codr_foreman.py`: unchanged original Codr enqueue/receipt adapter.

## Verify

From this directory:

```sh
python3 -B -m unittest discover -s tests -v
```

Tests use temporary repositories and fake/local transports, not live models or credentials.

## Setup and limits

See [the plugin guide](docs/HERDR_PLUGIN.md) and [the local pilot](docs/PILOT.md).
The pilot uses separate authorised checkouts. Workers may be sparse. Reviewers must be complete and pinned to a commit.
The supervisor does not create checkouts, approve shell commands, merge, push, deploy, or replay uncertain tasks.
Code permissions are not an operating-system sandbox. Existing phone admission and desktop draft races remain unqualified.
The pilot Foreman uses a project-local runtime profile. This does not impose that profile on other conversations.

Plugin ID: `local.teem`. Optional user service: `teem.service`.
Internal `foreman_*` tool names and native task identifiers remain stable from the prototype.
