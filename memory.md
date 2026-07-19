# Project memory

- `stackmap` is a macOS-targeted Rust TUI prototype for local Git branches with optional Graphite and GitHub enrichment.
- Structural refresh, diff enrichment, GitHub lookup, checkout, and platform actions use bounded independent coordinators so the UI and branch inventory remain responsive.
- Structural and enriched refresh events are distinct; only a successful structural inventory clears stale refresh health.
- App snapshot ordering is generation-monotonic; PR enrichment is preserved and matched by branch ID plus tip OID.
- Diff work is capped at four subprocesses with a 2,048-entry cache. Refresh/event/request queues are bounded.
- Obsolete diff generations stop scheduling between tasks, and identical OID pairs share one diff subprocess result.
- Graphite edges are trusted only when their complete local chain reaches one of the ordered configured local trunks, with metadata re-read before accepting a snapshot. Missing/invalid chains remain visible in the final Untrunked section.
- Git repository paths are discovered once per adapter. Structured or diff output truncation is rejected.
- Config updates use a bounded cross-process lock and atomic rename; parse errors retain the last valid in-memory config. Stack color changes apply immediately and persist through one active plus one coalesced pending background write with per-root sequence protection.
- The minimum supported terminal width is 40 columns. Persistent stale refresh health clears on the next valid snapshot.
- Deep narrow stacks use compact connectors so 40-column rows never wrap from indentation alone.
- GitHub failures remain typed through the UI, and macOS open/copy helpers have bounded subprocess timeouts.
- Child subprocesses run in separate process groups so a timeout terminates descendants as well as the direct child.
- The viewer uses fixed bottom-up Graphite lanes, aligned branch labels, ordered trunk sections, an Untrunked section, green additions/red deletions, and a fixed right-side `WT` marker.
- Reducer-owned `t`, `h`, `H`, and `s` views project in memory. `J/K` and shifted arrows use precomputed visible stack heads. `c` changes only the selected stable stack ID.
- Checkout and deletion share one mutation state. Post-mutation refreshes carry causal request epochs, so queued pre-mutation snapshots cannot release the mutation slot.
- Deletion is exact, local-only, confirmed with `x` then `y/n`, and non-force. Git-only deletion is merged-to-HEAD and expected-OID atomic. Graphite deletion is tracked-leaf-only, validates raw children, uses a cached allowlisted CLI contract, re-reads Git plus raw metadata after every invocation, and blocks inconsistent results.
- Graphite CLI 1.8.6 was characterized in a disposable repository: noninteractive leaf deletion removed the exact local ref and metadata while retaining the trunk. The provider still cannot offer expected-OID atomicity.
- Rendering uses direct snapshot-row, projected-row, lane-count, and lane-span indexes; viewport work does not scan the complete branch or stack set.
- The crate and lockfile are pinned to Rust 1.88.0. Formatting, strict offline Clippy, 78 tests, doctests, the offline release build, and PTY smoke tests pass on that toolchain.
- The release binary is 3.3 MB and measured about 11.8 MB resident after startup on a representative large multi-trunk repository. Benchmarks measured roughly 0.15-0.19 ms per 500-branch iteration and 1.41-1.93 ms per 5,000-branch iteration.
- `stackmap 0.0.0` is installed at `/Users/matt/.cargo/bin/stackmap` and resolves on `PATH`.

## Next steps

- Begin hands-on terminal testing in representative repositories by running `stackmap` from a worktree.
- Watch for terminal-specific shifted-arrow encoding; `J/K` remain the portable stack-jump fallback.
- Consider longer allocator/RSS soak profiling and render timing on unusually wide real branch inventories.
