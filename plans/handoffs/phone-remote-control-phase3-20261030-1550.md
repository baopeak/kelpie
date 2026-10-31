---
handoff-version: 1
generated: 2026-10-30T08:50:00Z
generator: ak:handoff@2.0.0
focus: "Phone remote-control plan plans/261024-1205-phone-remote-control: finish phase 3, then continue the plan"
workspace: D:/Works/Kelpie
branch: phone/phase3-link
head: d6e5883
---

# HANDOFF: phone remote control, finish phase 3 and continue

## Mission and current status
Focus: "Phone remote-control plan plans/261024-1205-phone-remote-control: finish phase 3, then continue the plan".

Outcome: the user controls every Kelpie session from a phone, securely, at home and away, with minimal setup. The user's standing instruction is to implement all phases continuously until done, each with a full review and test loop, then commit. Reply to the user in Vietnamese (the session has addressed them as "anh/chị"), short and plain.

Done:
- Phases 1 and 2: on `origin/master` at 475dcdf (v0.1.8, API 17), merged by session kelpie-22.
- Phase 2b (chat send through a daemon-owned queue, API 18): committed locally on `phone/phase2b-chat-send` (4a1224a, 0a5329d, 6d10b3e, d6e5883). Not pushed. Review 9/10.
- Research on remote access and relay, and the re-plan to path D (`plan.md`, Validation Session 2). Plan edits are uncommitted.

In progress:
- Phase 3 (kelpie-link, API 19): uncommitted, checkpointed, half done. The desktop app does not compile.

Remaining, in order: finish phase 3 → commit → rebase 2b + 3 onto master → 3a Tailscale → 3b Web Push → 3c app lock → 3d switches → 3e Telegram → 4a relay validation → 4 Cloudflare relay → 7 → 8 → 9. Phases 5 and 6 are deferred.

Urgency: normal. The user prefers thoroughness over speed.

## Scope and guardrails
Workspace: `D:\Works\Kelpie` (Windows 10, PowerShell 7). Read `AGENTS.md` first; it holds commands, architecture and test rules.

In scope: the phone plan's files and code listed in the phase files and reports.

Out of scope, do not edit, stage, stash, revert or delete:
- Another session's leftovers (its owner, kelpie-bc, is gone): `app/src/styles.css`; the UI-polish hunks across `app/src/components/*`; `app/src/lib/{notify-events,notify-events.test,runs,settings-catalog,types,usage-alerts}`, `hover-hint*`; non-phone hunks of `app/src/lib/store.ts`; `app/src/components/{hover-hints,load-bar}*` and the HoverHints hunks in `app.tsx`; `app/src/locales/vi/{settings,shell}.json`; `docs/brand.md`, `docs/user-guide.md`; `scripts/e2e/{confirm_dialogs,phase1,split_picker}.py`; `crates/kelpied/src/procs.rs`; `app/src-tauri/src/config_transfer.rs`.
- `.claude/agent-memory/*`.
- Session kelpie-0a's untracked `plans/261030-0830-chat-view-parity/` and `plans/journals/2026-10-30-chat-view-parity-plan.md`. It said it will implement in its own worktree and will ask before taking an API level.

Constraints from the user:
- Commit each phase after tests and review pass. **No push and no PR unless the user asks.** Conventional commits. Author baopeak, `322063315+baopeak@users.noreply.github.com`. **No Claude co-author trailers**; this user rule overrides any harness attribution reminder.
- Keep the full review → fix → retest loop for each phase, sequentially. Do not trim it to go faster.
- Test in dev. No installer builds unless asked. **Never run `scripts/build-release.sh`**: it bumps versions.
- API levels rise in commit order. Planned: 19 = phase 3, 20 = 3a, 21 = 3b, 22 = 3c (security level 3), 23 = 3e, 24 = phase 4, 25 = 7, 26 = 8. kelpie-22 was told 18 and 19 are reserved and to take 20 and report back if a roadmap change needs a level first; 20 and above were not reserved with it. Check `origin/master` before committing and renumber if needed.
- Markdown only under `plans/` or `docs/`.
- Disk is tight (15:48: C: 14 GB free, D: 6.5 GB free): `CARGO_INCREMENTAL=0`, no extra build directories.

