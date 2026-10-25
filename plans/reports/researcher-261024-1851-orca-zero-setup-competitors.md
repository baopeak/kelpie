# Orca + zero-setup phone-control references (2026-10-24)

Tag [U] = unverified. ~17 fetches. Stack of Kelpie assumed per brief.

## 0. Verdict (brutal)
- Zero-setup = **outbound-only to a vendor-run relay + vendor identity** (Claude RC, dev tunnels, CRD, Orca Relay, Happy). Every competitor that achieves "no networking knowledge" runs a **cloud service + account**. None does it with only LAN/Tailscale.
- Orca = closest clone of Kelpie's goal. It shipped LAN/Tailscale first (ws://IP:6768 + QR), then bolted on a hosted relay + native app + push gateway. Heavy ops burden (GCP Cloud Run, Postgres, directors/cells, APNs key).
- Kelpie cannot match "zero setup" off-LAN without a relay. Decision needed (see §6): ship **relay-optional**, default LAN/Tailscale, and make the relay a thin, self-hostable, E2EE-blind forwarder. Do NOT copy Orca's cloud complexity.

## 1. Orca (stablyai/orca, MIT, onorca.dev, Electron)
Latest: desktop v1.4.239 (2026-10-24); mobile app.json version 0.0.58, "beta" (docs). Repo pushed same day = very active.

### Stack / distribution
- Native app, **React Native + Expo** (`mobile/app.json`, `mobile/package.json`; bundle id `com.stably.orca.mobile`; scheme `orca`). WebView shell module `mobile/modules/orca-mobile-web-shell`. iOS: App Store "Orca IDE" id6766130217. Android: **sideloaded APK** (onorca.dev/download/android) - not Play. Sources: https://github.com/stablyai/orca , https://onorca.dev/docs/mobile
- iOS Info.plist: `NSLocalNetworkUsageDescription`, ATS exceptions for `100.64.0.0/10` + `fd7a:115c:a1e0::/48` (Tailscale CGNAT/ULA) -> plain `ws://` allowed to Tailscale IPs. Confirms Tailscale is a first-class path.
- App Store copy: no analytics SDK, no crash reporter; "traffic encrypted on LAN" wording (search snippet [U] for exact text).

### Connection (3 paths)
1. **Direct LAN/Tailscale**: desktop hosts WebSocket RPC on `:6768` (`mobile/README.md`: "mobile endpoint is ws://<desktop-ip>:6768"). Editable host address without re-pair (docs).
2. **Orca Relay (hosted by Stably)**: `cloud/README.md`: "Phones and desktops never talk to each other directly: each opens an outbound WebSocket to a relay cell, the relay pairs the two sessions and splices frames". Director `https://relay.onorca.dev`, cells `relay-c1.onorca.dev` (`src/main/orca-profiles/profile-cloud-auth-config.ts`, `src/shared/mobile-relay-pairing-fixtures.ts`). Infra: Cloud Run + Cloud SQL Postgres + Terraform (private `stablyai/orca-cloud`), director/cell/"fence-broker"/"region-correction" - massive. Relay requires **Orca account sign-in on both ends**; "Pairing fails: confirm both devices use the same Orca account" (docs).
3. "Remote Orca Server" pairing for headless machines (docs) - separate from phone.
- Desktop must stay running; closing app drops phone session (docs). No wake-on-demand.

### Pairing
- Desktop Settings > Mobile (older README) / account-status menu (docs) shows **QR + one-time code + deep link**. Phone scans (`mobile/app/pair-scan.tsx`, `pair-confirm.tsx`, `pair.tsx`); QR encoded via `src/main/runtime/mobile-pairing-qr.ts` (qrcode, ECC "M"). Pairing URL carries host endpoint(s) + **desktop X25519 public key** ("public key is embedded in the QR pairing offer" - `src/main/runtime/e2ee-keypair.ts`). Exchange mints a per-phone **device token** (docs). Relay provisioning is negotiated after pairing (`pairing.provisionRelay`, `pairing.getEndpoints` in `mobile/rpc-foundation/goldens/`). Direct races relay, "direct wins" then upgrades (goldens `pairing-pre-profile-direct-wins-and-provisions.json`, `relay-direct-upgrade-commits.json`). Credential rotation + pairing recovery exist.

