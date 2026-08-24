# Changelog

## 2026-08-24

- Collapsed the separate PR, pushed, and Graphite row fields into one right-hand status. Rows now show gray `local`, white `pushed`, yellow `#N`, green `✓ #N` for approved or merged PRs, or red `X #N` for closed PRs. PR state wins over pushed, and pushed wins over local.
- Removed `not pushed`, checking, unavailable, stale-match, and restack tokens from branch rows. Their underlying evidence remains available in branch details. Semantic status colors remain intact on selected rows.
- Passed 283 library tests, 15 binary tests, 2 terminal-shutdown tests, 2 doctests, strict offline Clippy, and the locked offline release build. Installed and ad-hoc signed `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `5ea37628f40abcd7930cc0050592e7224ee220c4098a298813d02b62c54dff73`.
- Fixed open PRs rendering blank on unchanged branches outside the current stack. The existing cached open-PR sweep now attaches branch-name matches across the complete local snapshot, while exact-head and historical API follow-ups remain capped at eight demanded branches.
- Confirmed the reported `admin/market-templates/descriptions` branch maps to open FactMachine PR #4999. Added a regression proving an open PR outside the targeted working set is published without expanding targeted API work.
- Passed 283 library tests, 15 binary tests, 2 terminal-shutdown tests, 2 doctests, strict offline Clippy, and the locked offline release build. Installed and ad-hoc signed `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `21ac70369e919808d4787c8497198d3aae2de10c77c14b70dd6d32cd150d2645`.
- Rebased `codex/graphite-status-actions` onto the new `main` at `62ba4dc` and resolved all overlap with Logan's merged PR #14.
- Preserved the separate PR, exact-tip pushed, and Graphite-health fields while adding Logan's `O` action to open every distinct PR in the selected stack. `o` still opens the selected PR and `y` copies its URL.
- Kept PR numbers visible for approved, merged, and closed states. Added regression coverage proving a stale `#~42` remains visible beside an independent not-pushed marker at 80 columns.
- Passed 282 library tests, 15 binary tests, 2 terminal-shutdown tests, 2 doctests, strict offline Clippy, and the locked offline release build. Installed and ad-hoc signed `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `2398e5525479cc563d8cb278476a767a49c4cd894b0a25054fbaab9ca96a26f0`.

## 2026-08-07

- Added the repo-local `$install-stackmap` skill to build the locked release, stop running Stackmap processes, install and ad-hoc sign the new binary, and verify it.
- Fixed terminal closure leaving Stackmap alive at roughly 100% CPU. On macOS, closing a PTY revoked descriptors 0–2 while Crossterm 0.29 remained in an internal read loop, preventing the main thread from observing SIGHUP or SIGTERM.
- Moved blocking terminal reads to a dedicated bounded-channel input worker and made the main loop independently detect controlling-terminal loss through the foreground process group. Terminal loss, Ctrl-C, SIGINT, SIGTERM, and SIGHUP can now drive complete process teardown even if Crossterm never returns.
- Made `TerminalGuard` the exclusive owner of raw-mode, keyboard-protocol, alternate-screen, and cursor restoration. Suppressed Ratatui's terminal destructor because its attempt to log cursor errors through revoked stderr turned otherwise successful hangup cleanup into exit code 101.
- Added real 80×24 controlling-PTY integration coverage proving both terminal closure and an actual Ctrl-C byte exit cleanly; Ctrl-C also restores the terminal. Passed 268 library tests, 15 binary tests, 2 process-level shutdown tests, responsiveness benchmarks, strict offline Clippy, and the locked offline release build.
- Killed all Stackmap processes, installed and ad-hoc signed the fixed release at `/Users/matt/.cargo/bin/stackmap`, then repeated both installed-binary PTY proofs. Each path captured five descendants and left zero survivors. Installed SHA-256 is `42516ab80704783b953f48e60757d167cea48dfaf99ee66db6a33b44fe9b6dec`.

## 2026-08-06

- Fixed a filesystem-watcher disconnect busy loop that could leave abandoned Stackmap sessions consuming most of a CPU core. The worker now distinguishes ordinary receive timeouts from permanent channel disconnection and exits on disconnect.
- Added a regression proving the watcher returns when its event producer disappears. Passed all 268 library tests, 15 binary tests, responsiveness benchmarks, strict offline Clippy, and the locked offline release build.
- Killed all Stackmap processes, installed and ad-hoc signed the fixed release at `/Users/matt/.cargo/bin/stackmap`, and verified SIGINT in a real PTY exits zero, restores the terminal, and leaves none of five captured descendants alive. Installed SHA-256 is `83fed22c1322e287fce7b9814449c77d798b1bec79be5c7114552f7b88fc4b79`.
- Restored leading-edge branch visibility by publishing a single-inventory candidate before the existing second-read Git/Graphite consistency verification, while retaining automatic corrected publication if verification detects drift.
- Ran independent inventory Git reads concurrently and removed the redundant startup inventory, reducing the FactMachine monorepo's warm strict two-read snapshot from roughly 0.73–0.82 seconds to 0.59–0.66 seconds; the live UI publishes after the first read.
- Replaced repository-wide historical PR enrichment with an eight-branch demand set prioritized by current, selected/current-stack, and recent changed branches. Navigation launches no provider work, and hiding status or entering Archive cancels GitHub alongside other optional providers.
- Limited background refresh application to four events per frame so input receives regular scheduling even during enrichment backlogs.
- Added regressions for bounded PR working sets, newly created branch demand, navigation without provider work, and complete optional-provider cancellation. Passed formatting, strict offline all-target/all-feature Clippy, 267 library tests, 15 binary tests, responsiveness benchmarks, release build, diff hygiene, code-simplifier review, and installed-path PTY startup/quit.
- Installed and ad-hoc signed the verified release at `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `6cd33675ca216ba2b8523a2919f10260922ded4014b1443c8bb6e2cf52cec24e`.

