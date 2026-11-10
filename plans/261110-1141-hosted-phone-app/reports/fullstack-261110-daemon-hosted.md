# Daemon: hosted pairing, device page, relay v2 client — implementation report

Date: 2026-11-10. Branch `phone/hosted-daemon` (worktree `D:\kelpie-hosted-p2`, on top of `relay/v2-open-rooms`), not pushed. Only `crates/**`,
`app/src/lib/attention.ts`, `app/src/lib/types.ts` and this plan's files changed.

## API level
**34** (`crates/kelpie-proto/src/lib.rs`, documented above the constant); `HOSTED_API = 34` in `app/src/lib/attention.ts`. Every branch was at 33 when
checked (no file-transfer phone branch holds 34).

## What changed
- **Store** (`mobile/store.rs`): `PageKind` (`ts` / `own` / `hosted` / `plain`); `Device.page`, `page_kind`, `page_mac` (own tag over the record's `mac`,
  the page and the kind; written only when a page is set). `verify` requires the tag whenever any of the three is present, and both fields with it: an edit,
  an addition or a half-removal does not verify. An older Kelpie ignores the three and still verifies `mac` + `secure_mac` (unchanged). `Store.lan:
  Option<bool>` (unsealed, `None` = listeners on), `lan_on()`, `is_new()`.
- **Codes and pairing** (`mobile.rs`, `mobile/link.rs`): `PageOrigin {kind, origin}` on `Code` and `Pending`; `code_page`, `spend_code` returns
  `(Kind, PageOrigin)`; `pair_start` records its page (`ts` when the link is the Tailscale origin or the relay's kept one, `own` with a certificate, else
  `plain`) and answers `hosted: false`; `pair_start {plain}` refused while the LAN listeners are off. New `pair_start_hosted`: relay live, connected, v2,
  hosted page found serving within 10 min; QR `https://<relay host>/#p=…` with the relay endpoint only; tells the room its origins, then the pair token.
  `link::code_fits`: over the relay the slot's origin must equal the code's page, and a hosted code needs v2; on `/link` a hosted code is refused. A
  mismatch is a failed attempt, closed with `BAD_CODE`, and the code stays live. `commit_approval` seals `page` / `page_kind` into every new record.
  `mobile.pair_request` and `pending` carry `origin_kind`.
- **Reconnect over the relay** (`link::relay_page`): a device with a page must come from it; one without (paired before this) goes by the room's
  Tailscale origin and is refused when the relay names any other. The connection's `page_origin` / `secure_page` become that page, so push
  subscriptions and passkey enrolment use the device's own origin.
- **Ceilings** (`record_ceiling`, `page_may_control`, `ceiling_refusal(d, …)`): hosted ⇒ `write`, never control, with the text the spec gives.
- **Lock** (`mobile/lock.rs`): `LockPath::PasskeyAlways` for `page_kind == hosted` (any transport); `lock_gate` treats it as `Passkey` with the lock off;
  `lock_state` reports `open` for it with the lock off; `lock_enrol` allows a hosted device to enrol with the lock off. `LockPath::passkey()` replaces the
  four `== Passkey` checks for `lock_now` and the re-check (re-check itself still follows the lock switch). `control_now` keeps `== Passkey`.
- **Relay v2 client** (`mobile/relay.rs`): `dial()` returns `x-kelpie-relay-version` (1 when absent or unreadable), kept in `facts.version` for the
  connection (cleared on `down`). v2: `{"t":"origins","o":[…]}` after `allow`; v1: `origin` as before, never `origins`. `relay_origins()` = Tailscale
  origin (`r.origin`) + hosted origin while it was found serving or a verified device is paired on it. `relay_tell_origins()` re-sends only when the list
  changed (after connect, Tailscale changes, a probe, a hosted code). `on_control` → `slot_origin`: v2 takes `o` only when it is in the list the room was
  told, else `None`; v1 falls back to `r.origin`.
- **Open registration**: `check_turn_on` needs no key; `register(url, Option<key>, pk)` sends `Authorization` only with a key (typed, or kept); texts
  for 401 without key, 429 `network` / `rate`, 503 `full` (`register_refusal`); a key typed and accepted is kept, a refused one is not. `recreate` uses the
  kept key if any.
