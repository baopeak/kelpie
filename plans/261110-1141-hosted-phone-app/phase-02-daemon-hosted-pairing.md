---
phase: 2
title: "Daemon: hosted pairing, device page, relay v2 client"
status: done
priority: P1
effort: "4d"
dependencies: [1]
---

# Phase 2: Daemon

## Context
- Pairing QR: `Mobile::pair_start(plain)` (`crates/kelpied/src/mobile.rs:1446-1488`), called from `daemon.rs:2074`; needs `running`, which exists with no LAN listener when the relay is live (`start()` `keep_going`, `mobile.rs:1794-1819`). `Code` (`mobile.rs:207-213`), `Pending` (`:191-204`), `add_pending` (`:1273-1274`), `commit_approval` (`:1529-1567`).
- Relay client: `check_turn_on` demands a key (`relay.rs:664-676`), `turn_on` (`:679-711`), `register()` (`:790-807`), `dial()` drops the 101 response (`:935-964`, `Ok((ws, _))` at `:957`), `connection()` sends `allow` then one `origin` (`:1026-1032`), `on_control` builds `Peer::relayed(ip, r.origin)` from the room's single stored origin (`:1148-1151`), `relay_note_origin` (`:500-519`), `relay_grant` (`:442-474`), `TOKEN_LIFE_MS` 30 d (`:50`).
- Link: `Peer` (`link.rs:56-70`), `Peer::relayed` (`:85-88`), pairing refuses a non-secure relay peer (`:522`), records `peer.secure_origin` (`:574`), grants the relay token at once (`:588`); reconnect refuses a non-secure device on the relay (`:630`).
- Permissions: `transport_cap` (`mobile.rs:1997-2009`: relay ⇒ `write`), `record_ceiling` (`:2013-2019`), `page_may_control` (`:2030-2032`). Lock: `lock_path` (`lock.rs:127-135`), `lock_gate` returns `Open` for every path while the lock is off (`:138-145`), `lock_enrol` refuses while off (`:254-256`) and takes the page origin as rpId (`:257`, `webauthn.rs:71`).
- Push: `push_subscribe` checks the page's claimed origin against the connection's (`mobile.rs:644-661`), which for a relay slot is the room's origin (`client()` `:2930`, `:2945`); alerts lead to `target.origin` (`notify/destinations.rs:485-488`).
- Store: `Device` (`store.rs:123-152`) with `secure_origin` + its own tag `secure_mac` (`:135-142`, sealed `:430`, checked `:439`); `RelaySettings.origin` single (`:215-217`); `VERSION` 2 (`:23`).
- Auth: `mobile.set_relay`, `mobile.set_lock`, `mobile.approve` are `APP_ONLY` (`auth.rs:59`); `mobile.pair_start`, `mobile.enable` are user-allowed (`auth.rs:486`).

## Overview
Teach the daemon to issue hosted pairing codes, to record which page a device paired on, to speak relay v2 (origins, version header, phone origin), to create rooms without a key, and to hold hosted devices to `write` behind a mandatory lock. Nothing changes for records without a page.