## 2026-08-05

- Installed the rebuilt agent-friendly release at `/Users/matt/.cargo/bin/stackmap`, replaced its ad-hoc signature, verified the code signature and archive help, and passed disposable-repository PTY startup/quit. Installed SHA-256 is `0b366c526a5e472e53950631eb79881d26c9bb177b1df9ddd4bf494ccefaf500`; pre-install artifact SHA-256 is `629c06d3afa4cc3d4f6943e158868ae3a1ee6ee69cc8e34a9d27db92438e7604`.
- Added the agent-native `stackmap archive [--repo PATH] [--dry-run] BRANCH...` command with subcommand help, deterministic batch output, protected-target/source-token validation, strict lock-scoped atomic config persistence, idempotence, and no Git/Graphite/remote/PR mutation.
- Added Command+C branch copy, Command+Shift+C deepest-section copy, and Command+Option+Shift+C complete-real-stack copy from keypress-captured authoritative topology; plain color keys and Ctrl-C remain unchanged.
- Made every nested visual section directly nameable from branches inside its indent and normalized Shift-Backspace/Ctrl-U to clear the complete Unicode draft without persisting.
- Reworked the status rail around PR lifecycle, exact-tip pushed evidence, and actionable-only `needs restack`; lowercase `s` now hides/shows and reclaims the full rail while uppercase `S` retains spacing.
- Replaced capped full-history PR loading with bounded staged open/head/OID enrichment, explicit checking/no-match/unavailable states, cache-preserving partial reducers, stale evidence across new local tips, and deterministic lifecycle/`updatedAt`/number ordering.
- Removed snapshot-wide Graphite startup work and automatic containment/patch-equivalence hot-path checks. Graphite now runs only for current/changed/explicitly demanded targets, cancellation recovers in-flight state, and transient CLI contract probe failures can retry.
- Fixed review-discovered archive concurrency, provider-error retention, Graphite cancel recovery, PR tie-break/retention, and 99/100-column geometry issues; focused Tier-2 re-review found no remaining actionable defects.
- Verified formatting, 264 library tests, 15 CLI tests, all responsiveness benchmarks, strict offline all-target/all-feature Clippy, offline release compilation, diff hygiene, disposable archive dry-run/write/idempotence with refs intact, and FactMachine PTY startup/status hide-show/quit. Live authenticated PR lifecycle confirmation remains blocked by the workstation's invalid `gh` token; provider failure rendered truthfully as `?` instead of blank.
- [LEARN] Large repositories need staged exact PR lookup rather than a longer global timeout, and hiding optional status must cancel lower-priority workers without discarding evidence or stranding `Checking` state.

## 2026-08-03

