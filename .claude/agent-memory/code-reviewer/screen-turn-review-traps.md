---
name: screen-turn-review-traps
description: Remote-desktop TURN/P2P review traps - sealed ScreenOptions form strings must be versioned, placeholder ways need narrowing, way_now/turn_path lag vs 250ms watch, Cloudflare TURN expiry/revoke facts
metadata:
  type: feedback
---

Checks that found real bugs in the phase 1d (TURN) review, 2026-11-14:

- `store.rs` `screen_input` seals ScreenOptions with a form string (`kelpie-screen-options/1`, `/2`). Adding fields to an existing form breaks every seal an older build wrote (options become `doubted`). The project's own pattern (decision 13, 1b→1c) is a new form, the old form still verified with defaults, plus a "the old seal still verifies" test in `mobile/tests/lock.rs`. Also, placeholder checkboxes that defaulted to true in earlier builds (p2p in 1b, relay_turn in 1b/1c) must be narrowed on load until chosen.
- `screen.rs` `watch` runs every 250 ms but peer stats arrive once a second, and stats of `rtc_next` are ignored. Any per-path state set from stats (`turn_path`) is stale right after `ready()`, so a policy check on `way_now` can end the screen. Test configs with one way switched off (p2p off, relay_turn on).
- Phone `screen.ts` opens peers only through `upgradesToP2p` / `upgradesToTurn` / `r.via === "p2p"`. A new daemon `Via` returned for Automatic needs a matching phone branch and the `can_p2p` guard in `screen_may_start`.
- Async mints (`spawn_blocking`) that store into `hold.state` must re-check `hold.live()` and the cap flags afterwards, or credentials leak past `end()`.
- The relay's TURN `ts` is strictly monotonic per room, and the daemon opens a new ureq Agent (TLS) per request, so concurrent signed requests arrive out of order and one gets a 403.
- Cloudflare TURN FAQ (checked 2026-11-14): an expired credential's connection drops after a short delay, and Refresh needs a valid credential. Revoking does something to live allocations that Cloudflare does not document. The docs suggest `setConfiguration()` for refresh, but whether browsers re-authenticate an existing allocation that way is unverified.

**Why:** these passed CI and unit tests in the reviewed commit.
**How to apply:** for any new screen way or sealed-option field, check the seal form, the placeholder default, the phone's open-peer branches, the watch-vs-stats timing and post-await liveness. Related: [[vault-host-identity]].
