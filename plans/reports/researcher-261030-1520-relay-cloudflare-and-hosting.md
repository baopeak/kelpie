# Relay on Cloudflare Workers + Durable Objects vs hosted alternatives (Vietnam / Asia)

Date: 2026-10-30. Scope: `phase-04-relay-connect-anywhere.md` (read first). Goal: max speed, fewest disconnects, VN/Asia users.
Tags: **[M]** measured by user today | **[D]** vendor doc, opened 2026-10-30 | **[T]** third-party / snippet only, not opened | **[E]** my estimate or inference | **[U]** unverified.
Bash was unavailable; all numbers come from WebFetch/WebSearch pages opened today.

## 0. Verdict (brutal)

- CF DO is fine on speed (HKG/SIN hosts DOs) but weak on the "fewest disconnects" goal. Every code deploy drops every socket [D]. 2026 shows many DO incidents, including 3+ in HKG/SIN in Aug 2026 [T] and 4 on 2026-10-29 alone [D].
- The home leg dominates latency. Desktop to HKG is 47-68 ms [M]; no relay placement can beat that leg. The relay can only add 0-30 ms on top.
- No DO or Cloudflare colo exists in Vietnam (SGN/HAN do not host DOs) [T: where.durableobjects.live]. "Added ≤30 ms in-region" in phase 4 cannot be true for VN. It is true only in HKG/SIN.
- Plan changes, Argo, and Containers do not fix routing or deploy drops (Section 1, 4).
- Best result: **two relay endpoints on different providers, client races them**. CF DO is one of them (zero ops); a ~$5-6 VPS in HKG/SIN is the other (Section 7).

## 1. Durable Object location and latency

| Question | Answer | Source |
|---|---|---|
| Where do DOs run | Only ~10.61% of CF PoPs host DOs (2026-10-30 08:15 UTC) | [T] where.durableobjects.live |
| In Vietnam? | No. SGN/HAN "do not host DOs". DO created from SGN lands at SIN (100%, site-reported 71 ms); from HAN lands at HKG (100%, 75 ms) | [T] where.durableobjects.live/colo/SGN, /HAN |
| Asia DO colos | SIN, HKG, NRT, KIX, ICN seen. `apac`: SIN 41.1%, HKG 19.5%, NRT 15.8%, KIX 15.4%, ICN 8.1% | [T] /region/apac |
| `apac-se` (added 2026-06-19) | SIN 65.5%, HKG 34.5% | [D] changelog 2026-06-19, [T] /region/apac-se |
| `locationHint` | Works, best effort, **only first `get()` of an object respects it**. Values: wnam enam sam weur eeur apac apac-ne apac-se oc afr me. sam/afr/me do not host, spawn nearby | [D] reference/data-location |
| Default (no hint) | Object created near the first `get()` caller | [D] |
| Jurisdictions | Only `eu`, `us`, `fedramp`. None for Asia; irrelevant here | [D] |
| Pinned for life | Yes today. "Do not currently change locations after they are created"; relocation "planned" | [D] |

Implications:
- Do NOT use `apac`: 39% chance of Japan/Korea for a VN user. `apac-se` is a coin flip SIN/HKG (65/35) and cannot be forced.
- Phase 4 says "room created with `locationHint` from the desktop". Better: **omit the hint and let the desktop's own Worker call `get()` first**. The DO then lands at the colo nearest the desktop's entry POP (HKG for the tested PC). Pinned for life, so a user who moves ISP or country keeps a stale location. Add "re-create room on high RTT" logic.
- `site-reported ms` figures (SIN 28 ms, HKG 57 ms, SGN→SIN 71 ms) have no stated method [U]. Do not budget with them.

### Path and added latency

phone → nearest CF POP (P_p) → backbone → DO host → backbone → POP (P_d) → desktop.
RTT(phone↔desktop) = RTT(phone,P_p) + RTT(P_p,DO) + RTT(DO,P_d) + RTT(P_d,desktop). Relay "added" = the two middle terms.

| Term | Value | Tag |
|---|---|---|
| desktop → CF HKG TCP connect | 47-68 ms | [M] |
| desktop → iroh SIN relay | 76-88 ms | [M] |
| desktop → iroh US/EU | 255-280 ms | [M] |
| POP and DO in same colo | ~1-5 ms | [E] |
| HKG ↔ SIN backbone RTT | ~30-40 ms | [E] public knowledge, not sourced |
| Worker→DO measured numbers published by CF | none found | search 2026-10-30 |
| Forum anecdote "never under 120 ms Worker→DO" | ~2 yrs old, unreliable | [T] answeroverflow |
| colo.do sample (ORD, same colo) | 20 ms Worker→DO | [T] github drivly/colo.do |

