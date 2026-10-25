# Unstable-network resilience: requirements for phone-remote-control plan

Date 2026-10-25. Scope: plan `261024-1205-phone-remote-control` phases 1,3,4,6,7,8 read in full; phases 2,5 not read (UX/platform triggers mapped to them by inference). ~22 fetches/searches. Tags: [V] verified in a fetched source this session, [U] unverified (memory / weak source / fetch failed). Numbers marked "pick" are my design choices, not sourced.

## 0. Verdict (brutal)

Plan's skeleton is sound (state-sync, per-device resume, latest-only, first-wins answers, connect race). Holes that will bite on a real phone:

1. **Noise stateful transport over a lossy relay hop desyncs.** Any desktop<->relay or phone<->relay socket flap loses frames in flight; the next frame fails decrypt (nonce counter). Plan has no `peer_reset` rule. Must add (Phase 3/4).
2. **No app-level heartbeat / dead-peer spec anywhere.** Browser WS and iOS URLSession detect half-open sockets in 1-3 min or never [V weak: Apple forums 60 s-3 min]. Perf gate "1 keepalive / 30 s" is at/above the low end of reported carrier NAT timeouts (30-120 s [V weak]). Use 20 s.
3. **No ack/credit window**: relay cannot expose backpressure (Workers have no `bufferedAmount` [U]); "latest-state-only" needs phone acks to work over relay.
4. **Phase 7 budget arithmetic is inconsistent**: 200 MB/h = 55 KB/s. "2 fps q50" implies <= 28 KB/frame, unlikely for 1280 px text pages [U]. Make the ladder byte-budget driven, not fps driven.
5. **str0m has no TURN client, ICE restart not documented, no FEC/RED, P2P "less tested", iOS/Android untested** [V: github.com/algesten/str0m]. Phase 8 says "TURN fallback" and "ICE restart" as if free. Spike gate must cover both; own TURN client (e.g. `turn` crate) or phone-only TURN path.
6. **Push "<= 10 s p95" success criterion is not controllable.** APNs/FCM have no latency SLA [U]. Gateway-accept latency is controllable; device delivery is only measurable.
7. **Stale-input hazard**: taps/keys delivered after a reconnect gap click on a changed page. Needs input TTL (Phase 7/8) and "never queue input" policy (Phase 1/2).

## 1. Network realities -> design consequences

