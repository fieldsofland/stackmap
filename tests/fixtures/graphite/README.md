# Graphite fixture

These sanitized fixtures characterize observed schema capabilities, not every
Graphite release. See `docs/graphite-compatibility.md` for the read/mutation
matrix and the disposable-repository procedure required before expanding it.

This minimal fixture captures only the `branch_name` and `parent_branch_name`
columns used by stackmap's read-only adapter. Graphite's database is private;
additional columns are tolerated and missing required columns degrade safely.
