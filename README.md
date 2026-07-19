# stackmap

`stackmap` is a local-first terminal map for Git branches and Graphite stacks.
It keeps the full local branch topology open in a compact, searchable TUI,
shows parent-relative diffstats, marks worktree safety state, refreshes after
external Git changes, and optionally adds matching GitHub pull requests.

This repository currently ships a `0.0.0` prototype for hands-on testing.
The v0 platform target is macOS; terminal rendering and Git reads may work on
other Unix systems, but open/copy integration is only supported on macOS.

## Install

Requirements:

- Rust 1.88 or newer
- Git
- Graphite CLI metadata (optional)
- GitHub CLI authenticated for the repository (optional)

Build and install from this checkout:

```sh
cargo test --locked
cargo install --path . --locked
```

Then open any local repository:

```sh
cd /path/to/repository
stackmap
```

You can also pass the repository path directly:

```sh
stackmap /path/to/repository
```

## What the prototype shows

```text
 stackmap  /repo                                      live
 [staging]
    │ ○ feature/stack-tip                 11m   +6   -1
    │ ○ feature/stack-child               35m  +18   -3
 ›  │ ● feature/stack-root                 2h +120  -14 WT
      ● staging                             1d   +?   -? WT

 3/4 graphite ALL spaced ↑↓ rows J/K stacks t/h/H/s views
```

Stacks read from top to bottom toward their trunk: the upstack tip is highest,
the first/downstack branch sits immediately above the trunk, and the trunk is
the final row of its section. Every configured local Graphite trunk gets a
section in configuration order. Invalid, unknown-parent, and Git-only local
branches remain visible in a final `Untrunked` section.

Markers are independent of color:

- `›` selected branch
- `○` ordinary branch and `●` current branch
- `*` dirty current worktree
- `WT` checked out in the current or another worktree

Stack colors are deterministic. Yellow is reserved for PR data, while green
and red represent insertions and deletions. Set `NO_COLOR=1` for non-color
output. At 120 columns and wider, a detail pane shows the selected branch's
exact local commit time and PR title.

## Keys

| Key | Action |
|---|---|
| `Up` / `Down`, `j` / `k` | Move through visible branches |
| `Shift+Up` / `Shift+Down`, `J` / `K` | Move between stack starts |
| `t` | Toggle Graphite order / oldest-to-newest stack order |
| `h` | Toggle the selected linear stack and its path to trunk |
| `H` | Toggle every stack belonging to the selected trunk |
| `s` | Toggle blank rows between adjacent stacks (on by default) |
| `/` | Filter by branch name; ancestors remain as dimmed context |
| `Enter` | Ask Git to switch to the selected branch |
| `c` | Cycle the selected stack's local color override |
| `x`, then `y` / `n` | Confirm or cancel guarded local branch deletion |
| `r` | Force repository reconciliation |
| `o` / `y` | Open or copy the selected PR URL |
| `?` | Show help and provider status |
| `Esc` | Close help/message or cancel a filter edit |
| `q`, `Ctrl-C` | Quit |

Checkout is intentionally conservative. `stackmap` runs an exact `git switch
-- <branch>` after re-reading live repository state. It never stashes, resets,
cleans, deletes, or forces. Git's normal overwrite and worktree protections are
preserved, and errors leave the current tree untouched.

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

## Refresh and resource behavior

- Git refs are authoritative; Graphite only supplies validated parent edges.
- Filesystem events and a 30-second reconciliation tick feed a one-slot
  refresh queue. There is at most one active refresh and one pending request.
- Structural snapshots publish immediately while an independent latest-state
  diff coordinator uses at most four workers and a 2,048-entry object-pair
  cache. Older enriched snapshots cannot replace newer structure.
- All inter-thread queues, subprocess output, and caches are bounded.
- Immutable snapshots are replaced as a unit; stale diff snapshots are rejected
  by generation, while delayed PR results require a matching branch and object ID.
- Rendering visits only visible rows. The full branch model remains internally
  scrollable and searchable.
- Structural topology indexes and lane intervals remain linear in branches and
  edges. `t`, `h`, `H`, `s`, search, and navigation project entirely in memory
  without Git, SQLite, Graphite, or GitHub work.
- The implementation contains no application `unsafe` blocks. Rust ownership,
  bounded queues/caches, subprocess timeouts, and snapshot-release tests protect
  the long-running process from retained generations and unbounded growth.

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

## Graphite and GitHub fallbacks

Graphite's SQLite database is private and versioned outside this project.
`stackmap` opens it read-only and requires the `branch_metadata.branch_name`
and `parent_branch_name` columns plus readable ordered `trunks` configuration.
The optional `children` column supplies default Graphite order when compatible.
Unsupported,
missing, busy, or corrupt metadata produces an explicit topology-unavailable
state; every Git-local branch remains visible as an independent root. The tool
never writes or migrates Graphite files.

GitHub enrichment is optional. A single-flight, TTL-limited bounded `gh pr list`
request retrieves open PRs, and results attach only when branch name and tip
object ID still match. Missing auth, offline operation, timeout, or malformed
JSON is shown as provider state and does not affect local navigation.

## Troubleshooting

| Symptom | Meaning / action |
|---|---|
| `topology unavailable` | Graphite metadata is missing or incompatible; Git branches are still complete |
| PR details are blank | Run `gh auth status`; local behavior does not require GitHub |
| Checkout is disabled | The branch is current, another worktree owns it, or Git has an operation in progress |
| Git blocks checkout | Commit/move the conflicting work yourself; stackmap deliberately performs no cleanup |
| Shift-arrow does not jump | Use the terminal-compatible `J` / `K` fallback |
| `needs at least 40 columns` | Widen the pane; no branches were removed from the model |

## Development verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
cargo build --release --locked
```

The integration suite uses real temporary Git repositories for inventory,
refresh, and checkout protection. Other tests cover cycles and duplicate
branches, Graphite schema degradation, fixed responsive fields, 500-branch
reachability, bounded command output/cache size, stale GitHub responses, and
release of obsolete snapshots across 100 refresh generations.
