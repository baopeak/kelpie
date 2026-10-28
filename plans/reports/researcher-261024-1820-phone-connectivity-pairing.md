# Phone connectivity + pairing + E2E beyond LAN (Kelpie)

2026-10-24 · scope: reachability anywhere, how peers do it, pairing/crypto, architecture. Builds on [remote-desktop report](research-261024-1820-remote-desktop-view-control.md) (not redone). ~13 searches/fetches. [U] = unverified/single vendor source.

## 1. TL;DR

- Recommended: **3-tier fallback LAN/Tailscale direct (exists) -> WebRTC P2P (ICE, STUN) -> self-hostable ciphertext-only relay**. Noise-based E2E *inside* all tiers, so transport is untrusted.
- Relay = small stateless Rust (axum) WS forwarder, OSS (MIT), default OFF, user picks host (own VPS / Cloudflare Worker-DO / project default if you choose to run one). Text sessions go via relay as Noise frames (cheap). Video goes P2P only; TURN = optional, off by default.
- Do NOT adopt iroh for the PWA: browsers are relay-only in iroh (no UDP). iroh only pays off if you ship a native Tauri-mobile app [U on iOS/Android build maturity].
- Pairing: QR carries high-entropy secret + desktop static pubkey + relay URL -> Noise `XXpsk`-style/IK. **PAKE (SPAKE2/OPAQUE) not needed** when secret comes from QR (128+ bit). Keep desktop approval step.
- Big unsolved design risk: **web E2E is only as trustworthy as the JS origin.** If relay serves the PWA, it can ship backdoored JS. PWA must be served by kelpied (installed/cached) or by a native app. See 5.

## 2. Reachability options