- Replaced misleading patch-equivalent `diverged` status with `Updated`, while preserving true remote-only divergence and reclassifying correctly when remote history changes later.
- Batched local remote-ref containment into one bounded Git command, retained status across structural refreshes, and moved remote/Graphite enrichment to rotating snapshot-wide batches so scrolling and visibility changes launch no status subprocesses.
- Added clean linked-worktree transfer into the primary checkout, with non-force removal, primary/linked cleanliness and repository-state checks, ignored-file protection, and best-effort restoration if switching fails.
- Added regression coverage for Updated-to-diverged transitions, local configured upstream invalidation, large-repository batch rotation with retained evidence, clean worktree transfer, and dirty/untracked/ignored refusal.
- Passed formatting, 219 library tests, 8 CLI tests, responsiveness benchmarks, strict offline all-target/all-feature Clippy, focused correctness/performance review, and the lockfile-exact offline release build.
- Installed and ad-hoc signed the release at `/Users/matt/.cargo/bin/stackmap`; version and Factmachine-monorepo PTY startup/quit pass. Installed SHA-256 is `513bd2f88692cc5d704cfe2a9813b98a0803f1376dc80439d2b99fa0cd0f6d16`.

## 2026-07-31

- Removed Graphite ancestry checks from structural loading and moved them to a cancellable four-worker coordinator scoped to the visible projection. Focused views exclude dimmed sibling stacks, and successful parent/tip OID results are reused across refresh generations.
- Added explicit checking state plus stale generation/OID/parent guards so asynchronous health results cannot overwrite newer topology or be lost to later diff enrichment.
- Made the rightmost Graphite status field fixed-width and left-aligned, and standardized metadata-column gutters at two cells while preserving readable 40-column archive and label rows.
- Verified formatting, diff hygiene, strict offline all-target/all-feature Clippy, 211 library tests, 8 binary tests, and all benchmark targets.
- Installed the matching optimized `stackmap 0.1.0-alpha.1` release at `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `7a228075139eec0d8eaf84d95e6fb178b80d67312adf6cf033c786e1a738d5d0`.

## 2026-07-30

- Added independent main-page remote-tip, PR lifecycle/number, and Graphite-health fields at every supported width, including explicit local-tip, stale-PR, restack-needed, and unavailable evidence.
- Added guarded Graphite 1.8.6 restack and move workflows: `r` confirms an exact affected upstack, `R` reconciles, and `m` previews subtree or `--only` topology before a second confirmation.
- Hardened Graphite mutations with exact version/help allowlisting, OID/topology/worktree preflight, descendant-aware postconditions, persistent blocking on ambiguous outcomes, stale-preview cancellation, and deferred `q`/Ctrl-C during live rewrites.
- Bounded Graphite ancestry health to 64 deduplicated concurrent checks; failed or unscheduled checks remain explicitly unavailable without blocking structural inventory.
- Made `c` and `C` target the effective visual section from anywhere inside its indent, and changed branch/title selection to a darker tint of the effective section or stack color with white foreground content.
- Added exact/stale GitHub PR matching, including merged/closed PRs whose head OID is unavailable, while preserving lifecycle color and a textual `~` warning.
- Updated help, README, feature/support policy, and Graphite compatibility documentation for the new status and action contracts.
- Passed formatting, strict offline Clippy, 207 library tests, 8 binary tests, responsiveness benchmarks, release compilation, diff hygiene, and build-tree plus installed-path PTY smoke tests. Installed the matching binary at `/Users/matt/.cargo/bin/stackmap` with SHA-256 `903c5fa1b53566d9d216d76c2b721930a0b5d9c87111b34e1c10753658ba4a42`.
- [LEARN] A visual section owns color interaction throughout its indented range; the selected cursor should reinforce that identity with a subdued tint rather than replace it with an unrelated highlight.

## 2026-07-29

- Added a detailed implementation plan for independent remote/PR/Graphite-health status, patch-equivalent rewrite labeling, stale-tip PR disclosure, guarded `r` restack, `R` refresh, and visual `m` Graphite move previews with `Tab`-selectable `--only` behavior.
- [LEARN] “No configured upstream” must not claim global remote absence; `local tip` describes only locally known remote refs, and rewritten patch-equivalent history must not be conflated with Graphite ancestry needing restack.
- Added a reviewed implementation plan for manual Stackmap-aware workflow preflight, dirty-safe committed-parent worktree routing, reversible local skill pilots, and color-coded main-row agent status beside the timestamp.
- Rendered user-authored stack and visual-section titles in white across ordinary, selected, focused, and `NO_COLOR` states while branch names retain their stack/section identity colors and selected-row contrast.
- [LEARN] “Names” in this UI refers to user-authored stack/section titles unless branch names are explicitly mentioned.
- Expanded GitHub enrichment from open-only PRs to all states and added compact `Merged`, `Closed`, and `Approved` status labels, with the full status included in branch detail.
- Added a right-side main-page remote safety column for pushed, ahead, behind, diverged, gone, no-remote, checking, and unavailable states; bounded no-fetch containment evidence now covers visible rows in Active and Archive views.
- Verified formatting, 206 all-target/all-feature tests, benchmarks, strict offline Clippy, and the offline release build; installed the matching `stackmap 0.1.0-alpha.1` binary at `/Users/matt/.cargo/bin/stackmap` with SHA-256 `125d5706162938ad224e1bbe9fe94c134c5fb07026469374758e237850b50e02`.

## 2026-07-28

- Kept focused trunks pinned while making Shift+Down and `J` onto the trunk reposition the scrollable stack rows to their bottom-most viewport state.

## 2026-07-27

- Increased diff-column precision so compact values below ten thousand render one decimal digit, such as `4.3K`, while preserving aligned fixed-width metadata.
- Extended downward stack navigation so Shift+Down and `J` move from the lowest stack to its configured trunk.
- Added standard cursor-aware inline name editing with Left/Right, Home/End, insertion, Backspace, and forward Delete.
- Added total owned branch counts to named stack rows.
- Installed the verified lockfile-exact `stackmap 0.1.0-alpha.1` release at `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `33e420645192bb2186a05da778ffef7331f35a45bdb0f3a141cc2620da5a3390`.
- Added deferred roadmap notes for cross-host agent activity awareness and assisted Stackmap organization, with explicit boundaries around lifecycle reporting, inference confidence, terminal scraping, and Graphite-owned restacking.
- Replaced the easy-to-miss footer-only branch checkout prompt with a centered confirmation popup that names the target and keeps Enter/Escape actions visible at the 40-column minimum.
- Made structural branch-tip updates move the cursor to the newest changed branch visible in the current view, helping surface branches advanced by coding agents without reacting to enrichment-only refreshes.
- Changed selected stack and visual-section labels to retain their identity-color highlight while rendering label and stack-summary diff text in white.
- Bottom-aligned short focused sections above their pinned trunk row, removing the large empty gap previously left by top-aligned content.
- Built the lockfile-exact offline release and installed the matching `stackmap 0.1.0-alpha.1` binary at `/Users/matt/.cargo/bin/stackmap`; SHA-256 is `c659871f5d0a386fd505344cd930dff066201541a9a64ab797b5e5a5492413bc`.

