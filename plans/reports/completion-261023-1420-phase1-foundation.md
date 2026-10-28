# Completion — Phase 1: Foundation (perf gate, tests, daemon capabilities, outbound text safety)

Date: 2026-10-23 · Plan: [261023-0048-multi-agent-ide-roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-01](../261023-0048-multi-agent-ide-roadmap/phase-01-perf-gate-foundation.md)
Branch `rebrand/kelpie` (uncommitted; other sessions' uncommitted work in the same tree — commit path-limited).

## Delivered
| Part | What | Key files |
|---|---|---|
| 1a | vitest (`pnpm test`), perf gate (`scripts/perf/perf_gate.py`, in-page probes via `__kelpie`, medians by ratio), baseline, CI test + bundle budget + bench | `scripts/perf/*`, `app/vitest.config.ts`, `*.test.ts`, `app/src/main.tsx`, `.github/workflows/ci.yml` |
| 1b | Daemon caller auth: user token file per endpoint, session tokens (SHA-256 of per-daemon secret), tiered capabilities (scripting surface kept; admin user-only; `run.approve` user-only), policy setting "Agents can control other sessions" (`policy.json`), `restricted` sessions, claimed role when the token is stripped, Input frames user-only, `session.list` without other sessions' `env`, `meta.set` canonical own-folder check, `KELPIE_SESSION_ENDPOINT` so a session token is only used on its own daemon, `API_LEVEL` 6 | `crates/kelpied/src/auth.rs`, `server.rs`, `daemon.rs`, `session.rs`, `chat.rs`, `client.rs`, `main.rs`, `kelpie-proto/src/lib.rs`, `app/src-tauri/src/daemon_client.rs`, settings UI |
| 1c | Control-char sanitiser: daemon `typed_text` (prompt delivery), app `typedText` in every terminal paste (clipboard, drops, inserted paths, quick commands, prepared prompts) | `session.rs`, `app/src/lib/typed-text.ts`, `terminal-host.ts`, `quick-commands.ts` |
| 1d | `kelpie_proto::redact` (key formats, PEM/PGP incl. cut blocks, env-style / quoted / YAML values, bearer, URL passwords) + `redact_batch` command; applied to hand-off, Copy Context, notes / issues / browser notes sent to agents | `kelpie-proto/src/redact.rs`, `app/src-tauri/src/lib.rs`, `ipc.ts`, `send-to-agent.ts`, `terminal-pane.tsx` |
| Docs | architecture (Callers), README (Scripting, Testing table), user guide | `docs/architecture.md`, `README.md`, `docs/user-guide.md` |

## Verification
- `cargo test`: kelpie-proto 15 (+2 ignored), kelpied incl. auth matrix / own-folder / tokens / `typed_text` — pass (one `proc_tree` timing test, outside phase 1, flaky under load).
- vitest 64/64 (incl. `typed-text.test.ts`), `pnpm typecheck` clean, `pnpm i18n:check` clean (per tester rerun).
- e2e `daemon_auth.py` 32/32 (anonymous / wrong / forged refused, scripting works, admin + `run.approve` refused, hook bound to caller, raw input ignored, env stripped, policy + persistence, restricted, real CLI inside a session).
- GUI e2e on a fresh release build (tester): smoke 15/16 (60 s prompt wait under load), tabs_panes 28/28, layouts 44/44 (incl. daemon restart + resume), parity 74/74, runs 27/27; setting toggle ↔ `daemon.policy` verified; `ipc.redact` verified.
- Code review: 2 High + 7 Medium findings, all fixed ([review](code-reviewer-261023-1100-phase1-auth-safety.md)); [tester report](tester-261023-1100-phase1-auth-safety.md).

## Performance
| Measure | Result |
|---|---|
| New connection + auth (debug build) | +0.2 ms (0.49 → 0.69 ms median) |
| Hook report from a session (connect + auth + report) | 0.49 → 0.85 ms median; hook process start-up dominates (tens of ms) |
| Request on an open connection | unchanged (app's requests skip lookup and copy) |
| Keystrokes (Input frames) | one enum comparison; echo median unchanged in perf gate |
| Redaction 63 KiB (release, secret on every line) | 1.37 ms (budget 2 ms) |
| Perf gate (tester, loaded machine) | pass: bundle, switch median/p95, echo median/p95, RSS, long tasks; fail: cold start, idle CPU, bench — load-related |
| A/B daemon HEAD vs working tree, interleaved ×4 | idle CPU equal (0.1 %), attach p50 equal (~5.2 ms); bench +18 % with CPU prioritisation on (consistent), mixed −19 %…+6 % with it off → attributable to the separate CPU-priority change (unfocused sessions yield by design), not phase 1 |

## Deviations from the phase file
- TS redact wrapper lives in `ipc.ts`; `typedText` in `typed-text.ts` (testable without DOM).
- Auth tiered per user decision (README scripting kept); `run.approve` user-only per user decision; claimed role added (review M6).

## Follow-ups (in plan.md)
Quiet-machine re-baseline (cold start, idle CPU); e2e for hand-off / quick-command typing / Copy Context click; budget `bench` with CPU prioritisation in mind.

## Unresolved questions
1. Commit scope: phase-1 files only (others' uncommitted work shares `session.rs`, `chat.rs`, `bench.rs`, `README.md`, `docs/*`) — needs hunk-level staging; confirm before committing.
