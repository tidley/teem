# Project-local worker references (#12)

This is source configuration support, not installation, runtime activation, or
qualification of a live OpenCode host. No existing session is changed.

An operator may add an optional list to an approved machine-local project policy:

```toml
[projects.example]
path = "/absolute/project/root"
reference_paths = ["/absolute/reference/library", "/absolute/reference/guide.md"]
# Keep the project's separately approved checks here.
```

Only explicitly approved reference locations belong in this list. Do not put
credentials, private runtime state, whole home directories or unrelated projects
in it. Paths must be absolute, existing regular files or directories, not globs,
relative paths or `~` expressions. At most 32 entries are accepted. Filesystem
roots, top-level directories, the current home directory and project ancestors
(including the project root) reject. Control characters and pattern metacharacters
reject in both input and canonical paths. Symlink/`.`/`..` aliases resolve to their
existing canonical targets; duplicate entries collapse and ordering is canonical.
Choose a narrow subtree; validation cannot determine whether arbitrary contents
are appropriate for an assignment.

The policy stores each canonical path and its file/directory type. Each new
managed implementation task inherits only its own project's captured list, also
on corrective rounds. Native permissions use an exact file resource or a directory
resource ending in `/**`. A file never causes a parent-directory, sibling or
`file/**` grant. Historical task/session permissions do not consult the filesystem
again, so deleting a reference or changing its type does not silently change a
native identity. Reviewer and legacy manual-task permissions are unchanged.

The new rules grant only `external_directory` access alongside the existing
read/glob/grep rules. Built-in edits remain restricted to the task's relative
scope, with Git metadata denied. Shell remains `ask`, or `allow` under the separate
captured `trusted_worker_shell` option. **Trusted shell is not a filesystem
read-only sandbox**: it has host-user authority, including out-of-scope writes.
An approved human shell request can also exceed built-in tool restrictions.
References add no shell, publication, delegation, spending or task authority.
Native permission rules are not an operating-system sandbox.

## Native contract limitation

Repository fixtures establish serialized permission/session contracts, not native
tool resource derivation or pattern matching. The added tests check exact emitted
rules and retained state; they do not simulate a matcher and call it host proof.

Before relying on this feature on a host, qualify the exact `external_directory`
resources requested by **read, glob and grep**, their wildcard matching (including
directory-self requests and symlink escapes), and their separate edit checks.
In particular, determine whether reading a file requests that exact file or its
parent directory/pattern. If the host requests only a parent-directory resource,
an exact-file entry will remain denied: **file-level access is not established by
this change**. Never solve that denial by granting the file's parent. The required
contract is a native exact-file external resource, or a separately demonstrated
read/search restriction that excludes siblings for every enabled tool. The latter
is not implemented here. Directory rules follow the existing `/mnt/fastbuild/**`
payload convention; native subtree isolation still needs host qualification.

## Adoption and retained work

After source checks and both independent reviews, installation and machine-local
configuration changes require separate operator approval. Use the application's
supported configuration reload/startup path; this issue does not implement #11's
reload correction or #2's parent permission inheritance.

Omitted and empty lists serialize exactly as before. Adding, changing or removing
a nonempty list changes policy compatibility and blocks old workflow progression
and queued launches after the new configuration is loaded. It does not retrofit,
interrupt, recreate or replay existing native sessions, or reopen budgets. Inspect
retained/uncertain identities before any handoff. An explicitly authorized eligible
reconciliation captures the current project's policy for a new workflow and worker;
its parent's request, sessions, snapshot and check/review evidence stay unchanged.
New candidates still require fresh checks and two exact-candidate reviews.

`tests/issue12_references.rs` supplies deterministic native Rust payload, validation,
alias, project-isolation, retained-policy and reconciliation tests. Supervisor-run
tests and reviews are pending; no live model or host-tool qualification is claimed.
