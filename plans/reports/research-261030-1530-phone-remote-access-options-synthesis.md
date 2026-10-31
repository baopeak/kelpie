# Phone remote access: options synthesis (2026-10-30)

Inputs: `researcher-261030-1500-p2p-mesh-connectivity.md`, `researcher-261030-1500-remote-agent-products.md`, `researcher-261030-1500-mobile-push-client-options.md`, `researcher-261030-1500-remote-control-security.md`.

## Facts that decide the options
- Value is away from home: alert (agent needs you) + one-tap answer, then full view. Every liked product: outbound-only host, QR pairing, agent-aware UI, push.
- Web Push can be sent by `kelpied` itself (VAPID, `web-push` crate): no own server, payload encrypted to the phone (RFC 8291). Needs an HTTPS secure context; iOS needs the PWA added to the Home Screen (16.4+).
- `http://` LAN is not a secure context: no service worker, push, WebAuthn or WebCrypto. LAN-only users cannot get push without a trusted cert.
- Tailscale gives reach + a trusted `*.ts.net` cert (`tailscale serve`), $0, no maintainer infra; cost: app + login on both ends, iOS one-VPN limit, hostname in CT logs.
- iroh 1.0 (Jun 2026): Rust, dial by key, ~90% direct [U], relay fallback, E2E; browser = relay-only WASM. Best for a native app; public relays are "dev/hobby only", ship with own `iroh-relay` or $19/mo.
- Native app = $99/yr Apple + APNs gateway (publisher-held key) + review risk (4.2.3 / 4.2.7 "LAN-based" wording) + Play closed testing.
- PWA served from an internet origin makes that origin a code-trust point (no standard for verifiable web apps in 2026).
- Competitors: Claude/Codex/Copilot relays are TLS-only (vendor can read); Happy/Paseo/HAPI are open + E2E. Kelpie's gap: one workbench, revocable device keys done right, no account, auto LAN/remote, content-free push.

## Options
| | A. Tailscale + PWA + Web Push | B. Own relay + hosted PWA | C. iroh + native app | D. Staged A then C |
|---|---|---|---|---|
| User setup | Tailscale on PC + phone (5 min), Add to Home Screen | Scan QR, Add to Home Screen | Install app, scan QR | A now, C later |
| Reach | Anywhere | Anywhere | Anywhere | |
| Push when locked | Yes (Web Push from daemon) | Yes (Web Push from daemon) | Yes (APNs/FCM gateway) | |
| Content privacy | WireGuard + Noise; Tailscale sees metadata | Noise E2E; relay sees metadata; PWA origin trusted | QUIC E2E; relay metadata | |
| Maintainer infra / cost | None / $0 | Relay ~$5/mo + domain + static host | iroh-relay VPS or $19/mo + push gateway + $99/yr + $25 | |
| Effort | ~1-1.5 weeks | ~2-2.5 weeks | ~4 weeks | |
| Main risks | Users without Tailscale get LAN only; iOS PWA push reliability [U] | Code-origin trust; billing abuse | Store review; new mobile product to maintain | |

## Cross-cutting must-haves (any option)
Capability tiers + remote-forbidden operations; SAS from handshake hash, 3 tries; daemon-computed risk labels and full command on approval; step-up (WebAuthn / Face ID) for dangerous approvals; one-shot approvals only from phone; audit log + kill switch; content-free push (title + deep link). Items 2, 4 (forbidden ops), audit, kill switch were folded into phase 3 on 2026-10-30.

## Recommendation
D: ship A first (fastest real value, no infra, best security short of LAN-only), keep phase 3 link security, then decide C (iroh + native) on measured demand; B only if a no-install web client for non-Tailscale users becomes a must. Telegram: optional alert-only adapter (deep link, no approve).

## Unresolved questions
- iOS 26 standalone PWA: Web Push reliability over a 7-day soak; WebAuthn UV inside Home Screen mode.
- iroh real CGNAT success on 2-3 carriers; iroh-relay access control for self-host.
- Apple review reading of 4.2.3 / 4.2.7 for a Kelpie client.
