# Completion — Phase 13: Processes, ports & preview

Date: 2026-10-29 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-13](../261023-0048-multi-agent-ide-roadmap/phase-13-processes-ports.md)

## Delivered
- **Daemon** (API 16): `process.list / ports / port_owner / kill / kill_children`, app connection only (session programs and phone connections cannot call them). Session tree = parent-PID descendants validated by start time, plus Job Object members; the session's own process (start time recorded at spawn) is never a child, always kept, never killed — also after Restart.
- **Kill guard**: every stop re-checks (pid, start time, exe) — through an open handle on Windows, fresh read on Unix. Kill children keeps the session process, and in agent sessions the agent's direct children (MCP servers, tool shells); what those start (dev servers) is stopped. Windows windowed programs get a close request and 3 s; console programs are stopped at once (docs and dialog say so).
- **Scan**: Windows reads the process table with one `NtQuerySystemInformation` per scan (2.58 ms median release, 5 servers; budget 5 ms); Unix uses one long-lived sysinfo `System`, threads excluded. No scans while idle; terminal output pulls a look forward (≥ 2 s apart). macOS `lsof` only when a session has children/leftovers or is a custom session.
- **App**: port chips on tabs/sidebar (open preview), processes panel with tree, Left-the-session leftovers, kill / kill children; hidden below API 16.

## Verification
- cargo kelpied 351, kelpie-proto 25, kelpie 169; clippy clean except master's `too_many_arguments` at `mobile.rs:892`.
- vitest 460, typecheck, i18n 0 missing; entry bundle 324.99 kB gz.
- GUI e2e `processes.py` 55/55 twice in a row (restart keeps the new root, custom-session server chip, idle = 0 scans); regressions finish 72, attention 15, event_automations 86, spec_panel 124, context_cost 26.
- [Review](code-reviewer-261029-phase13-processes-ports.md): round 1 High (restart made the session's own process a "child") + Linux threads + Mediums fixed; round 2 Medium (chips gated on all platforms) + 3 Lows fixed; round 3: no blocking items.

## Not done / follow-ups
- Unix-only code (shared reader, Linux thread test) first compiles in CI; never run on Linux.
- Gemini relaunch / Volta shim: the agent's helpers may be stopped by Kill children (agent survives); no clearly correct rule.
- Programs that escape the Job (e.g. Store pwsh `Start-Process`) survive session close (pre-existing).
- Unix check-then-signal race (needs pidfd on Linux); macOS libproc instead of `lsof` and a Mac measurement; e2e debug scan time 5.5–8.1 ms under GUI (budget asserted on release only).
