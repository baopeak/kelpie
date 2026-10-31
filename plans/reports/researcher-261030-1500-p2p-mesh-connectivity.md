# Phone -> home-PC daemon connectivity (researched 2026-10-30)

Scope: phone (PWA now, Tauri 2 mobile maybe later) reaches `kelpied` behind NAT/CGNAT. Needs: near-zero user setup, strong security. [U] = unverified / from memory or secondary source.
Limit: web search/fetch only (Bash broken); no hands-on testing, no measured success rates for Kelpie's paths.

## 1. Bottom line (read this)

1. **Native Tauri 2 phone app + iroh (dial by node key, QR pairing)** = best fit for "zero setup + strong security". No account, no VPN, no domain, E2E to daemon key, ~90% direct, relay fallback.
2. **The PWA cannot do this well.** iroh in browser = WASM, relay-only, no official npm pkg, you write a wasm-bindgen wrapper. So PWA stays a LAN / "bring your own tunnel" client; the native app is where remote lives.
3. **Tailscale = best "advanced user" escape hatch**, not the default: user must install the app + account on both ends; iOS one-VPN limit; cannot be embedded in Rust for prod (tailscale-rs experimental; libtailscale needs Go).
4. Avoid for default path: Cloudflare quick tunnels (TLS terminated at CF, URL changes, no SLA), ngrok free (1 GB/mo), WebRTC (you'd build signaling + TURN; no better than iroh).

## 2. iroh (n0-computer)

| Item | Finding | Source / date |
|---|---|---|
| Maturity | 1.0.0 released 2026-06-15; 1.0.x patches since (docs.rs shows 1.0.3). Wire + API stability promised for 1.x. Own QUIC impl `noq`. Four years, 50+ releases pre-1.0 | [rc.0 blog](https://iroh.computer/blog/iroh-1-0-0-rc-0), [docs.rs](https://docs.rs/crate/iroh/latest), [pinggy summary 2026-06-18](https://pinggy.io/amp/blog/iroh_1_0_dial_keys_not_ips/) |
| Model | Dial by Ed25519 EndpointId; connect via "home relay" first, then QUIC NAT traversal to direct path | [docs.rs](https://docs.rs/crate/iroh/1.0.0) |
| Hole punch | ~90% success, ~95% of bytes direct [U: secondary source, n0 figures, not independent] | pinggy 2026-06 |
| Encryption | E2E between endpoint keys; relay sees ciphertext only. Relay sees metadata: IPs, times, volume | [public relays doc](https://docs.iroh.computer/iroh-services/relays/public) |
| Public relays | Free, hardcoded (US/EU/Singapore), rate-limited (no numbers), **no SLA, "dev and hobby only"**, only latest stable supported, may block abusers | [public relays](https://docs.iroh.computer/iroh-services/relays/public), [pricing](https://iroh.computer/pricing) |
| Paid relays | Pro $19/mo: shared relays, auth, 10k concurrent, 5 MB/s, 100 GB egress then $0.09/GB. Dedicated +$199/mo/region | [pricing](https://iroh.computer/pricing) |
| Self-host | Open-source `iroh-relay` binary; auth only on managed relays (self-hosted must DIY access control) | [relays](https://docs.iroh.computer/concepts/relays) |
| Browser | WASM build, **relay-only** (browsers can't send UDP), `default-features=false`, no official npm pkg, wasm-bindgen wrapper needed. Examples: browser-echo, browser-chat | [wasm docs](https://docs.iroh.computer/deployment/wasm-browser-support) |
| Native bindings | iroh-ffi: Swift (SwiftPM/CocoaPods `IrohLib`), Kotlin (Maven `computer.iroh:iroh`), Python, Node; track 1.0 surface. FFI pkg versions not verified | [iroh-ffi](https://github.com/n0-computer/iroh-ffi) |
| Tauri 2 mobile | iroh is plain Rust -> compiles into Tauri Rust core; iOS needs SystemConfiguration framework. Community plugin `tauri-plugin-iroh-http` 0.6 (iOS 14+/Android API21+, pre-1.0, MIT/Apache) - don't depend on it, use iroh crate directly | [plugin README](https://raw.githubusercontent.com/momics/iroh-http/HEAD/packages/iroh-http-tauri/README.md) |
| Production users | Claims 200M+ endpoints/30 days, uses: streaming, messaging, file transfer [U: vendor claim]. No named customers verified | pinggy |
| Binary size | Not found [U]; QUIC+TLS+relay client Rust, expect low single-digit MB on mobile |  - |
| Licence | MIT/Apache-2.0 [U, standard for n0 crates] |  - |

Kelpie fit:
- Daemon already Rust/tokio: embed `iroh` endpoint in `kelpied`, accept on an ALPN (e.g. `kelpie/1`), bridge each QUIC stream to the existing framed protocol (`kelpie-proto`). Auth roles map to per-device keys.
- Pairing: QR holds daemon EndpointId (+ relay hint + one-time secret). Phone's own key then allow-listed by daemon. Zero typing, no account.
- QUIC streams multiplex nicely: control stream, terminal binary stream, later screen stream; connection migration handles Wi-Fi<->cellular switches.
- Maintainer cost: $0 on public relays for dev; for ship, run own `iroh-relay` (1 small VPS, few $/mo) or Pro $19/mo. You must handle relay abuse/auth yourself if self-hosting.
- Risk: Tauri mobile app is a new product (store review, signing, push). PWA users get nothing remote.

## 3. Tailscale

| Item | Finding | Source |
|---|---|---|
| Free (Personal) | $0, non-commercial, up to 6 users, unlimited user devices | [pricing](https://tailscale.com/pricing) |
| iOS | Only one VPN with On Demand at a time; another VPN connecting disables Tailscale On Demand until manual reconnect | [VPN on demand doc](https://tailscale.com/kb/1291/ios-vpn-on-demand) |
| Battery | Only anecdotal App Store reviews (10-30% overnight claims); no official figure [U] | search 2026-10 |
| Serve | `tailscale serve` gives HTTPS (valid cert on `*.ts.net`) inside tailnet; fixes PWA secure-context need | [Funnel doc](https://tailscale.com/kb/1223/funnel) |
| Funnel | Public internet -> device via relay; ports 443/8443/10000 only; fixed unconfigurable bandwidth cap; TLS terminated on your device (relay can't decrypt); needs MagicDNS+HTTPS+policy attr | [Funnel](https://tailscale.com/kb/1223/funnel) |
| Embedding | `tsnet` = Go only. `libtailscale` = C archive built with Go toolchain, BSD-3, no releases, bugs go to main repo. Official `tailscale` Rust crate 0.4.0 (2026-07-08): "unstable and insecure", needs `TS_RS_EXPERIMENT`, Linux + macOS ARM only, no Windows/mobile | [libtailscale](https://github.com/tailscale/libtailscale), [docs.rs tailscale](https://docs.rs/tailscale) |
| Headscale | Self-hosted control server; official iOS app supports alternate control URL since v1.38.1 (old guides; current state not verified) | [headscale iOS guide mirror](https://git.myriation.xyz/myriation/headscale/raw/branch/main/docs/iOS-client.md) |
| Privacy | Payload E2E (WireGuard), DERP can't decrypt. Tailscale sees metadata: which nodes talk, public IPs, keys. Coordination server trusted to distribute keys | [security](https://tailscale.com/security), [DERP](https://tailscale.com/kb/1232/derp) |

End-user setup: install Tailscale on PC and phone, sign in with SSO on both (same tailnet), approve device. Then PWA hits `http(s)://pc.tailnet.ts.net:port` in any browser - **this is the only option here where the PWA works remotely with E2E and no maintainer infra**.
Cost: user $0; maintainer $0. Ops: none. Fit: great as documented "advanced" path; poor as the built-in one (account friction, one-VPN limit, can't ship embedded).

## 4. Alternatives

| Option | NAT traversal | Who sees what | End-user setup | Cost user / maintainer | Ops | Maturity / licence | Rust + Tauri mobile | PWA? |
|---|---|---|---|---|---|---|---|---|
| **NetBird** | WireGuard + ICE, TURN fallback | Mgmt server sees metadata; payload E2E | Install app both ends + login (or setup keys) | Free cloud: 5 users/100 machines ([docs](https://docs.netbird.io/manage/settings/plans-and-billing)); team ~6 EUR/user [U secondary] / self-host ok | Self-host mgmt+signal+TURN is heavy | OSS (BSD-3 client; mgmt AGPL [U]) | No Rust embed; separate mobile app | Yes only via VPN app on phone |
| **ZeroTier** | Own P2P, root relays | Controller sees membership; payload E2E | App + network ID join + approve | Free Personal: 10 devices, 1 network, non-commercial ([pricing](https://www.zerotier.com/pricing.md), 2026-08) | Low | Mature; core licence is BSL-ish [U] | No embed | VPN app only |
| **Nebula** (Slack/Defined) | Lighthouses, punch | You run lighthouse (needs public IP) | Cert issuance + config per device: heavy | $0 + a VPS | High | Mature, MIT [U] | Go; mobile app exists [U] | VPN only |
| **Cloudflare Tunnel (named)** | N/A: outbound tunnel to CF edge, 100% reachable | **CF terminates TLS: sees plaintext** unless app-layer E2E | Cloudflare account + domain + `cloudflared` | Free; maintainer $0 but ToS: large non-HTML/video through free plan may be redirected ([CF doc](https://developers.cloudflare.com/fundamentals/reference/policies-compliances/delivering-videos-with-cloudflare/index.md)); WebSockets supported with idle timeout, need heartbeat ([CF](https://developers.cloudflare.com/support/network/using-cloudflare-with-websockets/)) | Low; vendor lock | Mature, closed SaaS | Can bundle `cloudflared` sidecar (Apache-2.0 [U]) | **Yes, plain HTTPS** |
| **Cloudflare quick tunnel** | Outbound | Same, CF sees plaintext | None (no account) | Free; 200 in-flight req cap, **no SSE**, no SLA, URL changes each run ([doc](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/do-more-with-tunnels/trycloudflare/)) | URL must be re-paired every restart | Dev-grade | Sidecar | Yes |
| **Cloudflare WARP/Zero Trust** | WARP client to CF; private network via tunnel | CF sees metadata/can inspect | Account + Zero Trust org + WARP on phone | Free tier ~50 users [U] | Medium | Mature SaaS | No | VPN app only |
| **ngrok** | Outbound | ngrok terminates TLS (sees plaintext) | Account + authtoken | Free: 1 GB/mo out, 3 endpoints, 20k req, one dev domain, interstitial for HTML ([limits](https://ngrok.com/docs/pricing-limits/free-plan-limits)) | Low | Mature SaaS | CLI sidecar / SDK | Yes |
| **frp / bore / pinggy / localtunnel** | Outbound to a server you (or a stranger) run | Server operator sees plaintext unless E2E over it | Needs a public VPS (frp, bore) or 3rd-party host (pinggy, localtunnel) | VPS ~$5/mo; public hosts have flaky free tiers [U] | You run & secure it | frp Apache-2.0 [U]; bore MIT Rust [U] | `bore` is Rust; fine as library idea only | Yes |
| **libp2p** (rust-libp2p) | Circuit relay v2 + DCUtR; ProbeLab: ~70% overall, ~83% for port-restricted cone ([ProbeLab](https://probelab.io/blog/can-libp2p-punch-through-nats/)) | Relay sees metadata, payload E2E (Noise) | None if you build it | $0 + relay nodes | You run relays + DHT/bootstrap | Mature but heavy, API churn [U]; MIT | Native Rust yes; browser via WebTransport/WebRTC-direct needs server with public addr | Partly |
| **Plain WebRTC data channels** | ICE STUN; ~75-85% direct, rest TURN; mobile carrier worse (symmetric NAT) - vendor blogs, no carrier-level data ([summary](https://www.expressturn.com/blog/webrtc-calls-fail-on-mobile-networks)) | TURN sees DTLS ciphertext + metadata; signaling server sees SDP (IPs) | None if you build signaling | Cloudflare TURN: first 1,000 GB free, then $0.05/GB; STUN free; creds <=48h; IPv4 only ([CF TURN](https://developers.cloudflare.com/realtime/turn/faq/)) | You run signaling + auth; no key-pinned identity unless you add it | Mature, browser-native | `webrtc-rs` / `str0m` [U] in daemon; works in WebView & **PWA** | **Yes (only truly browser-native P2P)** |

## 5. Trade-off matrix for Kelpie (1 = bad, 5 = good)

| Option | User setup | Security | Reach (CGNAT) | PWA works | Maintainer burden | Rust/Tauri fit | Overall |
|---|---|---|---|---|---|---|---|
| iroh (native app) | 5 | 5 | 4 | 1 (relay-only WASM, extra work) | 3 | 5 | **Best** |
| iroh (WASM in PWA) | 5 | 5 | 3 (all via relay) | 4 | 2 (relay bandwidth for screen view) | 3 | Viable phase 2 |
| Tailscale (user-managed) | 2 | 4 (trust control plane) | 5 | 5 | 5 | 1 for embed | Best opt-in |
| WebRTC + own signaling | 4 | 4 | 3-4 | 5 | 2 | 3 | Only if PWA-only is a hard rule |
| CF named tunnel | 2 | 2 (CF sees plaintext) | 5 | 5 | 4 | 3 | Opt-in |
| CF quick / ngrok free | 4 | 2 | 5 | 5 | 4 | 3 | Dev/demo only |
| libp2p | 4 | 4 | 3 | 2 | 2 | 4 | Not worth over iroh |
| NetBird/ZeroTier/Nebula | 2 | 4 | 4 | 5 via VPN | 3 | 1 | No |

## 6. Ranked recommendation

1. **Ship iroh in `kelpied` + Tauri 2 mobile app.** Pair via QR (EndpointId + one-time secret + relay URL). Per-device key allow-list, revocable, maps onto existing auth roles. Reuse `kelpie-proto` frames over QUIC bidirectional streams. Keep the LAN path (axum) for PWA/same-network, discover via same pairing.
2. **Interim for PWA remote**: document Tailscale (`tailscale serve`) as supported opt-in; optionally "Cloudflare named tunnel" with app-layer E2E (pairing key, Noise or AES-GCM over WS) so CF seeing plaintext is moot. Do not build more for this.
3. **Maintainer relay**: use n0 public relays during dev only; before ship run 1-2 own `iroh-relay` (US+EU) or Pro $19/mo. Public relays have no SLA, can drop support for older versions, may block abuse.
4. **If PWA remote becomes a must**: either iroh-WASM relay-only (write wrapper crate; all traffic through relay, screen view cost hits you) or WebRTC data channel with Cloudflare TURN (1 TB free). Decide after measuring.
5. Skip: libp2p, NetBird/ZeroTier/Nebula, embedded Tailscale, ngrok/frp/bore as default.

## 7. Key risks

- iroh 1.0 is only ~4.5 months old; vendor claims (90% punch, 200M endpoints) unverified; relay-dependency on n0 for public relays; auth on self-hosted relays is DIY (abuse / bandwidth).
- iOS/Android mobile iroh path unproven in this exact stack (no official Tauri example; iOS needs SystemConfiguration; Android 17 local-network permission for mDNS per plugin README).
- Carrier CGNAT/UDP-blocked networks: iroh falls back to relay (HTTPS-over-443 relay) so works but with latency; screen streaming over relay is bandwidth-heavy ([U] on actual quality).
- Native app means store distribution, signing, background limits (iOS suspends sockets; push notifications for approvals needs APNs/FCM relay = another metadata leak + infra) - this is the real hidden cost regardless of transport.
- Cloudflare ToS ambiguity on non-HTML traffic; Tailscale free plan is non-commercial.
- Metadata: any relay (n0, CF, Tailscale DERP) sees IPs/timing/volume; payload E2E only with iroh/Tailscale/WebRTC, not CF/ngrok unless app-layer.

## 8. Unresolved questions

- Does the user require the PWA to work remotely, or is a native app acceptable? (decides everything)
- Push notifications for approve/deny when app is backgrounded: APNs/FCM via a maintainer-run service? (not researched)
- Actual iroh binary size and iroh-ffi 1.x release status on Maven/SwiftPM (not verified).
- iroh-relay licence and its access-control hooks (`AccessConfig` mentioned in rc.0 notes) for self-hosted per-device auth: verify before committing.
- Real-world iroh success rate on carrier CGNAT (needs own test: 2-3 carriers, iOS+Android).
- Does Windows firewall / corporate AV treat a UDP-listening daemon badly? Not researched.
- NetBird browser/WASM client, Cloudflare Zero Trust current free limits, Nebula/frp/bore/localtunnel specifics were not verified from primary sources [U].