Scenario (all [E], VN mobile phone assumed to land on SGN POP at ~20 ms):

| Design | Phone leg | Middle | Desktop leg | Total RTT |
|---|---|---|---|---|
| CF DO at HKG (created from desktop) | ~20 | ~40 (SGN→HKG) + ~3 | 47-68 [M] | ~110-130 |
| CF DO at SIN | ~20 | ~5 + ~35 (SIN→HKG POP) | 47-68 | ~110-130 |
| HKG VPS | ~45 phone→HKG | 0 | ~47-68 | ~95-115 |
| SIN VPS | ~30-50 | 0 | 76-88 [M, iroh SIN] | ~105-140 |
| VN-hosted VPS (HAN/SGN, same-country) | ~10-30 | 0 | ~10-30 | **~20-60** [U] |

Reading: CF vs a same-city VPS differs by ~0-25 ms. A **Vietnam-hosted relay** is the only option that could change the order of magnitude when both ends are in VN (cross-ISP domestic peering is the risk [U]).

### Why VN ISPs land on HKG/SIN; do plans or Argo help

- CF doc: the visitor's ISP picks the data center from "its own routing and peering policies", which "do not always favor the geographically closest location"; "reliability takes priority"; selection "cannot be configured per zone, per account, or by support request" [D] developers.cloudflare.com/support/troubleshooting/general-troubleshooting/geographic-traffic-routing.
- Free-vs-Paid routing differences: the CF doc does not describe any. Anecdotal forum/DEV claims only (Ghost forum Bangalore, DEV post) [T/U]. Cheap test: same PC hits `cdn-cgi/trace` on a Free zone, a Pro zone, and `*.workers.dev`; compare `colo=`.
- Argo Smart Routing: docs do not mention Workers, DOs or WebSockets, give no latency figure, no price on the page [D]. Third-party: $5/mo + $0.10/GB, ~30-35% marketing claim [T]. Argo optimizes POP→origin; a DO relay has no origin. Verdict: skip [E].
- Cloudflare CDN peering to VN ISPs: could not verify (no Radar data, 403) [U].

## 2. WebSocket-on-DO limits and billing

### Limits [D unless noted]

| Item | Value |
|---|---|
| Incoming WS message size | 32 MiB (raised from 1 MiB, 2025-10-25) |
| WS per DO | max 32,768; CPU/memory may bind earlier |
| Request rate per DO | soft 1,000 req/s; overflow returns overloaded error. WS incoming messages count as requests |
| CPU per event | 30 s default (SQLite backend), up to 5 min; each message restarts budget |
| Memory | 128 MB billed per object regardless of use |
| `serializeAttachment` | 16,384 B; lost if socket closes |
| `setWebSocketAutoResponse` | one string pair, each ≤2,048 chars; no wake, no duration charge; `getWebSocketAutoResponseTimestamp` shows last reply |
| Hibernation | after 10 s idle; in-memory state lost; constructor re-runs on wake. Non-hibernatable idle: evicted at 70-140 s |
| Keeps awake | alarms, requests, scheduled callbacks, open outbound WS (up to 15 min) |
| `setHibernatableWebSocketEventTimeout` | up to 7 days |
| Close auto-reply | `web_socket_auto_reply_to_close` flag, compat date ≥ 2026-04-07. Without a reciprocated close the client sees 1006 |
| Batching advice | "every 50-100 ms or 50-100 messages" (too slow for keystroke echo; use for output only with ≤8-16 ms) |
| Edge idle timeout | exists, length not documented; enterprise-configurable; keepalive pings recommended [D network/websockets] |
| Account limits | Free: 100 DO classes, SQLite only, 5 GB. Paid: 10 GB per object |

### Deploys, eviction, resets

