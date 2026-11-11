# Trusted worker shell

On an isolated development machine, the operator can set this top-level option in
the private Teem `config.toml`:

```toml
trusted_worker_shell = true
```

The default is `false`. Only worker shell permission changes from `ask` to `allow`.
The setting is recorded with the inherited model before the first native dispatch.
Reviewer and Foreman permissions do not change through this setting. Other tool
permissions and external-directory rules do not change.

Shell commands have the host user's filesystem, process and network authority.
They can modify files outside the declared edit scope, delete files, or publish.
This mode does not enforce per-command approval for those operations. Use it only
with trusted tasks and repositories. Task instructions and check/review/integration
gates still apply; unattended shell access is not approval for unrequested work.

Changing the configuration affects future dispatches, not recorded sessions.
Existing tasks retain their pinned flag across restart and configuration changes.
There is no automatic permission reply, permanent card option, or copied saved
grant. Do not rewrite existing task flags, replay inputs, or replace uncertain tasks
to adopt this mode. Pending requests remain human-controlled.

To turn this mode off for new workers, set `trusted_worker_shell = false`. This does
not revoke permissions from recorded sessions or cancel already admitted commands.
The separate [trusted Foreman profile](FOREMAN_ACCESS.md) is not controlled by this
worker setting.
