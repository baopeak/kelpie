---
phase: 4
title: "Universal ACP chat"
status: completed
priority: P1
effort: "4d"
dependencies: [1]
---

# Phase 4: Universal ACP chat

## Overview
ACP is the cross-vendor protocol (Zed, JetBrains; 46 agents; SDKs 1.0 since 2026-06). Kelpie's ACP adapter serves only OMP.
Generalise it so every ACP-capable agent gets the structured chat view.

## Requirements
- Spike first: which installed CLIs speak ACP natively, exact flags, `initialize` capabilities; recorded in the phase report.
- Launch args stay decided by the app [RT-7]: `app/src/lib/agents.ts` gains `acp` args for known presets; custom agent defs gain an `acp` field
  (stored via existing `agent.set_custom`); `chatArgv` in `mode-switch.ts` builds argv as today and passes `chat_argv` to `session.create`. No "spawn by preset" in the daemon.
- `supportsChat` derives from `acp` instead of the hardcoded claude/codex/omp list (`mode-switch.ts:22`).
- Adapter handles `session/new`, `session/load` (only when `loadSession` advertised; ACP `sessionId` persisted as the session's agent id),
  `session/prompt`, `session/update` kinds, `session/request_permission`, `session_info_update` (title), `usage_update` (to phase 5), elicitation (form + inbox), `session/close`.
- Daemon status classifier [RT-7] (`chat.rs:387-395`): Working only for message / thought / tool-call update kinds; `usage_update`, `session_info_update`, plan updates leave status unchanged;
  elicitation request → Waiting; `stopReason` → Idle.
- Claude and Codex keep native adapters.

## Architecture
```
chat-pane ─ mode-switch.ts (chatArgv from agents.ts / custom def) ─┬ claude-adapter (native)
                                                                    ├ codex-adapter (app-server)
                                                                    └ acp-adapter (generic)
daemon chat.rs: relay + classifier by sessionUpdate kind
```

## Related Code Files
- Modify: `app/src/chat/acp-adapter.ts`, `app/src/chat/mode-switch.ts`, `app/src/chat/chat-model.ts`, `app/src/lib/agents.ts`, `crates/kelpied/src/chat.rs` (classifier),
  `crates/kelpied/src/agents.rs` (`acp` on `AgentDef` only), `app/src/components/chat-pane.tsx`, `app/src/components/new-session-menu.tsx`,
  `app/src/components/settings/agent-settings.tsx`, locales, docs.
- Create: `app/src/chat/acp-adapter.test.ts`, `scripts/e2e/fake_acp_agent.py`, `scripts/e2e/acp_chat.py`.

## Implementation Steps
1. Spike. 2. `acp` args + `supportsChat`. 3. Classifier + tests. 4. Generic adapter + elicitation + title + resume. 5. Settings UI for custom `acp`.

## Tests
- `cargo test` (chat.rs): classifier on recorded fixtures — trailing `usage_update` after `stopReason` keeps Idle; elicitation → Waiting.
- vitest: adapter fixtures. e2e: fake ACP agent covers every message type; real Gemini/OpenCode when installed.

## Perf gate
- Chat list with 2k items at 60 fps (virtualize if needed); adapter chunk lazy; idle CPU unchanged.

## Success Criteria
- [x] Every installed ACP agent completes a tool-using turn in chat view; status returns to Idle; elicitation answered from inbox.

## Risk Assessment
- ACP v2 drift → check `protocolVersion`, unknown kinds render as text and don't change status.
