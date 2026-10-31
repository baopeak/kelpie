---
phase: 4a
title: "Relay validation on a real Cloudflare deployment"
status: in-progress
priority: P1
effort: "1d + 7-day soak"
dependencies: [3e]
---

# Phase 4a: Relay validation on Cloudflare

## Overview
User decision 2026-10-30: one Cloudflare Workers + Durable Objects relay, personal use first (synthesis section 5). Before the full phase 4 build, measure the things research could not: where the object lands, real echo RTT from Vietnamese networks, how often sockets drop, and what it costs. About 1 day of work, then a 7-day soak. No product behaviour changes. The numbers we have are estimates, or Cloudflare-POP connect times from one home ISP, not relay round trips.

## Owner's decisions, 2026-10-31 (these override the text below)
- **Free plan, not Workers Paid**, while there are no users; revisit when usage grows. Durable Objects therefore use the **SQLite backend** (`new_sqlite_classes`).
- **`*.workers.dev` first, no custom domain.** So no budget alert is needed: the Free plan fails rather than bills. Measure 7 (WAF) cannot be run without a domain and is marked not run.
- **The cost threshold is now "projected use stays within the Free limits"**: 100,000 requests/day and 13,000 GB-s/day, with incoming WebSocket messages counted 20:1 (Cloudflare's pricing page, read 2026-11-01). Latency and disconnect thresholds stay as written. A failed threshold goes to the owner, never an automatic design change.
- **No SIMs**: measure from this PC's network only and report the carrier gap plainly, no estimates.
- **Run now, ahead of the listed order**: the owner asked for this validation before 3e. It touches no product file, so nothing else waits on it.

Found while building, from Cloudflare's current docs:
- `@cloudflare/vitest-pool-workers` is deprecated and renamed `@cloudflare/vitest-plugin`; the tests use the new name.
- A deploy's default is now a **deferred** code update (`max_delay` 300 s; `immediate` is opt-in): an active object keeps running its old code until it hibernates or the delay ends, and a hibernated one takes the new code on its next event. So measure 4 tests idle rooms and rooms kept active separately, and `Unverified; the docs say every version restarts every DO` below is out of date.

## Data flow
```
probe client (user's ISP, Viettel, VinaPhone, MobiFone) ─ wss ─▶ Worker (room ticket check) ─▶ DO room (hibernating) ─▶ probe "desktop" on the home PC ─ echo back
each client logs: RTT samples, close codes, wasClean, timestamps → plans/reports/; Cloudflare analytics → billed requests, GB-s
```

## Requirements
- Functional
  - The skeleton is the real `relay/` start, not throwaway: `src/worker.ts` (ticket check before any DO call, `/v1/health`) and `src/room.ts` (`acceptWebSocket`, `setWebSocketAutoResponse`, opaque forwarding). Phase 4 extends it. No HTML in the deployed Worker.
  - Probe: `scripts/perf/relay_probe.py` (stdlib + an existing WS client), no Kelpie data, random bytes only. Modes `desktop`, `phone`, `soak`. Run from a PC tethered to each carrier, plus one run from the phone's browser on one carrier for radio wake-up [optional].
  - **Measure**:
    1. **DO colo**: 20 rooms from the desktop with no hint, first `get()` from the desktop; record colo from inside the object. Expect the colo near the desktop's entry POP [D, not measured].
    2. **Echo RTT**: 1,000 samples at 100 B and 1 KiB, p50/p95/p99, on the user's ISP, Viettel, VinaPhone, MobiFone, at an evening peak too. Also TCP connect and time to first frame.
    3. **Disconnect rate**: 20 sockets for 7 days (home ISP plus at least one carrier for as long as practical): count, close code, `wasClean`; correlate with Cloudflare status and any deploy.
    4. **Deploy behaviour**: DO class in its own script (cross-script binding): deploy only the router Worker and count dropped sockets; deploy the DO script and count. Unverified; the docs say every version restarts every DO without naming scope.
    5. **Billing + hibernation**: after 24 h and 7 d, read Workers/DO analytics for billed requests and GB-s; confirm the 10 s pre-hibernation idle is not billed, whether auto-responses are billed; the DO hibernates ≤ ~15 s after the last event with 8 idle sockets.
    6. **Abuse floor**: 1,000 msg/s into one room and into 1,000 random room ids: the second must stop at the Worker with no DO call; note billed requests.
    7. **WAF**: whether blocked requests are billed.
  - Setup uses the maintainer's own Cloudflare account. ~~Workers Paid, a custom domain and a budget alert from day one (guard requirement, not a later step).~~ Free plan on `*.workers.dev`: the limit is the guard (owner's decision above).
  - **Output**: `plans/reports/` report with one table per measure and a recommendation against the thresholds below.
