---
name: install-stackmap
description: Build and install the latest local Stackmap release. Use automatically after finishing code changes in this repository when the user asks to build/install the new version, replace the installed binary, or kill running Stackmap processes before installation.
---

# Install Stackmap

1. From the repository root, run `cargo build --release --locked --offline`. Stop if it fails.
2. Find only exact-name `stackmap` processes with `pgrep -x stackmap`. Send them `TERM`, wait briefly, then send `KILL` only to exact-name survivors. Confirm `pgrep -x stackmap` finds none.
3. Install `target/release/stackmap` to `/Users/matt/.cargo/bin/stackmap` with mode `755`.
4. Ad-hoc sign the installed binary with `codesign --force --sign - /Users/matt/.cargo/bin/stackmap`.
5. Verify `codesign --verify /Users/matt/.cargo/bin/stackmap` and `/Users/matt/.cargo/bin/stackmap --version`.

Never use a broad command-line match when terminating processes. Report build, termination, install, signature, and version failures immediately.
