# Contributing to stackmap

Stackmap welcomes focused bug fixes, tests, documentation, and well-scoped
features. Open an issue before a large behavioral or architectural change so
the safety and terminal contracts can be discussed first.

## Development setup

Install Git and the Rust toolchain pinned by `rust-toolchain.toml`. Graphite and
GitHub CLI are optional unless the change exercises those providers.

Run the complete local gate before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
cargo test --locked --offline --all-targets --all-features
cargo build --release --locked --offline
scripts/check-package-contents.sh
```

Run `cargo deny check` when dependencies or the lockfile change. Network-backed
advisory checks intentionally remain separate from deterministic application
tests.

## Safety and test expectations

Preserve Stackmap's local-only, non-force mutation rules. Add a failure-path
test for changes to checkout, deletion, refresh generations, persistence,
subprocesses, Graphite compatibility, or bounded queues and caches. Use
sanitized disposable repositories and fixtures; never commit credentials,
private branch or pull-request names, Git configuration, or Graphite databases.

## Contribution license

No CLA or DCO sign-off is currently required. By submitting a contribution,
you agree to license it under the repository's MIT License and confirm that you
have the right to do so.

All participation is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