- "Code updates disconnect all WebSockets." "Deploying a new version restarts every Durable Object, which disconnects any existing connections." [D best-practices/websockets]
- Restart causes listed: new deployments, eviction, **Workers runtime updates**, "runtime decisions about where to host objects". Frequency: "occasionally", no number [D lifecycle]. Cannot quantify unplanned-reset rate from docs [U]. Measure it (Section 8).
- Gradual deployments do not help: "only one version of each Durable Object can run at a time" [D]. Version affinity does not apply to a DO.
- Edge: "we may restart servers, which terminates WebSockets" [D network/websockets].
- Possible mitigation [E, unverified]: put the DO class in its own Worker script (cross-script DO binding) and deploy only the router Worker routinely. DO restarts then only occur on DO-script deploys and runtime updates. Verify in spike.
- `workerd` self-host has the same drop-on-restart behavior; the phase-4 design already handles it (restart close code + jitter).

### Billing [D developers.cloudflare.com/durable-objects/platform/pricing, workers/platform/pricing]

| Item | Rate |
|---|---|
| Workers Paid | $5/mo min; 10M Worker requests, then $0.30/M; WS upgrade = 1 request, messages through Worker not billed |
| DO requests | 1M incl., then $0.15/M. Connect = 1. Incoming messages 20:1 (100 msgs = 5 requests). Outgoing not charged. Incoming protocol pings not charged |
| DO duration | 400,000 GB-s incl., then $12.50/M GB-s, 128 MB per object |
| Hibernation | idle hibernatable objects incur no duration; auto-responses incur no wall-clock |
| Non-hibernating (plain `accept()`) | duration for whole connection time: 128 MB × 2.59M s = **324k GB-s ≈ $4.05 per room-month** if open 24/7 |
| Free plan | 100k DO requests/day, 13k GB-s/day, reset 00:00 UTC. Hitting the cap stops service for everyone |
| SQLite storage | rows read/written billed; each `setAlarm()` = 1 row write; storage billing started 2026-01-07 |
| Egress | none for Workers/DO per phase-4 text [U, not re-verified today] |

CF's own worked example 4 (100 hibernating DOs × 100 conns, 100 msg/min per object): 552,960 GB-s total = ~10 ms billed per message, not 10 s idle. So the 10 s pre-hibernation idle is **not** billed in CF's example [D, inference E]. Confirm in spike billing analytics.

### Cost model (rooms = active users; 1 desktop + ~1 phone socket) [E, from the rates above]

Assumptions: per room-month 20 h streaming at ~10 incoming msgs/s = 720k msgs = 36k billed requests; +14k billed for heartbeats (~13k/user matches phase 4); duration ~10 ms per message (CF example) = ~900 GB-s. Correct hibernation API use.

| Users | DO requests (billed) | Request $ | Duration GB-s | Duration $ | Total/mo (incl. $5) |
|---|---|---|---|---|---|
| 100 | 5M | ~$0.60 | 90k (in 400k) | $0 | **~$6** |
| 1,000 | 50M | ~$7.35 | 0.9M | ~$6 | **~$19** |
| 10,000 | 500M | ~$75 | 9M | ~$108 | **~$188** |

Worker-side cost is extra: 1 request per connect + CPU ms; negligible (<$5 at 10k).
If hibernation is defeated (setInterval, outbound fetch, plain `accept()`, alarm loop, hb faster than the platform can idle): 324k GB-s per room-month → 1,000 rooms ≈ $4,000/mo; 10,000 ≈ $40,000/mo. This is the single biggest cost risk. Perf gate must assert "no setInterval/alarm per socket; DO hibernates when idle".
Bandwidth for VPS comparison: 1 GB/user/mo is plenty for terminal traffic. Screen streaming (later) changes everything (TURN/direct, not relay).

### Cost attack by a malicious client [E]

| Vector | Cost to victim | Defense |
|---|---|---|
| Open sockets to random room ids to wake DOs | each connect = DO request + new DO + SQLite rows | Worker validates room credential (HMAC of `room_id`, or signed pairing token) **before** `DO.get`; per-IP creation limit via Workers Rate Limiting [U not opened]; Turnstile/PoW for creation |
| Paired-but-malicious room, floods 1,000 msg/s (soft cap) | 50 billed req/s ≈ 130M req/mo ≈ **~$19 + duration up to ~$4 ≈ ~$23 per room-month** | Per-socket msg rate limit in DO (close at e.g. >200 msg/s); billing happens on arrival, so closing stops it quickly; 4 MB/s cap already in plan |
| Prevent hibernation to inflate duration | up to $4.05 per room-month | no timers; idle socket close after N min without pair-complete |
| Many rooms (storage) | $0.20/GB-month; row writes $1/M | cap rooms per IP, 90-day expiry already planned; expiry alarm = 1 row write per room |
| Free-plan daily cap | Whole relay down until 00:00 UTC | use Paid ($5); alert at 50% of usage |

