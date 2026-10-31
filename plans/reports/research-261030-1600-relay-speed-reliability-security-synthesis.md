# Relay: speed, disconnects, security (synthesis, 2026-10-30)

Inputs: `researcher-261030-1520-relay-cloudflare-and-hosting.md`, `researcher-261030-1520-iroh-p2p-deep-dive.md`. Two researchers could not write files, so the key content of their reports (Vietnam networks, relay hardening) is recorded here with sources.
Tags: [M] measured today from one Vietnamese home ISP (not mobile); [D] vendor doc; [T] third-party; [E] estimate; [U] unverified.

## 1. Measured today [M]
TCP connect from the user's PC, 5 samples each: Cloudflare (colo HKG, not a VN POP) 47-68 ms; iroh Singapore 76-88 ms (one 230 ms); iroh US/EU 255-280 ms (first up to 410 ms).
Not measured: the POP-to-DO hop, any mobile carrier, cross-ISP inside Vietnam.

## 2. Findings that decide the design
| Topic | Finding | Tag / source |
|---|---|---|
| DO location | None in Vietnam. SGN/HAN create DOs at SIN/HKG. `apac` lands in Japan/Korea ~39%, `apac-se` SIN 65 / HKG 35. Hint only on first `get()`; pinned for life. Omit the hint so the desktop's own `get()` places it | [D] data-location, [T] where.durableobjects.live |
| DO disconnects | Every code deploy, runtime update and eviction restart drops all sockets; gradual deploys do not help. Unplanned rate unknown | [D] websockets best practices |
| DO incidents | 2026-07-16 (1h38m, APAC + EU), 2026-10-29 (4 entries); HKG/SIN at least 3 in Aug 2026 (~6 h, aggregator data). Whether live sockets were cut is unknown | [D]/[T] |
| DO cost | ~$6/100 users, ~$19/1k, ~$188/10k per month with correct hibernation; if hibernation is defeated ~$4/room-month (1k rooms ~$4,000). No hard spend cap exists | [E] from [D] pricing, budget-alerts |
| VPS | $5-6/month in HKG/SIN; full control of restart timing. L3/4 DDoS usually means null-route of the IP | [D] price pages, TransIP/VPSDime docs |
| Latency estimate | CF DO ~110-130 ms; HKG VPS ~95-115; SIN VPS ~105-140; VN VPS ~20-60 [U]. The home leg dominates | [E] |
| Vietnam cables | 2026-08-25: 4 of 8 cables faulty, ~30% international capacity lost; AAE-1 down since Feb (~3,700 Gbps, ~10%). ~10 incidents/year; repairs months. Domestic services largely unaffected. SG/HK are where faulty cables land | [Light Reading](https://www.lightreading.com/cable-technology/four-subsea-cable-faults-hit-vietnam-s-internet), [w.media](https://w.media/four-out-of-six-subsea-cables-in-vietnam-out-of-service/), [VnExpress](https://e.vnexpress.net/news/tech/vietnam-innovation/submarine-cable-fault-hits-vietnam-international-internet-capacity-5044430.html) |
| Vietnam peering | Viettel and VNPT do not peer at VNIX, so a VN relay may be poor cross-ISP. Must be measured | [APNIC blog 2025-08-27](https://blog.apnic.net/2025/08/27/modernizing-viet-nams-internet-infrastructure-security-action/) |
| Vietnam IPv6 | 64.9% capable (2026-10-27). CPE firewalls still block inbound; direct IPv6 not dependable. CGNAT per plan, UDP blocking, NAT timeouts: no data found | [APNIC](https://data1.labs.apnic.net/v6stats/v6economy/VN.json), [Tailscale](https://tailscale.com/blog/how-nat-traversal-works) |
| Protocol | QUIC gains come from multiplexed web/video; 2024 scan: Cloudflare and Google do not seem to support connection migration; Workers have no WebTransport. Stay on WebSocket/TCP + app-level resume | [Meta](https://engineering.fb.com/2020/10/21/networking-traffic/how-facebook-is-bringing-quic-to-billions/), [arXiv 2410.06066](https://arxiv.org/abs/2410.06066), [D] workers protocols |
| Reconnect | Happy Eyeballs v3 draft: 250 ms stagger, keep earlier attempts, cancel on first success. Budget ~5 RTT (~300 ms) + radio promotion. Target foreground-to-live p50 < 1.2 s, p95 < 3 s (to verify) | [IETF draft-04](https://datatracker.ietf.org/doc/draft-ietf-happy-happyeyeballs-v3/) |
| Keepalive | Signal: 30 s app keepalive. Keep 20 s, may relax to 30 s when stable, foreground only | [Signal source](https://github.com/signalapp/Signal-Android/blob/main/lib/libsignal-service/src/main/java/org/whispersystems/signalservice/internal/websocket/OkHttpWebSocketConnection.java) |
| iroh | Hole-punch "90%" has no dataset; independent libp2p study 70% ± 7%. Open bugs on target networks (#4424 CGNAT drop every 50-75 s, #4644, #4319 ~30 s relay failover). No audit. PWA unsuitable. Only for a future native app, after a spike | iroh deep-dive report |

## 3. Relay hardening (performance cost in brackets)
Sources: [Noise spec](https://noiseprotocol.org/noise.html) sections 7.7, 7.8, 11.3, 11.4; [WireGuard protocol](https://www.wireguard.com/protocol/); Cloudflare pricing and [budget alerts](https://developers.cloudflare.com/billing/manage/budget-alerts/); [CGNAT rate-limit bias](https://www.theregister.com/2025/11/03/cloudflare_cgnat_bias_research/); [CVE-2026-30784 RustDesk](https://osv.dev/vulnerability/CVE-2026-30784); [CVE-2024-39894 OpenSSH chaff](https://www.freebsd.org/security/advisories/FreeBSD-SA-25:01.openssh.asc).

Must:
- mac1 prefilter on msg1, keyed from the desktop key / psk (~1 µs, +16 B). A garbage msg1 otherwise costs one X25519; the 4 MB/s room cap allows 30-40k msg1/s, a whole core [E].
- Daemon global handshake budget (e.g. 20/s, 5/s per slot, 16 pending) with silent drop; no side effects before the first valid initiator frame (0).
- Implicit-counter nonces, fail closed; heartbeat inside Noise catches truncation; `v` in the prologue, no downgrade retry (0).
- Room id = random + HMAC, checked in the Worker before any DO call (< 0.1 ms). Reject `Origin` and non-WS requests; serve no HTML (0).
- Per-credential limits; IP limits only on unauthenticated paths, since CGNAT makes per-IP limits unfair (0).
- Own Cloudflare account, budget alert + runbook, custom domain, WAF rule, per-socket message cap, no looping alarms (0).
- Revoke kills live sessions on all carriers; the daemon's allow-list is authoritative (0).
Should:
- Desktop slot admission by signature; relay tokens expiring in 30 days, rotated inside Noise; push and TURN in separate Workers/accounts.
- Noise Rekey() by counter; full re-handshake on reconnect and daily (10-30 ms in JS, 1 RTT).
- Pad control/chat frames to buckets. Raw-terminal chaff only (20 ms tick, ~10 ms added latency, ~7 KB/s while typing, identical frame sizes).
- Lower the room cap to ~2 GB/month; 24 h hashed-IP abuse state; publish spec and test vectors.
Skip: WireGuard cookies (IP meaningless behind a relay), TAI64N, handshake PoW, Turnstile, cover traffic, compression (CRIME-style risk, and ciphertext does not compress), paid audit for now.

Malicious relay: cannot read, impersonate, MITM pairing, downgrade or serve code. Can drop/delay (DoS), see metadata, replay msg1 (wasted CPU only), forge `desktop_offline` (treat as hints). Legal: US 18 USC 2258A has no monitoring duty; DMCA 512(a) conduit applies; realistic risk is account suspension on an abuse complaint, so publish an abuse contact and keep self-host possible. Not legal advice.

## 4. Options
| | R1 Two relays raced: CF DO + VPS HKG/SIN | R2 Two VPS, different providers | R3 CF DO only | R4 R1 + VN VPS as third racer |
|---|---|---|---|---|
| Disconnects | Best: provider diversity | Good | Weakest: deploys, incidents | Best |
| Speed (VN-VN) | ~95-130 ms [E] | ~95-140 ms [E] | ~110-130 ms [E] | ~20-60 ms if peering is good [U] |
| Survives cable faults | Partly | Partly | Partly | Best for domestic pairs [U] |
| DDoS | CF absorbs; VPS may be null-routed, race covers it | Weak | Strong | As R1 |
| Cost | ~$11/month + usage | ~$10-12/month flat | ~$6/month at 100 users | + ~$5/month |
| Ops | VPS patching | Most | Least | Most |

Common to all: same relay code (TS on Workers and `workerd`), relay list in the pairing record, staggered race with LAN/Tailscale candidates, relay-to-direct after 10 s stable, no compression, TCP_NODELAY, resume by revision.

## 5. Decision (user, 2026-10-30): one relay, Cloudflare
R1 (two relays) was rejected as needless work. A Hong Kong VPS was picked briefly, then replaced after the user asked why not Cloudflare, for scale and less server work. Chosen: a single Cloudflare Workers + Durable Objects relay, personal use first, after the Tailscale work. This is R3, close to the design approved on 2026-10-25.
- Why: nothing to patch or keep alive, no OS attack surface of ours, cost follows usage (~$6/100 users, ~$19/1k, ~$188/10k per month [E]), and DDoS is absorbed.
- Correction on data: the relay stores no content and cannot read it, on Cloudflare or on a VPS (Noise end to end). Only metadata differs: Cloudflare sees IPs, timing and sizes, as a VPS provider and its operator would. Cloudflare's edge is the visible difference: public incident history and no server for us to run, not more privacy.
- Accepted costs: about 0-25 ms slower than a same-city VPS [E]; sockets drop on deploys and runtime updates, and DO incidents happen. Mitigations: resume from phase 1, rare deploys, try the DO class in its own script (unverified), LAN and Tailscale stay in the race. The pairing record keeps a list with one relay entry.
- Cost guard: hibernation must hold (defeated, ~$4/room-month), no hard spend cap. Assert no per-socket timers, own account, budget alert, per-socket message cap, room ticket checked before any DO call.
- Placement: no `apac` hint; let the desktop's first `get()` place the object. DO is pinned for life, so re-create the room if RTT stays high.
- Validate on a real deployment (about 1 day plus a 7-day soak): DO colo, echo RTT on the user's ISP and Viettel, VinaPhone, MobiFone, disconnect rate, billing and hibernation.
- `workerd` self-host stays possible with the same code. iroh stays deferred to a future native app.

## Unresolved questions
- Echo RTT on Viettel, VinaPhone, MobiFone, FPT; cross-ISP RTT via a VN VPS.
- Do DO incidents cut live hibernating sockets? Does a router-only deploy keep them?
- Do auto-response replies bill? Are WAF-blocked requests billed?
- Hosted public relay or personal/self-host first (legal, abuse, budget)?
