# Remote control of coding agents/terminals from phone: product survey

Date: 2026-10-30. Researcher. Sources fetched 2026-10-30 unless noted. [U] = unverified / secondary-only / vendor-marketing claim.
Source weights: official docs > repo READMEs > HN threads > vendor/competitor blogs (low).

## 1. Comparison table

| Product | Transport | Relay operator / self-host | E2E | Pairing | Push | Account | Price | Notes |
|---|---|---|---|---|---|---|---|---|
| Claude Code Remote Control | Outbound HTTPS to Anthropic API, host polls; no inbound ports | Anthropic only; no self-host; blocked with custom ANTHROPIC_BASE_URL, ZDR, HIPAA | NO. TLS only. Transcript stored on Anthropic servers | Same claude.ai login; QR/URL; optional Trusted Devices (per-device keypair, 18h re-auth via passkey/biometric, beta) | Claude app (mechanism undocumented) | claude.ai Pro/Max/Team/Ent; no API key | Included in plan | Short-lived scoped credentials. Max 32 sessions/server. Laptop must stay on |
| Codex Remote (ChatGPT app) | Host outbound TLS to OpenAI relay (GA replaced beta direct WS) [U: 3rd-party guide] | OpenAI; no self-host (SSH hosts alternative) | NO documented; "TLS both hops" [U]; one guide mentions Noise only in title | Desktop app QR, confirm acct+workspace (SSO/MFA); headless `codex remote-control pair` short-lived code | ChatGPT app completion/approval | ChatGPT acct, same workspace | Paid ChatGPT tiers | Setup only from desktop app (macOS/Win). No documented revoke of lost phone |
| GitHub Copilot CLI remote | Details undocumented (streams session activity) | GitHub | Unknown | QR/link into GitHub Mobile; Agents tab | GitHub Mobile | GitHub | Copilot plans; Biz/Ent admin must enable | GA 2026-05-18 |
| Happy (slopus/happy) | WebSocket (Socket.IO) via relay server | Hosted by Bulka LLC; MIT, Dockerfiles, self-host supported | YES. Content: AES-256-GCM / NaCl secretbox; DEKs wrapped via tweetnacl.box (X25519). Server sees IDs, tags, versions, timestamps | CLI shows QR, scan in app | Own CF Worker -> APNs/FCM (content visibility to push server [U]) | No email; GitHub OAuth optional; server stores OAuth/vendor tokens server-encrypted, NOT E2E | Free (donation IAP $19.99 [U]) | Claude, Codex, Grok, Gemini via Antigravity, ACP agents. No public audit found. Windows sync bugs (HN 2025) |
| Omnara | Relay + cloud sessions | Omnara. Original open-source (Apache) tree archived 2026-01-19 [U], pivoted to "managed agent platform" | Not specified | n/a | Apple Watch, mobile | Yes | Conflicting ($9/$20/free) [U] | Treat as a cautionary tale: relay-startup pivot/abandonment risk |
| VibeTunnel | None built in: user adds Tailscale / ngrok / Cloudflare quick tunnel | User's choice; MIT | Only if Tailscale/WireGuard | Password/PAM/SSH key; "no auth" mode exists | None | No | Free | macOS only host; native iOS app WIP; browser UI |
| Termius | SSH (+mosh) direct, or via VPN | None; vault sync is Termius cloud | SSH | SSH keys | App-level only | Optional (sync) | Free; Pro ~$10/mo [U] | Historical "session dies on phone sleep" |
| Blink Shell | SSH + mosh native (iOS/iPadOS only) | None | SSH | SSH keys | No true push (escape-seq only; per Moshi, competitor [U]) | No | Free; Blink+ $19.99/yr | Best mosh |
| Moshi | mosh/SSH over Tailscale; webhook to getmoshi.app for pushes | Moshi hosts webhook endpoint (content handling undocumented) | SSH/Tailscale for terminal; webhook push NOT E2E [U] | SSH keys; webhook token pasted in host config | APNs, Live Activities, Watch, via moshi-hook | No | Free; Pro $7.99/mo, $69.99/yr, $199 lifetime | Vendor-only claims |
| Warp / Oz | Cloud agents; "remote control" publishes session to Warp cloud | Warp | Unknown | Warp account | Yes | Yes | Paid tiers | Phone = browser/app control of cloud agents; native app unclear [U] |
| Cursor | Cloud background agents (Cursor VMs), not your machine | Cursor | n/a | Cursor account | Yes [U] | Yes | Pro $20+, usage metered | Not control of local PC; no evidence found of local-machine phone control |
| Zed remote | Local ssh binary, control master multiplex | None (vendor-relayed mode removed v0.157) | SSH | SSH | None | No | Free | Desktop-only; no phone |
| VS Code `code tunnel` / Dev Tunnels | Host outbound to Azure relay; web-forwarding via `*.devtunnels.ms` | Microsoft; no self-host | TLS terminated at relay ingress (MS CA cert), so relay can see plaintext on web-forward path. Marketing says AES-256-CTR for tunnel data [U] | Device-code login, GitHub or Microsoft; tunnel private to creator | n/a | GitHub/MS | Free | Tokens scoped client/host/manage, 24h expiry; anti-phishing interstitial; `--allow-anonymous` footgun |
| JetBrains Gateway / Code With Me | SSH (Gateway); CWM via JB relay | CWM public relay shutting down Q1 2027 (sunset announced 2026-03) | n/a | n/a | n/a | JB account | n/a | Relay-dependent product killed by vendor |
| Chrome Remote Desktop | WebRTC; Google signaling; direct/STUN/TURN | Google | Encrypted all 3 modes per Google docs [U via blog]; PIN checked client-side (SRP-like) [U] | Google acct + >=6 digit PIN | n/a | Google | Free | Screen-sharing, not agent-aware |
| RustDesk | P2P hole punch, relay fallback (hbbr) | Public servers or self-host hbbs/hbbr (AGPL-3.0) | NaCl [U, sources inconsistent] | ID + password | n/a | No (Pro server has accounts/OIDC/audit log, paid) | Free OSS; Pro paid | No formal independent audit found; metadata visible to server |
| Paseo (getpaseo/paseo) | Local daemon; E2E relay (Elixir) or direct TCP/Tailscale | Official relay; Apache-2.0 relay self-hostable (Docker) | YES, X25519 handshake (`e2ee_hello`); full spec not read; relay README says protocols may change | Settings -> Pair Device (QR [U]) | Not documented | None, no telemetry | Free | Claude, Codex, Copilot, OpenCode, Pi, Antigravity. 116 stars on relay repo. DIRECT COMPETITOR to Kelpie |
| HAPI (tiann/hapi) | Local hub + optional `--relay` (WireGuard + TLS) or Tailscale/CF Tunnel | Hub self-run; AGPL-3.0 | Claims E2E via WireGuard; key mgmt undocumented [U] | Hub URL + 2 QR codes | PWA/native | No | Free | 10+ agents. Also direct competitor |

