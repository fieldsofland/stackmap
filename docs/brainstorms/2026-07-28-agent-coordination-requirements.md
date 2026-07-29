---
date: 2026-07-28
topic: agent-coordination
---

# Agent Coordination in Stackmap

## Problem Frame

Stackmap already makes branch topology, worktrees, committed diffs, pull requests, and cleanup safety legible to a person. It does not show which coding agents are operating in those worktrees, what they have claimed, whether they are active or waiting, or whether a branch changed while an agent was present. A solo developer running several Codex, Claude, or other coding agents must reconstruct that picture from separate application windows and terminal sessions.

The desired outcome is a trustworthy coordination layer inside Stackmap's normal topology view. It must combine authoritative Git/worktree facts with cooperative agent lifecycle and intent reports without claiming that presence proves authorship, progress, or completion.

---

## Actors

- A1. Developer/operator: Runs several coding agents and uses Stackmap to understand ownership, activity, collisions, and handoffs.
- A2. Codex desktop agent: Works in a local or linked worktree and reports lifecycle plus explicit intent through an installed Stackmap plugin.
- A3. Claude Code agent: Works in a local or linked worktree and reports through a first-class Stackmap hook/plugin adapter.
- A4. Other coding agent: Uses the documented provider-neutral CLI or MCP contract to publish compatible activity and intent.
- A5. Stackmap TUI: Reconciles Git truth, worktree evidence, and advisory activity into one responsive topology view.
- A6. Agent consumer: Queries Stackmap for repository, worktree, ownership, and attention context before starting or adopting work.

---

## Key Flows

- F1. Agent arrives in a worktree
  - **Trigger:** A supported agent session starts or resumes within a Git repository.
  - **Actors:** A2, A3, A4, A5
  - **Steps:** The adapter reports its provider/session and working directory; Stackmap resolves the repository, worktree, branch or detached state, and observed tip; an expiring presence record is created or refreshed; the ordinary branch row gains a responsive activity marker.
  - **Outcome:** The operator can see that an agent is present on the correct branch without inferring task success or Git authorship.
  - **Covered by:** R3, R4, R5, R9, R12

- F2. Agent declares and changes intent
  - **Trigger:** An agent begins planning, implementing, testing, reviewing, waiting, blocking, or handing off work.
  - **Actors:** A2, A3, A4, A5, A6
  - **Steps:** The agent explicitly reports a bounded intent and phase; Stackmap stores it as agent-reported evidence; the main view updates in place; other agents can query the same state before claiming work.
  - **Outcome:** Humans and agents share one current, provenance-labelled coordination picture.
  - **Covered by:** R5, R6, R7, R10, R11

- F3. Git changes while an agent is present
  - **Trigger:** A worktree becomes dirty or a branch tip advances.
  - **Actors:** A1, A5, A6
  - **Steps:** Stackmap observes the Git/worktree change independently; the row shows both lifecycle activity and Git evidence; unattributed changes remain unattributed; stale expected OIDs are visible.
  - **Outcome:** The operator can distinguish "agent is working" from "repository changed" and see where both coincide.
  - **Covered by:** R1, R2, R7, R9, R12

- F4. Agent stops, crashes, or hands off
  - **Trigger:** A turn stops, a session ends, a heartbeat expires, or an agent explicitly creates a handoff.
  - **Actors:** A2, A3, A4, A5, A6
  - **Steps:** A stopped turn becomes idle rather than complete; a clean session end releases presence; a missing session becomes stale and later expires; an explicit handoff may report ready or blocked with before/after OIDs and verification summary.
  - **Outcome:** Abandoned claims self-clean, and readiness is never inferred from a lifecycle event alone.
  - **Covered by:** R4, R5, R6, R7, R12

- F5. Operator installs and verifies an integration
  - **Trigger:** The developer installs the Stackmap Codex or Claude integration.
  - **Actors:** A1, A2, A3
  - **Steps:** The integration is installed from its supported package surface; hooks are reviewed and trusted where required; a new agent session exposes Stackmap skills/tools; two worktree sessions produce visible, correctly mapped records.
  - **Outcome:** Installation through real product surfaces—not a fabricated registry fixture—proves the integration.
  - **Covered by:** R8, R10, R13, R14

---

## Requirements

**Authoritative repository and worktree status**

- R1. Stackmap must expose a documented, versioned, deterministic, machine-readable repository status containing exact branch OIDs, validated topology, worktree ownership, committed diff evidence, provider health, and optional presentation metadata.
- R2. Every occupied primary or linked worktree must have independent `clean`, `dirty`, or `unavailable` evidence. Unavailable evidence must never be treated as clean, and uncommitted evidence must remain distinct from committed diff summaries.

