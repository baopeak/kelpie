# Mobile agent control: competitive landscape (2026-10-24)

Method: ~10 searches/fetches. Primary = Anthropic docs (fetched). Rest = search summaries, vendor blogs, app stores. [U] = unverified / single vendor-biased source. Gaps: no hands-on testing, no pricing page fetches, no Reddit/GH-issue complaint mining.

## 1. Bottom line

- Big labs have shipped the core pattern: **local agent keeps running on host, phone = window, relay brokers it, push for "needs you"**. Claude Remote Control (Anthropic) and Codex-in-ChatGPT-mobile (OpenAI, May 2026) both do it. That is table stakes now; "phone shows session + replies" is NOT a differentiator.
- Both vendor offerings are **single-vendor**. Kelpie's moat = multi-agent (Claude/Codex/Gemini/OpenCode/shell) + one attention queue across all + no-account LAN/Tailscale + MIT + desktop screen takeover. Happy is the only direct multi-agent open competitor; Omnara second.
- Biggest hole in Kelpie vs field: **real push notifications when phone app is closed** (LAN/Tailscale-only server cannot reach APNs/FCM) and **structured approval UI** (tool/diff-aware, not raw terminal keys).

## 2. Competitor matrix

| Tool | Controls | Transport / relay | E2E | Client | Push | Approval UX | Input UX | Screen/desktop | License / price | Maturity / complaints |
|---|---|---|---|---|---|---|---|---|---|---|
| **Claude Remote Control** (claude.ai/code + Claude iOS/Android) | Local `claude` CLI/Desktop/VS Code; server mode up to 32 concurrent sessions | Outbound HTTPS only, polls Anthropic API, Anthropic routes over streaming conn; short-lived scoped creds; no inbound ports | NO (TLS to Anthropic, they see traffic) | Native + web | Yes, task done / needs decision; but Feb-2026 issue said no iOS push for permission prompts [U current status] | In-app permission prompts, permission-mode flag | Attach photos/files, `@` file autocomplete, multi-surface sync, QR from terminal (spacebar) | None (text session only) | Proprietary; Pro/Max/Team/Ent only, **API keys unsupported**, blocked if custom ANTHROPIC_BASE_URL | First-party, best polish. Host must stay on, process must stay alive. Trusted Devices (biometric re-auth every 18h) for orgs. [docs](https://code.claude.com/docs/en/remote-control) |
| **Claude Code on web / cloud sessions** | Cloud VM agent | Anthropic cloud | n/a | Web+mobile | Yes | Cloud sandbox | Prompt | No | Plan-gated | [docs](https://code.claude.com/docs/en/mobile) |
| **Codex in ChatGPT mobile** (May 2026 preview) | Codex running on host (Codex desktop app, macOS first; Windows "soon", conflicting reports) | OpenAI "secure relay" to desktop app, no public exposure; QR pair | [U] not stated | Native (ChatGPT app) | [U] "real-time updates" | Approve commands, switch model, start new work | Review screenshots, terminal output, diffs, test results | Screenshots only in stream; Windows computer-use remote steer claimed by a weak source [U] | All plans incl. Free/Go | Host must stay awake. [Neowin](https://www.neowin.net/amp/openai-is-bringing-codex-to-the-chatgpt-mobile-apps-on-ios-and-android/), [SD Times](https://sdtimes.com/ai/openai-announces-codex-for-mobile-devices/) |
| **Cursor** | Cloud/background agents (Ubuntu VM, own branch, PR); Feb-2026 computer use + video recording | Cursor cloud | n/a | **No native app**; cursor.com/agents in mobile browser; mobile app is open feature request | Web only [U] | Reply to agent requests, review changes | Browser | Agent's VM browser video | Paid | [forum](https://forum.cursor.com/t/is-there-a-plan-create-an-app-like-codex/161082), [morph guide](https://www.morphllm.com/cursor-background-agents) |
| **Happy** (slopus/happy) | Local Claude Code, Codex, Grok via wrapper (`happy claude`); desktop apps too | Self-hostable WS relay of encrypted blobs; multi-device sync | **Yes** (README claim; TweetNaCl per a fork [U]; no audit found) | Expo: iOS, Android, web, desktop | Not evidenced on repo page [U] (App Store version likely has) | Remote mode: wrapper restarts session in remote mode (handoff friction) | [U] voice not evidenced; session sharing (colleague can join) | None | **MIT**, free, ~24k stars | Strongest OSS rival. Weakness: wrapper restarts/hand-off, repo split across 4 repos, no releases shown. [GitHub](https://github.com/slopus/happy), [App Store](https://apps.apple.com/us/app/happy-claude-code-client/id6748571505) |
| **Omnara** (YC) | Local Claude Code + Codex (wraps native terminal) | Hosted relay | [U] | iOS, Android, web | Yes (core pitch: push, answer questions, view git diffs) | Answer agent questions, approve next steps | Diff view, prompt | None | Apache-2.0 [U]; free 10 sessions/mo, $20 unlimited (YC page) vs $9 (AlternativeTo) conflict | App Store 4.3 / 18 reviews: small. [HN launch](https://hn.svelte.dev/item/44878650), [YC](https://ycombinator.com/companies/omnara) |
| **VibeTunnel** | Any terminal in browser (not agent-aware) | Local server + tunnel (Tailscale/ngrok) | no | Web, Mac app | None (Omnara pitch: lacks push, clean UIs) | Raw keys | Terminal | Terminal only | OSS | Maintenance status unknown [U] (search found nothing) |
| **Termius / Blink + mosh + tmux** | Any CLI via SSH | SSH/mosh (UDP, survives roaming) | SSH crypto | Native | None (DIY ntfy hooks) | Raw keys | Phone keyboard on a TUI = painful | Terminal only | Termius freemium; Blink $19.99/yr [per guide] | The DIY baseline. mosh can't forward ports. [harper.blog](https://harper.blog/2026/01/05/claude-code-is-better-on-your-phone/) |
| **Conductor** | Local agents in worktrees (Claude/Codex/Cursor/OpenCode) | none | n/a | macOS only | n/a | Diff review is its strength | n/a | n/a | [U] | **No mobile** [U agentsroom, competitor blog] |
| **Crystal** | Worktree desktop app | none | n/a | desktop | n/a | n/a | n/a | n/a | OSS | Marked deprecated [U] |
| **Claude Squad** | tmux+worktree TUI (Go), mac/Linux | none | n/a | TUI | n/a | n/a | n/a | n/a | OSS | No mobile [U] |
| **Sculptor** (Imbue) | Docker-sandboxed Claude/Codex | none | n/a | desktop | n/a | n/a | n/a | n/a | [U] | No mobile [U] |
| **DIY stopgaps**: claude-remote-approver (ntfy.sh push of permission prompts), Forge Remote (iOS app approving Claude permission requests with risk level) | Claude hooks | ntfy / own | partial | iOS | Yes | Approve/deny + risk level | - | - | OSS / app | Evidence of unmet demand: people hack push+approve themselves. [repo](https://github.com/yuuichieguchi/claude-remote-approver), [Forge](https://apps.apple.com/dk/app/forge-remote/id6760141378) |

Orchestrators (Conductor/Crystal/Squad/Sculptor) = desktop-only; Kelpie already competes there with worktrees/review queue. Their lack of mobile = open lane.

## 3. Top 10 valued features/patterns

| # | Pattern | Who has it | Kelpie today | Verdict |
|---|---|---|---|---|
| 1 | **Push when agent needs you / is done, app closed** | Claude, Codex, Omnara, Happy, DIY ntfy | Gap: LAN/Tailscale server cannot reach APNs/FCM. Status already exists (hooks) | **COPY, top priority.** Options: PWA Web Push (needs HTTPS + service worker; iOS requires installed PWA), ntfy self-host/ntfy.sh topic per device, or Tauri mobile + optional tiny relay. Rank: Web Push via Tailscale HTTPS cert first, ntfy as fallback |
| 2 | **One-tap structured approve/deny** (tool name, command, diff, risk level) | Claude, Codex, Omnara, Forge | Hook gives "needs you"; phone sends keys | **COPY.** Parse prompt via hook payload/ACP (chat relay) into Approve / Always / Deny buttons; fall back to key buttons for non-structured agents |
| 3 | Session list with live status, sorted by needs-attention | Claude, Omnara, Happy | Has it + attention queue | Keep; this is Kelpie strength across all agents |
| 4 | QR pairing, no account | Claude (QR), Codex (QR), Happy | Has QR pair | Parity. Add "revoke" visible on phone |
| 5 | Diff / test-result review on phone | Omnara, Codex, Cursor web | Desktop review queue exists; phone? [U] | **COPY light**: read-only diff + "approve/send back" |
| 6 | Voice input | Happy [U], Claude/ChatGPT app native dictation | Has `kelpie-stt` desktop | **DIFFERENTIATE**: phone browser mic -> daemon STT; or rely on OS keyboard dictation (free). Don't build until asked |
| 7 | Attach photo/file from phone | Claude RC | Unknown | Cheap copy: upload to host dir, pass `@path` |
| 8 | E2E encryption through relay | Happy | Not needed for direct LAN/Tailscale (WireGuard already E2E) | **Skip** unless adding relay; Tailscale is the E2E story. Say so in docs |
| 9 | Resilient reconnect / resume (mosh-like) | mosh, Claude RC | WS reconnect [U] | Must be seamless: replay missed output from ring buffer on reconnect |
| 10 | Desktop/GUI view+control when needed | Cursor VM video only; Codex screenshots; none for local host | Researched, not built | **DIFFERENTIATE**: nobody offers "agent stuck on a GUI/OAuth dialog -> tap, see screen, click". Biggest unique feature, see sibling reports |

## 4. Traps

- **Wrapper/restart hand-off** (Happy): starting session in "remote mode" restarts it. Kelpie owns the PTY so it should never need this. Keep that advantage.
- **Relay + accounts + subscription** (Omnara/Anthropic): cost, trust, availability. Kelpie's no-cloud story is the selling point; do not build a hosted relay v1.
- **Raw terminal on phone as the main UX** (Termius/Blink/VibeTunnel): the very thing Omnara/Happy exist to fix. Terminal view = fallback only.
- **Per-vendor lock-in** (Claude RC rejects API keys and custom base URLs; Codex needs its own app): users on gateways/API keys are stranded = Kelpie opening.
- **E2E-crypto marketing**: Happy's selling point, no audit found [U]. Building custom crypto over Tailscale = cost, no gain.
- **Native app before PWA push proven**: store review, signing, two codebases. Validate Web Push on PWA first.
- **"Always on" claims**: host must stay awake/process alive for all of them; Kelpie keep-awake exists, but lock screen kills desktop capture.
- **Over-notifying**: every hook event pushing = muted app. Push only needs-you + done-after-long-run; coalesce.

## 5. Recommendation (ranked)

1. **Push notifications** (Web Push over Tailscale HTTPS, ntfy fallback). Without it Kelpie phone is pull-only, loses to every rival.
2. **Structured approval cards** with Approve/Deny/Always, command + diff preview; keys fallback for dumb agents.
3. **Cross-agent attention inbox as hero screen** (copy none, differentiate: no competitor unifies Claude+Codex+Gemini+shell locally).
4. **Screen takeover on demand** from an agent card ("agent blocked on GUI -> view/control"). Unique.
5. Phone diff view + file/photo attach + reconnect replay (parity polish).
6. Defer: voice (OS dictation suffices), E2E relay, native store app.

Positioning line: "Claude's Remote Control, but for every agent, on your own network, with a screen when the agent needs hands."

## Pick (criteria: modern / performance / user-friendly)

Scores 1-5, my judgment from docs + secondary sources, not hands-on. Performance is inferred from architecture (no latency benchmarks found) [U].

| Product | Modern | Perf | User-friendly | Total /15 | Why |
|---|---|---|---|---|---|
| Claude Remote Control | 5 | 4 | 5 | 14 | Native app, QR, multi-surface sync, outbound-only, push, files/photos. Perf: Anthropic relay hop [U] |
| Codex in ChatGPT mobile | 5 | 4 | 4 | 13 | Native, secure relay, screenshots/diffs/tests streamed. Preview, macOS-first |
| Omnara | 4 | 3 | 4 | 11 | Mobile-first push+diff UI; hosted relay, small user base |
| Happy | 4 | 3 | 3 | 10 | OSS multi-agent, E2E; wrapper restart handoff, fragmented repos |
| Cursor web agents | 3 | 3 | 3 | 9 | Mobile browser only, cloud-only |
| Blink/Termius + mosh + tmux | 2 | 5 | 1 | 8 | Best raw link resilience (UDP), worst UX on a phone keyboard |
| VibeTunnel | 2 | 3 | 2 | 7 | Not agent-aware, no push [U] |
| Conductor/Crystal/Squad/Sculptor | n/a | n/a | n/a | - | No mobile |

**Closest to best-in-class: Claude Remote Control** (outbound-only link, QR pairing, push, file attach, sync across devices, zero config). Its only real weaknesses are lock-in and no screen view, which is exactly where Kelpie can win. Among multi-agent options, Happy is the nearest, but its restart handoff and fragmentation make it a worse experience than Kelpie can offer.

### 5 patterns Kelpie must copy to beat them

1. **Zero-friction attach**: no wrapper, no restart. Every Kelpie session is already remote-ready; one QR pair, then all sessions appear (Claude RC's QR flow without Happy's handoff).
2. **Push that works with the app closed**: Web Push (PWA over Tailscale HTTPS) with ntfy fallback; only "needs you" and "long run done", coalesced (Claude/Codex/Omnara).
3. **Structured approval cards**: command, diff, risk hint, Approve / Always / Deny in one tap; key buttons only as fallback (Codex, Omnara, Forge).
4. **Instant, resumable live view**: delta stream, replay missed output from ring buffer on reconnect, auto-reconnect with no spinner (mosh-grade resilience, Claude RC sync).
5. **Compose-first input**: bottom prompt composer, photo/file attach as `@path`, OS dictation; terminal view as secondary tab (Claude RC attachments, Omnara).

Beat-them move on top of the copies: unified cross-agent attention inbox plus on-demand screen takeover. Neither exists in any competitor.

## Unresolved

- Does Claude RC iOS now push on permission prompts (issue 29438 status)? Check issue + `code.claude.com/docs/en/mobile` push page.
- Happy push/voice/real feature list, release cadence, audit status; Omnara real pricing + license.
- VibeTunnel/Crystal/Conductor/Sculptor current status (only competitor-blog evidence).
- Codex mobile Windows support and relay encryption details.
- iOS Web Push constraints for a LAN/Tailscale-hosted PWA (needs trusted HTTPS origin; verify before committing).
- User complaint mining (Reddit/GH issues) not done.

Status: DONE_WITH_CONCERNS
Summary: Report written; covers Claude RC, Codex mobile, Cursor, Happy, Omnara, VibeTunnel, SSH tools, orchestrators, with ranked copy/differentiate plan.
Concerns/Blockers: Several rows rest on vendor-biased or single-source evidence tagged [U]; no hands-on testing or complaint mining within the search budget.
