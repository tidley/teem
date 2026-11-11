# Issue #2 candidate

Status: partial native-identity fix; not installed or host-qualified.

Reconciled onto source `2729a7a443d0624d41a01e9327d6b2e5b65a3b73`.
The retained patch was seeded successfully; its prior check receipts and two
review verdicts do not qualify this candidate. The current reconciliation adds
linked-source/common-directory coverage and fixes the unrelated-project fixture
to share the source's exact history while retaining a different remote.

The focused Git regression is designed to reproduce loss of the source's
network `origin` during a local clone. New supported single-origin managed
clones now retain the source's exact remote URL, allowing OpenCode's existing
remote-based project discovery and fresh persisted-grant evaluation to work
without a Teem permission cache or permission-template changes.

See [native CLI grants](PERMISSION_GRANTS.md) for supported URLs, conservative
fallbacks, denial precedence and revocation semantics. The fixture's exact
resource matching is not evidence for native wildcard behavior. Unsupported
remote layouts, cached-only/local-only identity, existing clones, global config
URL rewrites and other host discovery edge cases remain unqualified. Local URL
rewrites/includes and worktree-specific config use the conservative fallback.
Teem does not force identity files or copy saved grants to address them.

Supervisor checks required against the frozen candidate:

```sh
cargo test --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
bun test ui/teem-progress/progress.test.mjs
```

Two independent exact-candidate reviews (correctness/contracts and
tests/regressions/scope) are also required. The implementation worker did not
run those checks or delegate reviews; the managed supervisor owns them. No
execution result is review approval, and no integration/publication is requested.
