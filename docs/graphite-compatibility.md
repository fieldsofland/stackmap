# Graphite compatibility

Stackmap integrates with Graphite through two deliberately separate contracts.
Neither contract grants Stackmap permission to write Graphite metadata.

## Characterized matrix

| Capability | Graphite CLI 1.8.6 on macOS | Unknown/newer versions |
|---|---|---|
| Read topology | Characterized 2026-07-19 against sanitized SQLite fixtures | Enabled only when the required schema is proven; otherwise Git branches remain as independent roots |
| Read-only needs-restack health | Git ancestry check over the validated recorded parent/tip pair; no Graphite command | Available whenever the topology pair is readable; unavailable/not-requested remains diagnostic detail rather than a row warning |
| Delete tracked leaf | Characterized exact noninteractive single-branch CLI contract | Disabled until the CLI help contract is deliberately characterized, even if output appears similar |
| Restack selected upstack | Characterized 2026-07-30 with Graphite CLI 1.8.6: `--no-interactive restack --branch <source> --upstack` | Disabled unless the installed help contract exposes the characterized branch/upstack/only surface |
| Move selected subtree / branch only | Characterized 2026-07-30 with Graphite CLI 1.8.6: `--no-interactive move --source <source> --onto <target> [--only]` | Disabled unless the installed help contract exposes the characterized source/onto/only surface |

Read compatibility requires `branch_metadata.branch_name` and
`parent_branch_name`, plus readable ordered `trunks` configuration. The optional
`children` column supplies Graphite order. Missing columns, invalid chains,
cycles, unknown parents, busy/corrupt data, or metadata changing during a read
degrade without hiding Git-local branches.

Presentation health does not expand this SQLite contract. Stackmap caches the
read-only Git ancestry result by immutable recorded-parent/tip OIDs and renders a
row badge only when restacking is needed. It performs one bounded initial
discovery, then checks current/changed stacks or explicit `R`/`r`/`m` demand.
Scrolling, filtering, archive projection, and ordinary cursor movement do not
invoke Git or Graphite. Hiding status cancels/suppresses presentation checks and
showing it schedules one bounded refresh; live mutation preflight remains the
authority for a confirmed restack or move.

Mutation compatibility is narrower. Deletion still requires a tracked local
leaf. Restack and move require a tracked non-trunk source, a clean ready startup
worktree, exact source/target/affected OIDs, and no rewritten branch checked out
in another linked worktree. The installed CLI contract must match the allowlist,
and Git plus raw metadata must satisfy immediate postconditions. A failure or
mixed result blocks further Graphite mutation until a ready authoritative
refresh. Stackmap never uses force, cascade, remote deletion, push, fetch, or
pull-request closure, and never writes Graphite metadata directly.

## Characterizing a new version

1. Create a disposable Git repository with a trunk, a linear stack, a fork, and
   one Git-only branch. Never use a production repository.
2. Record the Graphite CLI version and macOS architecture. Capture CLI help and
   exercise read-only stack discovery, leaf deletion, non-leaf refusal, and a
   changed-postcondition failure.
3. Copy only a minimal sanitized schema/data shape into
   `tests/fixtures/graphite/`; remove paths, remotes, commit messages, people,
   credentials, and real branch names.
4. Add topology and guarded-deletion tests for the observed variant. Similar
   column names are not compatibility evidence.
5. Update the matrix and characterized date only after all existing degradation
   and mutation safeguards pass.

See [the fixture policy](../tests/fixtures/graphite/README.md) for current test
assets.
