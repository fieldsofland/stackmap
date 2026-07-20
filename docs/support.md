# Support policy

## Alpha platform contract

Stackmap `0.1.0-alpha.*` supports macOS 15 or newer on Apple Silicon (`arm64`)
and Intel (`x86_64`). CI runs the complete Rust 1.88.0 suite natively on the
explicit `macos-15` and `macos-15-intel` GitHub runners. A smaller current-stable
lane catches forward incompatibility without changing the release toolchain.

Terminal.app on macOS 15 is the reference interactive environment. Deterministic
render and PTY tests cover key decoding, minimum-width behavior, cleanup, and
portable `J/K` and `g/G` fallbacks. Those tests do not certify every terminal
brand; other terminals are best effort and may intercept modifier keys.

Other Unix systems are unsupported during the alpha. Core rendering and Git
reads may work, but macOS open/copy integration and release artifacts are not
provided for them.

## Optional providers

Git is required. Graphite CLI metadata and authenticated GitHub CLI are optional.
Provider failures degrade explicitly while all Git-local branches remain
available. See [graphite-compatibility.md](graphite-compatibility.md) for the
characterized Graphite boundary.

Only the newest alpha receives fixes. Report ordinary defects through the
sanitized bug form and vulnerabilities through private reporting described in
[SECURITY.md](../SECURITY.md).
