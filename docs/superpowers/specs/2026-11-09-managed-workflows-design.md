# Managed implementation, checks, review and revision

One fixed-binding `foreman_workflow` request supplies a retained UUID, configured
project alias, relative edit scope, and acceptance criteria. The supervisor chooses
the source's clean HEAD and prepares private, separate local clones. No worktrees,
source commits, merges, resets, remote publication, or automatic cleanup are used.
Existing dirty worker checkouts and source repositories are preserved. Submodules
are blocked until complete-checkout handling is qualified.

Machine-local project policy approves a source path and named test/build commands
once. It also bounds revisions (default one) and selects two independent reviewers.
Commands are exact argv arrays, never model-supplied shell text. A helper runs them
asynchronously in a complete clone of an immutable candidate, with a fixed cwd,
timeout, bounded output, private receipts, and no runtime credentials in its explicit
environment. The central build adapters remain on PATH. Shell commands outside this
path still need human approval. Approved project code is trusted: this is not an OS
sandbox, and approving tests/build scripts authorises their execution as this user.

Workers are told to implement and report, not ask to run the configured checks.
On native worker success, Teem validates scope, snapshots tracked and nonignored new
files with an alternate Git index, writes a binary patch plus SHA-256/tree manifest,
and materialises a fresh check clone. Git tree objects are not source commits. Every
check receipt is bound to the workflow round, snapshot and approved command. Changes
to HEAD or candidate contents after checks block progression.

Two separate reviewer clones receive that exact patch on the recorded base. The
supervisor checks their complete candidate trees before and after execution. Reviewers
have no edit or shell permissions. Their final result must be strict JSON containing
the exact snapshot digest, verdict `pass` or `changes_requested`, and structured
findings. Invalid/missing review evidence stops the workflow. Worker prose is never
parsed into test approval. A passing workflow requires all configured checks and both
snapshot-bound reviews to pass.

Check failures or demonstrated review findings can start a new bounded revision round
in another clone of the last snapshot. New round/session/task UUIDs are deterministic
and distinct from prior attempts. Uncertain native execution never starts a revision
or repeats an assignment. All previous checkouts, patches, logs and receipts remain.
When the revision budget is exhausted, the workflow stops for manual attention.
`ready` means this exact candidate passed its configured gates, not permission to
commit, integrate, merge, publish or deploy.

Durable operation markers precede clone, check launch and review preparation. A crash
in an uncertain side-effect window reserves the workflow for inspection instead of
repeating operations. Completed check receipts can be reconciled after restart. The
normal supervisor's 2+2 caps still govern every model task. Foreman stays responsive
and native progress shows workflow stages as well as individual task execution.

Project policy is snapshotted at admission. Removing or changing it blocks further
automatic progression and queued managed launches. The exact native Foreman location
must match the configured source project. Model inheritance remains per dispatched
task. Legacy manually prepared tasks retain their original contracts.

Qualification uses temporary Git repositories, local Python checks and fake native
transports. No paid worker/reviewer workflow is started during delivery. Activation
only refreshes the already running Teem integration when no live tasks are present.