## Requirements
1. **Relay v2 negotiation.** `dial()` returns the 101 headers; `x-kelpie-relay-version` ≥ 2 ⇒ v2, kept in `relay.facts` for this connection (lifetime: one connection; re-read on every dial). v2: send `{"t":"origins","o":[ts?, hosted?]}` after `allow`; v1: send `origin` as today. A v1 relay never receives `origins`.
2. **Origins the room admits**: the Tailscale origin while published or kept for Tailscale devices (`r.origin`, as today), plus the hosted origin while hosted is available. `relay_note_origin` (`relay.rs:500-519`) sends the whole list.
3. **Phone origin per slot.** `on_control` reads `o` from `open`; `Peer::relayed(ip, o)` only when `o` is one of the origins this daemon registered, else `None` (not secure). A v1 relay (no `o`) falls back to today's `r.origin`.
4. **Open registration client.** `check_turn_on` no longer requires a key; `register()` sends `Authorization` only when a key is given or kept. Answers: 401 with no key ⇒ "This relay needs a registration key" (Settings then asks); 429 `network` ⇒ "This network made too many rooms today: try again tomorrow, or use Tailscale"; 503 `full` ⇒ "The relay is not taking new rooms today"; other codes as now. A key typed and accepted is still kept (`relay.rs:695-698`).
5. **Hosted availability.** `hosted_origin()` = `http_url(r.url)` for `wss://` only. Probe `GET <hosted>/version.json` (ureq, 10 s, no redirects, ≤ 4 KiB, JSON `app == "kelpie-phone"`, `api_min ≤ API_LEVEL`) only when Settings asks for status or before a hosted `pair_start`, cached 10 min, never in a background loop (nothing runs while phone access is off). Status: `relay.hosted = {available, origin, version, reason, install_qr}` (`install_qr`: the SVG of `https://<hosted host>/`, no secret, as Tailscale's `install_qr`); `new_store: true` while the store holds nothing a person set up: phone access off, no device or legacy device, relay `None`, Tailscale mode off, no certificate, no chosen address, `lan` unset (phase 4's new-user path; derived, no new state — a person who turned access on and off with nothing paired counts as new, which changes nothing they rely on).
6. **`mobile.pair_start {hosted: true}`**: requires the relay live, connected, v2 and hosted available (else a plain error saying which). `Code` gains `page: PageOrigin {kind: hosted|ts|own|plain, origin}` (process lifetime, like `codes` today). QR = `https://<hosted host>/#p=<payload>` with endpoints = the relay endpoint only (no LAN, no `ts`: a hosted page can use neither). Format unchanged (`kelpie-link` `QrPayload`, `lib.rs:247-308`). The keys sit in the fragment, which browsers never send in a request or `Referer`; the page removes it from the address bar on load (`app/phone/src/phone-state.ts:747-755`). A browser's local history may still hold it, as with the Tailscale link today: the secret is single-use and dies in 120 s, and the hosted page's code reads it by design (the accepted trust). `relay_pair(&psk)` as today (queued and re-sent on reconnect, `relay.rs:397-402`, `:1030-1032`). Reply adds `hosted: true`. Existing calls set `page` by their own kind (Tailscale link ⇒ `ts`, TLS listener ⇒ `own`, plain ⇒ `plain`).
7. **Binding at pairing** (`link.rs:519-597`): `spend_code` also returns the code's `page`. Over the relay, the slot's origin must equal it (and, on a v1 relay, the code must not be `hosted`); directly on `/link`, a `hosted` code is refused. A mismatch counts as a failed attempt and does **not** spend the code (the right page may still use it). `Pending` gains `page`; `mobile.pair_request` adds `origin_kind`.
8. **Device page.** `Device.page: Option<String>`, `Device.page_kind: Option<PageKind>`, sealed together under a new tag `page_mac` (older Kelpie ignores both and still verifies the record; edited fields do not verify ⇒ treated as no page ⇒ a hosted device then fails closed on the relay). Set for every new pairing. Records without it behave as today.
9. **Reconnect over the relay** (`link.rs:608-661`): for a device with a page, the slot's `o` (v2) must equal `device.page`, else `RELAY_REFUSED`; the connection's page origin (`client()` `page_origin`/`secure_page`) is `device.page` for relay slots, falling back to the room origin for records without one. Push subscriptions and passkey enrolment then use the device's own origin.
10. **Ceilings.** `record_ceiling`: `page_kind == hosted` ⇒ `write`; `ceiling_refusal` explains ("a phone using the hosted app can read and type; taking control needs Tailscale or this computer's own HTTPS address"). `transport_cap` unchanged (relay ⇒ `write`). Takeover and screen entry points already require `control`.
11. **Lock for hosted devices [Q1, recommended]**: new `LockPath::PasskeyAlways` for `page_kind == hosted`; `lock_gate` treats it as `Passkey` even with the lock off; `lock_enrol` allows enrolment for it while the global lock is off; `plain_read` never applies to it. If the owner picks Q1 (b), skip this item.
12. **Relay token life [Q3, recommended]**: hosted devices get `max(30 d, dormant_days)` capped at 365 d (dormant 0 ⇒ 365 d); renewal stays 7 d; Tailscale devices unchanged.
13. **LAN listener optional [Q4, recommended]**: `Store.lan: Option<bool>` (`None` = legacy on). `start()` binds LAN listeners only when not `Some(false)`; with LAN off and neither Tailscale nor relay on ⇒ "Turn on the relay or Tailscale: nothing else reaches this computer". `pair_start {plain}` refused while LAN is off. `mobile.enable {lan}` sets it (not `APP_ONLY`: closing a port weakens nothing; opening one is today's behaviour of `mobile.enable`).
14. **Moving relay**: `mobile.status.relay.hosted_devices` = count of devices whose page is the current hosted origin, so Settings can warn before a URL change cuts them off.
15. **API**: next free `API_LEVEL` (34 expected) documented in `kelpie-proto/src/lib.rs` above `:57`; `HOSTED_API` in `app/src/lib/attention.ts` (beside `PHONE_API` `:12`, `TAILSCALE_API` `:15`); `MobileStatus`, device and pair-request types in `app/src/lib/types.ts`.
16. **Test seams** (debug builds with `KELPIE_E2E` set only, the gate of `KELPIE_PUSH_TEST_BASE`, `notify.rs:588-591`; a release build has no such path, grep-checked in the release workflow like `KELPIE_UPDATE_ENDPOINT`, `release.yml:83-94`):
    - `KELPIE_RELAY_TEST_CA` (PEM path): added to `dial()` roots (`relay.rs:944-945`) and to the `version.json` probe.
    - `KELPIE_RELAY_TEST_URL`: replaces `DEFAULT_URL` (`relay.rs:44`), so the new-user path in a GUI test turns on a local relay.
    - Guard: under `KELPIE_E2E` a debug daemon refuses any relay address that is not loopback/`localhost`, so no test can reach Kelpie's Cloudflare relay by mistake.

## Data flow
`pair_start{hosted}` → `Code{psk, page: hosted O}` → relay `pair` → phone at O pairs over the relay → `open{o: O}` → `Peer::relayed(slot, Some(O))` → `pair()` checks `o == Code.page` → person approves (`APP_ONLY` + OS verification) → `Device{secure_origin, page O, page_kind hosted}` sealed (`secure_mac`, `page_mac`) → relay token (life per item 12) → passkey enrol window 5 min (rpId = O host).

## Related code files
- Modify: `crates/kelpied/src/mobile.rs` (`Code`, `Pending`, `pair_start`, `commit_approval`, `record_ceiling`, `ceiling_refusal`, `client()`, `start()`, `enable`, status), `crates/kelpied/src/mobile/link.rs` (`pair`, `reconnect`), `crates/kelpied/src/mobile/relay.rs` (`dial`, `connection`, `on_control`, `check_turn_on`, `turn_on`, `register`, `relay_note_origin`, `relay_grant` token life, hosted probe, status), `crates/kelpied/src/mobile/store.rs` (`Device.page`, `page_kind`, `page_mac`, `Store.lan`), `crates/kelpied/src/mobile/lock.rs` (`LockPath::PasskeyAlways`), `crates/kelpied/src/daemon.rs` (`mobile.pair_start {hosted}`, `mobile.enable {lan}`), `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`), `crates/kelpied/src/mobile/tests/relay.rs`, `crates/kelpied/src/mobile/tests/lock.rs`, `app/src/lib/attention.ts`, `app/src/lib/types.ts`.
- Verify unchanged: `crates/kelpied/src/notify/destinations.rs:485-488`, `crates/kelpie-link/src/lib.rs` (no QR format change: `QrPayload`, `:247-308`).

