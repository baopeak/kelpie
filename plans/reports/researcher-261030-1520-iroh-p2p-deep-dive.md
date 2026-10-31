# iroh vs self-built relay for Kelpie: deep dive (2026-10-30)

Limits: web search and fetch only. No hands-on testing, and I found no independent iroh data from Vietnam. [U] means unverified.

## 0. Verdict first

| Client | Decision | Why |
|---|---|---|
| PWA (browser) | Do not use iroh. Use your own Noise-over-WebSocket relay, placed in VN, HK or SG. | iroh in a browser is relay-only WASM. It has no official npm package, and Safari relay connections have an open bug (#4519). It would give a worse relay than yours. |
| Native Tauri 2 phone app | Use iroh later, behind a `Transport` trait, with your own `iroh-relay` in the region. | You get QUIC, path migration and direct paths without writing a hole-puncher. |
| Never | n0 public relays in production. | n0's terms say "dev and hobby only", no SLA, and rate limits can change at any time. |

- **Direct paths mostly help typing feel, not approvals.** My estimate is about 100 ms or more saved per echo. That is derived from your measurements and not tested.
- **A direct path is not worth much for a small-message workload before you have a native app.**
- **Ship the relay first** and keep the protocol transport-agnostic.
- **Treat iroh as an upgrade path, not a prerequisite.** iroh's weakest area is mobile and CGNAT path churn, which is your target network (section 2).

## 1. Claims checked (prior report and vendor claims)

| Claim | Source | Verified? | Evidence |
|---|---|---|---|
| iroh 1.0 released 2026-06-15; latest is 1.4.1 (2026-10-22) | [iroh.computer/blog/v1](https://iroh.computer/blog/v1), [GitHub releases](https://api.github.com/repos/n0-computer/iroh/releases) | Yes | The prior report said 1.0.3 was current. It is stale: 1.2.0 (09-11), 1.3.0 (09-28), 1.4.0 (10-20), 1.4.1 (10-22). |
| Wire and API stability across 1.x | [blog/v1](https://iroh.computer/blog/v1) | Yes | A v1 endpoint talks to any other v1 endpoint. Wire-breaking relay changes get new URLs. Public relays are updated within about 24 hours of each release. |
| "~90% hole punch" | [pinggy](https://pinggy.io/blog/iroh_1_0_dial_keys_not_ips/), [docs holepunching](https://docs.iroh.computer/concepts/holepunching) | Claim exists, number not evidenced | n0 docs say "roughly 9 out of 10 network configurations". There is no dataset or method. Pinggy sells tunnels and gives no source. The official v1 post gives no hole-punch rate, only "95% of data direct" with no source. |
| "~95% of bytes direct" | [blog/v1](https://iroh.computer/blog/v1) | Unsourced | Phrased as "it's normal to see 95%". |
| "200M endpoints" | [blog/v1](https://iroh.computer/blog/v1) | Partly | "200 million endpoints created in the last 30 days" on public relays. That counts ephemeral endpoints, not users. No named customers anywhere in the post. |
| Independent hole-punch data | [arXiv 2510.27500](https://arxiv.org/abs/2510.27500) (2025-11) | Yes, but libp2p, not iroh | 4.4M attempts, 85k networks, 167 countries: 70% ± 7.1%. TCP and QUIC were indistinguishable. 97.6% of successes came on the first attempt. |
| iroh browser support is relay-only with no npm package | [wasm docs](https://docs.iroh.computer/deployment/wasm-browser-support) | Yes | You must write your own wasm-bindgen wrapper. No bundle-size figure found. |
| Relay is WebSocket over TLS | [client.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/src/client.rs) | Yes | `wss://`, expects a 101 upgrade, HTTP proxy support, no HTTP fallback. Native sends a Bearer header; WASM sends `?token=`. |
| Self-hosted relay has no auth, so it is DIY | [iroh-relay README](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/README.md) | Refuted | It has allowlist, denylist, `shared_token` (Bearer or `?token=`, needs restart to revoke) and `access.http.url`. The HTTP hook calls your service per connection and expects 200 with body `true`. Per-device auth is feasible. |
| `AccessConfig` exists | [rc.0 notes](https://iroh.computer/blog/iroh-1-0-0-rc-0) | Changed | It is now `dyn DynAccessControl` with `Access::{Allow, Deny}` (server.rs). The TOML keys above are the operator interface. |
| Public relays: free, no SLA, hobby only | [public relays](https://docs.iroh.computer/iroh-services/relays/public) | Yes | Only the latest stable release is supported. No numeric rate limits. Monitored for abuse. |
| Pro is $19/month with 5 MB/s | [pricing](https://iroh.computer/pricing) | Yes | Shared relays: 10k connections, 100 GB, then $0.09/GB. The only Asian region listed for shared relays is Singapore. Dedicated is +$199/month per region, with custom regions. |
| Metadata: relay sees IPs, times, volume | [public relays](https://docs.iroh.computer/iroh-services/relays/public) | Yes | Payload is end-to-end encrypted. |
| Android and iOS usable via ffi | [iroh-ffi](https://github.com/n0-computer/iroh-ffi) | Partly | Swift via SwiftPM xcframework and Kotlin via Maven are official and "mirror 1.0". The releases list was empty on the page I fetched. A third party ([iroh-kmp](https://klibs.io/project/Azula-App/iroh-kmp)) reports the Maven artifact's async calls hang on Android (JNA) [U, single source]. For Tauri you link the Rust crate directly, so this does not matter. |
| Binary size on mobile | none found | Not verified | Must measure. |
| Production apps | [Delta Chat via AlternativeTo 2024](https://alternativeto.net/news/2024/11/delta-chat-introduces-real-time-p2p-networking-with-hole-punching-and-forward-secret-e2ee) | Weak | Delta Chat uses it for webxdc realtime channels, an opt-in experimental feature. [moq.dev](https://doc.moq.dev/concept/layer/iroh) labels its iroh layer "experimental". I found no other named production app. |
| Audit | [noq blog](https://iroh.computer/blog/noq-announcement), [blog/v1](https://iroh.computer/blog/v1) | None found | No third-party audit of iroh, noq or iroh-relay mentioned anywhere. |

## 2. iroh internals

| Topic | Finding | Source / date |
|---|---|---|
| Design | Starts on the relay (usable immediately), exchanges candidates over the relay, sends simultaneous probes, then moves to the lowest-biased-RTT direct path. | [multipath blog 2025-08-05](https://iroh.computer/blog/iroh-on-QUIC-multipath), [holepunching docs](https://docs.iroh.computer/concepts/holepunching) |
| noq | A hard fork of Quinn (announced 2026-03-19), shipped in 0.96. It implements IETF QUIC multipath; relay and direct paths are first-class QUIC paths. It was interop-tested against picoquic. No benchmarks and no audit. | [noq blog](https://iroh.computer/blog/noq-announcement) |
| NAT traversal | A QUIC extension: transport parameter, `ADD_ADDRESS`, `REACH_OUT`, off-path `PATH_CHALLENGE` probes. Retries stop after 30 s. Only the client opens new paths. The draft says it documents "all current bugs", has a TODO Security section, and has no success-rate data. | [IETF draft -00, July 2026](https://www.ietf.org/archive/id/draft-bruynooghe-n0-quic-nat-traversal-00.txt) |
| QAD vs STUN | QAD (QUIC Address Discovery) is used since 0.32. It learns the public IP/port over QUIC to the relay, encrypted, no extra round trips. Relay default ports: QAD 7842, HTTPS 443. | [noq blog](https://iroh.computer/blog/noq-announcement), [defaults.rs via docs.rs](https://docs.rs/crate/iroh-relay/1.0.2/source/src/defaults.rs) |
| Symmetric NAT and CGNAT | The draft and iroh docs do not address them, and nothing says "both CGNAT = relay". Tailscale's analysis (below) implies it. | draft; docs |
| Relay protocol | WebSocket/TLS. Server pings every 15 s (half of the 30 s idle timeout). Max packet 64 KiB, frame 1 MiB, 512-packet send queue. `Status::RateLimited` signal. | [relay.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/src/protos/relay.rs) |
| UDP-blocked networks | The relay is TCP/443, so it works. Docs say traffic stays on the relay when punching fails. No timing data. | docs; client.rs |
| Network-change handling | 0.96–0.97 regressed: holepunch not re-triggered, stalls on Wi-Fi to LTE. Fixed in 0.98: relay ping at max(3×RTT, 500 ms) after a change (was up to 5 s), PTO capped at 2 s. No measurements given. Android apps must call `Endpoint::network_change` themselves. | [0.98.0 blog](https://iroh.computer/blog/iroh-0-98-0-getting-back-to-traversing-nats), endpoint.rs |
| Time to first byte | Relay path works from the first packet, then the QUIC handshake. 0-RTT is available. Not measured. | multipath blog |
| Home-relay failover | About 30 s of unreachability and dropped connections when a home relay dies, even with 2 relays configured (open, #4319, 2026-06-10). The cause is that the endpoint holds a connection only to its home relay, and home-relay switching waits for net-report (every 20–26 s). A brief restart recovers in about 20 s. | [#4319](https://github.com/n0-computer/iroh/issues/4319) |
| Dead-path detection | About 15 s of stall before falling back to the relay. A custom transport cannot drop a known-dead path (#4602). | [#4424](https://github.com/n0-computer/iroh/issues/4424), [#4602](https://github.com/n0-computer/iroh/issues/4602) |

### Open issues that hit your target network

| Issue | What | Impact for Kelpie |
|---|---|---|
| [#4424](https://github.com/n0-computer/iroh/issues/4424) (open, 2026-07-17) | iOS on T-Mobile US CGNAT to a home server. Punches in about 1 s, but the direct path drops to relay every 50–75 s. The CGNAT remapped IP and port; QUIC migration never fired. Each event costs 16–41 s on the relay. | This is your exact scenario (phone on carrier CGNAT, PC at home). The reporter used 1.0.2 then 1.1.0. Check against 1.4.x. |
| [#4644](https://github.com/n0-computer/iroh/issues/4644) (open, 1.4.0) | Android on 4G/5G in India. Dual-stack path flapping every 60 s resets congestion state. A "validated" direct path carries no data, 39% of one run stalled on it. Cellular to cellular once failed with no relay backup. Office Wi-Fi to home Wi-Fi was 99.8% relay. Maintainer flub agreed on the core problem. | Asian mobile networks; one reporter's data, but detailed. |
| [#4598](https://github.com/n0-computer/iroh/issues/4598) | Re-punch every 60 s when RTT is above 10 ms, about 160 B/s per idle connection. | Battery and metered-data cost on a phone. Not fixed. |
| [#4519](https://github.com/n0-computer/iroh/issues/4519) | Safari WASM relay fails on a trailing-dot hostname (duplicate of #4550). Workaround: custom relay URL without the dot. | PWA blocker if you ever used WASM iroh. |
| [#4639](https://github.com/n0-computer/iroh/issues/4639), [#4603](https://github.com/n0-computer/iroh/issues/4603), [#4393](https://github.com/n0-computer/iroh/issues/4393) | New connection times out while another is open; one relay's full queue stalls sends to all relays; paths opened and abandoned repeatedly. | Reliability, not security. |

- **Open bugs:** 42 on the issue list (I read page 1 of 2). None are labelled for iOS background or battery. The iOS/Android specifics come from reporter comments.
- **#4424 caveat:** the reporter later corrected themselves that some whole-connection deaths were iOS suspension, not path failures.
- **Maintainer response:** responsive, but the issues are tagged "Needs Triage".

### Hole-punch success: what exists

| Source | Number | Quality |
|---|---|---|
| n0 docs | "9 of 10 configurations" | Anecdotal; n0's own docs PR said they were still working on collecting it. |
| libp2p/IPFS (arXiv) | 70% ± 7.1% | Large and independent, but a different stack and different population. |
| Tailscale [How NAT traversal works](https://tailscale.com/blog/how-nat-traversal-works) | "over 90%" estimate with basic STUN plus simultaneous send | Analytical estimate, not a measurement. Hard NAT on one side is fine (birthday paradox: 1,024 probes gives about 98%). Hard NAT on both sides needs about 170k probes per side, roughly 28 minutes at 100 packets/s; "not looking good for interactive". CGNAT is usually workable but needs hairpinning if both peers sit behind the same CGNAT, and ISPs often disable port-mapping protocols. |
| HyperDHT/Keet | fails when both peers are on randomizing NATs, then relayed through peers | [Holepunch docs](https://docs.pears.com/howto/connect-two-peers-by-key-with-hyperdht.html); a 30% symmetric figure on a blog is unsourced. |
| Vietnam-specific | none found | Do the spike. |

**Vietnam factor:** IPv6 adoption is reported at about 67.7% nationally ([VNNIC via Tuổi Trẻ, 2026-06](https://news.tuoitre.vn/vietnam-ranks-2nd-in-se-asia-7th-globally-in-ipv6-adoption-103260619162332916.htm)). An older, undated VNNIC slide says mobile is 88% and FTTH 80% [U on date]. IPv6 avoids NAT, so direct paths are more likely than the CGNAT worry suggests. The home router firewall still needs hole punching.

## 3. iroh relay operations

| Item | Finding | Source |
|---|---|---|
| Binary | `cargo build --profile optimized-release --package iroh-relay --features server`. Dual-licensed Apache-2.0 and MIT. Dev mode is HTTP only on :3340. | README |
| Access control | `everyone` (default), allowlist/denylist by endpoint ID, `shared_token` (revocation needs restart), or `access.http.url` calling your service per connection. | README |
| Per-client limit | `ClientRateLimit { bytes_per_second, max_burst_bytes }` on the receive side. `accept_conn_limit` and `accept_conn_burst` are TODO in the source. | server.rs |
| TLS | Let's Encrypt via ACME, or manual certificates. QAD needs a manual certificate in the README. | server.rs, README |
| Metrics | Optional `metrics_addr`, default 9090. | server.rs, defaults.rs |
| Resource use | No published figures. The only number: the key cache is sized for 1M clients at about 56 MB. I found no per-connection RAM or CPU. Measure it yourself. | defaults.rs |
| Cloudflare Workers | No evidence it runs there. It is a tokio and TCP/UDP server, and QAD needs UDP, which Workers do not give you. Treat as no. [U] | none found |
| Security record | 1.0.2: a pre-auth DoS where a frame shorter than 32 bytes caused a panic and, with `panic=abort`, crashed the server. 1.1.0: three more, including a crafted batch that pins CPU at 100% and misdirected NAT probes. Fuzzing was added. 1.4.0: rustls bumped for four RUSTSEC advisories; relay operators must rebuild. | [1.0.2](https://iroh.computer/blog/iroh-1-0-2), [1.1.0](https://iroh.computer/blog/iroh-1-1-0), [1.4.0 release](https://github.com/n0-computer/iroh/releases) |
| Cadence | Releases every 2–4 weeks. Relay patching is on you if self-hosted. | releases |

**Cost, real deployment:**

| Option | Cost |
|---|---|
| Self-host in VN/HK/SG | One VPS at about $5–20/month plus egress; I did not look up regional prices [U]. |
| n0 Pro | $19/month, plus overage past 100 GB at $0.09/GB, and capped at 5 MB/s. Singapore is the only Asian region. |
| n0 Dedicated | $19 + $199/month per region. |

Terminal and approval traffic is tiny, so bandwidth is not the constraint. Operations and patch cadence are.

## 4. iroh security

| Topic | Finding | Source |
|---|---|---|
| Identity and TLS | Raw public keys (RFC 7250), mutual authentication, no SNI (so the endpoint ID is not in the ClientHello), AES-128-GCM required. The endpoint ID being the public key is from docs and prior sources; the algorithm (Ed25519) was not confirmed in the code excerpt. | [tls.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh/src/tls.rs) |
| 0-RTT | Enabled and supported; the docs warn to read the security notes (replay risk). Do not allow it for state-changing approvals. | tls.rs, endpoint.rs |
| What the relay sees | Source and destination IPs, times and volume, never payload. | [public relays](https://docs.iroh.computer/iroh-services/relays/public) |
| Discovery default | `PkarrPublisher` source says the default filter is `AddrFilter::relay_only()`, so only the home relay URL is published, to "avoid leaking IP addresses". The N0 preset code does not set a filter. [moq.dev](https://doc.moq.dev/concept/layer/iroh) claims the endpoint's addresses are published to n0 DNS regardless. These conflict; resolve by testing. | [pkarr.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh/src/address_lookup/pkarr.rs), [presets.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh/src/endpoint/presets.rs) |
| Privacy options | Records are signed and publicly resolvable by anyone who knows the ID. For Kelpie, skip discovery entirely: put id, relay URL and direct addresses in the QR code, with `RelayMode::Custom` and no n0 services. That means no contact with n0 at all. | address_lookup docs |
| Audit | None found for iroh, noq or iroh-relay. noq is a young hard fork of Quinn (since 2026-03). A pre-1.0 serde crash and relay DoS bugs were found by external reporters. | blogs above |

## 5. Alternatives, same axes

| | iroh | Own Noise-over-WS relay | WebRTC data channel | WebTransport | Tailscale | mosh/SSP | libp2p |
|---|---|---|---|---|---|---|---|
| Works in PWA | Relay-only WASM; Safari bug open | Yes | Yes (the only browser-native hole-punching) | Safari 26.4+ (2026-03-24), Baseline "Newly available"; WKWebView unconfirmed | Only via the VPN app | No (UDP) | Partly |
| Direct path | Yes, native | No | Yes via ICE | Only if the server has a reachable address; no NAT traversal | Yes | Yes, simple | Yes, about 70% |
| Rust daemon side | `iroh` (1.x) | You build it (small) | `str0m`: MIT/Apache, sans-IO, no TURN, P2P "less tested" than SFU use; `webrtc-rs` not checked | `wtransport` 0.7.1 seen on docs.rs, self-signed via `serverCertificateHashes` [U on current version] | Not embeddable (prior report) | You implement | rust-libp2p |
| Disconnect resilience | QUIC migration, but issues above | App-level resume (you have replay offsets) | ICE restart | QUIC migration | WireGuard roaming | Excellent (roams by crypto identity) | OK |
| Maturity / cost | 1.4.x, no audit, high churn | Low risk, you own it | Mature, TURN cost (Cloudflare TURN: 1,000 GB free, then $0.05/GB, from prior report) | New in Safari | Mature; peer relays GA Feb 2026 | Old, proven | Heavy, API churn |

Notes on the primary sources:
- **Tailscale** starts every connection on DERP and upgrades "after a few seconds" ([how NAT traversal works](https://tailscale.com/blog/how-nat-traversal-works)). It picks a home DERP by latency and falls back to the next region if one is unreachable ([DERP docs](https://tailscale.com/kb/1232/derp-servers)). Peer relays are GA, customer-run, and tried before DERP ([GA post](https://tailscale.com/blog/peer-relays-ga)). One anecdote reports a 12.5x speedup with a peer relay, single case.
- **SCTP head-of-line blocking:** RFC 8260 interleaving is mandatory for WebRTC data channels, and [Pion added it in May 2026](https://pion.ly/blog/sctp-interleaving). I did not verify that str0m or webrtc-rs implement it. Ordered channels still block on loss. For small messages that is mostly irrelevant; for bulk it matters.
- **mosh/SSP** ([paper](https://mosh.org/mosh-paper.pdf)): UDP, roaming by cryptographic credentials, AES-128-OCB, predictive local echo. A press report cites 4.8 ms median keystroke latency vs 503 ms for SSH on EV-DO 3G. The idea transfers (local echo masks RTT), but prediction works poorly with full-screen agent TUIs.
- **Keet/Holepunch:** direct hole-punch by key over a DHT, no relay by default; falls back to relaying through peers. No success-rate data.

## 6. Combination design: relay first, upgrade to direct

Industry pattern (Tailscale, iroh, ICE): start on the relay so users never wait, probe in the background, and swap paths transparently. Keet does the same through peers.

| Design | Fit | Verdict |
|---|---|---|
| Noise-over-WS relay, daemon dials out | Works in every browser and network (443). You own it. | Phase 1. |
| Same, plus a WebRTC data-channel upgrade for the PWA. SDP is signalled over the existing Noise channel, so the DTLS fingerprint is authenticated for free. STUN is free; TURN is only for hard cases. | The only browser-native direct path. Needs `str0m` plus ICE, maybe 1–2 weeks of work, and TURN or the relay as fallback. | Phase 3, only if spikes show direct beats the regional relay by a margin that matters. |
| Native app: iroh behind the same `Transport` trait, same frames inside | Gets QUIC, migration, NAT traversal and the Noise layer is optional. | Phase 2, when a native app exists. |
| WebTransport to the home PC's public IPv6 | No NAT traversal; depends on the home firewall allowing inbound, which often fails. Safari 26.4+ only. | Optional later spike for IPv6 users. |

**Is a direct path worth it for terminal and approval traffic?**

Latency estimate (derived, not measured). Relay RTT is roughly phone-to-relay plus PC-to-relay. From your numbers, PC to Singapore is about 80 ms. Phone to Singapore is probably 50–80 ms on VN mobile [U]. That makes a relayed round trip about 130–160 ms against perhaps 20–50 ms direct inside VN.

| Workload | Relay-only OK? |
|---|---|
| Approvals, chat | Yes. Latency is irrelevant. |
| Typing in a terminal | Acceptable at 150 ms with prediction; noticeable. |
| Screen or file streaming | Direct matters for bandwidth. |

The cheaper lever is relay placement: a relay in HCMC or Hanoi cuts the relayed RTT to about 40–60 ms [U]. That is the highest-leverage, lowest-risk step and works with either design. iroh does not prevent this; a self-hosted `iroh-relay` can sit anywhere.

## 7. Ranked recommendation

| Rank | Option | Rationale |
|---|---|---|
| 1 | Own Noise-over-WS relay in VN/HK/SG + app-level resume now. Define a `Transport` trait so frames are transport-agnostic. | Fast, secure, you own the failure modes. Browser-safe. Meets "few disconnects" because you control reconnects. |
| 2 | iroh (self-hosted relay, no n0 services) inside the native app, when one exists. | Native only. Gain depends on spike results. |
| 3 | WebRTC upgrade for the PWA. | Only if a measurable latency gain justifies it. |
| 4 | Do not: n0 public relays in production, iroh WASM in the PWA, libp2p, port-prediction hole-punching yourself. | |

Adoption risk for iroh: 1.x wire stability is promised, but there is no audit, noq is under a year old, relay patch cadence is every 2–4 weeks, and the CGNAT and mobile path bugs above are open.

## 8. Spike measurements needed

1. **Direct-path rate on VN networks.** Phone on Viettel, VNPT and Mobifone 4G/5G and on a Wi-Fi home line, to a home PC on VNPT, Viettel and FPT. Record direct vs relay share, time to direct, and any 50–75 s drops (#4424). Use iroh 1.4.x, with IPv6 on and off.
2. **Latency.** Keystroke echo RTT over: n0 Singapore relay, own relay at 2–3 locations (HCMC or Hanoi, HK, SG), and direct. Measure p50/p95.
3. **Time to first byte.** From cold start with a QR ticket (no discovery), to first echoed byte, relay then direct.
4. **Failover.** Kill the Wi-Fi, switch to cellular mid-session; kill the home relay; measure the stall time and whether the session survives. Compare with Noise-over-WS plus replay resume.
5. **iOS and Android background.** App suspended for 30 s and for 10 minutes; reconnect time; battery over one idle hour (the #4598 re-punch every 60 s).
6. **Binary size.** Tauri 2 mobile build with and without `iroh`, release plus LTO plus strip, APK/AAB and IPA.
7. **Discovery leak.** Packet capture of an endpoint with `RelayMode::Custom` and no address lookup, to confirm there is no n0 traffic. Also test the default N0 preset to resolve the pkarr vs moq.dev conflict.
8. **`iroh-relay` load.** Connections per core and RSS under 1k/10k idle and active clients, plus the HTTP access hook latency.
9. **Safari/WebKit.** WebTransport and WebRTC in WKWebView on iOS 26.4+, if the PWA direct path is pursued.

## 9. Unresolved questions

- Is the default N0 preset publishing direct IPs to n0 DNS or only the relay URL? The code and moq.dev disagree.
- Is #4424's remap-without-migration still present in 1.4.x?
- Does any iroh traffic on a custom `RelayMode` reach n0 servers?
- Does Cloudflare have a usable VN edge for a Durable Objects or Workers WebSocket relay, and what are the actual RTTs? [U]
- Do `str0m` and `webrtc-rs` implement RFC 8260 and ICE restart? `webrtc-rs` was not checked.
- Is `computer.iroh:iroh` on Maven actually broken on Android, and what is the Swift package's release status?
- Does `wtransport`'s latest version match 0.7.1? Does WKWebView expose WebTransport and `serverCertificateHashes`?
- Real iroh-relay memory and CPU per connection.
- Vietnamese carrier CGNAT types (Viettel, VNPT, Mobifone) are not documented anywhere I could find.
- Whether the actual Kelpie workload includes screen streaming, which changes the value of a direct path.

## Sources

- iroh: [1.0 post](https://iroh.computer/blog/v1) · [1.0.2](https://iroh.computer/blog/iroh-1-0-2) · [1.1.0](https://iroh.computer/blog/iroh-1-1-0) · [0.98.0](https://iroh.computer/blog/iroh-0-98-0-getting-back-to-traversing-nats) · [multipath](https://iroh.computer/blog/iroh-on-QUIC-multipath) · [noq](https://iroh.computer/blog/noq-announcement) · [holepunching docs](https://docs.iroh.computer/concepts/holepunching) · [pricing](https://iroh.computer/pricing) · [public relays](https://docs.iroh.computer/iroh-services/relays/public) · [WASM docs](https://docs.iroh.computer/deployment/wasm-browser-support)
- iroh source: [server.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/src/server.rs), [relay.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/src/protos/relay.rs), [client.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/src/client.rs), [tls.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh/src/tls.rs), [pkarr.rs](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh/src/address_lookup/pkarr.rs), [iroh-relay README](https://raw.githubusercontent.com/n0-computer/iroh/main/iroh-relay/README.md)
- iroh issues: [#4319](https://github.com/n0-computer/iroh/issues/4319), [#4424](https://github.com/n0-computer/iroh/issues/4424), [#4519](https://github.com/n0-computer/iroh/issues/4519), [#4598](https://github.com/n0-computer/iroh/issues/4598), [#4644](https://github.com/n0-computer/iroh/issues/4644)
- IETF [draft-bruynooghe-n0-quic-nat-traversal-00](https://www.ietf.org/archive/id/draft-bruynooghe-n0-quic-nat-traversal-00.txt) (July 2026)
- Third-party and other: [arXiv 2510.27500](https://arxiv.org/abs/2510.27500), [Pinggy](https://pinggy.io/blog/iroh_1_0_dial_keys_not_ips/), [iroh-kmp](https://klibs.io/project/Azula-App/iroh-kmp), [moq.dev](https://doc.moq.dev/concept/layer/iroh)
- Alternatives: [Tailscale NAT traversal](https://tailscale.com/blog/how-nat-traversal-works), [DERP docs](https://tailscale.com/kb/1232/derp-servers), [peer relays GA](https://tailscale.com/blog/peer-relays-ga), [str0m README](https://raw.githubusercontent.com/algesten/str0m/main/README.md), [Pion SCTP interleaving](https://pion.ly/blog/sctp-interleaving), [WebTransport baseline](https://web-platform-dx.github.io/web-features-explorer/features/webtransport), [mosh paper](https://mosh.org/mosh-paper.pdf), [HyperDHT docs](https://docs.pears.com/howto/connect-two-peers-by-key-with-hyperdht.html), [Vietnam IPv6](https://news.tuoitre.vn/vietnam-ranks-2nd-in-se-asia-7th-globally-in-ipv6-adoption-103260619162332916.htm)
