# Graphite compatibility

Stackmap integrates with Graphite through two deliberately separate contracts.
Neither contract grants Stackmap permission to write Graphite metadata.

## Characterized matrix

| Capability | Graphite CLI 1.8.6 on macOS | Unknown/newer versions |
|---|---|---|
| Read topology | Characterized 2026-07-19 against sanitized SQLite fixtures | Enabled only when the required schema is proven; otherwise Git branches remain as independent roots |
| Delete tracked leaf | Characterized exact noninteractive single-branch CLI contract | Disabled until the CLI help contract is deliberately characterized, even if output appears similar |

Read compatibility requires `branch_metadata.branch_name` and
`parent_branch_name`, plus readable ordered `trunks` configuration. The optional
`children` column supplies Graphite order. Missing columns, invalid chains,
cycles, unknown parents, busy/corrupt data, or metadata changing during a read
degrade without hiding Git-local branches.

Mutation compatibility is narrower. A branch must be a tracked local leaf, the
raw metadata child set must be readable, the installed CLI contract must match
the allowlist, and Git plus raw metadata must satisfy immediate postconditions.
A failure or mixed result disables further deletion. Stackmap never uses force,
cascade, remote deletion, or pull-request closure.

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