URLs (all 2026-10-30): code.claude.com/docs/en/remote-control; learn.chatgpt.com/docs/remote-connections (redirect from developers.openai.com/codex/app/mobile); codex.danielvaughan.com/2026/07/10/codex-remote-ga-qr-relay-architecture-...; github.blog/changelog/2026-05-18-remote-control-for-copilot-cli-sessions-...; github.com/slopus/happy (+ docs/encryption.md, docs/protocol.md); github.com/getpaseo/paseo, /paseo-relay; github.com/tiann/hapi; github.com/amantus-ai/vibetunnel; learn.microsoft.com/en-us/azure/developer/dev-tunnels/security (updated 2026-02-12); blog.jetbrains.com/platform/2026/03/sunsetting-code-with-me/; zed.dev/docs/remote-development; getmoshi.app/articles/best-ios-terminal-app-coding-agent; runtimewire.com Omnara article; seawork.ai Omnara-vs-Happy (competitor, weak).

## 2. Patterns in best-liked products
(Evidence = HN thread 47148454 "Claude Code Remote Control", 544 pts/313 comments; thread 47645915; Happy Show HN 44904039.)
1. Outbound-only from host. No inbound ports, no router config. Claude, Codex, Dev Tunnels, Paseo, Happy all do this.
2. QR scan pairing at the desktop; phone app then lists all hosts/sessions.
3. Agent-aware UI, not raw terminal: approvals, diffs, "needs attention" state. This is what push is for (Moshi, Codex, Copilot, Happy).
4. Session lives on host and survives phone loss/sleep; phone reattaches (mosh/tmux logic, or daemon replay with seq numbers like Happy `seq`).
5. Mirrored view: terminal and phone both drive same session (Claude).
6. Users who distrust the vendor relay fall back to Tailscale + SSH/mosh + tmux ("already a much better experience", HN paxys). That is the bar to beat.

