---
name: kelpied-logging-and-store-traps
description: Verification traps seen in Kelpie reviews - kelpied log level env is KELPIE_LOG (not RUST_LOG) and logs go to <data>/logs; Solid root setStore merges (stale keys)
metadata:
  type: feedback
---

Claims like "no secret in the daemon's debug log" need checking against how kelpied logs: `main.rs` builds `EnvFilter::try_from_env("KELPIE_LOG")` (default info) and writes to `<data>/logs/kelpied.log`, not stdout. An e2e that sets `RUST_LOG=debug` and scans the captured stdout proves nothing (seen in the 2026-10-24 notify.py review). tracing-subscriber's default `tracing-log` bridge forwards `log` records, so ureq 3 at trace logs full request paths (a Telegram bot token is in the URL path); at debug it prints only scheme and host.

Solid `setStore(obj)` at the root shallow-merges: keys missing from `obj` stay. A "reload list from daemon" written as `setX(Object.fromEntries(list))` leaves stale entries; it needs `reconcile`. Probe it from the scratchpad with a `.cjs` that uses `createRequire("D:/Works/Kelpie/app/package.json")` (avoids naming the blocked deps folder).

**Why:** both slipped past a developer's self-verification in phase 12 and changed the review's severity calls.
**How to apply:** when a report asserts log hygiene or store resync, check the env var name, the log path, and the setter semantics before accepting it. See [[shell-hook-blocked-words]] for probe workarounds.
