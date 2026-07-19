# Changelog

## 2026-07-19

- Added Graphite-style fixed lanes, open/filled branch circles, aligned labels, bottom-up stacks, ordered multi-trunk sections, and a final Untrunked section.
- Added live `t`, `h`, `H`, and `s` view controls; geometric Shift+Arrow and `J/K` stack navigation; and persistent per-stack `c` color cycling.
- Corrected diff styling so additions are green and deletions are red, and moved worktree occupancy to a fixed right-side `WT` field with exact path detail.
- Added guarded `x` then `y/n` local deletion with current/trunk/worktree/stale/degraded/non-leaf refusals, atomic expected-OID Git-only deletion, and characterized Graphite leaf deletion.
- Hardened Graphite deletion against option-shaped tracked names, stale/nonlocal raw children, slow contract checks, unreadable postconditions, and partial Git/metadata outcomes. No force, cascade, remote, or PR-close flags are available.
- Added causal refresh epochs so pre-mutation snapshots cannot release checkout/deletion reconciliation.
- Moved stack-color persistence off the input loop with bounded one-active/latest-pending coalescing and protection against stale results or replay over external writers.
- Indexed snapshot rows, projected rows, lane counts, and lane spans so visible rendering avoids full branch/stack scans.
- Expanded the suite from 40 to 78 tests, including fake Graphite provider behavior, mutation races, color contention/coalescing, trunk order/scope, 40-column rendering, and 5,000-stack visible rendering.
- Revalidated formatting, strict Clippy, doctests, offline release build, benchmarks, disposable Graphite 1.8.6 behavior, PTY key/terminal restoration, and installed-binary startup.
- Updated release baselines: 3.3 MB binary, about 11.8 MB resident on a large multi-trunk repository, about 0.15-0.19 ms per 500-branch projection/index iteration, and about 1.41-1.93 ms per 5,000-branch iteration.
- Reinstalled `stackmap 0.0.0` at `/Users/matt/.cargo/bin/stackmap`.

## 2026-07-18

- Completed all nine prototype milestones and installed `stackmap 0.0.0` at `/Users/matt/.cargo/bin/stackmap`.
- Pinned Rust 1.88.0 and verified formatting, strict clippy, 40 tests, doctests, and the release build on that exact toolchain.
- Established release baselines: 3.1 MB binary, about 8.6 MB idle RSS, and 242.47 ms for 1,000 projections of 500 branches.
- Added process-group termination so timed-out Git, GitHub, and platform commands cannot leave descendant processes behind.
- Distinguished structural and enriched refresh events so delayed diffs cannot mask structural failures.
- Added obsolete-generation diff cancellation and identical OID-pair task deduplication.
- Compacted deep stack connectors at 40 columns.
- Added bounded stdin support for clipboard subprocesses and timeouts for macOS open/copy helpers.
- Preserved typed GitHub spawn, timeout, exit, truncation, and malformed-response failures through the UI.
- Split structural refresh from bounded latest-state diff enrichment and rejected old-generation snapshot rollback.
- Added ID+OID PR preservation, single-flight TTL-limited GitHub fetching, and visible typed provider failures.
- Hardened Graphite chain validation and topology consistency reads.
- Added bounded cross-process config locking and last-valid config retention.
- Cached Git repository paths, rejected truncated structured/diff output, and preserved valid path whitespace.
- Added 40-column rendering, persistent stale health, search-mode Ctrl-C quit, async single-flight open/copy, and signal-driven normal shutdown.
- Added focused ordering, Graphite, config, linked-worktree, rendering, quit, and stale-recovery tests.
- Updated README resource, fallback, minimum-width, and macOS v0 support claims.