## 3. Reliability record

| Date (UTC) | Event | Duration | DO/Workers impact | Tag |
|---|---|---|---|---|
| 2024 | not researched (no data gathered) | | | [U] |
| 2025-06-12 | Workers KV storage dependency on third-party cloud; KV, Access, WARP, Workers AI affected | up to ~2h28m | KV-dependent Workers; DO not named | [T] secondary |
| 2025-11-18 11:20-17:06 | Bot-management feature file doubled, core proxy panic, 5xx | core ~3h10m, full ~5h46m | KV 5xx; Workers/DO/WebSockets not discussed in post | [D] blog 18-november-2025-outage |
| 2025-12-05 08:47-09:12 | WAF change, FL1 proxy bug, HTTP 500 | ~25 min, ~28% of CF HTTP traffic | `/cdn-cgi/trace` mostly unaffected | [D] blog 5-december-2025-outage |
| 2026-04 | APAC DO errors; Amsterdam overload | not stated | DO | [T] |
| 2026-05-19 | DO + R2 degraded latency WNAM | ~2h | DO | [T] |
| 2026-07-16 16:39-18:17 | Increased errors DO + downstream, Europe + APAC | 1h38m | DO, D1, Containers, Workflows | [D] cloudflarestatus.com/incidents/kdnfshk5vs51 |
| 2026-08-17 | DO errors SIN | ~1.3h | DO | [T] isdown |
| 2026-08-19 | DO overload errors HKG | ~25 min | DO, Realtime SFU | [T] |
| 2026-08-24 03:53-08:28 | DO + D1 degraded HKG and SIN | ~4.6h | DO | [T] openstatus |
| 2026-09-17/18 | DO errors APAC | 1h16m | DO | [T] |
| 2026-09-25 | Workflows stuck then DO errors | ~14h45m | DO | [T] |
| 2026-10-29 | (a) Worker services errors 09:22-11:49; (b) DO availability E. Europe 10:06-11:47; (c) DO/D1/Workflows errors 14:16-16:33; (d) DO errors ENAM 16:00-16:50 | 2h26m / 1h41m / 2h17m / 50m | DO | [D] cloudflarestatus.com/history |

Notes:
- Status-page entries say "increased errors"; whether established WebSockets were dropped is not stated [U]. New connections and DO-backed state are what fail.
- HKG/SIN DO incidents: at least 3 in 8 days of Aug 2026 totaling ~6.2 h [T]. This is the exact region a VN relay would use.
- The Nov and Dec 2025 outages were core-proxy failures. A VPS relay is immune to those by construction; a Workers custom-domain relay is not (routed via the proxy) [E].
- Verdict: **single-provider CF relay is not acceptable for "fewest disconnects"**. Acceptable as one of two racing endpoints. Fetched only first-party sources for 2025-11/12, 2026-07-16, 2026-10-29; the rest are aggregator snippets.

## 4. Non-DO Cloudflare options

| Option | Fit | Evidence |
|---|---|---|
| Realtime TURN | Anycast, allocation homed at nearest CF DC; UDP relaying only (RFC 6062 TCP not implemented); TURN over TLS to client; $0.05/GB after 1,000 GB free; warns drops above ~50-100 Mbps / 5-10 kpps per client; creds ≤48 h. Needs WebRTC/ICE (the design has TURN in phase 8 already). Not a drop-in WS relay | [D] realtime/turn/faq |
| Realtime SFU / RealtimeKit | Media-oriented; DataChannels exist but not described as a generic relay; WS adapter is PCM/JPEG bridging only. No. | [D] developers.cloudflare.com/realtime |
| WebTransport / raw UDP on Workers | **Not listed** as inbound protocol. Inbound HTTP/3 (zone toggle) listed; raw inbound TCP "coming soon" | [D] workers/reference/protocols |
| Queues / pub-sub pattern | Queues is async, batch consumers; CF-reported median send ~60 ms. Unsuitable for interactive terminal. CF Pub/Sub not present in docs index today | [T] blog how-we-built-cloudflare-queues; [D] pub-sub page does not mention it |
| Workers without DO | Workers are stateless and per-isolate; two sockets cannot meet. KV is eventually consistent. Impossible without DO/external state | [E] |
| Containers | Reached only through a Worker, no inbound raw TCP/UDP; placement "not guaranteed" near the DO; cold start 1-3 s; egress $0.025/GB NA/EU, $0.05 Oceania/Korea/Taiwan, $0.04 elsewhere (500 GB incl.); rollouts send SIGTERM and wait up to 15 min (then SIGKILL), but sockets still pass via DO/Worker. Adds a hop and cost; no edge relay advantage | [D] containers/platform-details, pricing, rollouts |

