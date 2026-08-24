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
  additions/deletions, and one status field are visible in each row.
- The status field is mutually exclusive: gray `local`, white `pushed`, yellow
  `#N` for open PRs, green `✓ #N` for approved or merged PRs, and red `X #N`
  for closed PRs. PR state replaces pushed state, and pushed replaces local.
- Raw upstream, PR lookup, stale-match, and Graphite-health evidence remains in
  details. `pushed` still requires an exact locally known remote or PR head tip.
- Named stack titles show a direct net diff from the displayed stack's validated
  base to its structural tip. Forked stacks use their own attachment parent;
  branch rows remain parent-relative.
- Structural Git state appears first. Diffstats and GitHub enrichment arrive
  independently and can never roll the model back to an older generation.
- GitHub CLI failures and incompatible Graphite metadata remain visible provider
  states; neither removes local branches or blocks navigation.
- GitHub lookup uses a cached open-PR sweep that attaches branch-name matches
  across every local branch, followed by serialized exact-head and exact-commit
  lookup for a bounded demand set. Exact-tip matches win; a
  deterministic same-name historical fallback uses `~`. Partial/failing work
  preserves applicable last-known PR evidence.

## Navigation and organization

- Arrow/Vim movement, stack jumps, section-edge jumps, and portable terminal
  fallbacks work across deep and broad topologies.
- Persistent stack names and colors are stored per repository and applied
  immediately through bounded, coalesced atomic writes. A named stack title is
  separated from its contents by one dedicated blank row.
- Visual feature sections add named, colored, cumulatively indented ranges
  inside a real stack. They move only branch-name text: Git circles,
  connectors, refs, and Graphite metadata remain unchanged. Labels are
  selectable and edited inline with exclusive keyboard input. Both `c` and `n`
  target the deepest effective section from any ordinary branch in its indent;
  outside sections they target the real stack. Named and unnamed labels can be
  edited, while Enter-on-label remains available. Shift-Backspace clears the
  whole draft when distinguishable; Ctrl-U is the portable fallback. Escape
  restores persisted text and saving an empty draft removes only that label.
- Command-C copies one branch ID, Command-Shift-C copies the deepest active
  section's anchor-to-tip suffix (including nested subsections), and
  Command-Option-Shift-C copies the complete real stack without its trunk or
  child/side stacks. Multi-branch output is newline-delimited base-to-tip and is
  derived from complete topology, independent of filter/focus/archive views.
- Fixed metadata gutters separate time, diff, worktree, and the unified status
  field. Lowercase `s` hides the status field and reclaims
  its gutters; uppercase `S` independently toggles separator rows.
- The wide branch-detail sidebar starts hidden and toggles with `d`.
- `--current` starts with only the current stack and shared ancestry in view.

## Agent stack naming

- `stackmap stack rename [--repo PATH] [--dry-run] BRANCH NAME` names an
  existing displayed stack without opening the TUI. `BRANCH` may be any member;
  Stackmap resolves the canonical stack ID through complete topology, so a forked
  child stack remains distinct from its Graphite component root.
- Names use the same repository-local config and validation as TUI naming. They
  must be non-empty, single-line, and at most 80 characters. Reapplying the same
  name is an idempotent success.
- The command revalidates repository identity, refs, trunks, and displayed stack
  identity before a locked atomic write. Dry-run creates no config artifacts. A
  competing rename of the same stack aborts, while unrelated concurrent config
  changes survive. Git, Graphite, worktrees, remotes, and PRs are read-only.

## Reversible archive workflow

- Lowercase `x` archives or restores a branch without modifying Git. Range mode
  applies the same reversible operation to a contiguous selection.
- `stackmap archive [--repo PATH] [--dry-run] BRANCH...` gives agents the same
  repository-local archive format without starting the TUI. It deduplicates
  targets, validates the complete batch and stable repository evidence, then
  performs at most one atomic config mutation. Dry-run creates or changes no
  config. Already archived targets are idempotent success.
- The command refuses missing/current/trunk targets, a non-ready repository,
  invalid config, unsafe target topology, or evidence drift. It never mutates
  refs, worktrees, remotes, Graphite metadata, or PRs. Restore remains available
  through Archive view `x`; there is no restore CLI in this release.
- Active and Archive rows use exact-tip pushed status rather than raw divergence
  labels. Archive view retains required ancestry as dim context. Local no-fetch
  evidence is not server truth, and unknown evidence is never presented as safe.
- Branches that become current or configured trunks are automatically restored
  to Active view.

## Guarded checkout and deletion

- Checkout requires two consecutive Enter presses on the same branch; Escape or navigation
  cancels before Git runs.
- Checkout delegates to exact `git switch -- <branch>` after live preflight. A clean,
  ready linked worktree can be removed non-force and transferred to the clean primary
  checkout; dirty or unsafe worktrees are preserved and refused.
- Destructive deletion is isolated on uppercase `X`, requires confirmation, and
  is local-only, exact, non-force, and fail-closed.
- Git-only branches require merged-to-HEAD evidence and an expected-object-ID
  transaction. Graphite branches must be characterized tracked leaves and pass
  CLI-contract, preflight, and postcondition checks.

## Guarded Graphite actions

- Lowercase `r` previews the selected branch's affected upstack and requires
  confirmation before running the characterized noninteractive Graphite
  restack command. Uppercase `R` remains force reconciliation.
- Lowercase `m` opens a temporary topology preview. Navigation selects the new
  parent, `Tab` toggles subtree versus branch-only movement, and `Enter`
  confirms the exact characterized Graphite move command.
- Both actions fail closed on dirty startup state, stale object IDs, linked
  worktrees, degraded topology, incompatible CLI help, or inconsistent
  postconditions. Stackmap never edits Graphite metadata directly.

## Reliability and scale

- Refresh, diff, upstream evidence, configuration writes, GitHub lookup,
  platform actions, caches, output, and queues all have explicit bounds.
- Stale generations are rejected, identical diff work is deduplicated, timed-out
  subprocess groups are terminated, and the last valid snapshot remains usable.
- Topology construction is indexed and iterative. Deep 5,000-branch stacks and
  broad 10,000-branch combs are covered without recursive emission or viewport
  scans over the entire repository.
- Remote pushed evidence comes from one bounded exact remote-tip map; automatic
  containment and patch-equivalence classification do not run per branch.
- Graphite health is cached by immutable parent/tip pair and limited to bounded
  initial discovery, current/changed stacks, or explicit `R`/`r`/`m` demand.
  Scrolling, filtering, archive toggles, and ordinary cursor movement launch no
  health subprocesses.
- Hiding status cancels/suppresses remote and Graphite work while retaining
  cached evidence; showing it schedules one bounded refresh. PR enrichment
  remains active because PR open/copy actions use it.

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