**Provider-neutral activity and ownership**

- R3. Stackmap must accept a versioned provider-neutral activity contract that can represent Codex, Claude, and other agent runtimes without making any provider's private session model the core domain model.
- R4. Each live activity record must identify its provider, opaque session, repository/worktree, proven branch or detached state, observed tip, lifecycle state, freshness, and provenance. Records must be bounded, atomic, concurrency-safe, and expire through explicit release or TTL.
- R5. Ownership must be an advisory soft claim. Multiple sessions may claim the same branch, worktree, stack, or section, and Stackmap must surface the collision rather than select a winner or block Git.
- R6. Meaningful intent, phase, readiness, and blockers must be explicitly agent-reported. Lifecycle hooks may establish presence, activity, waiting, idle, and freshness but may not infer task success or completion.
- R7. Stackmap must present agent lifecycle evidence and Git-observed dirtiness or tip movement as separate facts. It must not attribute an unclaimed change to an agent or claim that a live agent is actively modifying Git without Git evidence.

**First-class agent integrations**

- R8. A locally installable Codex desktop plugin must bundle the Stackmap skills, MCP server, lifecycle hooks, and a launcher that does not depend on the desktop application's inherited shell `PATH`.
- R9. Codex main chats and Codex subagents must appear on the correct worktree/branch, including active, waiting-for-approval, idle, stale, and released transitions. Resumes and compaction must refresh one session rather than duplicate it.
- R10. A first-class Claude Code integration must report main sessions, subagents or agent teams, task lifecycle, worktree changes, and explicit intent through the same provider-neutral model.
- R11. A documented generic CLI/MCP adapter must let another local agent claim, report, hand off, release, and query activity. Unsupported agents are shown only after they use this interface; Stackmap must not pretend to discover arbitrary runtimes automatically.

**Main-view coordination experience**

- R12. Agent activity must be visible inside Stackmap's ordinary topology view at the 40-column minimum and progressively disclose provider, count, phase, intent, worktree, freshness, provenance, and OID detail at wider widths without wrapping rows, displacing topology, or requiring a separate dashboard.
- R13. Zero, one, and multiple agents; waiting, idle, stale, blocked, ready, conflicting, and unassigned states must have non-color cues. With no integration or unreadable activity data, existing topology, navigation, filtering, archive, checkout, deletion, and resource behavior must remain usable.

**Agent workflows and privacy**

- R14. Supported agent workflows must include orient, coordinate/claim, preflight, query attention, report phase/intent, create handoff, and release. All repository facts must come from the shared Stackmap collector rather than reimplemented Git discovery.
- R15. Read tools must not mutate Git, Graphite, GitHub, worktrees, or remotes. Coordination writes may change only advisory Stackmap metadata; all future repository mutations require a separate approval and revalidation design.
- R16. Stackmap integrations must never automatically capture or persist raw prompts, transcript paths or contents, command strings or arguments, tool responses, assistant/subagent messages, approval descriptions, or file contents. Bounded semantic intent/handoff text is accepted only through an explicit report operation and is labelled agent-reported; adapters must not copy provider payloads into it. Privacy canaries must prove forbidden provider fields do not leak into storage, output, logs, or rendering.

**Reliability and verification**

- R17. Passive collection, hook ingestion, MCP operations, storage, subprocesses, queues, caches, and rendering must remain bounded and fail independently. Integration failure must degrade explicitly without hiding Git-local branches or blocking an agent turn.
- R18. Completion requires automated contract, real-Git multi-worktree, concurrency, lifecycle, privacy, responsive rendering, and PTY coverage plus a manual Codex desktop and Claude Code validation using multiple real worktree sessions.

---

## Acceptance Examples

- AE1. **Covers R2, R9, R12, R18.** Given two linked worktrees on different branches, when one Codex desktop chat runs in each worktree, both branch rows show the correct agent and independent cleanliness evidence without manual refresh.
- AE2. **Covers R5, R12, R13.** Given a Codex chat and a Claude session claiming the same branch, the row shows two agents and a collision cue; selecting it lists both claims and does not prevent either process from using Git.
- AE3. **Covers R6, R7, R12.** Given an agent reporting `testing` with no repository change, Stackmap shows reported testing only; when the worktree later becomes dirty, it separately shows Git-observed activity.
- AE4. **Covers R4, R6, R13.** Given a turn that stops normally, Stackmap shows idle rather than done; given a crashed session, it becomes stale and expires; given an explicit verified handoff, it may show ready.
- AE5. **Covers R8, R9, R14, R18.** Given a newly installed and trusted Codex plugin, a new desktop chat discovers Stackmap skills and MCP tools and automatically creates lifecycle presence; an already-open pre-install chat is not required to do so.
- AE6. **Covers R10, R11, R14.** Given a Claude Code session and a generic test adapter in separate worktrees, both can query existing claims, report their own intent, and appear with distinct provider identities in the same Stackmap view.
- AE7. **Covers R12, R13.** Given the same activity state at 40, 80, and 120+ columns, resizing changes disclosure but not association, counts, topology, selection, or row wrapping.
- AE8. **Covers R16, R18.** Given unique canary values in provider prompts, transcripts, commands, tool outputs, and assistant messages, none appear in registry files, CLI/MCP responses, logs, or the TUI unless a separate explicit semantic-report operation intentionally submits bounded text derived by the agent.

