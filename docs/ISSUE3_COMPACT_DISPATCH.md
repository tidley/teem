# Compact dispatch inputs (#3)

## Source contract

New `foreman_workflow` and `foreman_submit` calls should put their long inputs
inside an `assignment` object. The tool schemas advertise both compact and legacy
forms. They are alternatives, not mergeable fragments: supplying **any** assignment
field at top level alongside `assignment` is an error, even if values agree.

```json
{
  "workflow_id": "RETAINED_WORKFLOW_UUID",
  "project": "pilot",
  "issue": "tidley/teem#3",
  "title": "Fix compact dispatch",
  "assignment": {
    "scope": ["src/plugin.rs", "src/tools.json"],
    "prompt": "Full acceptance criteria and required checks, without abbreviation.",
    "workers": [
      {"scope": ["src/plugin.rs"], "prompt": "Full backend assignment."},
      {"scope": ["src/tools.json"], "prompt": "Full schema assignment."}
    ]
  }
}
```

`workers` is optional; omit it for the ordinary single-worker workflow. Existing
one/two-worker bounds, disjoint scopes, prompt byte limits and project policy still
apply. `title` remains optional and short; it is not a substitute for the prompt.
New managed admissions also require the explicit authorised `issue` at top level
in either form; see [issue tracking and historical compatibility](HERDR_PLUGIN.md#explicit-issue-tracking).

```json
{
  "task_id": "RETAINED_TASK_UUID",
  "role": "worker",
  "assignment": {
    "checkout": "/explicitly/authorised/checkout",
    "revision": "FULL_EXISTING_COMMIT_ID",
    "scope": ["src/plugin.rs"],
    "prompt": "Full bounded assignment, acceptance criteria and checks."
  }
}
```

The UUID placeholders above must be replaced with canonical retained UUIDs. The
manual submit checkout and revision still need explicit authorisation/validation.
No new top-level `title` is added to the legacy submit API.

## Why it stays compact

The supplied OpenCode host contract says its generic tool summary joins only
top-level primitive strings, numbers and booleans as `key=value`, without a length
bound. Objects and arrays are excluded. Its generic tool starts collapsed, and
clicking expands the full input/output. Nesting the assignment therefore keeps
long prompts, paths and revisions out of the default summary without truncating
the input. No host source, private rendering internals or UI plugin is changed.

The backend expands the compact form before existing admission and UUID identity
checks. The canonical stored request is the same as an equivalent legacy call:
full prompt bytes, revision, scope and worker assignments are preserved. Equal
retries may switch forms using the **same** UUID; changing immutable request
values remains an error. Never replace a UUID after an uncertain response.
Full canonical inputs remain retrievable through `foreman_status` detail mode
with the exact task/workflow UUID and `pointer: "/request"`; follow `next_offset`
and retain the same SHA-256 across chunks. Existing records are not rewritten.

## Compatibility and limits

- Legacy top-level string prompts remain accepted and **can still produce long
  default tool summaries**. Existing tool history is not retroactively compacted.
- Compact workflow summaries still contain primitive UUID/project/issue/title fields;
  this change does not impose a new global UI length bound. Use a short title.
- Native errors, security/permission prompts and human-only approval cards are
  not hidden or replaced. Admission is not execution, review approval or integration.
- Stop/inspection, once-only approval, runtime step policy and bounded status
  behavior are unchanged. No runtime permissions, configuration or state migration
  is part of this change.
- This is a source change, not an installed/running upgrade. Live UI behavior and
  client schema handling still require separately authorised qualification.

## Verification handoff

`tests/issue3_dispatch.rs` contains local native fixtures for long exact inputs,
single/explicit-one/two-worker admissions, legacy calls, cross-form UUID retries,
true mismatches, malformed/unknown/mixed fields, nested value validation, full
detail retrieval, rejection of nested controller-only fields, and visible MCP
validation/checkout-authorisation errors. Both dispatch tools cover retries in
both compact-to-legacy and legacy-to-compact directions. The primitive-summary fixture models
the supplied host filtering algorithm; it is **not live UI test evidence**.
Tests do not dispatch models or launch runtime work.

The supervisor owns approved Rust/Bun checks and two independent read-only
exact-candidate reviews. Required root commands are documented in
[`HERDR_PLUGIN.md`](HERDR_PLUGIN.md#verification). Worker completion alone is
neither passing check evidence nor review approval.