## 2026-07-21

- Added named-stack net diff summaries using each displayed topology group's validated base and real primary tip; branch rows remain parent-relative and side stacks keep independent endpoints.
- Published branch/shared diff enrichment before aggregate-only summary work, retained four-worker/cache/cancellation bounds, and added 500/5,000-group scheduling characterization.
- Added one dedicated nonselectable spacer below stack titles, kept section titles directly adjacent to their owned branches, and added a fixed diff-to-worktree metadata gutter.
- Verified formatting, strict all-target/all-feature Clippy, 194 unit/binary tests, responsiveness benchmarks, 2 doctests, release build, and diff hygiene; installed the lockfile-exact release binary and passed disposable-repository startup/quit smoke testing.
- Added a reviewed implementation plan for true cumulative stack-title diffs, clearer stack/section title hierarchy, and diff-to-worktree spacing, including fork/filter/archive correctness and an eager-enrichment performance gate.
- Added explicit spacing between timestamp and diff columns and between the PR column and the terminal edge, with focused geometry/rendering coverage.
- Added double-Enter branch checkout confirmation: the first Enter arms the exact selected branch, the second executes the existing protected checkout, and Escape or navigation cancels without invoking Git. Updated help/docs and installed the verified build.
- Added a reviewed implementation plan for a versioned read-only agent status CLI, complete per-worktree dirty evidence, and `fm-mobile-review`/`worktree-rules` integration. Deferred JSONL watch, agent annotations, MCP, and mutations until one-shot usage demonstrates need.
- Added persistent, purely visual feature sections: `i` creates/removes branch-anchored boundaries, section ranges accumulate name-only indentation, and Git/Graphite topology remains unchanged.
- Added atomic section name/color persistence with coalesced-write, stale-completion, authoritative-pruning, deletion-cleanup, and cross-refresh protection.
- Added adjacent-safe effective section colors, contextual `c`/`C`, colored dividers/labels/branch names, and non-color depth cues for 40-column layouts.
- Made stack and section labels selectable and editable inline. `n` creates only missing labels; Enter edits selected labels; empty Enter removes a label; Escape cancels and restores selection.
- Fixed name editing so printable navigation letters, key repeats, Backspace/Shift-Backspace, and ordinary text are consumed before global bindings; Ctrl-C cannot quit while editing.
- Added dedicated topology, reducer, input, archive/filter, refresh/coalescing, rendering, `NO_COLOR`, and narrow-width coverage. Parent verification passes formatting, strict offline Clippy, 178 library tests, 8 binary tests, benchmark targets, 2 doctests, and the offline release build.
- Installed the verified local test build at `/Users/matt/.cargo/bin/stackmap` and exercised section creation, inline `j/k/G/J` entry, persistence across restart, unchanged Git OID, and terminal restoration in a disposable repository.
- Made the wide branch-detail sidebar hidden by default and session-toggleable with `d`, returning its width to the branch map when dismissed.
- Improved row contrast: selected rows render branch names and all metadata in black over the full identity-color fill, while the unselected checked-out branch uses a 40% identity-color tint instead of a fixed dark background.