## 3. Top complaints (ranked by evidence)
1. Reliability of first-party remote features: disconnects, sessions not loading, stuck states, hours-long outages (HN 47148454, 47645915). Claude: ~10 min outage tolerance [U, single blog].
2. Vendor lock / account coupling: needs claude.ai login, no API keys, no custom base URL/gateway, org admin toggles, GitHub connection scopes (HN). Biggest blocker for power users.
3. Privacy: "don't want to trust an AI company with a remote access door"; Claude transcripts stored server-side; ZDR/HIPAA orgs blocked.
4. Host must be awake: laptop sleep/close kills it (Claude, Codex docs).
5. Setup friction for DIY: Tailscale+mosh+tmux+hooks is powerful but multi-step; QR rendering bugs in some terminals (Happy/Zed HN).
6. Phone UX: can't see UI the agent built (HN 47645915); terminal typing on phone; notifications absent in plain SSH apps (Blink).
7. Cost: low-weight complaint. Subscription lock-in more than price; Moshi $199 lifetime option shows market accepts one-time pay.
Battery: no solid evidence found either way [U]; Termius keep-alive drain mentioned only by competitor Moshi.

## 4. Security mistakes seen + current best practice

| Mistake observed | Where | Best practice |
|---|---|---|
| Relay sees plaintext (TLS-only hops) | Claude RC (stores transcript), Codex (TLS both hops per 3rd-party guide), Dev Tunnels web-forward (TLS terminated at ingress) | E2E between host daemon and phone; relay routes opaque frames. Happy/Paseo do this |
| Server holds third-party secrets outside E2E | Happy stores GitHub OAuth/vendor tokens with server-only key | Do not collect; or keep client-side |
| No documented lost-phone revocation | Codex guide notes none; Claude only via Trusted Devices (beta, off by default) | Per-device keypair, device list on host, one-click revoke, revoke kills live sockets |
| Long-lived bearer tokens | HAPI/Happy protocol uses bearer token; Codex `--remote-auth-token-env` | Short-lived, scope-bound tokens (Dev Tunnels: 24h, per-tunnel scopes; Claude: multiple short-lived single-purpose creds) bound to device key |
| "No auth" / anonymous modes | VibeTunnel no-auth + public ngrok/Funnel; Dev Tunnels `--allow-anonymous` | No unauthenticated mode when listening beyond loopback; refuse |
| Public tunnel URL as the only secret | VibeTunnel + trycloudflare/ngrok | Require device-key auth on top of tunnel |
| Input bugs in host agent bridge | Happy fixed shell injection and path traversal (commit msg on mirror) | Treat every phone message as untrusted; validate paths; allow-list actions |
| Metadata leakage | RustDesk, Paseo relay (serverId, frame sizes), Happy (ids/timestamps) | State it; minimize; pad if you care |
| Push content in plaintext through vendor push server | Moshi webhook [U], Happy push worker [U] | Send content-free pushes ("session needs you") and fetch on open; or encrypt payload with device key (iOS notification service extension) |
| Pairing code guessability | No incident found in these products. Codex headless uses short-lived manual code | High-entropy QR secret (>=128 bit) single-use TTL ~2-5 min; for manual codes use PAKE (SPAKE2/CPace) or SAS compare + rate limit [general practice, U for this domain] |
| Vendor audit absent | Happy: none found; RustDesk: no independent audit found | Publish protocol spec + third-party review |

Also: no RCE-by-remote-control incident found in public sources; agent CVEs found (CVE-2026-21852, -54316) are unrelated to remote link, but they raise the stakes of anything exposing agent input.

## 5. Gap Kelpie can win
Facts: Kelpie already has daemon-owned sessions, multi-agent (Claude/Codex/ACP/shell), `kelpie-proto` auth roles, and a SolidJS phone client served by daemon (recent commits).