- **Hosted page**: `hosted_origin(url)` = `https://<host>` for `wss://` only. `probe(origin)`: `GET /version.json`, 10 s, no redirect, ≤ 4 KiB, `app ==
  "kelpie-phone"`, `api_min ≤ API_LEVEL`. `hosted_check` (blocking, only while phone access is on, cached 10 min) runs before `pair_start {hosted}`;
  `hosted_look` runs it off-thread when `mobile.status` is asked and the cache is stale, then broadcasts `mobile.updated`. Nothing runs on its own.
- **Status**: `relay.version`, `relay.hosted {available, origin, version, reason, install_qr}`, `relay.hosted_devices`, top-level `lan` and `new_store`,
  `page` / `page_kind` per device.
- **Token life**: hosted devices `max(30 d, dormant_days)` capped at 365 d (0 ⇒ 365 d); renewal 7 d; others 30 d.
- **LAN option**: `mobile.enable {lan}` (bool, validated); `start()` binds nothing on this network when off, refuses with "Turn on the relay or Tailscale:
  nothing else reaches this computer" when nothing else is on, reports no error for the deliberate empty listener set, and skips the address follower.
- **Daemon** (`daemon.rs`): `mobile.pair_start {hosted: true}` answers asynchronously (probe on a blocking thread, then `pair_start_hosted`);
  `mobile.status` triggers `hosted_look`. `auth.rs` unchanged (`pair_start`, `enable` stay user-allowed; matrix test passes).
- **Test seams** (debug builds and `KELPIE_E2E` only; the env names appear only inside `#[cfg(debug_assertions)]` code): `KELPIE_RELAY_TEST_URL` replaces
  the default relay (`default_url()`), `KELPIE_RELAY_TEST_CA` (PEM) is added to `dial()` roots and is the probe's root set. `e2e_guard` refuses any
  non-loopback/`localhost` relay in `dial`, `register`, `probe` and `check_turn_on` under `KELPIE_E2E`, and always in unit tests.
- **TS** (`types.ts`): `MobilePageKind`, `MobileDevicePage`, `MobilePairRequest`, `MobileHostedStatus`, `MobileRelayHosted`, `MobileHostedFields`,
  `MobilePairStart`.

## Tests
- `cargo test -p kelpied --bin kelpied mobile`: **332 passed** (baseline 310), 1 ignored.
- `cargo test -p kelpied --bin kelpied`: **1205 passed**, 3 ignored. `cargo test -p kelpie-proto`: 33 passed. `cargo test -p kelpie-link`: 15 passed.
- `cargo clippy -p kelpied --tests`: clean for this change (one pre-existing `type_complexity` warning in `mobile/sync.rs`, untouched).
- `cd app; pnpm typecheck` clean; `pnpm test` 92 files, **1717 passed**.

New tests, by the phase's list:
- Store: `a_page_is_kept_with_its_own_tag_and_cannot_be_edited_in_the_file`, `an_older_kelpie_reads_a_record_with_a_page_and_a_record_without_one_is_as_before`,
  `a_store_is_new_only_while_nothing_in_it_was_set_up`.
- Binding: `a_hosted_code_pairs_only_from_the_hosted_page_and_the_record_keeps_that_page` (Tailscale page refused unspent and counted; Tailscale code from the
  hosted page refused; pairing, `origin_kind`, record page/kind, 90 d token, `hosted_devices`, `may_control` false),
  `a_hosted_code_is_refused_on_this_computers_own_link_and_stays_live`, v1 case in `a_relay_of_the_first_version_…`; pure `a_code_is_used_only_from_the_page_it_was_issued_for`.
- Reconnect/push: `a_hosted_phone_comes_only_from_its_page_and_its_alerts_lead_there`, `a_phone_paired_before_pages_comes_through_the_relay_only_from_the_tailscale_page`,
  `a_hosted_record_edited_in_the_file_is_refused_on_the_relay`, pure `a_device_on_the_relay_goes_by_the_page_it_paired_on`.