## 2026-07-20

- Made monitoring non-interfering and event-driven: passive Git commands disable optional locks, relevant filesystem events debounce after a quiet period, noisy `.git` paths are ignored, and periodic reconciliation moved from 30 seconds to five minutes.
- Separated passive and mutating Git execution so checkout and atomic deletion retain normal locks with a safer 30-second deadline, while timed-out commands receive a graceful TERM window before forced termination.
- Hardened shutdown by stopping and joining watcher/refresh/upstream workers and terminating every registered subprocess group instead of detaching active repository work.
- Verified formatting, strict offline Clippy, 176 all-target/all-feature tests, release build, and a live FactMachine-monorepo smoke test with 0.0% settled CPU, no index lock, and no process left after quit.
- Published the hardened source repository at `https://github.com/fieldsofland/stackmap`, protected `main` with required ARM64/Intel/stable/policy checks, protected prerelease tags, enabled security reporting and dependency updates, and configured the maintainer-approved prerelease environment.
- Installed `stackmap 0.1.0-alpha.1` from verified public `main` at `/Users/matt/.cargo/bin/stackmap`; version/help output and disposable-repository startup/quit smoke tests pass.
- Prepared Stackmap `0.1.0-alpha.1` for public open-source development with an MIT license, contribution and conduct policies, private vulnerability reporting guidance, issue forms, pull-request guidance, and feature/support/release documentation.
- Defined an explicit Cargo package allowlist, disabled crates.io publication, unified CLI and manifest version reporting, and documented source and GitHub-release installation.
- Added pinned macOS 15 ARM64/Intel CI, stable-Rust compatibility testing, cargo-deny policy, Dependabot, and a fail-closed two-architecture prerelease workflow with checksums and artifact attestations.
- Narrowed the supported Rust surface to an executable runtime facade plus a feature-gated benchmark seam; retained 175 all-target/all-feature tests, including a compile-fail check against former internal imports.
- Replaced recoverable production panic paths with typed or degraded outcomes and documented the retained iterative-topology invariants.
- Split topology, Git adapter, App, and tree-rendering responsibilities into private focused modules without changing the CLI, rendering, performance, or Git safety contracts.
- Documented Graphite 1.8.6 compatibility stewardship, safe fallback behavior, package invariants, release procedures, and the complete user-facing feature set.
- Fixed two environment-sensitive CI assertions by verifying selected trunk foreground/background behavior with and without `NO_COLOR`, and by testing deadline-bounded work directly instead of relying on runner wall-clock timing.
- Made dependency policy run on every pull request so the required `cargo-deny` branch-protection check cannot be skipped by path filtering.

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
