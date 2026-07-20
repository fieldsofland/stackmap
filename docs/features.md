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
- Structural Git state appears first. Diffstats and GitHub enrichment arrive
  independently and can never roll the model back to an older generation.
- GitHub CLI failures and incompatible Graphite metadata remain visible provider
  states; neither removes local branches or blocks navigation.

## Navigation and organization

- Arrow/Vim movement, stack jumps, section-edge jumps, and portable terminal
  fallbacks work across deep and broad topologies.
- Persistent stack names and colors are stored per repository and applied
  immediately through bounded, coalesced atomic writes.
- `--current` starts with only the current stack and shared ancestry in view.

## Reversible archive workflow

- Lowercase `x` archives or restores a branch without modifying Git. Range mode
  applies the same reversible operation to a contiguous selection.
- Archive view retains required ancestry as dim context and adds no-fetch local
  upstream/remote-ref evidence. Unknown evidence is never presented as safe.
- Branches that become current or configured trunks are automatically restored
  to Active view.

## Guarded checkout and deletion

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

See [support.md](support.md) for the platform contract and
[graphite-compatibility.md](graphite-compatibility.md) for provider boundaries.
