# Typing latency + chat view parity

Status: in progress · Branch: `rebrand/kelpie` · Started 2026-10-23
Research: [CLI latency](../reports/researcher-261023-0053-cli-tui-input-latency.md) · [chat parity](../reports/researcher-261023-0053-chat-view-cli-parity.md)

## Outcome
Typing into an agent stays quick while the machine is busy, and chat view does what the TUI
does (its commands, skills, files, model / effort / mode), handing over to the TUI for the rest.

## Acceptance
- `kelpied bench-echo` measures keystroke→screen latency through the daemon, idle / Kelpie load /
  external load; e2e `typing_latency.py` measures the full app path.
- With N busy background sessions, the visible session's echo p50/p95 drop clearly vs. policy off.
- Chat view: `/` lists the agent's own commands (Claude `initialize`, Codex mapped set, ACP
  `available_commands_update`), `$` Codex skills, `@` files; pickers for model / effort / mode;
  TUI-only or unmapped commands offer "Run it in the terminal" (resumes, types the command).
- `cargo test`, `pnpm typecheck`, `pnpm i18n:check`, e2e `chat_parity.py` pass.

## Phases
1. Measure — `bench-echo` (+ `bench-burn`), e2e typing latency. ✅ tool; numbers in report
2. CPU priority — the shown session's agent + console host and the daemon above normal;
   nothing lowered; setting "Prioritize visible terminals" (default on) →
   `daemon.set_priority`. ✅
3. Conversation upkeep — Compact / Clear / Restart keeping the conversation (session menu +
   palette). ✅
4. Chat parity — adapters (Claude initialize + control requests, Codex RPC
   mapping + `$skill` items, ACP config options), composer completions + pickers, TUI hand-off
   with typed prompt on resumed launches, block-streamed markdown, `content-visibility`. ✅ code
5. Verify — unit tests, typecheck, i18n, bench matrix, e2e (typing, chat parity, chat.py). ✅
   Numbers: [benchmark report](../reports/benchmark-261023-0405-typing-latency.md)
6. Docs — README/architecture/user guide for the setting, bench-echo, chat composer. ✅

## Decisions
- Raise the shown session; never lower hidden ones. First built the other way round; the
  regression audit found a below-normal shell/agent printing nothing for 30 s+ while other
  programs kept the CPU busy (hidden sessions froze). Raising alone gives the same echo times.
- Claude chat sends `control_request{initialize}` as a plain `-p` client (no SDK entrypoint
  variable: it is not needed — re-verified on an idle machine — and would label the usage as
  Agent SDK and change the toolset).
- A prompt for a resumed launch is typed, not passed as an argument (an argument starts anew).
- Visible session: agent + its console host above normal, daemon above normal — measured: each
  alone helps little, all three take outside-load echo from ~300 ms to ~9 ms.
- No Claude fullscreen toggle: no measured typing benefit on this machine (runs were noisy);
  `CLAUDE_CODE_NO_FLICKER=1` stays available through the agent profile's environment.

## Risks
- Unix: niceness cannot be raised back unprivileged → policy is Windows-only.
- Load from outside Kelpie (other apps, other agents) is not reduced by lowering Kelpie's own
  background sessions — see the boost experiment in the report.
