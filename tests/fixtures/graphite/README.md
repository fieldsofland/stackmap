# Graphite fixture

This minimal fixture captures only the `branch_name` and `parent_branch_name`
columns used by stackmap's read-only adapter. Graphite's database is private;
additional columns are tolerated and missing required columns degrade safely.

