---
title: Agent stack rename CLI
date: 2026-08-24
status: completed
---

# Agent stack rename CLI

## Goal

Add a deterministic, non-interactive command that lets an agent name an existing Stackmap stack:

```text
stackmap stack rename [--repo PATH] [--dry-run] BRANCH NAME
```

`BRANCH` may be any branch in the displayed stack. Stackmap resolves it through `TopologyIndex::stack_for()` and stores the name against the canonical displayed stack ID. This matters at forks, where a child stack has its own identity even though it belongs to the same Graphite component.

## Command contract

- Options precede operands. `--` permits option-shaped branch names or stack names.
- `NAME` is one shell argument, must remain non-empty after trimming, must contain no control characters, and may contain at most 80 characters.
- The target must exist in a ready repository, must not be a trunk, and must have safe topology evidence.
- A successful mutation prints `renamed STACK_ID to NAME`.
- A dry run prints `would rename STACK_ID to NAME` and creates no config, lock, or temporary files.
- Reapplying the same name prints `unchanged STACK_ID (NAME)` and does not rewrite config.
- The command changes only Stackmap's repository-local config. It never changes Git refs, worktrees, remotes, Graphite metadata, or pull requests.

## Scope boundaries

- This command renames existing stacks. It does not create, reorder, submit, or restack branches.
- Clearing a stack name is not part of this command.
- The interactive TUI naming flow remains unchanged and uses the same config field.

## Implementation units

### U1: Add the rename command service

Files:

- Create `src/commands/stack.rs`
- Modify `src/commands/mod.rs`
- Modify `src/commands/archive.rs` only if needed to share stable read evidence
- Modify `src/lib.rs`

Approach:

- Discover the repository and obtain the same bounded stable snapshot and source token used by the archive command.
- Build `TopologyIndex`, resolve any member branch to its canonical displayed stack ID, and reject missing, trunk, degraded, or unresolved targets.
- Load config strictly enough to validate the name without writing.
- For a real mutation, re-read repository evidence and ensure repository identity, source token, trunks, and resolved stack identity remain unchanged.
- Persist a field-aware `ConfigMutation::set_stack_name` with `Config::persist_mutation_strict`. Under the config lock, reject a competing rename of the same stack while preserving unrelated concurrent config changes.
- Verify the returned config contains the requested name.

Execution note: test-first for canonical stack resolution, dry-run behavior, idempotence, invalid targets, and persistence safety.

Verification:

- A branch in the primary path resolves to its displayed stack root.
- A branch after a fork resolves to the fork's displayed child stack ID.
- Dry-run and unchanged paths do not create or rewrite config.
- Invalid names and unsafe targets fail with `nothing changed`.
- A malformed config is not replaced, unrelated config fields survive, and Git and Graphite state remain unchanged.

### U2: Add CLI parsing, help, and output

Files:

- Modify `src/main.rs`

Approach:

- Add `stack` and `stack rename` help actions alongside the archive precedent.
- Parse `--repo`, `--dry-run`, `--`, and exactly two UTF-8 operands.
- Keep `run()` as a thin shell over the command service and emit only the documented deterministic result line.

Execution note: test-first for parser grammar and help behavior.

Verification:

- Parser tests cover valid input, duplicate or misplaced options, missing or extra operands, invalid UTF-8, `--`, and subcommand help.

### U3: Document and verify the feature

Files:

- Modify `README.md`
- Modify `docs/features.md`
- Modify `memory.md`
- Modify `changelog.md`

Approach:

- Document agent usage, resolution semantics, output, validation, and mutation boundaries.
- Run formatting, focused tests, the full test suite, strict Clippy, and a locked offline release build.
- Install and ad-hoc sign the verified release binary at `/Users/matt/.cargo/bin/stackmap`.

Verification:

- Installed `stackmap stack rename --help` describes the command.
- Installed binary version and signature verify.
- A smoke test in a temporary repository names a stack without changing Git refs or HEAD.