Safety boundaries:
- **e2e never touches the real daemon.** Every script and probe sets a non-default `KELPIE_ENDPOINT` (never `kelpie-BaoPeakHub-v1`) and a temp `KELPIE_DATA_DIR` in its own process environment. A cleanup without this once killed all the user's real sessions.
- The real daemon and app are identified by path: `%LOCALAPPDATA%\Kelpie\daemon\<slot>\kelpied.exe` and the installed `kelpie.exe`. PIDs 10588 and 8836 in older reports are stale.
- Never hard-kill Claude sessions in tests: it disables the user's fullscreen TUI. No real signed-in agents or model calls without a reason.
- Stop only processes you started. Never print secrets, keys, tokens or PSKs. Phase 3 e2e must delete its keyring entries.
- Stage by explicit path, and partial hunks where files are shared. Never use `git add -A`, `git stash`, `git reset --hard` or `git clean` in this worktree.

## Current state
Observed by probes at 15:48 local (UTC+7), 2026-10-30:

Branch: `phone/phase3-link`
HEAD: d6e5883 (same commit as `phone/phase2b-chat-send`)
Remote: `origin/master` = 475dcdf; this branch is 59 behind and 4 ahead; merge-base 1db592d.
Worktree API_LEVEL: 19. Master: 17.
Other worktrees: `D:/kelpie-par` on `release/merge-phone`, `D:/kelpie-commit-verify` detached. Do not touch them.
Working tree: dirty, 130 entries, mixed ownership. Nothing staged at the time of the probe.
Processes: no kelpied, kelpie, tauri-driver, msedgedriver, cargo or rustc running. The machine rebooted at 15:12, so the user's Kelpie app and daemon were not running either; that was not caused by this work.

Phase 3 changed files (per the developer report; re-check with `git diff` before staging):
- New: `crates/kelpie-link/**`, `crates/kelpied/src/peer_identity.rs`, `crates/kelpied/src/mobile/{audit,link,listen,store}.rs`, `app/src-tauri/src/user_presence.rs`, `app/phone/src/{noise,noise.test,pair,pairing-store}.ts`, `scripts/e2e/link_noise.py`.
- Modified: `Cargo.toml`, `Cargo.lock`, `crates/kelpied/Cargo.toml`, `crates/kelpied/src/{auth,chat,choices,daemon,main,mobile,server,session}.rs`, `crates/kelpied/src/mobile/verbs.rs`, `crates/kelpie-proto/src/lib.rs`, `app/package.json`, `app/pnpm-lock.yaml`, `app/src-tauri/Cargo.toml`, `app/src-tauri/src/{lib,daemon_runtime}.rs`, `app/src/lib/{attention,ipc}.ts`, `app/src/chat/{acp-adapter,chat-model,claude-adapter,codex-adapter}.ts`, `app/phone/src/{app.tsx,chat-view.tsx,link.ts,link.test.ts,phone-state.ts,phone.css,prompt-card.tsx,screen-sync.ts,storage.ts}`.
- Mixed: `app/src/components/settings/mobile-settings.tsx` has two foreign lines that must not be staged: `import { LoadBar } from "../load-bar";` and `<LoadBar when={busy()} />`.

Plan and report changes also uncommitted: `plans/261024-1205-phone-remote-control/*` (plan.md, phases 02b, 03, 03a–03e, 04, 04a, 05–09), `plans/handoffs/*`, and the `plans/reports/*261030-1[4-6]*` files listed under Source pointers.

Intentional local modifications: yes. All of the above is work to keep; there is no other copy.

