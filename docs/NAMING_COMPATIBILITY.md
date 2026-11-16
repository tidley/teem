# Retained naming contracts

Current branding, tool catalogs, documentation and profiles use **Teem** and
`teem_*`. New profiles use `examples/pilot-teem.json`, `agents.teem` and
`examples/teem.toml`.

The following old names are intentional compatibility/history, not current branding:

- `foreman_*` tool aliases remain accepted but are not advertised.
- `--foreman`, `source_foreman`, JSON fields such as `foreman` and
  `foreman_counts`, SQLite table/column names and `foreman-plan` progress source
  tags remain unchanged. Existing clients and retained evidence depend on them.
- `ses_foreman_*`, `msg_foreman_*`, `herdr_foreman` metadata, old event payloads
  and task ownership stay exact. A text rename must never replay an input or
  invalidate an accepted receipt.
- Existing `agents.teem_foreman` bindings remain valid. Do not remove, hide or
  rebind them automatically. The explicit migration remains separate.
- `examples/pilot-foreman.json`, immutable JSON/SQL fixtures, dated implementation
  plans and original prototype paths remain historical compatibility references.
  They are not used to generate the current shipped profile.
- `docs/FOREMAN_ACCESS.md` is a redirect for existing links.

Internal Rust identifiers can keep the old name where they implement these
contracts. No database, credential, permission, model, publication, deployment or
device authority changes as part of this wording update.
