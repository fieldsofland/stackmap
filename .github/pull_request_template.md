## What changed

## Why

## User or developer impact

## Safety and public contracts

- [ ] Local Git mutation remains exact, local-only, guarded, and non-force.
- [ ] CLI, configuration, keybinding, support, or release-contract changes are documented.
- [ ] Graphite/GitHub failures still degrade without hiding Git-local branches.
- [ ] Queues, caches, subprocess output, and refresh generations remain bounded.
- [ ] Fixtures contain no credentials or private repository data.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] strict Clippy
- [ ] all-target/all-feature tests
- [ ] offline release build
- [ ] package contract