Crowd map: first-party (Claude/Codex/Copilot) = single vendor, account-locked, plaintext-capable relay. Happy/Paseo/HAPI = open, multi-agent, E2E. So "multi-agent + open source + no account" is NOT empty: Paseo (Apache, no login, E2E relay, 7 agents) and HAPI/Happy cover it. Real gaps, ranked:
1. Desktop workbench + phone as one product: shared sessions/worktrees/diffs/chat+terminal handoff (`mode-switch`), not a separate bridge tool. Competitors are CLI wrappers; Kelpie owns the daemon, so no wrapper-launch requirement (Happy/HAPI need you to start agents via `happy`/`hapi`) - verify: can Kelpie attach to existing sessions (it owns them already).
2. Revocable device-key security done right, documented, with threat model + third-party review. Nobody ships this openly; Claude's Trusted Devices is the only comparable and is beta/opt-in.
3. Zero-account + zero-setup: QR over LAN direct, auto-fallback to E2E relay. Paseo asks at daemon start; HAPI needs flags; make it automatic.
4. Always-on daemon is already solved better than laptop-bound tools (daemon survives window close). Still dies on sleep; add wake/keep-awake guidance.
5. Content-free (or device-encrypted) push; competitors vague here.
6. Self-host relay as single binary in Rust (Paseo relay is Elixir; Happy is Node/Postgres [U]).

## 6. Recommendations (ranked)
1. Transport: host daemon outbound WebSocket to relay; also direct LAN/Tailscale mode. Relay = stateless Rust forwarder of opaque frames; offer default public relay + one-binary/Docker self-host. Avoid operating a plaintext-capable relay.
2. Crypto: Noise (XX or IK with pinned static keys) or X25519+ChaCha20-Poly1305 with per-device static keys; QR carries host pubkey + one-time secret (>=128 bit, TTL minutes, single-use); confirm on desktop with approve prompt + SAS. Do not invent a protocol; use `snow` crate. Avoid Happy's design of wrapping DEKs with a server-held account secret.
3. Device model: per-phone keypair stored in Secure Enclave/Keystore; host-side device list with names, last-seen, revoke (drops sockets); roles map to existing `kelpie-proto` roles (read-only vs control); biometric gate for dangerous actions (approvals in yolo/bypass mode).
4. Push: content-free push through a tiny optional push gateway (APNs/FCM; Happy uses own CF Worker, same idea); payload = opaque ID, app fetches over E2E channel. Gateway token not tied to any account.
5. Reliability: reuse seq-numbered replay (already in terminal path) for reconnect; show host online/offline/asleep state clearly; this is the #1 complaint.
6. No account, ever, for core flow. Optional account only for relay quota if hosted.
7. Publish threat model + protocol spec, then pay for one external review before marketing "secure". Kelpie's own AGENTS.md notes auth roles and frame limits exist; start from them.
8. Competitive monitor: read Paseo relay protocol and HAPI "How it works" before finalizing design; consider interop is not worth it.

## 7. Limitations
- Bash broken; no source code reading beyond READMEs/docs. Happy auth/pairing (api.md), Paseo handshake, HAPI key mgmt not read; E2E claims there are partly [U].
- Codex relay details come from a third-party guide (danielvaughan.com), not OpenAI. Official docs confirm only "secure relay layer", QR pairing, SSH host mode.
- Copilot, Cursor, Warp, Chrome RD, Omnara pricing/crypto weakly sourced; Cursor phone/local control not found.
- HN coverage thin on Happy security; no Reddit data retrieved. Battery complaints not substantiated.
- No public security incident write-ups for these relays found; absence != safe.
- Pricing figures from aggregators/competitors where marked [U].

## Unresolved questions
- Does Claude RC protect transcripts at rest beyond "Data usage policy" (retention period)? Not verified.
- Codex relay: real crypto between host and phone (Noise?) and revocation flow.
- Happy: exact pairing/auth handshake and whether the master-secret model lets a malicious server operator impersonate devices.
- Paseo: key-exchange pattern, MITM resistance on first pairing, push implementation.
- Does Kelpie want parity with Claude's "view existing local session from phone" without agent wrapper? (Likely yes via daemon ownership; confirm for externally-launched CLIs.)
- Hosted default relay: who runs and pays for it?
