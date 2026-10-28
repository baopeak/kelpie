# Completion — Phase 11: Event-triggered automations

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-11](../261023-0048-multi-agent-ide-roadmap/phase-11-event-automations.md)

## Delivered
- **Triggers**: file glob, agent turn finished in the project, commit (HEAD / branch tip change; detached HEAD and `refs/kelpie` never count), PR checks failed (app-side poll every 5 min, only for projects with a PR automation), worktree finished. File, PR and finish triggers need the window open; the settings page says so. Changes made while the triggered session is working or waiting are ignored.
- **Actions**: agent prompt (safe-send path), a saved quick command typed only into a session the automation started (idle, nothing typed for 30 s), a shell command spawned without a shell; event data only as whole argv entries or `KELPIE_*` env.
- **Command safety** (allowlist): shells, interpreters, remote / elevated runners, build tools, package managers and program-running wrappers get no event data unless the form is "interpreter [safe flags] <script file>" (`powershell -File`, `bash script`, `python script`) or a `.cmd`/`.bat` called directly with no `%`/`!`; cmd, ssh, wsl, sudo and the like never. Programs classified by given, resolved and canonical name (8.3 names, trailing dots / spaces); `.js`/`.vbs`/`.hta`/`.lnk`… never resolve as programs. Approval hash covers the resolved program, script contents and file arguments (an edited script pauses the automation); programs with no rule show a translated warning in the approval dialog. Only the app connection creates, edits, approves or fires automations.
- **Guards**: 1 run per trigger per 60 s, daily cap, chain limit 3, global pause chip (also stops clock runs and PR polling), dry run (disabled below API 15). Whole process tree killed on timeout (Job object).
- **Storage**: `version` field, daemon-side `.bak` before migration, damaged file → `.corrupt-<ts>`, saves refused and an inbox error until the app resolves it; every save also writes `automations.json.v2` so a downgrade can be restored. API level 15.

## Verification
- cargo kelpied 264 (allowlist: 56 refused / 22 allowed forms, wrappers, build tools, npm run, 8.3 names, extension pin, real `.cmd` / `.ps1` / `.sh` with hostile names; scoping for every param shape; corrupt file never overwritten; migration; commit detection; caps; chain), kelpie 169, kelpie-proto 19; clippy `-D warnings` clean.
- vitest 451, typecheck, i18n 0 missing; entry bundle 323.6 kB gz.
- GUI e2e `event_automations.py`: 86/86 (twice by the developer; again on the merged code); regression on the merged code: agent_setup 84, spec_panel 124, best_of_n 91, finish 72, notify 74, context_cost 26, attention 12/12.
- [Review](code-reviewer-261025-1700-phase11-event-automations.md): High (Windows shell pressure), 11 Mediums, B1–B3, F1, F3, F4 and Lows fixed over five rounds; final: no blocking items.

## Not done / follow-ups
- Allowlist is deliberately small (`deno run`, `python -u x.py`, `bash -x x.sh`, `npm run … {file}` refused); unknown programs rely on the approval warning.
- Downgrade is mitigated by the `.v2` restore, not prevented; guard-dropped events are not replayed; release-mode match time unmeasured (debug 0.44 ms for 100 triggers).
