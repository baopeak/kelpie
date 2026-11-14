# Screen over WebRTC: numbers (emulated phone, this machine)

Phase 1e of `plans/261112-1405-phone-remote-desktop` (local plan folder). Master after 1c (#60, API 42) with 1d (#61, API 43) on top, measured 2026-11-14 with `scripts/e2e/screen_rtc.py` (`--matrix` and the default checks).

## Setup and caveats

- Same machine and setup as Phase 0 (`tester-261114-0020-screen-flow-numbers.md`): Windows 10, Intel HD 530 (QSV), 2560×1440, 8 GB RAM; debug `kelpied` and app, **release** `kelpie-screen`; primary monitor shared; a topmost tkinter window scrolling text at 30 fps; isolated endpoint/data dir.
- The phone is split: an emulated phone (Python) holds the encrypted link; the **real** `app/phone/src/screen-rtc.ts` runs in Playwright Chromium 149 (matrix) and Firefox 151 (checks), offers, says the hello, reassembles `"m"`, and acknowledges each frame on `"i"` 8 ms after it arrives (standing in for decoding).
- Both ends on one computer: the pair is host ↔ host over the Wi-Fi address (STUN off). **No real network**: the impairment is the daemon's test hook (`KELPIE_RTC_TEST_IMPAIR_FILE`), on its UDP socket, both ways; its rate limit queues without a bound (no router buffer drops), which is harsher on delay than a real 300 kbps link.
- **This encoder is the ceiling** (≈ 15–20 fps at 1080p whatever the network, as in Phase 0); the fps targets cannot be judged here. Absolute numbers moved between runs with other load on the machine (two browser engines, other sessions): both runs are given.
- Phase 0's link numbers came through a TCP fault proxy (`screen_net.py`) whose loss is a model of TCP's; the UDP hook loses real packets and SCTP retransmits them. Comparable in kind, not to the millisecond.

## Impairment matrix (P2P, 30 s each after 3 s warm-up)

Command: `python -X utf8 scripts/e2e/screen_rtc.py <scratch> <debug app> --matrix --seconds 30 --browsers chromium`. Runs A and B were taken before
the baseline row got the same 3 s warm-up as the P2P rows; run C (last) has it.

Run A (quieter):

| # | condition | fps | kbps | age p50 / p95 ms | longest gap ms | freezes > 1 s | open ms | survived |
|---|---|---|---|---|---|---|---|---|
| 0 | link, no impairment | 17.7 | 87 | 60 / 67 | 90 | 0 | – | – |
| 1 | 25 ms one way | 17.2 | 64 | 94 / 105 | 83 | 0 | 623 | yes |
| 2 | 50 ms one way (100 ms RTT) | 15.1 | 55 | 123 / 138 | 128 | 0 | 858 | yes |
| 3 | 100 ms one way | 11.8 | 44 | 177 / 245 | 224 | 0 | 1446 | yes |
| 4 | 100 ms RTT, 1 % loss | 12.9 | 48 | 121 / 198 | 264 | 0 | 1017 | yes |
| 5 | 100 ms RTT, 5 % loss | 4.5 | 35 | 110 / 1079 | 1210 | 4 | 849 | yes |
| 6 | 100 ms RTT, 300 kbps | 5.0 | 20 | 122 / 135 | 242 | 0 | 1182 | yes |

Run B (busier; with the daemon's memory):

| # | condition | fps | age p50 / p95 ms | longest gap ms | freezes > 1 s | kelpied MB |
|---|---|---|---|---|---|---|
| 0 | link, no impairment | 14.2 | 71 / 116 | 116 | 0 | 25.7 |
| 1 | 25 ms one way | 14.2 | 87 / 126 | 157 | 0 | 32.3 |
| 2 | 100 ms RTT | 10.7 | 138 / 187 | 448 | 0 | 32.4 |
| 3 | 100 ms one way | 7.9 | 189 / 208 | 187 | 0 | 32.3 |
| 4 | 100 ms RTT, 1 % loss | 7.3 | 137 / 164 | 1201 | 2 | 32.5 |
| 5 | 100 ms RTT, 5 % loss | 3.5 | 128 / 2728 | 3215 | 7 | 32.6 |
| 6 | 100 ms RTT, 300 kbps | 5.0 | 141 / 155 | 265 | 0 | 32.6 |

Run C (busiest, final script): link 9.3 fps, age 108 / 136; 25 ms 11.1 fps, 121 / 155; 100 ms RTT 11.1, 147 / 179; 100 ms one way 10.5,
198 / 227; 1 % loss 7.7, 145 / 206, longest gap 751 ms, no freeze; 5 % loss 3.9, 162 / 1165, 6 freezes; 300 kbps 5.0, 158 / 205. Every condition
survived; kelpied 25.9 MB on the link, 31.8–32.7 MB with a peer.

Phase 0, link through the TCP proxy (same machine): 100 ms RTT 14.7 fps, age 96 / 200; 1 % model 13.8 fps, 99 / 219; 5 % model 13.9 fps, 100 / 285; 300 kbps 21.2 fps.

## Switches, restarts and rules (default checks, 30/30 PASS, Chromium + Firefox)

| What | Result |
|---|---|
| Link → P2P (offer to `sc_path p2p`) | 424–858 ms, first frame a keyframe |
| P2P → link by choice (`sc_via`) | 71–124 ms to the first link frame |
| Link → P2P again (new offer) | 316–597 ms to the first P2P frame, keyframe first |
| Wrong `rtc_hello` nonce | peer closed (`bad_hello`), picture stays on the link |
| 1 % loss + 100 ms RTT | session survives the run |
| Firefox interop | P2P, frames, a click lands |
| P2P way turned off on the computer | screen ends in 13–160 ms (`revoked`, from the call to `sc_end`) |
| Phone locks (`lock_now`) | the desktop's "shared" indicator (checked up first) goes in 216–247 ms (WebDriver polling: an upper bound); nothing more arrives on the peer after it; unlocks again |
| `screen.py` (link path regression) | 45/45 |
| `screen_relay.py` (relay WebSocket, Face ID, TURN credentials minted by a local workerd in coturn mode; no TURN server runs here) | 22/22 |

## Against 1e's acceptance

| Condition | Target | Here |
|---|---|---|
| 100 ms RTT, 0 % | fps ≥ 27; age p95 ≤ RTT + 120 ms | fps encoder-capped (15.1 / 10.7); age p95 138 / 187 ≤ 220: **met** |
| 100 ms, 1 % loss | fps ≥ 24; age p95 ≤ RTT + 300; no freeze > 1 s | fps capped (12.9 / 7.3); age p95 198 / 164 ≤ 400: met; freezes 0 (A) / 2 (B): **mixed** |
| 100 ms, 5 % loss | fps ≥ 15; age p95 ≤ RTT + 600; survives | survives; fps 4.5 / 3.5 and age p95 1079 / 2728: **not met** |
| 300 kbps | as Phase 0 | 5.0 fps vs 21.2 on the link; the hook's unbounded queue inflates the round trip the controller cuts on: **not met here** (worth re-measuring on a real slow link) |
| Automatic first frame (relay WS) ≤ 2 s; upgrade ≤ 4 s | field, median | link first frame immediate; upgrade 0.4–0.9 s on LAN (field owed) |
| Switch / ICE restart ≤ 3 s, same `sc` | | switch by choice and a new offer: 0.07–0.6 s, same `sc`: **met**; ICE restart on a network change / handover: **owed (field)** |
| Policy | switch off → end ≤ 1 s; lock → at once | ≤ 0.16 s; ≤ 0.25 s: **met**. Hotkey/idle/border/audit: `screen.py` was not re-run on the P2P path (argued path-independent: they live in the helper and the hold's task, which do not change with the sink) |
| Caps | debug cap → revoke + fallback ≤ 2 s | unit-tested (`bytes_through_turn_are_counted…`); no TURN server runs here, so no e2e (credentials are minted, not used) |

## Phase 3 gate (opens if any holds on P2P/TURN-UDP)

- 1 % loss: age p95 > RTT + 300 → no (198 / 164); fps < 24 → yes, but the encoder caps this machine below 24 on every path, so not a transport verdict.
- **5 % loss: fps < 15 (4.5 / 3.5) and freezes > 1 s more than once a minute (4 and 7 in 30 s) → holds.**
- Field 4G → owed.

**Verdict: the gate opens on the 5 % loss criterion.** Reliable, ordered SCTP backs off like TCP under heavy loss (the spike's finding), and the ceiling cut (0.85 × acked while `"m"` is full) does not avoid the head-of-line stalls. Phase 3 (partial reliability / RTP) is the remedy the plan names; out of this run's scope.

## Size and memory

- `kelpied.exe` (release, from CI's Windows installers): **18.58 MB → 22.33 MB (+3.75 MB)** before 1c (70aa170) vs after (191fd84); the installer 20.13 → 21.20 MB (+1.07 MB).
- `kelpied` working set (debug, this run): 25.7 MB with a screen on the link → ≈ 32.5 MB with a peer (+≈ 7 MB), steady across conditions.

## Owed on the owner's devices (not possible here)

1. Safari iOS PWA and Chrome Android, on Wi-Fi and each available carrier: ≥ 50 starts per network recording the path (P2P / TURN-UDP /
   TURN-TLS / relay WS) and time to the first P2P/TURN frame, as the plan asks (the spike's counsel: at least 20 per carrier).
2. 4G with no Tailscale (carrier NAT): ICE outcome, a 10-minute session per path with *Copy measurements* taken at its end, the Wi-Fi → 4G
   handover (picture back ≤ 3 s), lock/unlock.
3. TURN: create a Cloudflare TURN key (the token here has no Realtime permission), set `TURN_KEY_ID` / `TURN_API_TOKEN` on staging, then a forced-TURN run (`relay_turn` choice) and a 20-minute session across the credential renewal.
4. Windows Firewall prompt for `kelpied.exe` on a machine with the firewall on (off here).
5. Field numbers: the live view's debug panel (tap the quality line) now has *Copy measurements* (sizes, timings and the kind of connection,
   never the picture) to paste into a note per run.

## Deviations from the phase file

- The field summary is copied to the clipboard instead of uploaded over the link to the daemon log (a new verb would need an API level; 1e
  is API-neutral).
- No forced-TURN row in the matrix: no TURN server is reachable here (the Cloudflare token cannot make a TURN key; no coturn runs).
- Metrics: `screen_support.frame_stats` for screen_rtc.py; screen_net.py keeps Phase 0's own summary (same formulas, freezes over 2 s) and
  shares `pct`.

## Unresolved

- Whether the hosted relay should give TURN to open rooms at all (`TURN_OPEN_ROOMS`), since the relay counts credentials, not bytes.
- Re-measure 300 kbps on a real link (or with a bounded queue in the hook).
