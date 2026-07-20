# Project memory

- `stackmap` is a macOS-targeted Rust TUI prototype for local Git branches with optional Graphite and GitHub enrichment.
- Structural refresh, diff enrichment, GitHub lookup, checkout, and platform actions use bounded independent coordinators so the UI and branch inventory remain responsive.
- Structural and enriched refresh events are distinct; only a successful structural inventory clears stale refresh health.
- App snapshot ordering is generation-monotonic; PR enrichment is preserved and matched by branch ID plus tip OID.
- Diff work is capped at four subprocesses with a 2,048-entry cache. Refresh/event/request queues are bounded.
- Obsolete diff generations stop scheduling between tasks, and identical OID pairs share one diff subprocess result.
- Graphite edges are trusted only when their complete local chain reaches one of the ordered configured local trunks, with metadata re-read before accepting a snapshot. Missing/invalid chains remain visible in the final Untrunked section.
- Git repository paths are discovered once per adapter. Structured or diff output truncation is rejected.
- Config updates use a bounded cross-process lock and atomic rename; parse errors retain the last valid in-memory config. Stack colors, archive membership, and bounded single-line stack names apply immediately and persist through one active plus one coalesced pending background write with per-identity sequence protection.
- The minimum supported terminal width is 40 columns. Persistent stale refresh health clears on the next valid snapshot.
- Deep narrow stacks use compact connectors so 40-column rows never wrap from indentation alone.
- GitHub failures remain typed through the UI, and macOS open/copy helpers have bounded subprocess timeouts.
- Child subprocesses run in separate process groups so a timeout terminates descendants as well as the direct child.
- The viewer uses fixed bottom-up Graphite lanes, aligned branch labels, ordered trunk sections, an Untrunked section, green additions/red deletions, and a fixed right-side `WT` marker.
- Reducer-owned `t`, `h`, `H`, and `s` views project in memory. `J/K` and shifted arrows use precomputed visible stack heads. `c` changes only the selected stable stack ID.
- Recent is the startup order. `T` selects Recent/Alphabetical/Graphite, `+/-/0` controls global lane pitch, `C` opens the color picker, and `g/G` are the terminal-portable section-edge fallback for Option+Arrow.
- `x` is reversible archive/restore and moves focus to the nearest visible row above, `a` toggles Archive view, and `v` previews an inclusive contiguous range committed with Enter. Archive state persists by branch name in the repository-local config; structural refresh automatically unarchives names that become current or configured trunks. Archive view renders required unarchived ancestry as dim, nonselectable stack context.
- `n` edits a persistent repository-local name for the selected stable stack ID. The white label is a nonselectable semantic row immediately above that stack's head; Enter saves, empty Enter clears, and Esc cancels. Trunks are intentionally unnamed.
- Archive remote-ref/upstream evidence is local-only and no-fetch. Containment work runs only for a bounded visible Archive working set, uses one active/latest-pending coordinator, and reports unavailable/loading states without claiming `local only`.
- Checkout and deletion share one mutation state. Post-mutation refreshes carry causal request epochs, so queued pre-mutation snapshots cannot release the mutation slot.
- Deletion is exact, local-only, confirmed with uppercase `X` then `y/n`, and non-force. Lowercase `x` never deletes. Git-only deletion is merged-to-HEAD and expected-OID atomic. Graphite deletion is tracked-leaf-only, validates raw children, uses a cached allowlisted CLI contract, re-reads Git plus raw metadata after every invocation, and blocks inconsistent results.
- Config persistence drains active and latest coalesced writes after terminal restoration during quit. The drain is capped at five seconds, retries one failed final write, and exits nonzero if persistence still fails.
- Graphite CLI 1.8.6 was characterized in a disposable repository: noninteractive leaf deletion removed the exact local ref and metadata while retaining the trunk. The provider still cannot offer expected-OID atomicity.
- Rendering uses direct snapshot-row, projected-row, lane-count, and lane-span indexes; viewport work does not scan the complete branch or stack set.
- Connector junction cells retain the parent/left rail color; the horizontal segment and child corner switch to the child stack color. The selected row fills edge-to-edge with its stack/trunk identity color while the checked-out branch retains its subtler tint when not selected.
- Enhanced terminals receive the complete Crossterm keyboard protocol flags. Shift+Arrow and Command+Arrow both map to stack jumps when the terminal reports their modifier; Stackmap decodes standard `ESC [1;2A/B` Shift sequences, and `J/K` remains the portable fallback when a terminal intercepts or erases modifiers.
- The crate and lockfile are pinned to Rust 1.88.0. Formatting, strict offline Clippy, 175 all-target/all-feature tests, the offline release build, focused Tier-2 review, real Git/Graphite integration, and PTY smoke tests pass on that toolchain.
- The open-source alpha identity is `0.1.0-alpha.1`. Cargo publication is disabled; GitHub source and two-architecture macOS prerelease assets are the intended distribution paths.
- The executable exposes a narrow documented runtime facade. White-box integration coverage is crate-internal and benchmarks use a feature-gated benchmark facade rather than public application internals.
- Release policy includes pinned macOS 15 ARM64/Intel CI, a stable-Rust compatibility lane, cargo-deny policy, Dependabot, fail-closed release aggregation, checksums, and artifact attestations.
- Core responsibilities are split behind private modules: topology projection/index/emission, Git inventory/mutation, App state/overlays/archive/mutation, and tree details/connectors.
- Recoverable production paths return typed or degraded outcomes. Remaining topology `expect` calls represent documented iterative-emission programmer invariants.
- Render tests cover both ordinary color output and `NO_COLOR`; selected colored rows use the identity accent as their background with a black identity glyph. Deadline tests assert bounded work counts instead of scheduler-sensitive wall-clock thresholds.
- The installed release binary is 3,713,072 bytes. In a synthetic 2,001-branch repository the preceding release measured about 15.4 MB RSS after startup, warmed to about 19.3 MB with Archive evidence, and plateaued at 19.856-19.888 MB after 100 refresh requests and repeated view/order/layout/navigation toggles.
- Release benchmarks measured about 0.108 ms per 500-branch projection, 1.10 ms per 5,000-branch projection, and 0.94 ms per projection of one 5,000-branch deep stack. A 5,000-level/10,000-branch deep comb emits iteratively without call-stack recursion; broad attach-parent lookup is indexed.
- `stackmap 0.0.0` is installed at `/Users/matt/.cargo/bin/stackmap` and resolves on `PATH`.

## Next steps

- Exercise the public alpha in representative repositories and record manual Terminal.app evidence for both macOS architectures.
- Cut `v0.1.0-alpha.1` only after reviewing the protected prerelease environment and the release checklist in `docs/releasing.md`.
- Resolve alpha feedback and make the Developer ID signing/notarization decision before a stable `0.1.0` release.