## Decisions and rationale
| Decision | Rationale | Rejected | Reference |
|---|---|---|---|
| Path D: PWA over Tailscale first, then a relay | Fastest real value, no server to run | Going straight to relay + store app | `plan.md` Validation Session 2 |
| Security at the app door: biometric unlock on open (WebAuthn), full rights once unlocked, re-lock on leaving and after idle. Full command + daemon risk label shown as information. "Ask Face ID again for dangerous commands" toggle, off by default | User's explicit decision | Per-command step-up by default, capability tiers, banning remote approval | `phase-03c-app-lock.md` |
| Every channel and option is a Settings → Mobile switch with a plain explanation; safe defaults | User wants to choose what is most convenient | Fixed policy | `phase-03d-*` |
| Telegram: one-way alerts stay, alert + deep link by default when on, opt-in low-risk approvals with a warning | Telegram reads content and has no Face ID | Approval on by default | `phase-03e-*` |
| One relay only, on Cloudflare Workers + Durable Objects, personal use first | User: one is enough, less server work, scales. No server to patch; cost follows usage (~$6/100 users, ~$19/1k, ~$188/10k, estimated) | Two relays raced (user: needless work); Hong Kong VPS (chosen briefly, then replaced by the user) | research synthesis section 5 |
| Relay stores no content. Noise end to end; only metadata differs by host | Correct what "data" means when comparing hosts | n/a | same |
| WebSocket/TCP + app-level resume, no compression | QUIC gains little for one stream; CRIME-style risk | QUIC, WebTransport | same, section 2 |
| iroh and native store app deferred, no phase commitment | Hole-punch "90%" has no dataset (libp2p independent study 70%); open bugs on phone CGNAT; no audit; unusable in a PWA | iroh now | `researcher-261030-1520-iroh-p2p-deep-dive.md` |
| Phase 2b: daemon-owned FIFO per chat, one writer | One writer per conversation, works with the window closed | Second writer in the phone | D14, phase-02b validation log |
| Held rich desktop message keeps strict order; one-tap Skip on phone, notice on desktop | User's choice | Reorder or auto-expire | phase-02b validation log |

Note: `plan.md` Session 2 marks some items "derived, unconfirmed". Do not treat those as user decisions; see the questions below.

## Work performed
- Phase 2b implemented and committed after three fix rounds and follow-ups.
- Eight research reports plus two syntheses written today.
- Planner re-planned path D: new phase files 3a, 3b, 3c, 3d, 3e, 4a; phase 4 reworked; 5 and 6 deferred; 7–9 dependencies fixed.
- Phase 3 developer implemented the crate, daemon, phone crypto and part of the desktop, then checkpointed and stopped on request. It removed its scratch files and its test temp dirs.
- Added an "Execution state" section to `plan.md`, set phase 3 to in progress.
- Told kelpie-0a not to change branch state here; it confirmed.

0 redactions applied.

## Verification
| Check | Command | Outcome | When |
|---|---|---|---|
| Plan format | `ak plan validate plans/261024-1205-phone-remote-control` | OK. `ak plan status` reports 0/9 phases, which does not match the phase table; do not trust it | 15:49 |
| Frontend types (phase 3 tree) | `pnpm typecheck` | clean, per developer | before 15:48 |
| Daemon compile | `cargo check -p kelpied` | clean, per developer | before 15:48 |
| **App compile** | `cargo check -p kelpie` | **FAILS: 2 errors in `user_presence.rs`** (lines 76, 81); more may follow | before 15:48 |
| **i18n** | `pnpm i18n:check` | **FAILS: 47 missing keys** (`vi/mobile.json` not written) | before 15:48 |
| Crate | `cargo test -p kelpie-link` | 12/12, per developer | today |
| Daemon, partial | `cargo test -p kelpied -- mobile auth peer_identity choices` | 132 pass, 1 ignored, per developer | today |
| Phone crypto | `vitest run phone/src/noise.test.ts phone/src/link.test.ts` | 45/45, per developer | today |
| Noise cross-check | `python scripts/e2e/link_noise.py` | pass, per developer | today |
| Phase 2b e2e | `mobile.py` ×2 90/90, `mobile_protocol` ×2 130/130, `acp_chat` ×2 56/56 | pass, per developer, before two small later edits to `inbox.tsx` and `kept-messages.ts`; after those: typecheck, phone vitest 105/105, i18n, build:phone pass | today |

