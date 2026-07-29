# Stackmap features

Stackmap is a live, local-first map of every branch and Graphite stack in a Git
repository. It is designed for repositories where a flat branch list no longer
explains which work belongs together or what is safe to clean up.

## Stack topology

- Bottom-up sections place each configured trunk at the bottom and its stacks
  above it, with an explicit final Untrunked section.
- Parent-child connectors, stable stack lanes, and per-stack colors make forks
  visible without hiding malformed or Git-only branches.
- Recent, alphabetical, and Graphite ordering can be changed without rerunning
  Git. Stack and trunk focus retain dim ancestry context.
- Search, optional separators, automatic or manual lane pitch, and a 40-column
  compact layout keep large repositories navigable.

## Branch evidence

- Current branch, dirty worktree, linked worktree, commit age, parent-relative
  additions/deletions, and optional open pull request details are visible in one
  row or the wide detail pane.
- Named stack titles show a direct net diff from the displayed stack's validated
  base to its structural tip. Forked stacks use their own attachment parent;
  branch rows remain parent-relative.
- Structural Git state appears first. Diffstats and GitHub enrichment arrive
  independently and can never roll the model back to an older generation.
- GitHub CLI failures and incompatible Graphite metadata remain visible provider
  states; neither removes local branches or blocks navigation.

## Navigation and organization

- Arrow/Vim movement, stack jumps, section-edge jumps, and portable terminal
  fallbacks work across deep and broad topologies.
- Persistent stack names and colors are stored per repository and applied
  immediately through bounded, coalesced atomic writes. A named stack title is
  separated from its contents by one dedicated blank row.
- Visual feature sections add named, colored, cumulatively indented ranges
  inside a real stack. They move only branch-name text: Git circles,
  connectors, refs, and Graphite metadata remain unchanged. Labels are
  selectable and edited inline with exclusive keyboard input.
- Fixed metadata gutters separate time from diff, diff from worktree, remote
  safety state, and the PR column from the terminal edge whenever those columns
  are present.
- The wide branch-detail sidebar starts hidden and toggles with `d`.
- `--current` starts with only the current stack and shared ancestry in view.

## Reversible archive workflow

- Lowercase `x` archives or restores a branch without modifying Git. Range mode
  applies the same reversible operation to a contiguous selection.
- Active and Archive rows show no-fetch local upstream/remote-ref evidence:
  pushed, ahead, behind, diverged, gone, no remote, checking, or unavailable.
  Archive view retains required ancestry as dim context. Unknown evidence is
  never presented as safe.
- Branches that become current or configured trunks are automatically restored
  to Active view.

## Guarded checkout and deletion

- Checkout requires two consecutive Enter presses on the same branch; Escape or navigation
  cancels before Git runs.
- Checkout delegates to exact `git switch -- <branch>` after live preflight and
  preserves Git's dirty-tree and linked-worktree protections.
- Destructive deletion is isolated on uppercase `X`, requires confirmation, and
  is local-only, exact, non-force, and fail-closed.
- Git-only branches require merged-to-HEAD evidence and an expected-object-ID
  transaction. Graphite branches must be characterized tracked leaves and pass
  CLI-contract, preflight, and postcondition checks.

## Reliability and scale

- Refresh, diff, upstream evidence, configuration writes, GitHub lookup,
  platform actions, caches, output, and queues all have explicit bounds.
- Stale generations are rejected, identical diff work is deduplicated, timed-out
  subprocess groups are terminated, and the last valid snapshot remains usable.
- Topology construction is indexed and iterative. Deep 5,000-branch stacks and
  broad 10,000-branch combs are covered without recursive emission or viewport
  scans over the entire repository.

## To-do

- **Cross-host agent activity:** After the read-only `stackmap agent status`
  contract is proven, consider a separate local agent registry that maps
  sessions and current working directories to worktrees and branches. It should
  support reported states such as working, testing, blocked, waiting, and done;
  use heartbeats and TTL expiry; and visibly distinguish reported status from
  process/Git inference. Claude Code lifecycle hooks, a cooperative Codex skill,
  and optional cmux CLI/socket enrichment can feed the same bounded local
  protocol. Do not depend on terminal scraping, private application databases,
  or cmux ownership of the agent process.
- **Assisted organization:** Consider a reviewable `stackmap-reorganize` skill
  that proposes section boundaries, indentation, and names from topology,
  diffs, commits, and agent intent. Stackmap should own validated visual-config
  mutations; Graphite should continue to own branch movement and restacking.

See [support.md](support.md) for the platform contract and
[graphite-compatibility.md](graphite-compatibility.md) for provider boundaries.