### Encryption
- **Yes, app-layer E2EE over plain ws://**: v1 = tweetnacl `box` (Curve25519 + XSalsa20-Poly1305) `mobile/src/transport/e2ee.ts`; **v2** = `orca-mobile-e2ee` hello/ready handshake with transcript binding, nonces, counters, framing v2, context `transport: 'direct'|'relay'` (`src/shared/mobile-e2ee-v2-contract.ts`, `src/main/runtime/rpc/mobile-e2ee-v2-desktop-session.ts`). Relay only splices ciphertext (design; relay never needs plaintext). Desktop host also authenticates to relay/push with the same X25519 key (host proof / "host pins", `cloud/README.md`).
- Weakness: no forward secrecy in v1 (static desktop key + phone key; v2 may fix - not verified [U]). Relay-side metadata (host id, IPs, timing) visible to Stably; account identity tied to Orca cloud.

### Push
- Separate **push gateway** `cloud/apps/push` (APNs + FCM); phones hold no Orca credential for it; desktop host (key-authenticated) registers each phone's native push token and requests pushes. Per-host quotas, durable queue, Postgres. Payload content: titles/bodies pass through gateway (not logged) - **not E2EE to Apple/Google/Stably** [U for exact payload]. Requires native app -> App Store / APK / APNs key.

### What phone can do (docs)
View worktrees/agent status/file tree; Chat UI or raw terminal + scrollback; reply with text/photos/files/dictation; Quick Commands; session tabs; **browser-session preview**; Source Control stage/commit; switch agent accounts; create workspaces/start agents. "Not a full editor." **No desktop screen control.**

### SSH / remote
- `onorca.dev/docs/ssh`: desktop -> remote machines (worktrees, terminals, port forward, auto-reconnect). Tooling in `src/main/ssh/*`. This is desktop-as-client; separate from phone.

### Where Kelpie can beat Orca
| Orca weakness | Kelpie angle |
|---|---|
| Off-LAN needs Stably account + proprietary-ops relay (MIT code, but prod relay private-infra) | Self-hostable tiny relay or Tailscale-guided path, no vendor account |
| Native app: App Store review + sideloaded Android APK; version-skew "app blocks host, update" | PWA served by kelpied = always matches daemon version; no store |
| Desktop app must be open; session dies on close | `kelpied` is a per-user daemon, survives UI close |
| No screen/OS control | Planned takeover/screen (prior synthesis) |
| Static-key E2EE v1; huge protocol surface (goldens, rotation, recovery) | Per-device tokens + revocation UI on desktop, small surface |
| Push via cloud gateway holding APNs key | Web Push self-hosted (VAPID), no vendor in path |
| Beta, version 0.0.x, relay features churn daily | Stable, small |
Orca strengths to respect: QR + code + deep link, direct-wins-then-relay racing, editable host address, E2EE even on ws://, push mirrors desktop notification categories.