| Option | Direct/NAT success | Latency | Who runs infra | Cost | Phone browser? | Rust | License | Verdict |
|---|---|---|---|---|---|---|---|---|
| Tailscale (user-installed, current) | WG hole punch + DERP fallback; high | best-case direct ~LAN+RTT | Tailscale/user | free personal tier [U limits] | yes (via Tailscale app VPN on phone) | n/a | proprietary clients/control plane | keep as tier 0; requires phone app + account = the friction we want to remove |
| Embedded tsnet / libtailscale | same | same | Tailscale (or Headscale) | free tier | needs native app (tsnet is Go; no Rust crate found) | FFI only, Go runtime in daemon | tsnet BSD-3 ([pkg.go.dev](https://pkg.go.dev/tailscale.com/tsnet@v1.98.2)) | reject: Go runtime + user still needs a tailnet identity; Win MSVC pain |
| Tailscale Funnel | relayed via Tailscale ingress | +RTT | Tailscale | free | yes (public HTTPS) | n/a | - | ports 443/8443/10000 only, TLS, non-configurable bandwidth cap ([docs](https://tailscale.com/docs/features/tailscale-funnel)); exposes daemon to public internet - bad. Reject |
| Headscale | like Tailscale | same | user self-host | VPS | native Tailscale app | n/a | BSD-3 [U] | too much ops for end users. Reject |
| iroh 1.0 (n0) | QUIC hole punch, relay fallback; "~90%" is a third-party claim [U] ([pinggy](https://pinggy.io/blog/iroh_1_0_dial_keys_not_ips/)) | direct = QUIC RTT | n0 public relays (default) or self-host `iroh-relay` | free/n0 paid hosting | **NO direct from browser: relay-only, still E2E** ([docs](https://docs.iroh.computer/languages/wasm-browser)); needs wasm wrapper, no npm pkg | native, tokio, rustls | MIT/Apache-2.0 ([docs.rs](https://docs.rs/crate/iroh/latest)) | 1.0 released 2026-06-15 [U date], wire/API stable ([n0 blog](https://iroh.computer/blog/the-road-to-iroh-1-0)). Strong for native app; irrelevant for PWA |
| WebRTC DataChannel + signaling + STUN/TURN | 70-80% direct on open internet, 20-25% need TURN, ~5% fail; vendor figures, mobile worse (CGNAT symmetric NAT) ([ExpressTURN](https://www.expressturn.com/blog/webrtc-calls-fail-on-mobile-networks), [Fora Soft](https://www.forasoft.com/learn/video-streaming/articles-streaming/nat-stun-turn-ice-webrtc)) [U: no primary data] | direct best; TURN +RTT | signaling+TURN: you/user | coturn VPS; TURN bandwidth is the cost (video!) | **yes, works in any phone browser**, DTLS built in | str0m (already chosen for video) | str0m MIT | best fit: reuses planned str0m; only path to browser P2P |
| Cloudflare Tunnel (named) | outbound tunnel, always works over HTTP(S)/WS | +CF edge hop | Cloudflare | free named tunnel; needs user CF account+domain | yes | cloudflared Go sidecar | - | quick tunnels: 200 in-flight cap, no SSE, "testing only" ([CF docs](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/)). Power-user doc recipe only. No UDP. Daemon exposed to internet auth only |
| ngrok-style | same as CF | | vendor | paid for stable domain | yes | - | closed | reject (vendor lock, metadata) |
| Self-hosted dumb relay (WS) | 100% (outbound-only both sides) | +1 hop | user/project | tiny VPS or Worker | yes | axum, ~500 LOC | MIT (ours) | **best reliability floor**; carries ciphertext only |

## 3. How existing agent-mobile tools connect

| Tool | Relay | E2E | Pairing | Notes |
|---|---|---|---|---|
| Claude Code Remote Control | Anthropic API; local process outbound-only HTTPS, polls; no inbound ports | TLS to Anthropic; not E2E (server sees content) [U: secondary sources only: [DataCamp](https://www.datacamp.com/tutorial/claude-code-remote-control)] | account login | vendor-hosted; no self-host |
| OpenAI Codex Remote (GA 2026-06) | OpenAI relay; host registers, phone auths via ChatGPT acct | **Noise IK** (Curve25519/ChaChaPoly/BLAKE2s); relay sees ciphertext | QR one-time token + same ChatGPT account/workspace | revocation undocumented; no self-host ([source](https://codex.danielvaughan.com/2026/06/26/codex-remote-ga-qr-pairing-noise-relay-digitalocean-plugin-remote-first-development/)) [U: blog, not OpenAI doc] |
| Happy (slopus/happy, MIT) | `happy-server` relay, Dockerfile present (self-host plausible) ([repo](https://github.com/slopus/happy)) | per-session data key AES-256-GCM, wrapped with tweetnacl box; legacy NaCl secretbox; server stores opaque blobs but sees ids/tags/timestamps and holds 3rd-party OAuth tokens (server-key encrypted, NOT E2E) ([encryption doc](https://raw.githubusercontent.com/slopus/happy/main/docs/encryption.md)) | QR scan to CLI [U details] | closest OSS prior art; model to copy: relay blind to content, metadata visible |
| Omnara | cloud service | "not fully specified" on marketing ([comparison](https://seawork.ai/en/blogs/omnara-vs-happy-vs-seawork/)) [U] | - | cloud-first; treat as not E2E until proven |
| VibeTunnel | not found in this research | - | - | unresearched; typically rides Tailscale/ngrok/CF [U] |
| Others (C3, etc.) | self-hostable relay, cert pinning, per-device revocation, X25519+XChaCha20-Poly1305 | yes | - | marketing claims [U] |

Pattern: all hosted relays; two with real E2E (Codex Noise, Happy NaCl). Nobody does P2P-first. Kelpie differentiator = LAN/P2P first, relay as last resort, self-hostable, MIT, no account.

## 4. Pairing + crypto best practice

- **Transport-agnostic E2E session layer: Noise via `snow` crate** (Rust, Noise Framework implementation; maintained [U: not re-verified this session]). Use `Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s` for first pairing (psk = QR secret, mutual static key exchange), then `IK` for reconnects (phone knows desktop static key, 1-RTT, forward secrecy). Same pattern as Codex.
- **PAKE not required**: PAKE exists to bootstrap from low-entropy typed codes. QR carries 128-256 bit secret -> psk in Noise is enough. Only add SPAKE2/OPAQUE if you want "type 6-digit code" fallback; then brute-force resistance and rate limit matter (current 2-min one-time code + desktop approval already covers online guessing). KISS: skip PAKE v1.
- QR payload: `{v, desktop_static_pub, relay_url|null, lan_endpoints, one_time_secret(2 min), device_nonce}`. Phone proves secret in handshake; desktop shows device name + fingerprint and requires approval (keep current behavior).
- **Key pinning**: desktop stores phone static pubkey (replaces/augments current SHA-256 bearer token); phone pins desktop static key from QR. Relay routing ids = hash of pubkeys; relay authenticates only by signature, no accounts, no PII.
- **Revocation**: delete phone pubkey on desktop -> all future handshakes fail, live session closed (already existing semantic). Relay mailbox for that id dropped. Rekey every N min/GB via Noise rekey.
- **Browser crypto**: Safari/Chrome WebCrypto X25519 / ChaChaPoly availability uneven [U, verify iOS 17+]. Safest DRY route: compile same `snow`-based Rust crate to **wasm** and use in PWA and Tauri mobile; one handshake implementation both sides.
- **WebRTC tier**: DTLS fingerprint exchanged inside the Noise-protected signaling channel -> signaling server (relay) cannot MITM media. Don't rely on relay-delivered fingerprints raw.
- **Web E2E honesty**: if relay origin serves JS, relay operator can exfiltrate. Mitigations: PWA served only by kelpied (LAN pairing visit, installed to home screen, SW-cached with hash pinning); relay serves *no* app code; or use native Tauri-mobile app from store. State this limit in docs.
- Metadata a relay still sees: client IPs, connection times, byte volumes, pubkey-hash ids. Say so.

### Keeping "no data leaves the machine / no telemetry"
- Relay is **opt-in per machine, default OFF**; LAN/Tailscale behavior unchanged for users who never enable it.
- Ciphertext-only; zero analytics, zero logging of ids beyond rate-limit counters in memory; publish relay source (MIT) + reproducible docker image.
- Settings pick relay: "My own relay (URL)" / "Kelpie community relay" (only if project wants to run one; cost + abuse + legal burden [U decision]) / off. Rewrite README promise precisely: "no content leaves your machine unencrypted; with relay enabled, encrypted traffic and connection metadata transit the relay you chose."
- No STUN default to Google: use configurable STUN; ship none by default or the relay doubles as STUN (own UDP port). STUN leaks public IP to the STUN host - disclose.
- Push notifications via APNs/FCM would break the promise (needs vendor relay with token); keep PWA Web Push (VAPID, self-sent) or make it separate opt-in. See open Q.

## 5. Recommended architecture (ranked)

Connection ladder, phone tries in parallel (happy-eyeballs style), prefers first success:

1. **LAN direct / Tailscale** (existing: bind IP:4817, QR, tokens). Zero new infra. Add Noise on top only when over plain HTTP (today HTTP LAN has no E2E; tailscale cert HTTPS optional) - phase later.
2. **WebRTC P2P** (str0m already planned for screen video): DataChannel for session text + input; video track for screen. Signaling over tier 3 relay (or LAN). STUN only. Success ~70-80% [U], poor on CGNAT 4G.
3. **Relay fallback**: kelpied holds one outbound WSS to relay; phone opens WSS to relay; Noise frames forwarded. Carries text sessions, chat, input, small CDP screencast. Cost trivial. Latency = RTT to relay (pick region; Cloudflare Worker/DO gives edge presence [U fit for long-lived WS]).
4. (Optional, later) **TURN (coturn) for video**: only when user self-hosts; default no video over relay (bandwidth, privacy cost). Show "video needs direct connection; relaying disabled".

Ranked options for the project:

| Rank | Option | Why | Risk |
|---|---|---|---|
| 1 | Ladder above: keep LAN/Tailscale, add Noise-over-relay WS first (ships agent-session remote anywhere = the 90% use case), WebRTC P2P second | Browser-compatible, no phone app, reuses str0m, tiny new infra, self-hostable, matches Codex/Happy proven pattern | web-JS trust (above); you own relay protocol + abuse; Noise wasm in Safari [U] |
| 2 | Native Tauri-mobile app with **iroh** (direct QUIC + relay), PWA stays LAN/relay-only | Best NAT traversal, one library does P2P+relay+identity (pubkey = id), MIT/Apache, stable 1.0 | needs store app (Apple $99/yr, review); iroh on iOS/Android + Windows MSVC builds unverified [U]; depends on n0 public relays by default (telemetry/metadata question; self-host `iroh-relay` possible [U docs not read]); duplicates WebRTC video stack |
| 3 | Document Tailscale / Cloudflare named tunnel recipe, no code | Zero dev cost | user config burden (the thing we want to eliminate); CF tunnel exposes daemon to internet; no UDP |
| 4 | Hosted relay run by project, account-based like Codex | Easiest UX | violates no-cloud ethos, ops/legal cost |
| Reject | tsnet embed, Funnel, Headscale, ngrok | see table |

Suggested sequence: (a) relay protocol + `kelpie-relay` crate + Noise handshake + QR v2 (feature flag off); (b) PWA wasm Noise; (c) WebRTC DC/video P2P with relay signaling; (d) evaluate iroh only if native app is greenlit.

## 6. Limitations
- Searches returned vendor blogs/marketing for success rates and E2E claims; no primary NAT-success data for mobile/CGNAT; Happy pairing flow, VibeTunnel, Omnara internals, `snow` maintenance status, iroh-relay self-host docs, iOS/Android iroh builds, Safari X25519 WebCrypto were not verified.
- No prototype or measurement. Latency figures not measured.

## 7. Unresolved questions
1. Will the project operate a default public relay (cost, abuse, legal, GDPR/VN Luat 91/2025 data-law exposure) or ship self-host-only?
2. Native store app planned (unlocks iroh, APNs, Keychain) or PWA-only? Drives rank 1 vs 2.
3. Acceptable to say "web E2E trusts kelpied-served JS"? Need SW hash-pinning design.
4. Push when phone locked: Web Push (VAPID) only, or opt-in vendor push relay?
5. Is TURN video-over-relay ever wanted (privacy/cost)?
6. Spike needed: Noise-wasm in iOS Safari; WebRTC DC over 4G CGNAT hit rate for real users; Cloudflare DO WebSocket hibernation for relay.
7. Do we also keep bearer tokens on LAN, or migrate all tiers to pubkey identity (breaking change to current pairing)?

## Sources
[iroh wasm/browser](https://docs.iroh.computer/languages/wasm-browser) · [iroh 1.0 road](https://iroh.computer/blog/the-road-to-iroh-1-0) · [docs.rs iroh](https://docs.rs/crate/iroh/latest) · [pinggy iroh 1.0](https://pinggy.io/blog/iroh_1_0_dial_keys_not_ips/) · [Happy repo](https://github.com/slopus/happy) · [Happy encryption](https://raw.githubusercontent.com/slopus/happy/main/docs/encryption.md) · [Codex Remote Noise IK](https://codex.danielvaughan.com/2026/06/26/codex-remote-ga-qr-pairing-noise-relay-digitalocean-plugin-remote-first-development/) · [Claude Remote Control (DataCamp)](https://www.datacamp.com/tutorial/claude-code-remote-control) · [seawork comparison](https://seawork.ai/en/blogs/omnara-vs-happy-vs-seawork/) · [Tailscale Funnel](https://tailscale.com/docs/features/tailscale-funnel) · [tsnet](https://pkg.go.dev/tailscale.com/tsnet@v1.98.2) · [CF quick tunnels](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/) · [ExpressTURN](https://www.expressturn.com/blog/webrtc-calls-fail-on-mobile-networks) · [Fora Soft NAT](https://www.forasoft.com/learn/video-streaming/articles-streaming/nat-stun-turn-ice-webrtc)

## Pick (criteria: modern, performance, zero-config user-friendliness)

**Recommended: connection ladder = LAN/Tailscale direct -> WebRTC P2P (str0m + STUN) -> self-hostable Noise-encrypted WSS relay, QR pairing with Noise XXpsk/IK, PWA served by kelpied.**

| Criterion | Score /5 | Note |
|---|---|---|
| Modern | 4 | Noise + WebRTC + QUIC-free but current 2026 pattern (Codex Remote uses Noise IK; Happy E2E relay). Not 5: iroh QUIC multipath is newer |
| Performance | 4 | Direct LAN/P2P near-best; relay tier adds 1 hop (text only, fine). 4G CGNAT may fall to relay -> video unavailable there [U rate] |
| User-friendliness | 5 | Scan QR, works on Wi-Fi/4G, no phone app, no VPN, no port forwarding, no account. Relay choice is the only setting, with a sane default |
| Privacy | 4 | Relay sees ciphertext + metadata only; opt-in; self-hostable. Not 5: web-JS trust limit, STUN leaks IP, default relay host question open |
| Effort | 3 | Relay crate + Noise wasm + QR v2 + signaling; str0m already planned. Moderate, incremental (relay text first) |

**Runner-up: native Tauri-mobile app with iroh (direct QUIC + relay, pubkey identity).** Lost because: (1) browsers are relay-only in iroh, so PWA gets no benefit and a store app becomes mandatory (install friction, Apple $99/yr + review) vs scan-and-go; (2) iOS/Android/Windows-MSVC iroh builds and self-host relay docs unverified [U]; (3) default n0 relays strain the no-telemetry promise; (4) duplicates the WebRTC video stack. Scores: modern 5, performance 5, user-friendliness 3, privacy 3, effort 2. Revisit if a native app is greenlit anyway.