- Ceilings: `a_hosted_phone_never_goes_past_typing` (record ceiling, refusal text, `set_permission`, panes/tk_start/screens/sc_start → `no_control`).
- Lock: `a_hosted_phone_is_locked_with_the_lock_off_and_may_set_up_its_passkey_then` (also: Tailscale device with the lock off stays `Open` and cannot enrol).
- Token life: `a_hosted_phones_token_lives_as_long_as_an_unused_phone_stays_paired`, `a_hosted_phone_gets_the_longer_token_and_a_tailscale_phone_the_usual`.
- v1/v2: `a_relay_of_the_second_version_hears_every_page_it_lets_in_and_never_the_single_origin`, `a_relay_of_the_first_version_is_told_one_origin_and_never_of_the_hosted_app`,
  `the_relays_version_is_what_its_answer_says_and_one_when_it_says_nothing`, `a_slot_comes_from_a_page_only_when_this_computer_told_the_room_of_it`.
- Registration: `a_room_is_made_with_no_key_and_a_key_goes_only_when_given_and_is_kept_only_when_taken`, `a_room_refused_says_why_in_plain_words`.
- Probe: `the_hosted_page_counts_only_when_it_says_it_is_the_phone_app_of_an_api_this_daemon_has` (redirect, oversize, wrong app, newer API, no API,
  not JSON, 404), `the_hosted_page_is_never_looked_at_while_phone_access_is_off`.
- LAN: `with_this_networks_listeners_off_phone_access_runs_on_the_relay_alone` (refusal text, no listener bound, relay keeps working, `pair_start {plain}`
  refused, `lan: None` binds as before).
- Seams: `test_seams_count_only_in_an_e2e_run_and_reach_no_relay_but_this_machines`.
- Existing relay, lock, takeover and pairing tests pass unchanged (only `add_pending`'s new argument and `Device` literals were extended).

All network tests use stand-ins on 127.0.0.1; nothing reached Cloudflare or the real relay; no daemon was started.

## Deviations from the spec
1. **Records without a page are bound to the Tailscale origin on the relay.** The spec says such records "behave as today"; today the relay could only
   admit `r.origin`, so this keeps the behaviour, but it also closes a gap: a hosted record whose three page fields are *deleted* from the file would
   otherwise read as an old record and escape the mandatory lock. With this rule it is refused on the relay (the only way a hosted page reaches this
   computer).
2. **Editing `page`/`page_kind` makes the whole record unverified** (refused everywhere, `UNCONFIRMED`), rather than "treated as no page": both fail
   closed; this is the stricter reading of "stops it verifying".
3. **Origins the room admits**: the hosted origin is included while the probe found the page serving *or* a verified device is paired on it, so hosted
   phones keep reaching the room after a daemon restart (the probe cache is in memory and only runs when asked).
4. **`relay::turn_on` starts phone access when it is on but not running** (`autostart`), so the new-user path (LAN off ⇒ `mobile.enable` fails until a
   way in exists) works once the relay is turned on. Phase 4 may still choose to call `mobile.enable` again.
5. **Status trigger**: `mobile.status` returns the cached hosted state at once and probes off-thread, broadcasting `mobile.updated` when done (the
   dispatcher thread never waits on the network).
6. **`MobileStatus` lives in `components/settings/mobile-settings.tsx`** (and `RelayStatus` in `mobile-relay-settings.tsx`), which this phase may not
   touch. The new shapes are in `types.ts` as building blocks; phase 4 composes them into those interfaces.
7. **Release-workflow grep** for `KELPIE_RELAY_TEST_` belongs to phase 5 (workflows): the names exist only in `#[cfg(debug_assertions)]` code.
8. A test-only branch (`#[cfg(test)]`) lets a `ws://` stand-in relay stand for a `wss://` one in `hosted_origin`, so the binding is exercised end to end
   without TLS.

## Unresolved / for later phases
- Phase 4: compose the new TS types into `MobileStatus`/`RelayStatus`; show `relay.hosted.reason`; use `hosted_devices` to warn before a URL change;
  `mobile.enable {lan: false}` for new setups; map `origin_kind` in the approval dialog.
- Phase 5: add `KELPIE_RELAY_TEST_URL` / `KELPIE_RELAY_TEST_CA` to the release binary grep.
- Phase 6: e2e with `KELPIE_RELAY_TEST_URL` / `KELPIE_RELAY_TEST_CA`; the probe uses only the test CA when it is set.
- Revoking the last hosted device while the page is not serving does not shrink the room's origins until the next change (harmless: no token lets
  anyone in).