## 2. Zero-setup references
| | Claude Code Remote Control | VS Code Remote Tunnels / dev tunnels | Happy (slopus/happy, MIT) | Chrome Remote Desktop |
|---|---|---|---|---|
| Network | Outbound HTTPS 443 only, polls Anthropic API, **no inbound ports** | Outbound to Azure relay `*.rel.tunnels.api.visualstudio.com`; no inbound | Outbound Socket.IO `/v1/updates` to relay server (`packages/happy-server`: Fastify+Postgres+Redis+S3); self-hostable (`standalone:dev` PGlite) | Signaling via Google; **ICE/WebRTC P2P (direct, STUN 3478), TURN relay fallback** |
| Relay operator | Anthropic | Microsoft | Happy project (hosted) or self-host | Google |
| Identity | claude.ai account (Pro/Max/Team; no API key) | Same GitHub/Microsoft acct on both ends | **Keypair account, no email**: `POST /v1/auth {publicKey, challenge, signature}`; terminal shows QR w/ ephemeral key, phone approves (`/v1/auth/request`, `/v1/auth/response`) | Google account + host **PIN** (per-host secret, 6+ digits) |
| E2E? | **No** - TLS only; transcript **stored on Anthropic servers**; short-lived scoped credentials | **Yes**: SSH inside the tunnel (AES-256-CTR) so relay can't read; TLS terminated at relay only for web-forwarding of arbitrary ports | **Yes**: client-side NaCl secretbox / AES-256-GCM per-session dataKey; server "blind" (`docs/encryption.md`); some server-only secrets not E2EE | Yes (session encrypted P2P; TURN relays ciphertext) [U details] |
| Phone client trust | Official signed Claude app / claude.ai web (vendor-served JS) | vscode.dev (Microsoft-served JS) - JS served by the relay's owner | Store-signed native app (iOS/Android) + web build | Official app / remotedesktop.google.com |
| Pairing UX | `claude remote-control`, QR via spacebar, or open claude.ai/code and pick machine (same account = automatic) | `code tunnel` prints vscode.dev URL, sign in once with GitHub | `happy` wrapper CLI shows QR; scan once | Install host app, set PIN, pick machine in list |
| Revocation | "Trusted devices" page (enrolled device credential, 18h sign-in freshness step-up w/ Face ID/passkey) | Tunnel tokens expire 24h; delete tunnel | Delete account key / re-pair | Remove host / PIN reset |
| Notable limits | Machine must be on; Anthropic base-URL gateways/ZDR/HIPAA orgs excluded | One client at a time; machine must stay up; anti-phishing interstitial | Must launch agents through `happy` wrapper -> restart sessions (weakness per prior report) | Needs OS permissions, host service |
Sources: https://code.claude.com/docs/en/remote-control (Connection and security) ; https://code.visualstudio.com/docs/remote/tunnels ; https://learn.microsoft.com/en-us/azure/developer/dev-tunnels/security ; https://github.com/slopus/happy (docs/encryption.md, docs/api.md, docs/protocol.md, docs/deployment.md) ; https://support.google.com/chrome/answer/1649523 ; https://support.google.com/chrome/a/answer/16364503 .

### Key insight on "how phone client code is trusted"
- Relay-served JS (vscode.dev, claude.ai) = E2EE impossible to prove; you trust the operator. Claude RC skips E2EE entirely and accepts that.
- Native store app (Happy, Orca) = code is pinned by Apple/Google signing; E2EE meaningful.
- Kelpie PWA served by kelpied over LAN/Tailscale = code from user's own daemon (best), but off-LAN via a relay breaks it unless the PWA is still fetched from the daemon through an opaque tunnel (TLS passthrough/E2EE'd stream) - consistent with prior synthesis rule "relay never serves JS".

## 3. Best-of UX flow (zero networking knowledge)