## 5. Alternatives compared

Latency uses desktop-leg measures where available; other cells are [E]/[U]. Prices from pages opened today unless tagged.

| Option | Asia latency | Price | Sticky long WS | Deploy without dropping | Ops |
|---|---|---|---|---|---|
| CF DO (baseline) | HKG/SIN POP+DO, ~0-25 ms over VPS | $5 + usage (~$6/100u, ~$19/1k u) | yes (hibernation) | **No** (all sockets) | lowest |
| Fly.io | regions `sin`, `nrt` listed (no `hkg` in current table [D docs.fly.io/reference/regions]); anycast shared IPv4/IPv6 free | $2.19/mo shared-1x 256MB; egress Asia/Oceania/SA $0.04/GB, NA/EU $0.02, inbound free; dedicated IPv4 $2 | proxy forwards TCP; idle_timeout configurable | rolling (default), bluegreen, canary, immediate. Old Machine gets `kill_signal` (default SIGINT), `kill_timeout` default 5 s, max 300 s [D]. Sockets still end at kill; only drain window | low-medium |
| VPS Singapore / HK / Tokyo | direct to host; tested SIN 76-88 ms [M, iroh], HKG 47-68 to CF edge [M] | Vultr: overage $0.01/GB flat [D], $5 plan 1-2 TB (sources conflict [T]). DO $4/mo 512 MiB 500 GiB [D]. Hetzner SG: CPX12 €15.99/mo (1 vCPU/2 GB), 0.5-5 TB incl., overage $8.49/TB [D hetzner.com/cloud-singapore] | yes | restart drops sockets, but timing is yours; `SO_REUSEPORT` + drain gives 1 RTT reconnect with app resume | you: TLS, patching, DDoS |
| VN provider (Viettel IDC, FPT) | possibly 10-30 ms for VN↔VN; cross-ISP peering unknown | Viettel IDC T1.Base 01 129,000 VND/mo (~$5) "150 Mbps clean traffic"; T2.Gen 01 900,000 VND, 4 vCPU/4 GB, 300 Mbps unmetered [T whtop.com aggregator, VAT excluded] | yes | as VPS | KYC/contract friction [U]; poor for non-VN users |
| AWS Global Accelerator | AWS edge → ap-southeast-1; VN edge presence unknown [U] | $0.025/h per accelerator (~$18/mo) + DT-Premium from APAC to VN $0.032/GB, within APAC $0.010 [D]; plus the EC2 behind it | yes (TCP) | endpoint health shift, sockets still drop | medium; overkill |
| Fastly Fanout | Asia POPs not stated | Compute-only; billed on bandwidth, connection-minutes, messages; no public rates ("contact sales") [D] | designed for it | n/a | no; pub/sub model, not opaque pipe |
| Ably | "11+ regions, 700+ PoPs"; Asia not named [D] | Free: 6M msgs, 200 conns, 64 KiB msg. Standard $29 + $2.50/M msgs + $1/M conn-min; no SLA below Enterprise (SLO) | yes | managed | rank low: fan-out counted per delivery. 1,000 users × ~1.4M msgs ≈ $3.5k/mo [E] |
| Pusher Channels | Singapore cluster not confirmed on page [U] | Sandbox 100 conns/200k msg/day; $49: 500 conns/1M msg/day; $99: 2,000/4M [D] | yes | managed | rank low; message/day caps clash with terminal traffic |
| Self-written Rust relay (axum/tokio-tungstenite, same stack as daemon) | same as VPS | VPS only | yes | same as VPS | needs new code; contradicts "one TS codebase" in phase 4 |

