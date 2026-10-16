# Version branch review, 2026-10-16

Source: `codex/unified-branch-status` at `dd6d245`. Base: GitHub `main` at `2601eec`, including Logan's PRs #14 and #17. Destination: `codex/version-2026-10-16`.

The two source commits were replayed onto main in an isolated worktree. The original branch, uncommitted notes, and installed executable were preserved. This branch is a comparison candidate, not a release or a change to the package version.

## Comparison with main

| Area | Main | Version branch |
| --- | --- | --- |
| Commit hashes | Seven-character hashes on active rows at map widths of 100 columns or more | Preserved, including when status is hidden; uses the local two-cell gutters |
| Branch status | Separate remote and PR fields | One status: local, pushed, open PR, approved/merged PR, or closed PR; diagnostics in details |
| PR discovery | Logan's branch-name matching and PR-opening actions | Preserves those actions, adds staged cached lookup, exact/stale evidence, and bounded targeted work |
| Graphite | Existing topology and deletion support | Adds guarded restack and move previews, including branch-only moves |
| Agent commands | No archive or stack-rename subcommands | Adds dry-run-capable archive and stack rename |
| Worktrees | Existing checkout handling | Adds guarded transfer from a clean linked worktree with recovery |
| Refresh and shutdown | Earlier implementation | Adds demand-scoped enrichment, leading-edge refresh, and terminal-loss shutdown fixes |

Logan's hash column was new functionality. The local branch had related status and workflow work, but no branch-row hash implementation. Both belong in the combined version. The extra metadata reduces branch-name space slightly at the 100-column boundary; the minimum name-width policy is retained.

## Review and repairs

Eleven review passes covered correctness, testing, maintainability, project standards, agent access, prior learnings, security, performance, reliability, failure cases, and CLI readiness. Reviewers inspected changes against main; focused follow-up review checked the repairs.

| Priority | Finding | Repair |
| --- | --- | --- |
| P1 | Rebase left a hash test calling a deleted helper | Extract rendered hash cells directly |
| P1 | Cancelling an only-move confirmation discarded grandchildren | Retain the complete original preview through confirmation |
| P1 | Graphite preflight allowed newly attached descendants | Require exact live subtree and affected-branch membership before mutation |
| P2 | A negative PR cache result could overwrite fresh discovery | Prefer fresh exact evidence and retain fresh stale evidence alongside cached lookup results |
| P2 | Head lookup timeouts could retry indefinitely | Publish a terminal outcome and begin cache expiry at completion |
| P2 | Switch execution errors skipped linked-worktree recovery | Inspect live ownership and restore non-force when the branch remains available |
| P2 | Docs described obsolete status and cancellation behavior | Align current docs and memory with the final implementation |

The cache repair also received a regression test for repeated requests with a stale PR head, so retained evidence does not bypass lookup backoff.

## Validation

Passed `cargo fmt --all -- --check`, strict offline Clippy across all targets/features, 301 library tests, 21 CLI unit tests, two doctests, and `git diff --check`. The independent follow-up review found no remaining actionable issue in the repairs. The version-branch push does not trigger this repository's main/PR-only CI workflows. No standalone application build, installation, live Graphite mutation, performance run, or terminal-process integration run was authorized or performed. Library and CLI unit-test harnesses exercise the code without installing an application.

## Remaining limits

Graphite preflight cannot prevent an external process from changing refs after the final read. The existing characterized-CLI guard and post-mutation validation remain necessary. Targeted GitHub cache entries have expiry times but no size cap; long-session memory growth was not measured. Older agent-coordination branch work was outside this newest-version comparison.