### First-time pairing (target <60 s, 3 taps)
1. Desktop Kelpie: Settings > Phone > "Connect phone" (one toggle; daemon starts listener + detects best address: Tailscale IP if present, else LAN IP; hides IPs/ports from user).
2. Show QR + short word-code + copy link (Orca: QR + code + deep link). QR = {endpoint list, daemon public key fingerprint, one-time pairing secret (60-120 s TTL, single use)}.
3. Phone camera scans -> opens PWA URL from kelpied -> "Add to Home Screen" prompt (needed for iOS push).
4. **Desktop shows approval dialog** "Phone 'iPhone of X' wants control - Allow (view only / full)" with a 4-6 digit confirmation shown on both screens (SAS) to defeat QR shoulder-surfing. (CRD PIN / Claude Trusted Devices equivalent.)
5. Per-device token minted, stored in IndexedDB (non-exportable WebCrypto key if possible - device-bound signing, Claude "enrolled device" analogue). Done. Existing sessions already visible (no wrapper/restart - Happy's flaw).

### Off-LAN with no knowledge (the hard part)
- Option A (recommended default): detect Tailscale; if absent show one screen "Install Tailscale on both devices, sign in with same account" with deep links; pairing QR then contains the 100.x address. Orca has the same fallback.
- Option B (opt-in): Kelpie relay (outbound WSS from kelpied; phone connects to same relay; **opaque to relay**, Noise/E2EE keyed from pairing secret + token; relay never serves JS). Orca racing pattern: try direct first, fall back to relay, upgrade back.

### Daily use
- Open home-screen PWA -> auto-reconnect to last good endpoint (race direct vs relay) -> unified "needs you" inbox across agents -> tap approve/answer -> optional "take over screen".
- Push (Web Push, VAPID) -> tap -> deep link to the exact prompt. Mirror desktop notification categories (Orca).
- Daemon survives UI close (beats Orca). Wake: show "desktop asleep" state with last-seen, not an infinite spinner (Orca release notes mention "Connecting via Relay..." stuck-state bug).
- Revocation: desktop list of devices with last-seen + one-click revoke; phone-side "forget host". Auto-expire idle tokens (Claude, dev tunnels 24h model) + biometric/passkey step-up for risky actions (screen control, approve-all/bypass-permissions).

## 4. Security trade-offs each competitor made
| Product | Chose | Cost |
|---|---|---|
| Claude RC | Convenience + central account; TLS only; transcripts stored server-side; optional Trusted Devices w/ biometrics | Vendor sees all content; vendor-only; not for ZDR/HIPAA orgs |
| Dev tunnels | Account-gated private tunnels, SSH E2E inside; 24h scoped tokens; anti-phishing interstitial | Tied to Microsoft/GitHub; web-forwarded ports TLS-terminated at relay (readable); anonymous mode footgun |
| Happy | Keypair identity, client-side E2EE, blind server, self-host | Wrapper must start sessions; secret-key loss = lose account (backup UX burden); native apps for trust; server-only secrets for integrations not E2EE |
| CRD | P2P (ICE) with TURN fallback, Google account + PIN | Google identity/signaling dependency; OS-level privileged host service; PIN brute-force surface [U rate limits] |
| Orca | Direct+relay racing, app-layer E2EE incl. ws://, host-key-authenticated push, per-device token | Stably account + cloud for off-LAN; static-key E2EE v1; push content passes Apple/Google/Stably; huge relay ops surface; Android outside Play; desktop-only host |

## 5. Ranked recommendation for Kelpie plan
1. **Keep PWA-from-kelpied + QR + desktop approval + per-device token** (already approved architecture). Add SAS confirmation, TTL/single-use QR, device list/revoke, passkey/biometric step-up for screen control. Fit: 5/5, no vendor.
2. **Auto-detect Tailscale + guided install screen** as the "off-LAN" default. Fit 4/5; costs user a Tailscale install (honest "least setup" given no relay).
3. **Optional self-hosted Noise-E2EE relay (outbound WSS, no JS served, direct-then-relay race like Orca)**, only if product demands true zero-install off-LAN. Fit 3/5: someone must run it; if Kelpie hosts a public default, it inherits Orca's ops burden (Orca needed Cloud Run + Postgres + directors/cells + fence-broker).
4. Defer native app/APNs gateway. Orca proves it costs a store pipeline, push gateway, version-skew handling.
Rejected: Claude-RC style TLS-only vendor relay with stored transcripts (contradicts strong security + local-first); Happy wrapper model.

## 6. Limitations
- Orca relay protocol read at README/contract-header level, not full `relay-contract` spec; v2 forward secrecy and exact push payload content unverified [U].
- Orca/Happy "latest version/date" for Happy not captured (GitHub GraphQL timeouts). Happy hosted relay domain [U] (believed api.cluster-fluster.com, not confirmed in this pass).
- CRD internals only from Google support/network guide; PIN rules, OAuth host-registration not from primary doc.
- Claude RC E2EE absence inferred from official "TLS... transcripts stored on Anthropic servers" - no explicit "not E2EE" statement.
- No device testing of iOS PWA push/WebRTC.

## Unresolved questions
1. Does Kelpie ship/host any public relay by default, or only Tailscale + self-host? (Drives complexity 10x.)
2. Is a Kelpie account/identity acceptable at all (Orca & Claude require one), or strictly keypair-only like Happy?
3. Is "view-only vs full control" a pairing-time choice or per-action step-up?
4. Is Android-without-Play (APK) acceptable if native shell ever added? Orca did it.
5. Does Orca v2 handshake add forward secrecy (ephemeral client key present; desktop key static?) - read `src/shared/paired-e2ee-v2-client-session.ts` if it matters.
