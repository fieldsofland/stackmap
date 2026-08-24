# Runtime invariant policy

Stackmap distinguishes recoverable external/runtime state from programmer
invariants. Recoverable repository, provider, subprocess, channel, persistence,
and stale-UI conditions return typed errors, degraded state, cancellation, or a
fatal error at the startup/shutdown boundary. They must not panic inside the TUI.

## Classified production assertions

| Owner | Sites | Classification and evidence |
|---|---|---|
| App configuration editors | Validated palette/name conversions and overlay snapshot access | Values enter through bounded validators; `n` resolves the projected deepest effective section or real stack, and overlays are revalidated on every structural snapshot. Terminal/navigation/render tests cover modifier normalization, Unicode clearing, disappearing targets, and stale persistence. |
| App archive/range/deletion reconciliation | Snapshot access and config cleanup | Actions are created only from selectable snapshot rows and cancel on structural change. Archive and mutation-race suites cover disappearance, OID replacement, and protected identities. |
| Archive command service | Stable repository snapshot, target eligibility, and strict config persistence | The complete deduplicated batch is checked before one mutation, then repository identity/source token/trunks and targets are revalidated. Dry-run and every failure path perform no write; real-Git/config tests cover idempotence, malformed config, linked worktrees, degraded targets, and drift. |
| App topology selection | Projected row resolves into the snapshot topology | Projection is rebuilt from the same immutable snapshot; topology/navigation tests cover malformed provider relationships and selection repair. |
| App Graphite move preview | A selected validated source has an expectation record | Preview construction runs only after selected-row and Graphite-action validation; move-preview drift and reconciliation tests cover source disappearance and OID replacement. |
| App clipboard capture | Selected branch/label identity resolves through authoritative topology | Payloads are captured synchronously before asynchronous platform work. Copy tests cover branch/section/stack scope, filtering/focus/archive invariance, child-stack exclusion, and post-keypress refresh races. |
| Topology trunk rows | A row classified as a trunk carries its trunk identity | Trunk-row construction sets both fields together; multi-trunk and Untrunked tests cover every row class. |
| Topology iterative emitter | Nonempty frame stack and prepared branch state | Frames are pushed before phase transitions and popped only at completion. Deep 5,000-level and broad 10,000-branch comb tests prove the iterative state machine. |
| Refresh/diff/upstream/GitHub/Graphite-health coordinators | Mutex/condvar ownership, latest work item, bounded worker creation | A poisoned coordinator indicates internal state may be invalid and the capability stops rather than continuing. Failure paths preserve applicable last-known state; queue, generation/OID, cache, deadline, and hide/show cancellation tests cover bounded latest-state behavior. |
| Shutdown persistence queue | Active request exists when a retry begins | The retry is entered only after a completed active write; unit tests cover immediate quit, coalescing, and final visible failure. |

Test-only `unwrap`, `expect`, assertions, and deliberate panics are fixture
diagnostics and are outside the runtime inventory. Retained production assertions
represent internal state-machine corruption, not user input or provider failure.
New production panic sites require an owner, invariant rationale, and focused
surrounding test before merge.

The current production inventory contains the classified editor-target and
move-source assertions in `src/app.rs`, plus the trunk-row and iterative-emitter
assertions in `src/model/topology.rs`. UI/config validation, subprocess-pipe,
thread-spawn, mutex/condvar, event-queue, and shutdown retry failures otherwise
have explicit cancellation, unavailable/degraded state, typed errors,
capability shutdown, or top-level fatal outcomes. There are no unclassified
production panic sites.