## Implementation steps
1. Store fields and tags with round-trip, edit-detection and older-reader tests.
2. `Code.page` / `Pending.page` / `spend_code` return; `pair_start {hosted}`; daemon dispatch.
3. Relay v2: version header, `origins`, `o`, registration without key, error texts, hosted probe and status.
4. Binding checks in `pair` and `reconnect`; page origin for relay slots in `client()`.
5. Ceilings, lock path, token life, LAN option.
6. API level, TS constants and types; `cargo test -p kelpied -p kelpie-proto -p kelpie-link`, `cargo clippy`; `cd app && pnpm typecheck`.

## Tests (cargo)
- A hosted code pairs only through a slot whose `o` is the hosted origin; a Tailscale code through a hosted slot, a hosted code on `/link`, and a hosted code on a v1 relay are refused without spending the code.
- The record gets `page`/`page_kind`; editing either in the file stops it verifying (hosted device refused on the relay, `secure_origin` record still intact for an older reader); a record without them behaves as before (existing relay and lock tests unchanged).
- Reconnect: `o` ≠ `device.page` ⇒ `RELAY_REFUSED`; push subscription from a hosted device accepted with the hosted origin, refused with the Tailscale one.
- `record_ceiling(hosted) == write`; `raise_prompt(control)` refused for hosted; takeover/screen start refused.
- Lock (if Q1 a): hosted device locked with the global lock off; enrol allowed then; a Tailscale device with the lock off still `Open`.
- Token life: hosted 90 d by default, 365 d for "never"; Tailscale 30 d.
- v1 relay (no header): only `origin` sent; v2: `origins` sent, never `origin`.
- Registration: no key ⇒ no `Authorization`; 401/429 `network`/503 `full` give their texts; a key accepted is kept, a refused one is not.
- Hosted probe: redirect, oversize, wrong `app`, `api_min` above ours ⇒ not available; never called while phone access is off.
- LAN off: no listener bound, relay-only start succeeds, `pair_start {plain}` refused; `lan: None` binds as today.
- Test seams: ignored without `KELPIE_E2E` and absent in release (`cfg(debug_assertions)`); under `KELPIE_E2E` a non-loopback relay address is refused.
- `auth.rs` capability matrix unchanged except new parameters.

