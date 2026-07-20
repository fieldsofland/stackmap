# Runtime invariant policy

Stackmap distinguishes recoverable external/runtime state from programmer
invariants. Recoverable repository, provider, subprocess, channel, persistence,
and stale-UI conditions return typed errors, degraded state, cancellation, or a
fatal error at the startup/shutdown boundary. They must not panic inside the TUI.

## Classified production assertions

| Owner | Sites | Classification and evidence |
|---|---|---|
| App configuration editors | Validated palette/name conversions and overlay snapshot access | Values enter through bounded validators and overlays are revalidated on every structural snapshot. Navigation/archive tests cover disappearing targets and stale persistence. |
| App archive/range/deletion reconciliation | Snapshot access and config cleanup | Actions are created only from selectable snapshot rows and cancel on structural change. Archive and mutation-race suites cover disappearance, OID replacement, and protected identities. |
| App topology selection | Projected row resolves into the snapshot topology | Projection is rebuilt from the same immutable snapshot; topology/navigation tests cover malformed provider relationships and selection repair. |
| Topology trunk rows | A row classified as a trunk carries its trunk identity | Trunk-row construction sets both fields together; multi-trunk and Untrunked tests cover every row class. |
| Topology iterative emitter | Nonempty frame stack and prepared branch state | Frames are pushed before phase transitions and popped only at completion. Deep 5,000-level and broad 10,000-branch comb tests prove the iterative state machine. |
| Refresh/diff/upstream coordinators | Mutex/condvar ownership, latest work item, bounded worker creation | A poisoned coordinator indicates internal state may be invalid and the capability stops rather than continuing. Failure paths preserve the last valid structural snapshot; queue/cancellation tests cover bounded latest-state behavior. |
| Shutdown persistence queue | Active request exists when a retry begins | The retry is entered only after a completed active write; unit tests cover immediate quit, coalescing, and final visible failure. |

Test-only `unwrap`, `expect`, assertions, and deliberate panics are fixture
diagnostics and are outside the runtime inventory. Retained production assertions
represent internal state-machine corruption, not user input or provider failure.
New production panic sites require an owner, invariant rationale, and focused
surrounding test before merge.

The current production inventory contains only the trunk-row assertion and the
iterative-emitter frame assertions in `src/model/topology.rs`. UI/config,
subprocess-pipe, thread-spawn, mutex/condvar, event-queue, and shutdown retry
failures have explicit cancellation, unavailable/degraded state, typed errors,
capability shutdown, or top-level fatal outcomes. There are no unclassified
production panic sites.