---

## Success Criteria

- The developer can open Stackmap and immediately see every integrated Codex, Claude, or generic local agent mapped to the branch/worktree it has claimed, including collisions, freshness, and whether Git is actually changing.
- A new agent can query Stackmap before working and correctly discover current worktrees, cleanliness, claims, intents, and attention items without reconstructing topology through separate commands.
- A morning test can install or enable the integrations, start multiple real sessions in linked worktrees, exercise approval waiting, subagents, reported phase changes, Git changes, handoff, collision, clean end, and crash expiry, and observe every transition in the ordinary Stackmap view.
- The implementation is not considered complete until the real Codex desktop and Claude Code surfaces are exercised; fabricated registry fixtures alone are insufficient.
- The existing format, strict lint, complete tests, doctests, release build, bounded-resource checks, no-lock smoke tests, and terminal behavior continue to pass.

---

## Scope Boundaries

- No hard branch, worktree, stack, or file locks.
- No automatic discovery claim for an agent runtime that has not installed or invoked an adapter.
- No file-level ownership, live cursor location, token accounting, progress percentage, or attribution of specific dirty files.
- No inferred task completion, review verdict, or readiness.
- No prompt, transcript, private application database, terminal, or tool-output scraping.
- No dependency on attaching to Codex's experimental internal App Server.
- No agent-facing Git, Graphite, GitHub, worktree, PR, or remote mutation tools.
- No implicit fetch or server-truth claims from local remote-tracking evidence.
- No cross-machine or cross-user synchronization in the first delivery.
- No separate agent dashboard replacing the topology view.
- Cross-project aggregation is a separate planning deliverable; repository-local coordination remains authoritative for the first implementation.

---

## Key Decisions

- CLI collector first: A stable repository/worktree status contract is the shared foundation for TUI, MCP, skills, and future consumers.
- Provider-neutral registry: Codex and Claude are first-class adapters over one common model; other agents integrate without new Stackmap subsystems.
- Soft claims with provenance: Coordination improves visibility without becoming a second source of Git authority.
- Main-view integration: Agent information augments the existing topology rather than creating a disconnected dashboard.
- Explicit semantic reports: Hooks provide lifecycle and presence; agents provide bounded intent and readiness.
- Per-session storage: Independent atomic records avoid a high-contention shared heartbeat file and allow bounded degradation.
- Real-surface proof: Desktop/CLI integration evidence is required because registry fixtures cannot prove product installation or hook behavior.

---

## Dependencies / Assumptions

- Codex desktop plugins continue to support bundled skills, local stdio MCP servers, and trusted lifecycle hooks for new chats.
- Claude Code continues to support plugin or settings-based hooks, MCP servers, skills, worktree events, and subagent lifecycle events.
- Supported integrations can execute a small local Stackmap launcher from their installed package.
- Agent activity remains useful when semantic intent is absent; the UI must say `intent not reported` rather than guessing.
- The current dirty worktree contains valuable uncommitted Stackmap work and must be preserved throughout implementation.

---

## Outstanding Questions

### Deferred to Planning

- [Affects R4, R13][Technical] Choose the production freshness, stale, expiry, and retained-handoff durations while keeping them testable with a short clock override.
- [Affects R8, R10][Needs research] Decide whether the integration package embeds architecture-specific Stackmap binaries, launches a separately installed release, or supports both with explicit fallback.
- [Affects R10][Needs research] Confirm which Claude hook events and plugin package shape provide the most reliable main-session, subagent/team, task, and worktree coverage in the installed version.
- [Affects R12][Technical] Allocate responsive activity geometry without weakening the existing 40-column topology and metadata guarantees.
- [Affects R18][Technical] Determine which desktop and Claude validation steps can be automated and which require final operator interaction.

---

## Next Steps

-> `/ce-plan` for structured implementation planning.