## Validation
`cargo test -p kelpied`, `cargo test -p kelpie-proto -p kelpie-link`, `cargo clippy -p kelpied`, `cd app && pnpm typecheck`. Isolated `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR` for any manual run against a local `wrangler dev`; never the default daemon.

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| New daemon sends `origins` to a v1 room ⇒ socket closed in a loop | M×H | version header gate; test with a header-less fake relay |
| Lock change weakens or breaks Tailscale devices | M×H | new path variant used only for `hosted`; existing lock tests must pass unchanged |
| Relay lies about `o` | M×L | code-bound `page`; sealed `device.page`; a liar still lacks the phone's keys |
| Store downgrade | L×M | additive fields with own tag; older reader test |
| Longer token life widens a stolen-token window | L×M | token admits to the room only; Noise key + passkey still needed; revoke removes it from the allow-list |
| Parallel file-transfer phone work edits `mobile.rs` | M×M | rebase at commit, renumber API, move gates together |

## Rollback
Revert the commit. Stores written by it load in older builds (unknown fields ignored); hosted devices are then refused on the relay (no hosted origin registered) — fail closed, Tailscale devices unaffected.

## Success criteria
All tests above green; `mobile_relay.py` (phase 6 update) passes against a v2 relay and, with the header suppressed, as v1.

## Completion note (2026-11-10)
Done on branch `phone/hosted-daemon` at API 34; report: [reports/fullstack-261110-daemon-hosted.md](reports/fullstack-261110-daemon-hosted.md). Where the code differed from this spec:
- A record without a page reaches the relay only from the room's Tailscale origin (any other `o` ⇒ `RELAY_REFUSED`). Today that was the only origin a room admitted, so nothing changes for it; it also stops a hosted record whose page fields were deleted from the file from passing as an old record and escaping the mandatory lock.
- `page`/`page_kind` edited in the file make the whole record unverified (`UNCONFIRMED` everywhere), the stricter reading of "stops verifying".
- The room admits the hosted origin while the page was found serving *or* a verified device is paired on it, so hosted phones keep working after a daemon restart (the probe is in memory and runs only when asked).
- `mobile.status` answers from the cached probe and probes off-thread, broadcasting `mobile.updated` when it is known; `mobile.pair_start {hosted}` answers asynchronously after its probe.
- `relay::turn_on` starts phone access when it is on but not running (LAN off and nothing else on), so the relay-only setup comes up once the relay is turned on.
- `MobileStatus` and `RelayStatus` live in Settings components (phase 4's files): the new shapes are building blocks in `app/src/lib/types.ts` for phase 4 to compose.
- The release-binary grep for `KELPIE_RELAY_TEST_*` belongs to phase 5 (workflows); the names exist only in `#[cfg(debug_assertions)]` code.