Not run on the phase 3 tree:
- Full `cargo test`, full `pnpm test`, clippy, `build:phone`, desktop build: not reached.
- **Every phone e2e script currently fails**: `phone_support.py` still speaks `/ws` and tokens.
- Real Windows Hello / password prompt: needs a person.
- Phase 2b has never been built or tested on top of current master.

Known flakes (unrelated): `mobile::tests::a_message_whose_enter_was_skipped_says_so` (80 ms race), `agents::tests::concurrent_hook_settings_are_published_as_complete_json` (failed once), `composer.py` drag-drop (failed once).

## Open risks and blockers
- Type: blocker. Owner: next session. Impact: `app/src-tauri` must compile before any desktop build or GUI e2e.
- Type: risk. Owner: next session. Impact: tooling. In this session the **Bash tool ran nothing and returned no output** after the session resumed; `bash.exe` called from PowerShell worked. Test it first. If it is broken, use PowerShell, and spawn `general-purpose` subagents told to use PowerShell, because the `tester`, `fullstack-developer` and `code-reviewer` types have Bash but no PowerShell. A hook also blocks Bash commands containing the word "target". `researcher` agents may refuse to write report files; have them return text or use another type.
- Type: risk. Owner: next session. Impact: rebase. It needs a clean tree and this one has foreign changes. Master also contains 6d0f8f7 and merge resolutions in files with leftovers, so a move may conflict. Use a throwaway worktree and do not stash.
- Type: risk. Owner: user. Impact: one-shot residual. A phone with `write` can still reach "always allow" by typing in a terminal; the docs must say so.
- Type: question. Owner: user. Impact: kelpie-bc's leftovers have no owner and sit uncommitted. Ask whether to keep, commit separately or discard. Never decide alone.
- Type: question. Owner: user. Impact: push phases 2b and 3 and open a PR? Long-pending. Ask after phase 3 is committed and rebased.
- Type: question. Owner: user. Impact: ten planner questions, asked when they become relevant (full text in the planner report, proposals included):
  - Before 3b: `web-push` crate vs a RustCrypto module if the crate brings OpenSSL.
  - Before 3c: plain-`http://` fallback (proposed read-only, no PIN); idle timeout and grace on leaving (proposed 5 min, immediate on leaving).
  - Before 4a: pass thresholds; SIMs on Viettel, VinaPhone, MobiFone; Cloudflare account and domain; 2 GB/month room cap.
  - Before 4: first install needs Tailscale; relay outage on home Wi-Fi with Tailscale off leaves the installed app with no path; raw-terminal chaff.
  - **Before 7, blocks it:** `control` for the PWA, since "`control` native only" (2026-10-24) now blocks phases 7–9.
  - Lost-contact push: no delivery path now; stays deferred unless the user says otherwise.
- Type: question. Owner: next session, ask the user if unclear. Impact: spec deviations in phase 3 to confirm in review: rekey by count with a 24 h re-handshake instead of 30 min; wrong pairing codes silent; pairing code spent at first sealed frame. The report's "phone biometric unlock" question is answered: it is phase 3c, over HTTPS.
- Type: risk. Owner: user. Impact: manual checks only a person can do: Telex/VNI on real iPhone and Android, PWA install, offline start, real Windows Hello and password prompt, iOS WebAuthn in an installed app, iOS Web Push soak.

