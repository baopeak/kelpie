---
phase: 1
title: "Foundation: perf gate, tests, daemon capabilities, outbound text safety"
status: completed
priority: P1
effort: "5d"
dependencies: []
---

# Phase 1: Foundation

## Overview
Four things every later phase needs: (a) a repeatable perf gate + TS unit tests, (b) caller identity and per-method
authorization on the daemon socket, (c) one sanitiser for text typed into PTYs, (d) one redaction helper for text sent out.

## Requirements
### 1a Perf gate & tests [RT-12]
- `scripts/perf/perf_gate.py --label <name> [--compare baseline.json] [--update-baseline]`, JSON out, non-zero exit over budget.
- Reuse the existing runtime-gated test hook `window.__kelpie` under `KELPIE_E2E` (`app/src/main.tsx:20-26`); extend it with a
  long-task observer and an in-page echo probe. No new hook, no new env flag; measured build = shipped release build.
- Metrics measured **in page** with `performance.now()` (not WebDriver polling): keydown → xterm `onWriteParsed` for echo;
  project switch via the existing `smoke.py` method but ≥ 200 samples. Budgets compare **medians** by ratio; p95 reported as warning.
- RSS scope stated: app + WebView2 children + daemon; PTY children (shells/agents) reported separately, not budgeted.
- `pnpm test` (vitest) for pure TS modules; first tests: `layout.ts`, Claude/Codex adapters.
- CI (no GUI): `cargo test`, `pnpm test`, `kelpied bench`, bundle budget.
### 1b Daemon capabilities [RT-1]
- At `session.create` the daemon issues a random per-session token, injected as `KELPIE_SESSION_TOKEN` (alongside `KELPIE_SESSION_ID`).
- The app connection authenticates with an app token (generated per install, kept in the OS keyring, read by app core only).
- Each RPC method declares a capability: `app` (admin: session.create/input/close, automation.*, agent.set_custom, mobile.*, daemon.shutdown, process.kill, notify config…)
  or `session` (own-session only: hook.report, run.report, meta.set for own cwd, note.add for own worktree, `kelpied terminal` read/list as today's CLI allows).
  Unauthenticated callers get nothing except `hello`.
- Existing CLI verbs keep working from inside sessions (token in env); scripts outside sessions use `kelpied call` with the app token only if the user opts in (setting).
- Bump `API_LEVEL`; every later phase that adds an RPC declares its capability and bumps `API_LEVEL` (DoD item).
### 1c Paste sanitiser [RT-10]
- One daemon function used by every text-typing path (`session.rs` prompt delivery at `session.rs:539`, `session.input` when flagged `paste:true`):
  strip C0/C1 controls except `\n` `\t`, reject bodies containing `\x1b[201~`; app helpers `typeInto` / `sendToSession` set `paste:true`.
### 1d Redaction [RT-10, RT-11]
- `kelpie_proto::redact()` (AWS/GitHub/OpenAI/Anthropic keys, JWT, PEM blocks, secret-named `KEY=value`) + Tauri command `redact_batch`
  + TS wrapper `app/src/lib/redact.ts`; wired now into existing outbound TS paths: hand-off, Copy Context, `sendToSession` (opt-in per call), review-notes prompt.

## Architecture
```
app core ──(app token)──► daemon server.rs ─► dispatch: method.capability check ─► handler
agent CLI ─(session token)┘                     session token → session id binding (no env spoofing)
```
Token check in `crates/kelpied/src/server.rs` dispatch; capability table next to the method match in `daemon.rs`.

## Related Code Files
- Create: `scripts/perf/perf_gate.py`, `scripts/perf/baseline.json`, `scripts/perf/README.md`, `app/vitest.config.ts`, `app/src/lib/layout.test.ts`,
  `app/src/chat/claude-adapter.test.ts`, `crates/kelpie-proto/src/redact.rs`, `app/src/lib/redact.ts`, `scripts/e2e/daemon_auth.py`.
- Modify: `app/package.json`, `.github/workflows/ci.yml`, `app/src/main.tsx` (extend `__kelpie`), `crates/kelpied/src/server.rs`, `crates/kelpied/src/daemon.rs`,
  `crates/kelpied/src/session.rs` (token env, sanitiser), `crates/kelpied/src/cli.rs` (send token), `crates/kelpied/src/main.rs` (hook client sends token),
  `crates/kelpie-proto/src/lib.rs` (API_LEVEL, auth frame), `app/src-tauri/src/daemon_client.rs` (app token), `app/src-tauri/src/lib.rs` (`redact_batch`),
  `app/src/lib/quick-commands.ts`, `app/src/lib/send-to-agent.ts`, `app/src/components/terminal-pane.tsx`, `docs/architecture.md`, `README.md` Testing.

## Implementation Steps
1. vitest + first tests. 2. perf gate metrics (in-page probes via `__kelpie`); 3-run baseline on release build. 3. CI job.
4. Auth frame + token issue + capability table; migrate CLI/hook client; bump API_LEVEL. 5. Sanitiser in daemon + `paste:true` from app helpers.
6. `redact` + Tauri command + TS wiring into existing outbound paths.

## Tests
- vitest green; gate catches an injected 100 ms busy loop; median spread over 3 runs reported (target ≤ 10 %).
- `cargo test`: capability matrix (session token cannot call `session.create`, `automation.save`, other session's `hook.report`); forged `KELPIE_SESSION_ID` without token rejected;
  sanitiser fixtures (`\x1b[201~` break-out, CSI, `\x9b`); redaction corpus (true positives, no mangling of normal code).
- e2e `daemon_auth.py`: agent shell calling `kelpied call session.create` fails; `kelpied worktree set` and `run report` still work; existing suites green.

## Perf gate (this phase)
- Auth check ≤ 5 µs per call; redaction ≤ 2 ms per 64 KB; no change to session throughput bench.

## Success Criteria
- [x] Baseline committed; gate catches a 10 % median regression on switch / echo / bundle / long tasks / RSS (3-run spread ≤ 2 %), and an injected 100 ms loop. Cold start and idle CPU need a quiet-machine re-baseline (follow-up in plan.md).
- [x] No RPC callable without a token; session tokens get the scripting surface but no admin method, reports bound to their own session; policy off / restricted → own session only (user decision: tiered, keeps README scripting).
- [x] No outbound TS path skips redaction; no paste (prompts, clipboard, drops, inserted paths, quick commands) can break out of bracketed paste.

## Result (2026-10-23)
Report: [completion-261023-1420-phase1-foundation](../reports/completion-261023-1420-phase1-foundation.md).
Deviations: TS redact wrapper in `ipc.ts` (no `redact.ts`); `typedText` in `typed-text.ts`; `run.approve` user-only (user decision); claimed role for sessions whose token is stripped.

## Risk Assessment
- Breaking users' own scripts that call `kelpied call` → opt-in setting + clear error naming it. Signal: support reports; response: document.
- Gate noise → medians + ratios; if spread > 10 % re-baseline with written reason.