Rust relay footprint: I found **no credible per-connection memory or throughput figure** for tokio-tungstenite/fastwebsockets (sockudo-ws and others publish msg/s only) [U]. Engineering expectation [E]: each idle socket ≈ one tokio task + small buffers, order of 10-20 KB plus kernel socket buffers; thousands of sockets fit in tens of MB; a single vCPU is not the bottleneck at this traffic. Verify with a 5,000-idle-socket test (Section 8). `workerd` on a VPS is a lighter lift than a Rust rewrite and keeps the single codebase.

## 6. Relay design for speed

| Topic | Recommendation | Evidence |
|---|---|---|
| TCP_NODELAY | Set on every hop: desktop dial, phone, VPS listener (`TcpStream::set_nodelay(true)`). Small interactive frames suffer 40 ms Nagle × delayed ACK otherwise. CF edge handles its side | [E] standard; DERP code buffers+flushes per frame |
| Write coalescing | Keystrokes/approvals: flush immediately. Terminal output: coalesce 8-16 ms or 16 KiB, whichever first, one Noise frame per batch. CF's 50-100 ms batch hint is for cost, too slow for echo. Fewer frames also means fewer billed DO requests and fewer AEAD ops | [D] CF websockets doc for the hint; [E] rest |
| Frame size | Keep ≤64 KiB (matches DERP `MaxPacketSize = 64<<10`) | [D] derp.go on GitHub main |
| permessage-deflate | **Do not enable on the relay hop.** Payload is Noise ciphertext, incompressible; deflate burns CPU and, with context takeover, holds up to 32 KiB window per connection. RFC 7692 allows window bits 8-15 and `*_no_context_takeover` to bound memory | [D] RFC 7692 |
| Compress before encrypt | Only if terminal bandwidth proves a problem. Noise hides content but **not length**; the relay or a path observer sees ciphertext size. Compression before encryption gives a CRIME-style length oracle when attacker-influenced text (e.g. repo content, web page, prompt injection) shares a compressed stream with a secret (tokens in env output). RFC 7692 explicitly warns about history-based compression with secure transport. If used: per-frame independent compression (no shared history), pad to size buckets, never compress frames mixing input from different trust domains. Default: off for v1 | [D] RFC 7692 security considerations; [E] threat reasoning; TLS 1.3 dropped compression for this reason [E, not re-fetched] |
| Backpressure | Bounded per-slot send queue (e.g. 32-64 frames AND byte cap). Per-phone queues independent so one slow phone never blocks desktop. On overflow or write timeout, close slot with `slow_consumer`; client resumes via existing replay offsets. Never buffer unbounded in relay | [D] DERP: default per-client send queue 32 packets, TCP write timeout 2 s (30 s for mesh), drop-reason metrics for queue head/tail |
| Keepalive | CF edge ping/pong via `setWebSocketAutoResponse` every 20 s (free). On VPS use WS ping 20 s. Keep application `hb` inside Noise only while a phone is attached (idle desktop-only room then costs ~0) | [D] |
| Idle memory | Pool buffers, allocate on demand (DERP pools rings and write buffers held only during writes) | [D] derpserver.go |

Tailscale DERP, what is actually documented:
- DERP is a last-resort relay over HTTPS/TCP; clients choose a "home" DERP by latency, fail over inside the region, then to the next closest region [D tailscale.com/kb/1232/derp-servers]. Asia DERP: Singapore, Hong Kong, Tokyo, Bengaluru, Dubai.
- Tailscale docs say DERP limits throughput for fairness and has varied QoS [T snippet]; a Jan 2026 post says relays optimize availability/coverage, not raw throughput [T snippet]. Not opened.
- Code facts [D]: frame write then `Flush()`; histogram of frames per flush; per-client queue 32; write timeout 2 s; keepalive 60 s; pooled power-of-two buffers 1 KiB..64 KiB.
- No Tailscale tuning write-up on NODELAY or kernel settings found [U]. Hyper-DERP (self-reported, io_uring/kTLS): 19.9 Gbps vs 7.8 Gbps Go derper on 25GbE [T, not independent]. Irrelevant at Kelpie's scale.
- Takeaway: Tailscale's strategy for speed is geography (nearest region, multi-region failover), not clever TCP tuning. That matches Section 7.

## 7. Ranked recommendation (Vietnam / Asia)

Note: the hosted-on-CF + `workerd` self-host design was user-approved 2026-10-25. Rank 1 below changes the hosted default to "two endpoints raced", keeping the same TS code. Needs your confirmation before any plan edit.

