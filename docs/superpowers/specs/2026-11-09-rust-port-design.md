# Teem Rust port

The requested outcome is a native Rust supervisor and Herdr/MCP command, not a
Rust launcher around Python. Keep the TypeScript OpenCode TUI adapter: the host
loads that interface as TypeScript. Keep Python as a reference/test oracle until
the Rust parity suite qualifies its replacement; never invoke Python at runtime.

Use one `teem` binary with modules for SQLite state, current OpenCode HTTP,
managed checkouts, bounded subprocess checks, workflow progression, approvals,
passive progress and the plugin/MCP CLI. Preserve `local.teem`, command names,
configuration, tool schemas, SQLite tables, immutable requests, event payloads,
UUID-derived native identities, JSON receipts and private progress format.
Normalise JSON structurally when reading existing rows; preserve Python's
canonical digest encoding for old check receipts. Existing native reservations
are reconciled only by reads. Never restart work because the implementation
language changed. Preserve legacy event rows without native replay.

Use standard Rust facilities and small established crates for JSON/TOML, HTTP,
SQLite, UUIDs, hashes and Unix signals. Do not change the OpenCode service, live
configuration or account/task state. No new publication or deployment is implied.

Verification combines Rust tests and Python-oracle differential tests, temporary
Git repositories, copied SQLite fixtures, local fake HTTP, crash-marker tests,
check output/time bounds and two independent read-only reviews. Existing Python
and TUI tests remain regression evidence, not proof of Rust parity alone.
