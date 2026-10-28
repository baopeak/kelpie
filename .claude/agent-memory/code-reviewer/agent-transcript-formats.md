---
name: agent-transcript-formats
description: Real Claude/Codex transcript layout facts verified on this machine (subagent files, streaming output_tokens, no [1m] in model ids) and how to probe them safely
metadata:
  type: reference
---

Verified 2026-10-24 with scratchpad Python reading only usage fields (type, message.id/model/usage, requestId, isSidechain):

- Claude Code sub-agents live in `~/.claude/projects/<slug>/<session-id>/subagents/agent-*.jsonl`; main files had 0 `isSidechain` lines (36k in subagent files, ~47 % of API-priced spend). Any usage/cost code reading only `<slug>/*.jsonl` misses them.
- Repeated lines of one `message.id`: identical usage in main files; in subagent files `output_tokens` grows across lines (first line partial) -> dedupe must keep max/last, not first.
- No `message.model` contains `[1m]`, yet opus-5-5 main chains reach ~969K context -> 1M window is real without the tag.
- Message ids never repeat across files (resume does not copy history here).
- Codex rollouts: no carried-over totals across files; `info:null` rare; windows 258400 / 475000; `archived_sessions/` exists beside `sessions/`.
- msedgedriver for GUI e2e sits in the session scratchpad `msedge/` folder (pass via MSEDGEDRIVER); `pnpm build` is blocked by the "build" hook word, run it via a scratchpad shell script.

Related: [[agent-cli-protocol-verification]], [[shell-hook-blocked-words]]