- **Proposed thresholds** (user to confirm; failing one means a user decision, never an automatic design change):
  - echo RTT p50 ≤ 200 ms and p95 ≤ 400 ms on each carrier (terminal echo stays usable; the estimate is ~110–130 ms);
  - ≤ 1 unplanned drop per socket per day, and reconnect ≤ 3 s p95;
  - ~~projected ≤ $10/month for personal use~~ projected use stays within the Free limits (owner's decision above); hibernated idle costs no duration;
  - Worker ticket check stops random-room traffic before any DO.
- Non-functional: test traffic ≤ 2 GB in total; the abuse test is capped at about 20,000 requests so a day's Free quota is not spent.

## Backwards compatibility
Nothing shipped; no boundary change; no API level. The relay is unused by any released client.

## Resilience requirements
- Soak clients reconnect with full jitter and log every attempt; they keep running across PC sleep and record it, so sleep is not counted as a drop.
- A DO incident during the soak is data: keep it, tag it from the status page, and do not restart the clock.

## Related Code Files
- Create: `relay/src/worker.ts`, `relay/src/room.ts`, `relay/wrangler.toml`, `relay/package.json`, `scripts/perf/relay_probe.py`, the report under `plans/reports/`
- Modify: none in the product. `plans/261024-1205-phone-remote-control/phase-04-relay-connect-anywhere.md` gets the measured results.

## Implementation Steps
1. Maintainer: Cloudflare account on the Free plan (done; token supplied).
2. Skeleton relay + probe; hibernation guard test (no `setInterval`, `setTimeout`, plain `accept()` or repeating alarm).
3. Deploy; measures 1, 2, 4, 6 in a day, from this PC's network. Carriers and measure 7 not run (owner's decisions above).
4. Start the 7-day soak (3, 5) while phases elsewhere continue.
5. Report; adjust phase 4 limits and the RTT threshold for room re-creation; ask the user on any failed threshold.

## Tests
- `relay/` vitest (`@cloudflare/vitest-pool-workers`): ticket checked before DO, forwarding, auto-response, no timers.
- `python scripts/perf/relay_probe.py --help` smoke; probe against local `workerd` before spending deploys.

## Perf gate
None. This phase produces the numbers later gates use.

## Success Criteria
- [ ] DO colo, echo RTT on four networks, 7-day disconnect rate, deploy behaviour, billing and hibernation are in one report, each tagged measured.
- [ ] Each threshold is marked pass or fail, and the user has been told about any fail.
- [ ] "DO class in its own script" has a yes or no answer.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| No SIM on a carrier | M × M | Borrow or buy prepaid; report the gap rather than estimate |
| Soak skewed by PC sleep or local outages | M × L | Log sleep and network events |
| Abuse test spends the day's Free quota and disturbs the soak | L × M | Short runs, hard cap in the probe; the Free plan fails rather than bills |
| Real numbers fail the thresholds | L × H | Stop, report; user decides (e.g. `workerd` self-host with the same code) |

**Rollback**: delete the Worker and the `relay/` directory; nothing depends on it until phase 4.
