# stackmap

`stackmap` is a local-first terminal map for Git branches and Graphite stacks.
It keeps the full local branch topology open in a compact, searchable TUI,
shows parent-relative branch diffstats and named-stack base-to-tip totals, marks worktree safety state, refreshes after
external Git changes, and optionally adds matching GitHub pull requests.

The current release is the unsigned, not-notarized `0.1.0-alpha.1` preview for
macOS 15+ on Apple Silicon and Intel. See the [feature guide](docs/features.md),
[support policy](docs/support.md), and [release verification guide](docs/releasing.md).

## Install

Requirements:

- Rust 1.88 or newer
- Git
- Graphite CLI metadata (optional)
- GitHub CLI authenticated for the repository (optional)

Download the matching `arm64` or `x86_64` archive from
[GitHub Releases](https://github.com/fieldsofland/stackmap/releases), verify it
using [the release guide](docs/releasing.md), or build from a tagged checkout:

```sh
cargo test --locked
cargo install --path . --locked
```

Stackmap is not published to crates.io, so `cargo install stackmap` is not a
supported installation path during the alpha.

Then open any local repository:

```sh
cd /path/to/repository
stackmap
```

You can also pass the repository path directly:

```sh
stackmap /path/to/repository
stackmap --current /path/to/repository
```

`--current` opens only the current branch's stack plus its shared ancestry and
trunk. Without it, every configured Graphite trunk and its stacks are visible.

Agents can archive an explicit batch without starting the TUI:

```sh
stackmap archive --dry-run --repo /path/to/repository branch-one branch-two
stackmap archive --repo /path/to/repository branch-one branch-two
```

Archive options must precede branch names. `--repo` defaults to the current
directory, duplicate names are reported once in first-seen order, and `--`
allows an option-shaped branch name. Use `stackmap -- archive` to open a legacy
repository path literally named `archive`.

Dry runs perform the same repository, config, branch, current/trunk, and
topology validation as a real archive, but do not create or change config.
Output is deterministic: `would archive <branch>`, `archived <branch>`, or
`unchanged <branch> (already archived)`. The complete batch is validated before
one repository-local atomic config mutation; any error changes nothing. The
command never changes refs, worktrees, remotes, Graphite metadata, or pull
requests. There is no restore subcommand yet: press `a`, select the branch in
Archive view, and press `x`.

Agents can also rename an existing displayed stack without starting the TUI:

```sh
stackmap stack rename --dry-run --repo /path/to/repository feature/branch "Payments cleanup"
stackmap stack rename --repo /path/to/repository feature/branch "Payments cleanup"
```

`BRANCH` may be any branch in the stack. Stackmap resolves the displayed stack
ID, including child stacks created by forks, and stores `NAME` against that ID.
Names are single-line, non-empty, and limited to 80 characters. Output is
deterministic: `would rename <stack> to <name>`, `renamed <stack> to <name>`, or
`unchanged <stack> (<name>)`. Dry-run creates no config artifacts. The command
revalidates repository topology before its atomic config write and never changes
Git refs, worktrees, remotes, Graphite metadata, or pull requests.

## What Stackmap shows

```text
 stackmap  /repo                                      live
 [staging]
       │ ○ child-stack-B                  11m   +6   -1
       └─┐
     │ ○ child-stack-A                    35m  +18   -3
     └─┐
 ● › │ ○ feature/stack-root                2h +120  -14 ⎇ repo
     └─┐
 ○ staging                                 1d   +?   -?

 3/4 recent All pitch:auto s status S spacing a View Archive x archive ? help
```

Each section grows upward from its trunk at the bottom. The first child stays
on the direct lane; additional children split one lane from their exact parent.
All branches in one linear stack share a lane and label column, while sibling
stacks move over by one lane. The trunk connector touches the trunk circle, and
trunk names are bold with a reserved color. Every configured local Graphite
trunk gets a section; invalid, unknown-parent, and Git-only branches remain
visible in a final `Untrunked` section.

Recent ordering is the default: recently active stacks sit nearest the trunk,
with progressively older stacks above them. Stacks still precede one-off
branches. The order picker also provides Alphabetical and Graphite ordering.

Markers are independent of color:

- `›` selected branch; its accent spans the full terminal row
- `○` topology node
- `●` checked-out branch in the fixed left status column
- `◉` checked-out trunk (`○` for any other trunk)
- `*` dirty current worktree
- `⎇ name` checked out in the named current or linked worktree
- `■` included in the current archive/restore range

Stack colors are deterministic. The single right-hand status field shows
exactly one state: gray `local`, white `pushed`, yellow `#N` for an open PR,
green `✓ #N` for an approved or merged PR, or red `X #N` for a closed PR.
PR state replaces pushed state, and pushed replaces local. Provider lookup and
Graphite-health diagnostics stay in branch details. Set `NO_COLOR=1` for
non-color output. At 120 columns and wider, press `d` to show or hide a detail pane for
the selected branch's exact local commit time, PR title, raw upstream state,
and provider diagnostics. It starts hidden.

Visual feature sections split a real stack into repository-local presentation
ranges without changing Git or Graphite. Press `i` on a branch to start or
remove a section. Successive sections indent branch-name text while leaving
the real circles, rails, and connectors fixed. Custom stack and section titles
remain white while branch names, dividers, and topology retain identity colors.

A named stack gets its own title row, followed by one blank hierarchy row.
That title shows the net diff from the validated parent of the stack's bottom
branch to its real displayed tip; it is not a sum of branch rows. Named visual
sections remain directly above the first visible branch they own.

Press `n` from any ordinary branch to edit the deepest visual section active at
that indentation; outside visual sections it edits the real stack. Named and
unnamed targets both open directly, and `Enter` still edits a selected label.
Inside the editor, Shift-Backspace clears the entire draft when the terminal
reports that chord distinctly; Ctrl-U is the portable clear-draft fallback.
Escape restores the persisted name, while saving an empty draft removes only
that stack or section label.

## Keys

| Key | Action |
|---|---|
| `Up` / `Down`, `j` / `k` | Move through visible branches |
| `Shift+Up` / `Shift+Down`, `Command+Up` / `Command+Down`, `J` / `K` | Move to the adjacent stack head; move 10 branches when outside a true stack |
| `Option+Up` / `Option+Down`, `g` / `G` | Move to the top / bottom branch of the current trunk section |
| `t` | Toggle Recent / Graphite stack order |
| `T` | Open the Recent / Alphabetical / Graphite order picker |
| `h` | Focus the selected stack and shared ancestry; repeat to show all |
| `H` | Focus the selected trunk (or Untrunked); repeat to show all |
| `s` | Hide/show the complete right status rail and reclaim its gutters (shown by default) |
| `S` | Toggle blank rows between adjacent stacks (on by default) |
| `d` | Show/hide the wide branch-detail sidebar (off by default) |
| `+` / `-` / `0` | Increase / decrease lane pitch; reset to automatic width |
| `/` | Filter by branch name; ancestors remain as dimmed context |
| `Enter`, `Enter` | Arm and confirm switching the selected branch; one `Enter` edits a selected label |
| `i` | Add/remove a purely visual section boundary on the selected branch |
| `c` / `C` | Cycle/open color for the active indented section, otherwise its stack |
| `n` | Edit/create the deepest active visual-section label, otherwise the real stack label |
| `Command-C` | Copy the selected branch ID (a selected label uses its anchor branch) |
| `Command-Shift-C` | Copy the deepest active visual section, anchor-to-tip, as newline-delimited branch IDs |
| `Command-Option-Shift-C` | Copy the complete real stack, base-to-tip, excluding trunk and child/side stacks |
| `x` | Archive/restore the selected branch and move focus to the nearest branch above |
| `v`, arrows, `Enter` | Preview and apply a contiguous archive/restore range |
| `a` | Toggle Active / Archive view |
| `X`, then `y` / `n` | Confirm or cancel guarded deletion of one exact local branch |
| `r` / `R` | Confirm Graphite restack for the selected branch / force reconciliation |
| `m`, arrows, `Tab`, `Enter` | Preview a Graphite move, choose parent, toggle branch-only mode, and confirm |
| `o` / `O` / `y` | Open the selected PR, open every PR in its stack, or copy the selected PR URL |
| `?` | Show help and provider status |
| `Esc` | Close help/message or cancel a filter edit |
| `q`, `Ctrl-C` | Quit |

Checkout is intentionally conservative. The first `Enter` arms the selected branch and the
second confirms it; `Esc` or navigation cancels. `stackmap` then runs an exact `git switch
-- <branch>` after re-reading live repository state. It never stashes, resets,
cleans, deletes, or forces. Git's normal overwrite and worktree protections are
preserved, and errors leave the current tree untouched. If another linked worktree owns
the branch, the same confirmation transfers a clean, ready worktree to the primary
checkout: dirty linked worktrees and dirty primary checkouts are refused, removal is
non-force, and a failed primary switch attempts to restore the original worktree.

Deletion is local-only, exact-target, confirmed, and non-force. Stackmap never
deletes remotes or closes pull requests. It refuses the current branch,
configured trunks, worktree-owned branches, unsafe repository states, stale
tips, degraded Graphite topology, and tracked branches with children.
Definitely untracked branches must already be merged into the current `HEAD`
and are deleted with an atomic expected-object-ID ref transaction. Eligible
Graphite-tracked leaves use only the installed CLI's characterized
noninteractive single-branch command; Graphite provides no expected-OID
transaction, so Stackmap performs immediate preflight and post-read checks and
blocks further deletion when the observed result is inconsistent.

Archiving is the normal cleanup operation and never changes Git. Archived names
persist in `<git-common-dir>/stackmap/config.toml`, are hidden from Active view,
and can always be restored from Archive view. The current branch and trunks
cannot be archived. The CLI additionally refuses a non-ready repository,
missing target, invalid config, unsafe target topology, or repository evidence
that changes before persistence; mixed valid/invalid batches never partially
apply. Ordinary definitely-untracked Git-only branches remain supported.

Active and Archive rows use the same exact-tip predicate for `pushed`; every
other branch without a PR renders as `local`. `pushed` requires equality with the
configured upstream, an exact locally known remote branch tip, or an exact PR
head OID. Reachability, stale PRs, and divergence arithmetic do not qualify.
Raw configured-upstream and remote-ref evidence remains in branch details.
Dim, nonselectable ancestry keeps archived branches oriented in their stacks.
All remote evidence is local and no-fetch, so it is not current server truth.
Unknown evidence is never treated as safe deletion evidence.

## Refresh and resource behavior

- Git refs are authoritative; Graphite only supplies validated parent edges.
- Filesystem events and a five-minute reconciliation tick feed a one-slot
  refresh queue. There is at most one active refresh and one pending request.
- Structural snapshots publish immediately while an independent latest-state
  diff coordinator uses at most four workers and a 2,048-entry object-pair
  cache shared by branch and stack-title diffs. Older enriched snapshots cannot
  replace newer structure. Branch/shared results publish before aggregate-only
  stack work, so title summaries cannot delay ordinary branch evidence.
- All inter-thread queues, subprocess output, and caches are bounded.
- Immutable snapshots are replaced as a unit; stale diff snapshots are rejected
  by generation, while delayed PR results require a matching branch and object ID.
- Rendering visits only visible rows. The full branch model remains internally
  scrollable and searchable.
- Remote exact-tip and Graphite health enrichment are demand-scoped and cached;
  scrolling, filtering, archive projection, and ordinary cursor movement render
  remembered evidence without launching provider subprocesses.
- Structural topology indexes and lane intervals remain linear in branches and
  edges. `t`, `h`, `H`, `S`, search, and navigation project entirely in memory
  without Git, SQLite, Graphite, or GitHub work.
- The implementation contains no application `unsafe` blocks. Rust ownership,
  bounded queues/caches, subprocess timeouts, and snapshot-release tests protect
  the long-running process from retained generations and unbounded growth.
- Remote-ref checks read one bounded exact remote-tip map instead of running
  containment or patch-equivalence commands per branch. Graphite health is
  cached by immutable parent/tip pair and runs for bounded initial discovery,
  current/changed stacks, or explicit `R`/`r`/`m` demand.
- Lowercase `s` cancels/suppresses remote and Graphite presentation work while
  preserving last-known evidence; showing status schedules one bounded refresh.
  PR enrichment remains active because `o` and `y` consume it.

Run the deterministic 500-branch projection benchmark with:

```sh
cargo bench --bench responsiveness --locked
```

## Local configuration

Color overrides are stored outside tracked files at:

```text
<git-common-dir>/stackmap/config.toml
```

Writes use a bounded cross-process lock, synced temporary file, and atomic
rename. Before saving, stackmap reloads the latest configuration so separate
worktrees changing different stack roots do not overwrite each other. Invalid
on-disk edits leave the last valid in-memory configuration active.

The TUI and `stackmap stack rename` share the same `stack_names` entries. A CLI
rename refuses a concurrent rename of that same stack while preserving unrelated
config changes.

## Graphite and GitHub fallbacks

Graphite's SQLite database is private and versioned outside this project.
`stackmap` opens it read-only and requires the `branch_metadata.branch_name`
and `parent_branch_name` columns plus readable ordered `trunks` configuration.
The optional `children` column supplies default Graphite order when compatible.
Unsupported,
missing, busy, or corrupt metadata produces an explicit topology-unavailable
state; every Git-local branch remains visible as an independent root. The tool
never writes or migrates Graphite files.

GitHub enrichment is optional. A bounded staged coordinator first sweeps open
PRs and attaches branch-name matches across every local branch, then serializes
cached exact-head and exact-commit lookups for unresolved current,
selected-stack, and remaining targets. Exact branch plus head OID wins;
the deterministic newest same-name fallback is marked stale with `~` and cannot
prove pushed. Open, approved, merged, and closed lifecycle states remain
visible. Checking (`…`), confirmed no match (`—`), and unavailable (`?`) are
distinct; partial or failed work preserves applicable last-known PRs. Missing
auth, offline operation, timeout, rate limiting, or malformed JSON never affects
local navigation.

## Troubleshooting

| Symptom | Meaning / action |
|---|---|
| `topology unavailable` | Graphite metadata is missing or incompatible; Git branches are still complete |
| PR status is `?` | Run `gh auth status`; GitHub enrichment is unavailable, but local behavior does not require it |
| PR status is `—` | The exact staged lookup completed and confirmed no matching PR |
| Checkout is disabled | The branch is current, another worktree owns it, or Git has an operation in progress |
| Git blocks checkout | Commit/move the conflicting work yourself; stackmap deliberately performs no cleanup |
| Shift-arrow does not jump | This is terminal encoding, not Vim. Use `J` / `K`, or configure the terminal to send `ESC [ 1 ; 2 A` / `ESC [ 1 ; 2 B` for Shift+Up / Shift+Down. Stackmap enables complete modifier reporting when the terminal supports the enhanced keyboard protocol. |
| Option-arrow does not jump | Use the terminal-compatible `g` / `G` fallback |
| A branch vanished from Active view | Press `a`, find it in Archive, then press `x` to restore it |
| Pushed status is `?` | Exact-tip evidence is unavailable or insufficient; fetch manually if you need current server evidence |
| Command copy does nothing | Terminal.app is the reference; a terminal may intercept Command chords before Stackmap receives them |
| Shift-Backspace types text or deletes one character | The terminal did not report a distinct shifted Backspace; use Ctrl-U to clear the full name draft |
| Footer still shows a completed checkout | Press `R`; reconciliation is target-aware and returns normal controls after success or a bounded timeout |
| `needs at least 40 columns` | Widen the pane; no branches were removed from the model |

## Development verification

```sh
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
cargo test --locked --offline --all-targets --all-features
cargo build --release --locked
scripts/check-package-contents.sh
```

The integration suite uses real temporary Git repositories for inventory,
refresh, and checkout protection. Other tests cover cycles and duplicate
branches, Graphite schema degradation, fixed responsive fields, 500-branch
reachability, bounded command output/cache size, stale GitHub responses, and
release of obsolete snapshots across 100 refresh generations.

## Project policies

- [Feature guide](docs/features.md)
- [Platform support](docs/support.md)
- [Graphite compatibility](docs/graphite-compatibility.md)
- [Contributing](CONTRIBUTING.md)
- [Security](SECURITY.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Release process](docs/releasing.md)
