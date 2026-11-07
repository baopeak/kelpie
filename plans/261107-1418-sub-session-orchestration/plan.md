---
title: "Sub-session orchestration"
description: "An agent in a Kelpie session opens child sessions (any installed agent CLI), steers them, gets their reports pushed back and messages sessions of other projects; shown as a tree under the parent."
status: in-progress
priority: P1
effort: "~6d"
tags: [multi-agent, orchestration, daemon, mcp, ux]
created: 2026-11-07
branch: feat/sub-sessions (worktree D:\kelpie-subsessions, off origin/master 7b9debf)
---

# Sub-session orchestration

## Research
- [Claude Code sub-sessions, cross-session messaging, AgentKit orchestrate/atoz](../reports/researcher-261107-1403-claude-subsessions-agentkit-orchestrate.md)
- [Orca orchestration, Claude primitives, peers (agent-deck, claude-swarm…)](../reports/researcher-261107-1403-orca-orchestration-peers.md)
- [pstack (Lauren Tan / Cursor) and gstack principles](../reports/researcher-261107-1403-pstack-philosophy.md)

## Brainstorm contract
- **Outcome**: from one agent session the user says "open sub-sessions to fix #1 #2 #3"; the agent opens
  child sessions (Claude, Codex, Gemini, OMP… whatever is installed), each visible as a row nested under the
  parent and openable with one click; children report back on their own (done, blocked, needs the user,
  exited) and the parent gets those reports as a new turn without polling; the parent can steer a child and
  any session can message a session of another project. Nothing to install: it works in agents Kelpie
  launches as soon as the feature is on.
- **Constraints**: daemon owns sessions, mailboxes and delivery (window may be closed); reuse the session
  token, `typed_text`, agent status, presets and run worktrees; no model calls for status; one writer per
  conversation; messages never count as the user's consent and are never typed into a session that waits
  on a prompt; agents get at most the parent's permission level; bounded by machine size; EN + VI;
  Windows first, macOS parity; API level gate.
- **Non-goals (this plan)**: benchmark scraping from the web, cloud/remote-host children, a shared
  file-locked task list (Runs already is the DAG), Claude-only channels/sockets, auto-merging children's
  work (Finish/Review stay the user's), a new "environment" entity (projects + cross-project messages cover it).
- **Acceptance**: see each phase; overall: a real Claude parent spawns a Codex/Claude child through the
  injected tool, the child's "done" report arrives in the parent as a turn, the tree shows both, closing
  works, caps refuse a spawn beyond the limit with a clear message.

## Key decisions (defaults, overridable in Settings)
| # | Decision | Why |
|---|---|---|
| D1 | Agent surface = **both** an MCP server `kelpied mcp` (injected at launch into Claude `--mcp-config` and Codex `-c mcp_servers.kelpie…`, env passed by name, never written to disk) **and** a CLI `kelpied sub …` (`$KELPIE_CLI`) for every other agent and scripts. One set of daemon RPCs behind both. | MCP = zero-install, structured, survives Codex stripping `*TOKEN*` from shell env; CLI = universal (Orca/agent-deck choice). |
| D2 | Reports are **pushed**: daemon mailbox per session, delivered when the recipient is `idle` (not `waiting`, not `working`, nobody typed for 30 s) as one bracketed-paste turn with a header "Kelpie: message from sub-session …, not from the user". `kelpie_wait` / `sub inbox` pull the same mailbox. | Claude-style "wakes the parent"; Orca rule "spawn, then end your turn". |
| D3 | Automatic reports without model calls: child → `waiting` (with the prompt's question), child idle after work without an explicit report (last 12 lines, redacted), child exited (code). Explicit `kelpie_report` wins and is shown on the child's row. | Agents forget to report (agent-deck lesson); state from hooks/screen is truth. |
| D4 | Spawning asks the user by default: **Allow once / Allow for this session / Always / Deny** (policy `sub_sessions: off\|ask\|allow`). Kelpie's tools are pre-allowed inside Claude so there is one prompt, Kelpie's. | Awareness of token cost without the double prompt the post complains about. |
| D5 | Caps: depth ≤ 3, ≤ 10 live children per parent, global live children ≤ `auto` = clamp(min(cores/2, (RAM GB − 4)/2), 2, 8) or a set number; refused (not queued) at machine-load level L3. Refusals say what is running and suggest waiting. | pstack bounded hierarchy, AgentKit device awareness. |
| D6 | A child never gets more than its parent: it is launched with Yolo flags only if the parent was; `restricted` parents cannot spawn. | Claude Code / Orca permission rule. |
| D7 | `worktree: true` per spawn creates a worktree via the run engine's `create_task_worktree` (branch `kelpie/sub-<slug>`). Closing never removes a worktree; Finish/Review do. | pstack "separate before serializing"; no data loss. |
| D8 | Parent may close only its own descendants (`sub.close`); the user's close of a parent leaves children (they rise to their project) and the parent row offers *Close finished sub-sessions*. | Cleanup without killing work. |
| D9 | `kelpie_agents` lists installed agents with Kelpie's own outcome record per agent (sub-sessions done / failed, median minutes) as routing hints. | AgentKit idea from local, verifiable data only. |
| D10 | API level: next free at commit time (30 today: master 25, phone branches 26–29). | Memory rule on API levels. |

## Phases
| # | Phase | Effort | Depends | Status |
|---|---|---|---|---|
| 1 | [Daemon core: lineage, sub.* RPCs, policy, approvals, mailbox, delivery, caps](phase-01-daemon-core.md) | 2d | — | completed (edc1076) |
| 2 | [Agent surface: `kelpied mcp`, `kelpied sub`, launch injection, child brief, hints](phase-02-agent-surface.md) | 1.5d | 1 | completed |
| 3 | [App UI: tree, children strip, approval card, settings, i18n](phase-03-app-ui.md) | 1.5d | 1 | completed |
| 4 | [Docs, end-to-end and real-agent check](phase-04-docs-e2e.md) | 1d | 2, 3 | pending |

Sequential, one phase at a time; each: implement → tests → code review → fix until no blocker → local commit.

## Definition of done (every phase)
1. `cargo test -p kelpie-proto -p kelpied` (phase 1–2), `cd app && pnpm typecheck && pnpm test && pnpm i18n:check` (phase 3), clippy clean on touched crates.
2. Auth `capability_matrix` test covers every new method for every role.
3. No new always-on thread beyond one delivery tick; status classification stays µs-level.
4. Isolated daemon only (`KELPIE_ENDPOINT=kelpie-sub-e2e`, own `KELPIE_DATA_DIR`); never the user's daemon.
5. No push / PR without the user's go-ahead.

## Risks
- Codex env stripping / Claude MCP env inheritance: verified empirically in phase 2 before relying on it; fallback = claimed-id + job-object membership check is out of scope, documented.
- Typed delivery racing the user's typing: 30 s quiet guard + delivery only at `idle`.
- Prompt injection from child reports: header marks them as not from the user; never typed into `waiting`.
- 8 GB RAM machine: `auto` cap = 2 here; builds use own target `D:\kelpie-subsessions-target`, `CARGO_INCREMENTAL=0`, `-j 2`, deleted after merge.