| Rank | Option | Why |
|---|---|---|
| 1 | **Two-endpoint race, same relay code**: CF DO (no hint, created from desktop's Worker) + one `workerd` (or later Rust) VPS in HKG or SIN. Desktop registers on both; phone races both with the 250 ms stagger already planned; first handshake wins; fail over on `relay unreachable`. Fix phase-4 wording: no `apac` hint, no "≤30 ms in region" claim for VN | Provider diversity covers CF incidents (Section 3); VPS covers deploy-drop control; CF covers VPS outage. Cost ~$5 + $5-6 flat. Reuses existing code, deploy flow, and protocol |
| 2 | **VPS only** in HKG (or SIN) with `workerd`/Rust, plus a standby VPS at another provider | Best control of disconnects and flat cost; weaker if the single host dies and clients have no second endpoint. Pick this if you refuse CF dependency |
| 3 | **CF DO only** | Lowest ops and fine speed, but fails "fewest disconnects": deploy drops all sockets, many 2026 incidents in HKG/SIN. Acceptable only with jittered resume and a separate-script DO deploy pattern |
| 4 | Fly.io `sin`/`nrt` | Anycast + rolling deploys, cheap, but no `hkg` in docs, and deploys still end sockets; adds nothing a VPS does not give at this load |
| 5 | VN-hosted VPS as a **third racer** | Only candidate for large latency wins when both phone and desktop are in VN. Defer until spike data proves cross-ISP RTT; poor for non-VN users |
| 6 | AWS Global Accelerator, Ably, Pusher, Fastly Fanout, CF Containers/Realtime SFU | Cost or model mismatch; no speed gain shown |

Adoption risk: DO location pinned for life [D]; DO pricing/limits have changed repeatedly (WS message limit 1 MiB→32 MiB in Oct 2025; SQLite storage billing from Jan 2026). VPS risk is ops (patching, DDoS, TLS) and a single failure domain. Both mitigated by racing.
Team fit: TS relay already planned for `workerd` self-host, so rank 1 adds a Docker deployment and a client "multiple relay URLs" list, no new language.

## 8. What to measure in the spike (3-4 days)

1. **Routing per network**: from at least 3 VN fixed ISPs (VNPT, Viettel, FPT) and 2 mobile carriers, record `cdn-cgi/trace` `colo=` on a Free-plan zone, a Paid-plan zone, and `*.workers.dev`. Repeat at peak evening. Confirms/denies plan-based routing.
2. **DO placement**: create 50 rooms from the desktop with no hint, 50 with `apac-se`, 50 with `apac`; record the DO colo (response header or `cf` trace from inside DO). Compare to 65/35 and 41/19/16/15/8.
3. **Echo RTT** phone-sim → relay → desktop-sim, 1,000 samples, p50/p95/p99, for: CF DO (HKG-pinned vs SIN-pinned), `workerd` on VPS in HKG, SIN, Tokyo, and one VN VPS (Viettel IDC/FPT). Use fixed 100-byte and 1 KiB frames; test both phone-on-Wi-Fi and phone-on-4G/5G.
4. **Added ms**: same-POP vs different-POP Worker→DO RTT (instrument with `performance.now()` in DO, log).
5. **Disconnect rate**: soak 20 sockets × 7 days on DO and on VPS; log close code, `wasClean`, timestamps; correlate with deploys, CF status page, and runtime updates. Gives the unplanned reset rate (currently [U]).
6. **Deploy behavior**: deploy only the router Worker vs only the DO-script; count dropped sockets. Deploy `workerd` with `SO_REUSEPORT`/drain.
7. **Billing**: after 24 h, read Workers/DO analytics for GB-s and billed requests; confirm (a) idle 10 s before hibernation is not billed, (b) auto-response pings are not billed, (c) `desktop_offline` alarm row writes.
8. **Hibernation guard**: assert DO is evicted/hibernated within ~15 s of last event with 8 idle sockets; assert no per-socket timers.
9. **Abuse**: flood 1,000 msg/s into one room and into 1,000 random room ids; confirm Worker gate stops the second case before the DO; measure billed requests.
10. **Throughput/backpressure**: slow phone (1 Mbps shaped) while desktop streams 4 MB/s; verify slot closes with `slow_consumer`, desktop and other phones unaffected; RSS for 5,000 idle sockets on a 1 vCPU/1 GB VPS.
11. **Nagle**: p99 echo with and without `TCP_NODELAY` on the desktop and VPS side.

## 9. Limitations

- Single-ISP measurements only [M]; no mobile carrier data; no VN peering evidence for CF found.
- No first-party Worker→DO latency numbers exist; all middle-hop numbers are estimates.
- Incident list for 2026 mixes first-party (Jul 16, Oct 29) and aggregator snippets (the rest); 2024 not covered.
- Did not open: Tailscale Jan 2026 blog, AWS GA edge locations, Linode/Akamai, Alibaba/Tencent HK, Workers Rate Limiting docs, Fly hkg deprecation.
- Pricing pages change; Hetzner SG figures conflicted between aggregators and the first-party page (used first-party).

## Unresolved questions

1. Does deploying only the router Worker (DO class in a separate script) keep existing DO sockets alive? Docs say "new version restarts every DO" without naming scope.
2. Is the 10 s pre-hibernation idle billed? CF worked example 4 implies no; confirm via analytics.
3. Do auto-response replies count as billed incoming requests? Docs say pings are free; replies not stated.
4. Which colo do VN mobile carriers hit (SGN/HAN vs HKG/SIN), and does a Paid zone change it?
5. Is there a real latency gap between VN-hosted and HKG-hosted relays for VN↔VN pairs (cross-ISP peering)?
6. Does Fly still offer `hkg`? Current docs table shows only `nrt` and `sin`.
7. Rate of unplanned DO restarts/close events per week in HKG/SIN.
8. Do 2026 DO "increased errors" incidents actually sever established hibernating sockets, or only fail new events?
9. Budget for a second endpoint and whether you accept changing the approved hosted default (Section 7 note).

## Sources (all opened 2026-10-30 unless marked)

- https://developers.cloudflare.com/durable-objects/reference/data-location/ ; https://developers.cloudflare.com/changelog/post/2026-06-19-apac-ne-apac-se-location-hints/
- https://developers.cloudflare.com/durable-objects/platform/limits/ ; /platform/pricing/ ; /platform/changelog/ ; /platform/known-issues/
- https://developers.cloudflare.com/durable-objects/best-practices/websockets/ ; /concepts/durable-object-lifecycle/ ; /api/state/
- https://developers.cloudflare.com/workers/platform/pricing/ ; /workers/reference/protocols/ ; /workers/configuration/versions-and-deployments/gradual-deployments/
- https://developers.cloudflare.com/network/websockets/ ; /support/troubleshooting/general-troubleshooting/geographic-traffic-routing/ ; /argo-smart-routing/
- https://developers.cloudflare.com/realtime/ ; /realtime/turn/faq/ ; /containers/ ; /containers/platform-details/ ; /containers/pricing/ ; /containers/platform-details/rollouts/
- https://where.durableobjects.live/ , /colo/SGN , /colo/HAN , /colo/HKG , /colo/SIN , /region/apac , /region/apac-se (site-reported, method unstated)
- https://github.com/drivly/colo.do
- https://www.cloudflarestatus.com/history ; https://www.cloudflarestatus.com/incidents/kdnfshk5vs51
- https://blog.cloudflare.com/18-november-2025-outage/ ; https://blog.cloudflare.com/5-december-2025-outage/
- Aggregator snippets (not opened): isdown.app Cloudflare incident pages 642901, 640837, 656491; uptimerobot/openstatus Cloudflare DO pages
- https://docs.fly.io/reference/regions ; /about/pricing ; /reference/configuration/ ; /launch/deploy/
- https://www.hetzner.com/cloud-singapore/ ; https://egresscost.com/hetzner ; https://docs.vultr.com/support/platform/billing/what-is-the-bandwidth-overage-rate ; https://www.digitalocean.com/pricing/droplets
- https://www.whtop.com/plans/viettelidc.com.vn/ (aggregator snippet)
- https://aws.amazon.com/global-accelerator/pricing/ ; https://docs.fastly.com/products/fanout ; https://ably.com/pricing ; https://pusher.com/channels/pricing/
- https://tailscale.com/kb/1232/derp-servers ; https://github.com/tailscale/tailscale/blob/main/derp/derp.go ; .../derp/derpserver/derpserver.go ; https://github.com/hyper-derp/Hyper-DERP
- https://datatracker.ietf.org/doc/html/rfc7692
- Phase doc: D:\Works\Kelpie\plans\261024-1205-phone-remote-control\phase-04-relay-connect-anywhere.md