| Reality | Evidence | Consequence |
|---|---|---|
| Carrier/CGNAT idle TCP timeouts: reports from 30-120 s up to ~5 min; RFC 5382 says >= 2 h 4 min but carriers ignore | [websocket.org](https://websocket.org/guides/troubleshooting/timeout/) (30-120 s, generic guide), [wirelessmoves](https://blog.wirelessmoves.com/2020/09/carrier-grade-nat-timeouts-and-how-to-configure-your-xmpp-server.html) (operator < 10 min), HN anecdote UK 5 min [U] | Heartbeat <= 20 s (1.5x margin on 30 s floor). Never rely on TCP keepalive (Linux default 7200 s [V]). |
| Wi-Fi<->4G/5G: new source IP, all TCP dies, QUIC could migrate but WS cannot | RFC 9000 [U], WG roaming [U] | New connection + IK re-handshake is the only path. Cost is transport (TCP+TLS1.3+WS = ~3 RTT) + Noise IK 1 RTT, crypto < 5 ms. **No resumption tickets needed**: KISS, keep state resume at app layer (rev/base, since_seq). |
| Half-open sockets (radio gone, no RST) | Apple forums: failure notice after 60 s-3 min; tearing down on path change cut recovery 30-60 s -> ~1 s [V weak] ([thread](https://developer.apple.com/forums/thread/99991)) | Path-change = proactively kill + reconnect, don't wait for socket error. |
| iOS background | App suspended; sockets die in varied ways, Apple DTS advice: retry [V weak] | No keepalive in background; push is the only wake path; on foreground probe first. |
| Android Doze | Network suspended except maintenance windows; wakelocks ignored; WorkManager/JobScheduler don't run [V: [doze-standby](https://developer.android.com/training/monitoring-device-state/doze-standby)] | Same: no background socket. High-priority FCM wakes briefly [U exact quota]. |
| Captive portal | [U] | WS/TLS handshake fails or returns 30x/HTML. Noise means no leak, just UX: show "Wi-Fi needs sign-in" when OS reports captive (Android `NET_CAPABILITY_VALIDATED` absent, iOS captive assist) [U]; don't hammer backoff. |
| Low Data Mode / Data Saver | iOS `NWPath.isConstrained`/`isExpensive`, Android `getRestrictBackgroundStatus` [U API names] | Constrained => subscribe only visible session, min frame interval 500 ms, screencast/WebRTC starts at lowest rung, takeover needs explicit tap. |
| 1-20 % loss, high RTT | Sunshine docs: stability > bandwidth, loss ideally < 5 % [V weak] | TCP/WS: loss shows as stalls (head-of-line), so heartbeats + ack windows detect it; media: NACK handles <= 5 %, beyond that degrade. |

## 2. Techniques: decisions

| Topic | Decision | Numbers |
|---|---|---|
| Heartbeat | Encrypted `hb` frame over link, both directions; any inbound frame resets the idle timer | Send after 20 s idle; pong deadline 8 s; daemon declares phone dead after 45 s silence; phone declares dead after 28 s (20+8). Fast probe on foreground / path change: 3 s deadline. Daemon also sets SO_KEEPALIVE 30/10/3 on LAN sockets as backstop (secondary). |
| Why e2e not relay auto-response | DO `setWebSocketAutoResponse` replies without waking DO, free of duration billing [V: [DO ws docs](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)] but only proves phone<->edge, not desktop liveness | Relay may use auto-response for desktop->relay edge pings (20 s), but phone liveness of *desktop* comes from e2e `hb` + relay `desktop_offline`/`desktop_online`. Cost trivial: 3 msg/min/socket. |
| Backoff | Full-jitter exponential, reset on trigger | Phone fg: attempt 1 at 0-200 ms jitter; then base 1 s x2, cap 30 s. Desktop->relay: 1 s -> 60 s (plan) but reset to attempt 0 on network-change trigger (debounce 2 s) and after 30 s stable. Relay deploy close => extra spread 0-10 s (deploys drop ALL sockets [V]). Background: none. |
| Triggers | NWPathMonitor (iOS), `registerDefaultNetworkCallback` (Android, [V exists](https://developer.android.com/training/monitoring-device-state/connectivity-monitoring)), `visibilitychange`, desktop `if-addrs` poll/NotifyIpInterfaceChange. `navigator.onLine` = secondary hint only (true behind dead Wi-Fi/captive [U]) | Native plugin forwards events to JS; PWA uses visibilitychange + online/offline + failed-heartbeat. |
| Resume | Phone sends `sub {id, have_rev}` (last rev it **applied**), daemon diffs vs cached rev if present (5 min), else full snapshot. Chat: `since_seq`. `daemon_epoch` change => drop everything | Full snapshot <= 64 KiB/session; only visible + attention sessions resubscribed at once, rest lazily. |
| Replay/idempotency | Noise nonce handles in-order replay per connection; app-level: `client_id` per write, per-device dedupe, plus `prompt_id`/`request_id` guards (already first-wins) | Dedupe window 10 min for answers (plan: 60 s too short for a phone offline then reconnecting); 60 s fine for input/key. |
| Lost frames vs Noise | Relay/socket flap => `peer_reset` to the surviving side; both drop Noise transport state and redo IK (1 RTT); discard partial chunk reassembly; chunk reassembly timeout 10 s. Alternative: snow `StatelessTransportState` with explicit nonces + replay window [U] — rejected as more complex | See Phase 3/4. |
| WireGuard roaming | Endpoint updates on any authenticated datagram; replay window ~2000 [V: [protocol](https://www.wireguard.com/protocol/)]. Not portable: needs datagram transport. | Do not build UDP transport for control path. WebTransport/QUIC on Workers/DO not available [U]; revisit only if relay moves off DO. |
| Mosh SSP lessons | State-sync with diffs vs acked state, frame-rate cap 50 Hz / 8 ms collection interval, heartbeat to learn roaming and warn user "no contact" [V: [paper](https://mosh.org/mosh-paper.pdf), exact heartbeat 3 s [U]] | Adopt: diff base = last *acked* rev, "last contact" indicator, sync cap per plan (<= 1 / 100 ms). |
| Happy-eyeballs race | RFC 8305 style stagger [U]: LAN candidates first, relay after 250 ms, instead of fully parallel with 1.5 s LAN timeout | Skip RFC1918 LAN candidates when phone's active interface is cellular and address not on-link. Tailscale 100.64/10 always eligible. |
| Offline queue | Queue nothing that is an intent about *current* state. See table 7 | - |

## 3. Relay / edge

- DO hibernation keeps client sockets open while the object is evicted; constructor re-runs on next message; attachments <= 16 384 B and survive hibernation only while the socket lives [V]. In-memory counters (rate limits, 20 GB/month, 200 MB/h takeover budget) are lost on eviction => persist budget counters to DO storage (batched, e.g. every 5 s or 1 MB) or accept soft limits. Slot id + role + token hash fits in attachment.
- Code deploys restart every DO and drop all WebSockets [V] => every client sees a simultaneous close. Requirement: relay close code for "restart" + clients add 0-10 s jitter; deploy only when no armed rooms ideally [pick].
- Wall time unlimited while WS open; inbound message resets 30 s CPU budget; ~1000 req/s soft per DO [V: [limits](https://developers.cloudflare.com/durable-objects/platform/limits/)] => 8 phones x frames fine; batch frames 50-100 ms [V guidance].
- In-flight frames at drop: lost (relay stores nothing — correct). Recovery = state-sync + `peer_reset`.
- DO is pinned to one location (near first creator [U]); phone far away pays that RTT. Pass `locationHint` from desktop at room creation. "<= 30 ms added median in-region" holds only in-region; state this.
- Edge failover/multi-region: Cloudflare anycast reconnects to the same DO; if the DO colo degrades, room is down. Do NOT build multi-relay. Fallback = direct LAN/Tailscale (race continues every 30 s while relay down) + custom relay URL. Show "relay unreachable" distinctly from "desktop offline".
- Idle timeout of Cloudflare proxy for WS: not verifiable [U]; 20 s heartbeat makes it moot.
- Relay emits: `desktop_offline`, `desktop_online`, `peer_reset`, restart close code. Desktop reconnect uses generation counter: new desktop socket replaces old (last wins) even if old not yet detected dead.

## 4. WebRTC under poor networks (Phase 8)

Verified: RFC 7675 consent: check every 5 s (randomized 0.8-1.2x, never < 4 s), consent expires after 30 s without valid response, then MUST stop sending on that 5-tuple; late responses don't re-establish; **same ICE creds cannot be reused => ICE restart or new session** [V: [RFC 7675](https://www.rfc-editor.org/rfc/rfc7675.html)]. str0m: ICE-TCP, TWCC/BWE, NACK, simulcast yes; **TURN not built in, FEC/RED and ICE restart not mentioned, P2P less tested, mobile targets untested** [V: [str0m](https://github.com/algesten/str0m)]. Cloudflare TURN: TLS supported, **RFC 6062 TCP allocations not implemented**, creds valid up to 48 h, egress $0.05/GB after 1000 GB free, STUN free [V: [CF TURN FAQ](https://developers.cloudflare.com/realtime/turn/faq/)]. Port list (3478/5349/443) not confirmed [U].

Decisions:
- **Only the phone needs TCP/TLS-443 TURN.** Browser dials `turns:...:443?transport=tcp` to Cloudflare; TURN relays UDP to the helper's UDP allocation. Helper needs UDP-only TURN client (str0m gets sockets from it). Document: desktop with UDP fully blocked => unsupported (use Tailscale/RustDesk).
- **Restart ladder** (phone is offerer, owns restart): trigger A = native/path-change event => `restartIce()` immediately; B = `iceConnectionState=disconnected` for 3 s; C = `failed` => immediately; D = no `framesDecoded` and no DataChannel `ping` reply for 5 s. Attempts at t=0, 2, 5, 10 s (3 max in 30 s); then tear down, UI "Screen lost - tap to reconnect". Offer/answer/ICE go over kelpie-link, so restart waits for link re-handshake (Phase 3/4); budget: network change -> first decodable frame <= 5 s p95 on 4G-good, <= 10 s 4G-poor [pick].
- Fresh TURN creds fetched over the link on every restart (plan: 10-min creds; make 1 h; refresh allocation every 5 min; RFC 8656 default allocation 10 min [U]).
- **Helper side**: no STUN/RTCP response for 5 s => pause encoder (keep capture warm, no frames into void); at 30 s consent expiry (RFC) => stop sending, keep ICE ufrag state 60 s awaiting restart, then stop helper stream (helper itself exits 30 s after last viewer per plan).
- **Freeze detection** (static desktop sends nothing, so absence != freeze): helper sends DataChannel `ping` 1 Hz carrying {t, seq}; phone: 2 missed => "poor connection" badge, 5 missed => restart ladder. If packets flow but `framesDecoded` flat >= 1 s => request keyframe over DataChannel.
- **Keyframe storm control**: respond to PLI/FIR at most 1 IDR / 1 s (coalesce); cap IDR size at ~3x mean frame by raising QP for that frame (HW MFT support per vendor [U]); prefer NACK (when RTT < 150 ms) and only fall to PLI when retransmit fails; long GOP stays. No FEC/RED in v1 (str0m lacks); acceptable to ~5 % loss; 5-20 % loss => rely on degradation ladder, document as "usable but degraded".
- **Degradation ladder for text-heavy screens**: keep resolution, drop fps first. Ladder: 1080p30 -> 1080p15 -> 1080p8 -> 720p8 -> 720p4 [pick]. BWE range 150 kbps-4 Mbps, start 1.5 Mbps; < 300 kbps => 720p/5 fps. Up-step needs 10 s stable (hysteresis), down-step immediate. Idle desktop <= 50 kbps (plan). Show "degraded" when fps < 8 or scaled.
- After handover do not silently resume input: input has TTL (below); grant re-bound only after same-device handshake (plan, keep).
- Others on handover: Moonlight 12.1 notes "improved reliability during temporary network interruptions" [V weak: release notes, mechanism unknown]. Parsec, Chrome Remote Desktop, RustDesk, Jump: **[U] nothing verified**. Common pattern assumed: treat handover as fast re-signal + new transport while keeping capture/encoder warm; matches ladder above.
- Test plan must include a UDP-blocked phone network (TURN-443 only) and an IP-change mid-stream.

## 5. JPEG screencast (Phase 7)

- CDP: Chrome stops producing screencast frames until `screencastFrameAck` [U — docs fetch failed; widely documented behaviour]. Plan acks CDP on phone ack: RTT-bound (fps <= window/RTT). Fix: **ack Chrome as soon as the frame is queued in the broker; broker holds exactly 1 pending frame (newest wins) and sends when the phone has credit**; credit window = 2 unacked frames. Phone acks `{seq}` after decode.
- Ladder driven by measured ack RTT (EWMA) and byte budget (token bucket): direct: q70/1280 -> q55/1024 -> q45/800; fps 10 -> 5 -> 2. Relay: bucket 55 KB/s avg (=200 MB/h), burst 500 KB. Degrade order for text: fps first, then scale, then quality floor q40. Down: ack RTT p50 > 400 ms over 3 frames or bucket empty; up: < 150 ms over 10 frames and +5 s dwell.
- Drop policy: never queue frames; coalesce mouse-move to latest; link not live (no ack 3 s) => stop sending, Chrome keeps produced-on-change only.
- **Input TTL**: each input op carries sender-age; daemon/app discards ops older than 1 s (clicks/keys/scroll). `Input.insertText` (passwords) gets explicit per-op ack, shown "sent / not sent", never auto-retried, not TTL-dropped silently (it reports failure).
- Freeze: broker sends 1 Hz tiny `fhb {seq,t}`; 3 s without => dim canvas "connection lost", input disabled.
- On reconnect: send a fresh frame (restart screencast or `Page.captureScreenshot`), never replay.
- Constrained/Low Data: start at lowest rung, 2 fps cap, require tap.

## 6. UX (Phase 2 client, Phase 5 shell)

- States (single enum, one indicator, always visible): `live`, `degraded` (RTT > 800 ms or hb late), `reconnecting (n, next in s)`, `no network`, `sign-in needed`, `desktop offline (since hh:mm)`, `relay unreachable`, `desktop epoch changed`.
- Never block reading: render cached sessions dimmed with "as of 14:03:22 (2 min ago)"; cached state persists to local storage (encrypted by native store; PWA IndexedDB). First paint from cache 0 ms.
- Offline interaction policy (table 7). Drafts kept locally always; send button replaced by "Waiting for connection" (disabled), no hidden queue.
- Conflict messaging: `answer` result enum `ok | stale{answered_by: desktop|device name, at} | dup | refused | unknown_after_drop`. Daemon keeps last 20 resolved waits for 10 min to say "already answered on desktop 2 min ago". Needed because tapped-from-push is usually late.
- Push tap while offline: open app, show cached attention card + "checking..." until sync; if wait gone => "already handled".
- Pushes carry no offline info; use distinct generic loc-keys: `needs_you`, `lost_contact` (see Phase 6).

## 7. Offline queue policy (remote-control app)

| Message | Offline behaviour | Retry on reconnect |
|---|---|---|
| list/sub/chat_sub | Not queued; re-issued from current UI state with have_rev/since_seq | state |
| key / input (terminal) | **Refused** (disabled UI) | never auto; if dropped in flight => `unknown_after_drop`, user decides |
| answer / chat_answer | Allowed only while `live`; if link drops in flight => `unknown` | one auto-retry within 60 s, safe because `prompt_id`/`request_id` first-wins + `answered_wait`; after 60 s ask user |
| takeover / screen input | Refused; TTL 1 s | never; insertText explicit ack |
| settings/approve (user presence) | Desktop only | n/a |
| Push events (daemon -> phone) | Durable outbox on daemon (Phase 6) | state-based send |

## 8. Phase-mapped requirements (testable)

### Phase 1 (state-sync)
- R1.1 Ack window: phone sends `ack {id, rev}` every 1 s or 8 frames; daemon sends at most 4 unacked frames / 128 KiB per (device, session); beyond => coalesce to latest-state-only diff against last acked rev. Test: throttled socket (toxiproxy bandwidth 20 KB/s) with 1 MB/s PTY => daemon RSS flat, phone converges within 2 s of unthrottle, no frame > 64 KiB.
- R1.2 `sub {id, have_rev}`; base = acked rev; mismatch => full snapshot. Test: kill TCP mid-stream 100x, rows identical to daemon after each reconnect.
- R1.3 Heartbeat verb `hb` (read permission); daemon closes phone after 45 s silence; server SO_KEEPALIVE 30/10/3.
- R1.4 `answer` response enum incl. `stale{answered_by, at}`; resolved-waits ring 20 entries/10 min. Test: desktop answers, phone answers 5 s later => stale with who.
- R1.5 Answer dedupe window 10 min per device; input/key 60 s. Test: replay of same client_id after reconnect => `dup`, never double-fires (100 fuzzed disconnect/replay trials, 0 duplicates).
- R1.6 Resume cache 5 min kept; add cap: <= 8 devices x 20 sessions; evict LRU. Reconnect after 6 min => full snapshot <= 64 KiB.

### Phase 2/5 (client, shell) — inferred, phases not read
- R2.1 Connection-state enum + indicator as in section 6; cached render; "as of" stamps. Test (Playwright + fake link): drop link => UI shows reconnecting <= 1 s after detection, cached cards stay visible, composer disabled.
- R2.2 Triggers: fg/bg, path change (native plugin NWPathMonitor / ConnectivityManager callback), online/offline. Each triggers immediate probe (3 s) then reconnect, bypassing backoff once per 2 s. Test: emulate Wi-Fi->cell by changing bind address; first state frame <= 3 s p95 (4G-good profile).
- R2.3 Constrained network flag => visible-session-only subscribe, 500 ms min frame interval.
- R2.4 No socket/timer in background; on foreground resume sequence: paint cache -> probe -> reconnect -> resubscribe visible first.

### Phase 3 (kelpie-link)
- R3.1 **`peer_reset` semantics**: any transport drop/replace between peers => both sides discard Noise transport state + partial chunks, redo IK; messages sent on the dead session never decrypt-fail the new one. Test: kill relay-desktop socket mid-chunk 100x => phone re-handshakes, no decrypt-error loops, no stuck partial buffers.
- R3.2 Handshake timeout 8 s; handshake attempts rate-limited per source (already) but allow reconnect burst of 5 within 10 s for known device keys.
- R3.3 **Address advertisement (gap G1)**: `addrs {seq, list, hostname}` frame sent after handshake and on every rebind (debounce 2 s), carried over relay too. Phone persists last-known set + hostname (Tailscale MagicDNS name if present, else `<host>.local`). Dial order: last-known addrs (staggered 250 ms) || mDNS `_kelpie._tcp` browse (native only; iOS needs Local Network permission + `NSBonjourServices`, Android NsdManager) || relay. Test: change daemon IP while phone connected via relay => phone dials new IP <= 5 s (before link drop); change IP while phone offline => reconnects via mDNS <= 10 s on LAN, via relay otherwise.
- R3.4 PWA limitation: its origin is the old IP; after DHCP change page is unreachable. Docs: bind "Pair a browser" QR to hostname when available; recommend DHCP reservation/Tailscale; native app is the recovery path.
- R3.5 Rekey cadence unchanged; replay/dedupe per section 2.
- R3.6 Reconnect budget: IK complete <= 4 RTT after TCP reachable; document no resumption tickets (rejected: complexity, no measurable gain).

### Phase 4 (relay)
- R4.1 `desktop_offline/online`, `peer_reset`, restart close code, generation counter (last-wins desktop socket).
- R4.2 Heartbeat: relay<->desktop edge ping/pong 20 s via `setWebSocketAutoResponse` [V exists, no wake]; desktop declares relay dead after 45 s; **change perf gate "<= 1 keepalive / 30 s" to "<= 1 per 20 s per socket"**. Cost: ~3 msg/min, WS incoming messages billed 20:1 [U] => negligible.
- R4.3 Backoff/trigger numbers from section 2; relay deploy: clients spread 0-10 s. Test (workerd kill/restart x20, 8 phones): all reconnect <= 15 s, no thundering retries (max 3 attempts/client/10 s).
- R4.4 Persist rate/budget counters to DO storage (batched); test eviction does not reset 200 MB/h budget [U exact eviction trigger].
- R4.5 Connect race: LAN staggered 250 ms vs relay; skip non-on-link RFC1918 on cellular; **replace "probe LAN every 30 s" with trigger on interface change + 30 s only on Wi-Fi with unknown subnet**; direct->relay fallback immediate, relay->direct only after direct stable 10 s (hysteresis, no flap).
- R4.6 Relay-unreachable UI state and direct retries each 30 s.
- R4.7 locationHint at room creation.

### Phase 6 (push) — gap G2
- R6.1 **Durable outbox** in daemon (disk, <= 50 entries) per destination; state-based send: re-check "still waiting" right before each attempt, drop if resolved. Retry 1,2,4,8,16,32,60 s (full jitter) on network error/5xx/429(Retry-After); no retry on 4xx. Max age 10 min; if still waiting at recovery after a gap, send **one** summary ("N sessions need you"), not N stale pushes. Replaces the single 3 s retry (`notify.rs:683-707`). Test: sink returns 503 x3 then 200 => exactly one delivered, <= 1 min late; sink down 12 min + session answered => none.
- R6.2 Gateway sets `apns-expiration = now+600 s` (store-and-forward, not 0), FCM `ttl=600s`, collapse key = `session_id+wait_id` (APNs collapse-id <= 64 B [U page body unseen; known]) so a late delivery replaces rather than stacks. FCM: ttl 0-28 d range, TTL 0 = never stored [V: [FCM lifespan](https://firebase.google.com/docs/cloud-messaging/customize-messages/setting-message-lifespan)]. Priority high/10 for needs-you. At-least-once + dedupe: app dedupes by nonce/`wait_id`, tap always resolves against synced state, push is a hint not truth.
- R6.3 **Desktop-offline push**: daemon tells relay `armed=true` while >= 1 session is Working/Waiting (content-free bit; document the timing leak). If desktop socket lost > 90 s while armed, relay-gateway sends one `lost_contact` push (loc-key, collapse id per room), re-arm only after desktop returns. Opt-in per device toggle. Covers: laptop sleep, Wi-Fi loss, daemon crash. Test: kill desktop socket with armed=true => push at 90-100 s; armed=false => none.
- R6.4 Success criterion rewrite: gateway accepts <= 2 s p95 from event; device delivery p50/p95 measured on real iOS+Android over 100 events and reported (no hard 10 s). If kept, label "best effort".
- R6.5 Android: user-visible notification messages (not silent data) for needs-you; App Standby / battery optimization documented [U]. iOS: alert type, priority 10, `apns-push-type: alert`.
- R6.6 Stale-tap UX per section 6.

### Phase 7 (browser takeover)
- R7.1 Single-slot broker queue + credit window 2 + ack-timed ladder (section 5). Test (toxiproxy 300 ms RTT, 256 kbps): fps adapts, bytes/s <= bucket, no growing queue, latency of newest frame <= 2x RTT + encode.
- R7.2 Relay budget enforced as bytes (55 KB/s avg), not fps; fix plan text "<= 2 fps".
- R7.3 Input TTL 1 s; no input while not live; insertText ack, no auto-retry. Test: freeze link 5 s while tapping => zero queued clicks executed after resume.
- R7.4 `fhb` 1 Hz; 3 s stale => canvas dimmed; reconnect => fresh frame.
- R7.5 Holder survival: a link reconnect within 15 s by same device (after handshake) re-binds the hold instead of releasing it (plan: "unless re-bound") — pick 15 s window; beyond => release + overlay stops.

### Phase 8 (desktop screen)
- R8.1 Spike gate additions: ICE restart works in str0m (or patch/own wrapper), TURN client for helper (UDP), phone on TLS-443-only network, Wi-Fi->4G switch mid-stream, 5 % and 10 % loss (netem/clumsy), keyframe storm check. Failing ICE restart => fall back to full re-offer (new session) <= 5 s.
- R8.2 Restart ladder, consent stop at 30 s, helper pause at 5 s, DataChannel ping 1 Hz (section 4). Test: kill phone network 3 s => stream resumes without UI action <= 5 s; 40 s => "Screen lost" and helper stops sending by 30 s.
- R8.3 IDR rate limit 1/s; IDR size <= 3x mean; test at 10 % loss: IDR/min <= 30, no oscillation of bitrate (> 3 down-up cycles/min fails).
- R8.4 Ladder + BWE bounds; "degraded" badge.
- R8.5 TURN creds 1 h, refetched on restart over link; egress cost note (1.5 Mbps = ~675 MB/h/viewer; 1000 GB free then $0.05/GB [V]).
- R8.6 Input TTL 1 s and refusal when DataChannel unhealthy; reliable channel keys carry seq, helper drops seq older than TTL.
- R8.7 Success-criteria rewrite: add "Wi-Fi<->4G handover: stream resumes <= 5 s p95; no stuck black frame > 5 s".

## 9. Testing

| Tool | OS | Good for | Limits |
|---|---|---|---|
| In-process deterministic shim (virtual clock, `tokio::time::pause`, lossy/latency pipe around sans-I/O `kelpie-link` and str0m socket wrapper) | all, CI | state machine, backoff, resume, peer_reset, replay | not real radio |
| toxiproxy | all (Go binary, no root) [V: [overview](https://dev.co/testing/open-source/toxiproxy)] | TCP: latency, bandwidth, down, reset, slow close, timeout on WS to daemon/relay; deterministic | **TCP only, no UDP/WebRTC** |
| tc netem in Docker (`NET_ADMIN`) | Linux CI [V: [netem](https://oneuptime.com/blog/post/2026-03-04-simulate-network-latency-packet-loss-tc-netem-rhel-9/markdown)] | loss/jitter/reorder/rate, UDP; relay (workerd) + Rust test client + str0m peer with synthetic source | `kelpie-screen` is Windows-only, so WebRTC netem tests use synthetic encoder source on Linux |
| clumsy (WinDivert) | Windows dev [U] | local loss/lag/throttle/drop on real helper; needs admin | manual, antivirus noise |
| Network Link Conditioner | macOS (Xcode Additional Tools) / iOS device Developer menu [U] | real phone, system-wide profiles | manual |
| Real phone on cellular + airplane-mode toggles, Wi-Fi off/on, captive-portal Wi-Fi | physical | handover, background kill, push delivery | manual checklist before release |

Profiles (pick): P0 LAN clean; P1 4G-good RTT 60 ms jitter 15 ms loss 0.5 % 10/2 Mbps; P2 4G-poor RTT 150 jitter 50 loss 3 % 1 Mbps/256 kbps; P3 loss 10 %; P4 loss 20 % (must degrade, not die); P5 asymmetric 5 Mbps/128 kbps; P6 blackhole 10/30/90 s then recover; P7 half-open (drop one direction without RST); P8 RST storm; P9 IP change (rebind client to other local address); P10 NAT rebinding (proxy restart on new port); P11 relay deploy (kill all DOs/workerd); P12 desktop sleep/wake; P13 captive portal (HTTP 302/HTML on 443).

Matrix: {state-sync, answer-once, chat resume, push outbox, takeover, WebRTC} x profiles; hard assertions = no duplicate answer, no stale input executed, convergence time bounds, bounded memory/queues. Release gate: P1, P2, P6, P7, P9, P11 on CI (shim + toxiproxy + netem); P3/P4 WebRTC and P12/P13 manual.

## 10. Ranked recommendations (adopt order)

1. R3.1 `peer_reset` + R4.1 relay signals (correctness bug otherwise).
2. R1.1/R1.2 ack window + have_rev (enables everything over relay).
3. Heartbeat 20 s/8 s/45 s + triggers + backoff (Phase 1/2/4).
4. Offline policy table + stale-answer UX (R1.4, R2.1).
5. Push outbox + `lost_contact` (R6.1/R6.3) — closes gap G2.
6. Address advertisement + mDNS + hostname (R3.3) — closes gap G1.
7. Phase 7 credit window + byte-budget ladder + input TTL.
8. Phase 8 restart ladder + spike gate expansion — closes gap G3; highest technical risk (str0m).

Adoption risk: str0m desktop P2P little-tested (High); snow 0.10 unaudited (plan); DO deploy drops sockets (Medium, mitigated by jitter); APNs/FCM latency uncontrollable (Medium, reworded criterion). No new dependencies needed except a UDP TURN client for the helper (e.g. `turn` crate) [U maturity].

## Limitations
- Phases 2, 5, 9 and architecture docs not read; client/shell mappings are inferred.
- No carrier-published NAT timeouts exist in what I found; 20 s heartbeat is chosen from weak sources. Measure on target carriers.
- Parsec/CRD/RustDesk/Jump handover behaviour not found; CDP ack semantics, clumsy/NLC, Cloudflare TURN ports, APNs page body, str0m ICE restart API, Workers WebTransport availability all unverified.
- No benchmark of JPEG sizes for text pages; the relay budget math assumes 28 KB/frame limit.

## Unresolved questions
1. Does str0m support ICE restart (new ufrag/pwd on live Rtc) or must the helper recreate the Rtc per restart? Check source/issues before the Phase 8 spike.
2. Which TURN client for the helper (UDP allocation exposed as str0m candidate)? Is Cloudflare TURN reachable at `turns:443` TCP?
3. Should the daemon prevent Windows sleep (execution-state) while agents are running and a phone is armed? User policy call; otherwise `lost_contact` push fires on every laptop sleep.
4. Is the `armed` bit (activity timing to relay/gateway) acceptable under the privacy stance ("gateway sees timing of attention events" already documented)?
5. Real JPEG frame size for text-heavy pages at q40-q70/800-1280 px: measure in the Phase 7 spike; may force a lower relay fps than 2.
6. Does Cloudflare's WS idle behaviour or DO eviction reset in-memory counters in practice (need load test)?
7. Hold re-bind window for takeover (15 s proposed) vs security preference for immediate release.

## Sources
- Cloudflare DO WebSockets: https://developers.cloudflare.com/durable-objects/best-practices/websockets/
- Cloudflare DO limits: https://developers.cloudflare.com/durable-objects/platform/limits/
- Cloudflare TURN FAQ: https://developers.cloudflare.com/realtime/turn/faq/
- RFC 7675 consent freshness: https://www.rfc-editor.org/rfc/rfc7675.html
- str0m: https://github.com/algesten/str0m
- WireGuard protocol: https://www.wireguard.com/protocol/
- Mosh paper: https://mosh.org/mosh-paper.pdf
- Android Doze/Standby: https://developer.android.com/training/monitoring-device-state/doze-standby
- Android connectivity monitoring: https://developer.android.com/training/monitoring-device-state/connectivity-monitoring
- FCM message lifespan: https://firebase.google.com/docs/cloud-messaging/customize-messages/setting-message-lifespan
- APNs request headers (page body not retrieved): https://developer.apple.com/documentation/usernotifications/sending-notification-requests-to-apns
- WebSocket timeouts guide: https://websocket.org/guides/troubleshooting/timeout/
- CGNAT timeout blog: https://blog.wirelessmoves.com/2020/09/carrier-grade-nat-timeouts-and-how-to-configure-your-xmpp-server.html
- Apple forums (suspended-app socket failures): https://developer.apple.com/forums/thread/99991
- Sunshine troubleshooting: https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2troubleshooting.html
- Toxiproxy overview: https://dev.co/testing/open-source/toxiproxy
- tc netem: https://oneuptime.com/blog/post/2026-03-04-simulate-network-latency-packet-loss-tc-netem-rhel-9/markdown
