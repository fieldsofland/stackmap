# Changelog

## 2026-07-19

- Added persistent `n` stack naming keyed by stable stack identity. Names render as white, nonselectable rows directly above stack heads; the editor prefills existing text, Enter saves, empty Enter clears, Esc cancels, and trunks remain unnamed.
- Changed selection to use the selected stack/trunk identity color across the full row, while unselected checked-out branches keep the subtler current-branch tint.
- Changed lowercase `x` archive/restore to focus the nearest visible branch above after the selected row disappears. Archive view now renders required unarchived ancestry as dim, nonselectable stack context.
- Clarified the footer action as `a View Archive` / `a View Active`, while preserving high-priority stale health and mutation status visibility.
- Corrected connector color handoff: the parent-side junction keeps the left rail's color, then the horizontal segment and child corner switch to the child stack color.
- Enabled complete enhanced-terminal modifier reporting, added Command+Up/Down as another stack-jump binding, and documented the standard `ESC [1;2A/B` Shift mapping for terminals that otherwise emit unmodified arrows; `J/K` remains the portable fallback.
- Completed the stable bottom-up stack workflow and reinstalled `stackmap 0.0.0` at `/Users/matt/.cargo/bin/stackmap`; normal and `--current` PTY startup/quit smoke tests pass outside the source repository.
- Added reversible persistent branch cleanup: lowercase `x` archives/restores, `a` opens Archive view, and `v` applies a contiguous range. Uppercase `X` remains the only path to guarded local deletion.
- Added Archive-only no-fetch evidence for configured upstream equal/ahead/behind/diverged/gone state and containment in local remote-tracking refs, with bounded targets, cache, output, queue, timeout, cancellation, and stale-token rejection.
- Added Recent default ordering, the `T` order picker with Alphabetical/Graphite modes, stack/trunk focus, `--current`, global `+/-/0` lane pitch, separator toggling, `c/C` stack colors, fixed `⎇` worktree evidence, and complete in-app marker help.
- Hardened topology projection with iterative deep-comb emission and indexed attach-parent children. A 5,000-level/10,000-branch comb and a broad-comb regression now cover stack safety and linear lookup.
- Hardened quit-time configuration durability: terminal state is restored first, active/latest writes drain within five seconds, the final failed write is retried once, and persistent failure exits visibly instead of losing cleanup state.
- Added real Git+Graphite coverage for the `3 -> {4, 3b}` fork and protected archive cleanup when a hidden branch becomes current or a configured trunk.
- Completed Tier-2 correctness/testing/maintainability/safety/CLI/performance/reliability/requirements review. All actionable findings were fixed and the focused re-review is clean.
- Final verification passes formatting, diff hygiene, strict offline all-target/all-feature Clippy, 174 tests, release build, benchmarks, local PTY modifier/fallback exercise, terminal restoration, and RSS plateau sampling.
- Final release artifact is 3,713,072 bytes. The preceding allocator/RSS run measured roughly 15.4 MB startup RSS and a 19.856-19.888 MB post-warm-up plateau on 2,001 branches; release projection benchmarks were about 0.108 ms per 500 branches, 1.10 ms per 5,000 branches, and 0.94 ms for one 5,000-branch deep stack.
- [LEARN] Keep checkout status separate from topology (`●` in the fixed left column plus the ordinary `○` node), keep each stack vertically aligned, connect roots directly to the bold reserved-color trunk, and fill selected/current backgrounds edge to edge without changing green additions or red deletions.
- [LEARN] Cleanup must default to reversible archive (`x`), with destructive deletion isolated on uppercase `X`; Shift/Option modifier behavior always needs `J/K` and `g/G` terminal-portable fallbacks.
- Added Graphite-style fixed lanes, open/filled branch circles, aligned labels, bottom-up stacks, ordered multi-trunk sections, and a final Untrunked section.
- Added live `t`, `h`, `H`, and `s` view controls; geometric Shift+Arrow and `J/K` stack navigation; and persistent per-stack `c` color cycling.
- Corrected diff styling so additions are green and deletions are red, and moved worktree occupancy to a fixed right-side `WT` field with exact path detail.
- Added guarded uppercase `X` then `y/n` local deletion with current/trunk/worktree/stale/degraded/non-leaf refusals, atomic expected-OID Git-only deletion, and characterized Graphite leaf deletion.
- Hardened Graphite deletion against option-shaped tracked names, stale/nonlocal raw children, slow contract checks, unreadable postconditions, and partial Git/metadata outcomes. No force, cascade, remote, or PR-close flags are available.
- Added causal refresh epochs so pre-mutation snapshots cannot release checkout/deletion reconciliation.
- Moved stack-color persistence off the input loop with bounded one-active/latest-pending coalescing and protection against stale results or replay over external writers.
- Indexed snapshot rows, projected rows, lane counts, and lane spans so visible rendering avoids full branch/stack scans.
- Expanded the suite from 40 to 174 tests, including real and fake Graphite provider behavior, mutation races, archive/color/name contention and coalescing, trunk order/scope, 40-column rendering, remote-ref races, and 5,000-stack visible rendering.
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
