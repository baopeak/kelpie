# Phase 4 — Docs, end-to-end, real-agent check

## Requirements
1. `docs/architecture.md`: RPC list + events (`sub.*`, `sub.request`), callers section (new methods per role),
   a "Sub-sessions" section (lineage, mailbox limits, delivery rules, caps, approvals, injection, stats).
2. `docs/user-guide.md`: "Sub-sessions" — what it is, how to ask an agent ("open sub-sessions to fix #1 #2"),
   approvals, the tree/strip, caps, cleanup, cross-project messages, CLI for other agents. EN (+ VI if the guide has VI).
3. `scripts/e2e/sub_sessions.py` against an isolated daemon (`KELPIE_ENDPOINT=kelpie-sub-e2e`, temp
   `KELPIE_DATA_DIR`, set in the script's own process; cleanup kills only that daemon): shell-kind parent
   created by the user token, then as that session (token from its env via `kelpied` inside it) spawn with
   policy allow → child appears with `parent`; ask mode → `sub.request` event → `sub.answer` → spawn returns;
   caps refuse; `sub.message` delivered into an idle shell (visible in `session.read`); `sub.report` reaches the
   parent's inbox; `sub.close` of a non-descendant refused.
4. Real agents (cheap, one each, isolated dev daemon, temp folder, clean exit): Claude parent with injected tool
   spawns a child and receives its `done` as a typed turn; Codex child reports through MCP. Record results.
5. Plan status → completed; journal note if something notable broke.
