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
Command-C, Command-Shift-C, and Command-Option-Shift-C require the terminal to
deliver Super-modified key events rather than consuming them as terminal copy
commands. Shift-Backspace clears a name draft only when reported distinctly;
Ctrl-U is the supported portable full-draft clear sequence.

Other Unix systems are unsupported during the alpha. Core rendering and Git
reads may work, but macOS open/copy integration and release artifacts are not
provided for them.

## Optional providers

Git is required. Graphite CLI metadata and authenticated GitHub CLI are optional.
Provider failures degrade explicitly while all Git-local branches remain
available. See [graphite-compatibility.md](graphite-compatibility.md) for the
characterized Graphite boundary.

Remote-tip evidence is derived from one bounded map of locally known remote
branch tips and does not fetch. Configured-upstream equality and exact PR head
OID can also prove pushed; containment, raw ahead/behind state, and stale PRs
cannot. Hiding status cancels remote and Graphite presentation work but retains
cached evidence. GitHub enrichment continues because PR open/copy consumes it.

GitHub first performs a cached open-PR sweep, then serialized bounded exact-head
and exact-commit lookups for unresolved local branches. PR matches prefer an
exact branch-tip object ID; a same-name fallback is shown with `~` so stale or
missing head-OID data is never presented as exact. Checking, confirmed no-match,
and unavailable are separate states, and partial provider failure preserves
applicable last-known PR evidence.

Graphite row health is advisory read-only Git evidence. Only `needs restack` is
shown in the row; all states remain in details. Checks are cached by immutable
parent/tip pair and limited to bounded initial discovery, current/changed stacks,
and explicit `R`/`r`/`m` demand. Navigation and rendering never start them.
Graphite restack and move are enabled only when the installed CLI help matches
the characterized noninteractive command surface. Each mutation performs
live object-ID and topology preflight, delegates to Graphite, then reconciles
from Git and blocks further Graphite writes if the outcome is inconsistent.

Only the newest alpha receives fixes. Report ordinary defects through the
sanitized bug form and vulnerabilities through private reporting described in
[SECURITY.md](../SECURITY.md).