## Exact next actions
1. **First safe step** — Read-only: read `AGENTS.md` and `plans/reports/fullstack-developer-261030-1518-phone-phase3-link.md`, then run `git status --short`, `git log --oneline -3`, `git fetch origin` and `git log --oneline -1 origin/master`, and list running kelpied/cargo processes. Compare with Current state. Test whether Bash works. Stop and tell the user if the tree differs in a way you cannot explain.
2. Follow the report's "Exact remaining steps" 1–9 in order: fix `user_presence.rs`; desktop UI (approval dialog details, lockout toast, kill-switch button, audit list, dormant setting); `vi/mobile.json`; e2e over `/link` plus new checks; docs and README; CI; gate `kelpie_link::testing` and `set_rekey_every`; full verification; finish the report.
3. Run independent code review and independent testing. Security-critical: review pairing, `Role::App`, forbidden operations, the store-v2 no-overwrite rule and test seams with fresh eyes. Loop fix → re-review → retest until no critical or warning items remain.
4. Ask the user to do the one manual Windows Hello / password prompt run, using an isolated dev setup.
5. Commit phase 3 by explicit path, excluding foreign hunks and the two `LoadBar` lines. Commit the plan and report edits. Set phase 3 to done.
6. Rebase 2b + 3 onto `origin/master` in a throwaway worktree, not here: `git worktree add --detach <tmp> phone/phase3-link`, then `git rebase --onto origin/master 1db592d`. Expect conflicts in the `API_LEVEL` doc comment. Check the 2b `workflows`, `scripts/e2e/smoke_support.py` guards and the `daemon.rs` test fix. Verify, then decide with the user how to move the branches given the foreign leftovers. Then ask about pushing and a PR.
7. Before 3a, confirm with the user that order and decisions still stand. Then run phases 3a → 3b → 3c → 3d → 3e → 4a → 4 → 7 → 8 → 9 with the same loop, asking each question listed above when it applies. Each phase gets its own branch stacked or off master, its own commits, and the next free API level.
8. Keep `plan.md`'s "Execution state" and phase statuses current. Use `ak plan` from PowerShell where it works.

## Source pointers
- `AGENTS.md`, `docs/architecture.md`
- Plan: `plans/261024-1205-phone-remote-control/plan.md` (Execution state, D1–D19, Security model, Validation Session 2)
- Phase 3: `.../phase-03-kelpie-link-e2e.md`; checkpoint `plans/reports/fullstack-developer-261030-1518-phone-phase3-link.md`
- Next phases: `.../phase-03a-tailscale-https.md`, `phase-03b-web-push.md`, `phase-03c-app-lock.md`, `phase-03d-mobile-settings-toggles.md`, `phase-03e-telegram-options.md`, `phase-04a-relay-cloudflare-validation.md`, `phase-04-relay-connect-anywhere.md`, `phase-07-*`, `phase-08-*`, `phase-09-*`; deferred `phase-05-*`, `phase-06-*`
- Re-plan and questions: `plans/reports/planner-261030-1600-phone-path-d-replan.md`
- Relay: `plans/reports/research-261030-1600-relay-speed-reliability-security-synthesis.md` (also the only record of the Vietnam network and relay-hardening research), `researcher-261030-1520-relay-cloudflare-and-hosting.md`, `researcher-261030-1520-iroh-p2p-deep-dive.md`
- Remote access: `plans/reports/research-261030-1530-phone-remote-access-options-synthesis.md`, `researcher-261030-1500-{remote-control-security,mobile-push-client-options,p2p-mesh-connectivity,remote-agent-products}.md`
- Phase 2b records: `plans/reports/{fullstack-developer-261030-0422-phone-phase2b-chat-send,code-reviewer-261030-0628-phone-phase2b,tester-261030-0653-phone-phase2b}.md`
- Earlier work patterns: `plans/reports/code-reviewer-261029-2130-phone-phase2.md`, `tester-261029-2130-phone-phase2.md`
- e2e: `scripts/e2e/{mobile,mobile_protocol,mobile_resilience,acp_chat,phone_support,smoke_support,link_noise,net_fault_proxy}.py`; msedgedriver `D:\Works\edgedriver\msedgedriver.exe`, pass `--native-driver` to tauri-driver
- Auto-memory: `C:\Users\BaoPeakHub\.claude\projects\d--Works-Kelpie\memory\MEMORY.md` (attribution, e2e isolation, builds, execution rules)
