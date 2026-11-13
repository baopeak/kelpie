# Code review: screen through the relay (trust model d)

Commit `a674dcf` ("wip: screen through the relay") on `screen/remote-policy`, worktree `D:\kelpie-rd-0`. I reviewed it read-only and did not run cargo, as instructed. I checked it against `phase-01b-trust-policy-and-connect-choice.md` and the owner decisions 3, 4, D4, D6–D8 and D10 in `plan.md`.

Checks I ran: `pnpm exec vitest run` on `phone/src/{screen-paths,screen,conn-path,lock,prefs}.test.ts` and `src/lib/mobile-options.test.ts` (6 files, 246 tests passed). I also ran a script that checks every new `t("…")` string in the phone and settings files has a Vietnamese entry. None is missing.

## Critical

None found.

## High

### H1. A screen started over a direct link can be rebound onto the relay, where the switch, the rate cap and the monthly budget no longer apply

- **Where:** `crates/kelpied/src/screen.rs:793` (`relay: via == Via::Relay`, fixed when the screen starts), `screen.rs:871` (`watch` checks `screen_held(.., hold.relay)`), `screen.rs:307/467/494` (relay bytes are counted only `if hold.relay`), and `screen.rs:825` `rebind`. `rebind` checks only the device, that the hold is live and the gone window. It never looks at the route.
- **What happens:**
  1. A PWA paired over Tailscale has `control` and starts a screen over Tailscale. That hold has `relay=false`, the helper runs without `max_kbps`, and no Face ID is asked (correct).
  2. The phone loses Tailscale, for example on 4G. The link reconnects through the relay, which `Prompt::Relay` allows for Tailscale-paired phones. With the switch on, `sc_rebind` on that relay connection passes `screen_refused` (relay rule, no Face ID, which D8 allows). The hold now streams through the relay.
  3. On the relay the stream is:
     - **Uncapped.** The helper was started without `max_kbps`, so it can climb to 4000 kbps (MAX_KBPS), against D4's limit of 500.
     - **Uncounted.** `hold.relay=false`, so `screen_relay_use` never grows and the 1 GB budget is never checked. 4 Mbps is about 1.8 GB an hour, so the room's 2 GB allowance runs out and the relay cuts the link with 4010. D4 exists to prevent exactly this.
  4. The person turns *See and control the screen without Tailscale* off. `watch` asks `screen_held(device, browser, relay=false)`, which applies the **direct** rule. That rule still passes, so video keeps flowing through the relay after the switch is off. New `sc_input` on that connection is refused, but frames keep going out. This breaks requirement 2 of the spec ("Turning `remote` off ends every relay-route hold within 1 s") and acceptance criterion 4. `set_screen` calls `stop_grants` only for *hosted* devices that it lowers, so nothing else ends this hold.
- **The reverse case:** a screen started through the relay that rebinds onto Tailscale stays capped at 500 kbps, keeps counting its bytes as relay use, and is still ended by the switch. That is safe but wrong: D6 says a direct link keeps today's rules.
- **The docs claim otherwise:** `docs/architecture.md` ("The hold remembers its way… turning `remote` off ends every screen through the relay"), and `docs/user-guide.md` / `user-guide.vi.md` ("ends every screen through the relay within a second").
- **Suggested fix:**
  - Keep the route on the bound link rather than only on the hold: add `relay: bool` to `Link`, set it in `Link::of(phone)`.
  - `watch` uses the current link's route, or the start's route while the link is gone.
  - Count relay bytes by the route of the link the frame actually went out on (`st.conn`).
  - When a hold rebinds onto the relay, either:
    - (a) refuse `sc_rebind` across routes until the 1c mid-session switch exists (code `sc_gone`, so the phone starts again and asks for Face ID), or
    - (b) check the budget and send the helper a new cap (`screen.limit {sc, max_kbps}`, which needs a helper verb).
  - Option (a) is a small change and consistent with D8 being about "the same session". Option (b) is the eventual design.
  - Add a broker test: start over a direct link, rebind on a relay `Phone`, turn the switch off, and expect `sc_end revoked` within one `WATCH`. Then extend `screen_relay.py` with the same case.

## Medium

### M1. The monthly relay budget is checked only when a screen starts

- **Where:** `crates/kelpied/src/mobile/screen_access.rs:224` (only `screen_may_start`, and `screen_paths` at :249, call `relay_screen_spent`), and `screen.rs` `watch`.
- **Scenario:** with 5 MB left, a person starts a screen through the relay and keeps using it. Input resets the 10-minute idle stop. At 500 kbps that is about 225 MB an hour, so the 1 GB cap is overrun without limit within one session, and several sessions in a month can exhaust the room's 2 GB. D4's reason for the cap ("4010 would cut the phone off") is not upheld.
- **Fix:** in `watch`, once a hold whose link is the relay pushes the month past `relay_ws_month_mb`, end it with a new reason, for example `relay_budget_spent`, which needs adding to the phone's `endedText`. Or give a grace margin and then end it. Add a unit test with `count_relay_screen`.

### M2. Doubted screen options keep the untrusted file's other fields, and a later save seals them

- **Where:** `screen_access.rs:50` (`ScreenOptions { remote: false, ..o }`), `requested_screen` (it starts from `screen_options()`), and `widened_by` (it compares against that base).
- **Scenario:**
  1. A local program edits `screen` in the store, for example `relay_ws_month_mb: 1000000`. The tag fails, so the options are doubted and `remote` is false.
  2. Later the person turns the switch on and verifies `Prompt::RemoteScreen`.
  3. `next` is the doubted base with `remote: true`. `set_screen` seals it, so a 1 TB monthly budget is now trusted. `requested_screen`'s range checks (64–1024) apply only to fields named in the request, so they are bypassed. `relay_ws_kbps` is clamped where it is used (`min(MAX_RELAY_WS_KBPS)`); `relay_ws_month_mb` is never clamped.
  4. A narrowing edit (no prompt) also seals the tampered fields.
- `check_lock_options` handles the same case with `LockOptions::tightest()` and drops everything from the file.
- **Fix:** when doubted, keep `ScreenOptions::default()` in memory (remote off, defaults), and do not write the file. Also clamp `relay_ws_month_mb` / `relay_ws_kbps` to their ranges on load.

### M3. `screens` through the relay lists every window title without a Face ID check or an audit line

- **Where:** `screen.rs:699` `targets` and `mobile.rs:2874` `screen_refused`. `Conn::note` (`mobile.rs:2685`) skips takeover verbs, so `screens` is never in the audit log.
- **Scenario:** with the switch on, code served by the relay can call `screens` at any time while the app is unlocked. Each call returns `targets` with full window titles: mail subjects, browser tab titles, document names. Before this commit nothing about programs outside Kelpie ever reached a relay connection. The audit line deliberately names "the program, never the window's title", which shows that titles are treated as sensitive. The policy gates only the start behind Face ID.
- **Fix (product decision):** one of these:
  - through the relay, answer `screens` with program names and monitors only, and send titles after a Face ID start;
  - or require the Face ID proof for `screens` through the relay as well;
  - or at least audit `screens` on the relay.

  See the unresolved questions.

### M4. Phones are not told when the switch changes, so the screen entry stays stale

- **Where:** `screen_access.rs:86` `set_screen`. It sends `revoked` only for lowered hosted devices. `set_lock` calls `tell_lock` for every device.
- **Scenario:** the person turns the switch on while a phone is connected through the relay. The hello's `device.screen` stays `screen_remote_off`, so the phone keeps hiding the entry (`screenOffered`) until it reconnects or unlocks again. The person turned it on and it "doesn't work". Turning it off leaves a stale `ok`, which is only cosmetic because the start is refused.
- **Fix:** after `set_screen`, have each connected device's connection send its hello again (or a `screen` notice). That can be the same mechanism `tell_lock` uses.

## Low

- **L1. The "Tailscale" way covers every direct route.** `screen_policy.rs` `may_start` maps any direct `auto` to `Via::Tailscale`, so `methods.tailscale = false` also blocks LAN and own-HTTPS screens, and the phone and audit label them "via tailscale". The Settings label "Tailscale" understates this. Turning a way off also does not end a running direct screen (`allowed()` ignores `methods.tailscale`), while turning `relay_ws` off does end relay screens. That inconsistency is pinned by the grid test. Either name the way "Direct (Tailscale or this computer's address)" or document it.
- **L2. One prompt text for every widening.** `user_presence.rs:169` uses `Prompt::RemoteScreen` text "let your phones see and control this computer's screen without Tailscale, through the relay" for every widening, including ticking *Tailscale* again or raising kbps or MB. That misdescribes what the person is verifying. Either word the prompt by what widened, or use a generic "let the screen use more ways" text when `remote` is unchanged.
- **L3. Two `set_screen` calls can interleave.** `next` is computed before `after_verifying`, so a narrowing applied while a widening prompt is open is overwritten by the stale `next` once the prompt is verified. For example, relay_ws unticked during the "turn on" prompt comes back. Recompute `next` from the current options inside the closure, applying only the requested fields. The `set_lock` pattern has the same issue, and it was already there.
- **L4. A status refresh can restore old options in memory.** `check_screen_options` (from `verify_devices` on every `status()` and phone connect) reads `config.screen`, then calls `mac_key(false)`, which can be slow, then writes the in-memory options. A concurrent `set_screen` (switch off) can therefore be overwritten in memory by the pre-change options until the next `status()`. It heals itself on the app's refresh after `mobile.updated`, but for that window a switch-off may not end relay holds. Fix: take the options mutex across the read-verify-write, or give the options a generation counter.
- **L5. An app older than the daemon ignores `max_kbps`.** The relay stream is then uncapped; only the monthly count protects the room. The comment says so. Consider refusing `Via::Relay` when the app's provider version does not advertise `max_kbps`.
- **L6. The Face ID expiry has no margin.** The phone sets expiry = estimated server now + exactly 60 000 ms (`app/phone/src/lock.ts:89/373`), and the daemon accepts at most `now + 60 000` (`mobile/lock.rs:37`). The estimate is always slightly behind the server, so this works today, but any positive clock-offset error (an NTP step on the computer between the hello and the start) makes every start fail with `needs_face_id`. Use 50 s on the phone, or allow a few seconds of skew in the daemon. Dangerous approvals have a 60 s margin (120 s accepted, 60 s used).
- **L7. Used Face ID checks are kept only in memory.** `screen_checks` is lost when the daemon restarts, and iOS passkeys keep `sign_count` at 0, so an assertion captured within its 60 s could be replayed once against a restarted daemon. The impact is small: the capture would have to come from the page itself, and the trust model already gives that page this power. Noting it only.
- **L8. Settings and the user guide say things that are not true:**
  - `settings-catalog.ts` (`mobile-screen-remote` description): "typing into sessions and taking over browser panes stay with Tailscale" is wrong. Hosted phones can type, as the guide and `ceiling_refusal` say. Suggested wording: "Control then means the screen only: taking over browser panes stays with Tailscale."
  - `phones:` in the same entry, and the user guide: the switch also covers a Tailscale-paired PWA that falls back to the relay (D6). That is not said.
  - `Prompt::Raise` for a hosted device still says "take control of this computer". Consider "see and control this computer's screen".
- **L9. Minor issues:**
  - `user_presence.rs:170-171,979`: the new match arms and the test list entry are indented 8 instead of 12, so the file is not rustfmt-clean. CI has no fmt gate, but it is still drift.
  - `mobile/tests/lock.rs` has no trailing newline.
  - The commit message `wip:` is not a conventional commit; squash it before merging.
  - The commit includes two `plans/reports/*.md` files. Fine if intended.
- **L10. The phone shows the starting route after a rebind.** It records `usedVia` once at the start and shows it in the details line from then on. After a rebind onto another route it shows the old way: "Tailscale" while frames go through the relay. This is related to H1. Show the live route once a rebind happens, or clear `usedVia` on a rebind.
- **L11. Some direct-route phones now lose the screen entry in their lists.** On a daemon at 41 or above, `screenOffered` hides the entry when `device.control` is `lock_off`, `no_passkey`, `locked` or `plain_page`. Before, the entry showed and the start explained the refusal. The settings fact still explains it, so this may be intended. Confirm it is.

## Verified OK

**(a) The gate**

- **Path to the relay screen.** Through the relay, screen verbs go through `screen_refused`: `granted >= Write` (gate open, not ReadOnly or Locked), then `screen_policy::allowed`. That requires:
  - a record with `control`;
  - `remote` and `relay_ws` on;
  - a secure origin;
  - the lock on, a passkey, and `window_open`.

  `sc_start` then checks all of it again in `screen_may_start`, plus the budget and a fresh UV assertion over `recheck_challenge("screen:"+target, "relay", expiry)`. UV is enforced in `webauthn::check_assertion` (`UP|UV`). The target signed is the same string sent to the helper.
- **The relay never reaches a takeover.** Non-screen verbs still go through `control_refused`. The relay's `transport_cap` is `Write`, and `handle` lowers `granted` to `Write` whenever `control_now` fails, so `tk_*`/`panes` are refused. Hosted devices also fail `control_rule` (`hosted_app`). Screen verbs return before the generic `needs() > granted` check, and only `sc_*`/`screens` take that branch (`verbs.rs:173`, `screen()`).
- **The direct route gets nothing it did not have.** `screen_refused` defers to `control_refused`, the same as before. `screen_may_start` only adds `method_off`. For direct holds, `screen_held` equals the old `control_held`: the record, `enabled`, `broken`, verified, `page_may_control` and the window. It is stricter for an App record used from a browser.
- **Native records.** An App record over the relay (native, not a browser) gets the screen without the switch. That matches its earlier behaviour (`transport_cap` gave it Control). Pairing now always creates `Kind::Pwa` (`mobile.rs:1632/1677`, `link.rs:605`), so this applies only to legacy records.

**(b) Replay and races**

- The used-challenge set is checked before verifying and checked again, then inserted, under one lock after verifying. Two concurrent starts with one proof give exactly one `Fresh`.
- Starts are serialized globally by `starting`.
- Expiry must be greater than now and at most now + 60 s, and entries are pruned at expiry.
- Failures count towards `lock_attempt_allowed` through `lock_failed`.
- The counter is kept only on success.

**(c) Settings integrity**

- The MAC input `kelpie-screen-options/1|remote|ts|p2p|turn|ws|kbps|mb` is unambiguous and separated from the lock options.
- A doubted file is never written over without the person (test `screen_options_that_do_not_verify…`).
- `widened_by` covers all seven fields.
- Switch-off lowers Control hosted devices to Write, calling `stop_grants` outside the config lock and sending the events.
- `mobile.reset` clears `screen` and checks again.
- `set_lock` off also lowers hosted pages (`Kind::Pwa`).
- `set_permission`/`raise_prompt` check `record_ceiling` again with the current options when they apply.
- `mobile.set_screen` is in `APP_ONLY` (daemon and Tauri `SECURE_CALLS`) and requires `is_verified_app`.

**(d) Locks**

- No nested `config.lock()` in the new code. `status()` reads `screen_status()` before taking the lock. `screen_facts` takes config last with no calls under it. `set_screen` uses temporaries. `count_relay_screen` releases config before `keep()`.
- `Broker::end` is never called with `holds`, a hold's state, or the config lock held.
- No lock-order inversion between `screen_memory.options` and `config`.
- `keep()` from `watch`/`end` is a once-a-minute file write on an async worker. That is acceptable, though `spawn_blocking` would be cleaner.

**Month and rate accounting**

- The month key is local `%Y-%m` and resets on the first count of a new month. The status uses the same filter.
- Only frames actually sent are counted, using their wire length.
- `Bandwidth::set_cap` clamps the cap to between MIN and MAX. Only increases are bounded by it, and the controller is never rebuilt mid-stream, so the cap holds (`win/stream.rs:365-368`). Tauri passes `max_kbps` through.

**(e) Compatibility**

- A phone older than 41 talking to a new daemon: direct is unchanged; through the relay it gets a clear refusal.
- A new phone talking to a daemon older than 41: no `paths` means no `via` and no proof is sent, and `device.screen` is ignored.
- `SCREEN_REMOTE_API = 41` matches `API_LEVEL`. The proto doc comment matches the wire format: field names, codes, `device.screen`, `paths`, `relay {ours, host}` and `via`.
- The phone's `screenChallenge` equals `recheckChallenge("screen:"+t, "relay", e)` (tested), and the daemon uses `way.word()` = "relay".
- The phone asks for Face ID exactly when the daemon will: via `relay`, or `auto` on the relay route.

**(f) Text and docs**

- No plan or phase IDs in code, tests or strings.
- Every new string has a Vietnamese translation.
- `user-guide.vi.md` follows `writing-style.md`: full sentences joined by connectives.
- `scripts/e2e/screen_relay.py` uses `screen_support` isolation (its own endpoint and data folder; it drops `KELPIE_ALLOW_REAL_DAEMON`). The `screen_support.start` change now also stops the app if setup fails.
- The grid test covers page kind × route × switch × lock × passkey × window × record × methods × UV × via (over 10 000 rows).

## Unresolved questions

1. M3: is listing window titles through the relay, with no Face ID, acceptable under trust model (d), or should `screens` through the relay hide titles until a start?
2. H1: should a rebind across routes be refused for now (consistent with D8's "same session") until the mid-session switch in 1c sends the helper a new cap?
3. L1: should the "Tailscale" way also govern LAN and own-HTTPS direct screens?

Status: DONE_WITH_CONCERNS
Summary: The relay gate itself is sound: it needs the switch, `control`, an open gate, the lock, a passkey, a window and a fresh one-time UV assertion bound to the target, and the relay still never reaches a takeover. But a screen started over a direct link that rebinds onto the relay escapes the switch-off, the 500 kbps cap and the monthly count (H1), and the monthly budget is checked only at start (M1).

## Re-review (uncommitted fixes on top of b8ca904)

I read `git diff` in `D:\kelpie-rd-0`, without running cargo. I ran vitest on `phone/src/{lock,screen,screen-paths}.test.ts` and `src/lib/mobile-options.test.ts` (4 files, 211 tests passed) and the Vietnamese coverage script again (nothing missing).

### Resolved

- **H1: fixed in the daemon.** `Broker::rebind` (`screen.rs:834-841`) now compares `hold.relay` with `phone.relay` before any other check. When they differ it ends the hold with `route_changed` (the helper is told, the audit says it, and the month's leftover relay bytes are counted in `end`) and refuses with `sc_gone`.
  - Rebinding is the only place a hold is bound to a new connection (`st.conn = Some(Link::of(..))`, :852), so no screen can now run on a route other than the one it started on. That keeps the rule, the cap and the count consistent.
  - Because the route check comes first, a hold that is still bound to a live older connection is also ended when its phone asks again from the other route. That is the right outcome: before, it lingered until the older socket closed.
  - D8 still holds: a rebind on the same route asks for no Face ID.
- **M1: fixed.** `watch` (`screen.rs:888-896`) counts the relay bytes and ends the hold with `relay_budget_spent` once the month is spent, within one 250 ms look. That uses the same `>=` comparison as the start check, and the call takes the config lock with no other lock held. The phone's `endedText` has the reason, and `sc_end` reaches a live connection. The desktop falls back to its generic "no longer sees this screen" text, which is fine.
- **M2: fixed.** Settings that fail verification now fall back to `ScreenOptions::default()` (`screen_access.rs:50`), and the test asserts the tampered `relay_ws_month_mb` is not taken. The file is still not written over. The defaults' `methods` (all on) are no wider than deleting the entry would give, so this does not fail open.
- **M3: fixed, with your default.** Through the relay, `screens` sets `title := app` for windows (`screen.rs:704-710`). Monitors carry no title. The full title reaches the phone only in the `sc_start` answer, after Face ID, which fits trust model (d). Direct connections are unchanged. The owner question remains whether program names alone are acceptable.
- **L6: fixed.** The phone uses `SCREEN_RECHECK_MS = 45_000` against the daemon's 60 s maximum, leaving 15 s for clock error or a slow Face ID sheet. The test was updated.
- **L8: fixed.** The catalog `description` and `phones` text are accurate now: hosted phones keep typing, and D6's Tailscale-paired phone on the relay is covered. The Vietnamese keys match the English strings exactly and read in full sentences.
- **Docs:** `architecture.md` and `user-guide.md` now describe what the code does (`route_changed`, the in-flight budget stop, program names through the relay).

### New or remaining findings

- **R1 (Low, the claimed phone text is not wired).** `endedText("route_changed")` (`app/phone/src/screen.ts:788`) can never be shown.
  - When a rebind is refused, the daemon sends nothing on the new connection except the error `sc_gone` (the hold's own link is `None`, so `end`'s `sc_end` goes nowhere). `ScreenSession.reconnected()` (:1093-1098) turns any rebind error into `endedText("link_lost")`.
  - The person is therefore told "The connection was lost for too long" when the real reason is that the phone came back another way and needs to start again.
  - **Fix:** refuse with code `route_changed`, which older phones still treat as an error and so as ended. In `reconnected()`, map `r.code === "route_changed"` to `endedText("route_changed")`, and add a phone test.
- **R2 (Low, test quality).**
  - `a_screen_whose_phone_comes_back_another_way_ends_and_is_started_again` (`screen/tests.rs:779-793`) ends with `let _ = r.next("sc_end").await;`. That line waits up to 3 s and asserts nothing; no `sc_end` can arrive because the link was closed. Remove it, or assert what the new connection hears.
  - There is no test for the reverse direction (a relay start coming back direct).
  - There is no unit test for M1's end inside a session. The rig could set `relay: true` on the hold through a relay `Phone` start, call `count_relay_screen(1 GiB)`, and expect `sc_end relay_budget_spent`.
  - `tests.rs` has no trailing newline.
- **R3 (Low, stale comment).** The `Hold` doc comment (`screen.rs:158-159`) still says "A rebind keeps them: a screen started through the relay is held to the relay's rule, whichever connection it is on." It should now say that a rebind from the other way ends the hold (`route_changed`).
- **R4 (Low, Vietnamese docs).** `user-guide.vi.md` was not updated for the stop in mid-session when the month runs out, for windows named by program, or for `route_changed`. The English guide has all three. The Vietnamese guide is condensed, so one sentence would cover it.
- **R5 (Low, cosmetic).** Through the relay a window row's title and its subtitle (`r.app` in `screen-view.tsx` `Group`) are now the same string. The phone could drop the subtitle when they are equal. A window with no `app` gets an empty title.
- **R6 (Low, legacy only).** An App record over the relay has no budget check at start but is now ended by `watch` within 250 ms once the month is spent. It is harmless, since pairing no longer creates `Kind::App`.

### Not changed, to be listed by the coordinator as owner questions or follow-ups

- **M4:** phones are not told about `set_screen` until they reconnect or unlock again.
- **Q3 / L1:** the "tailscale" way covers every direct route.
- **L2:** one prompt text for every widening.
- **L3:** two `set_screen` calls can interleave.
- **L4:** a concurrent `check_screen_options` can restore old options in memory.
- **L5:** an app older than the daemon ignores `max_kbps`.
- **L7:** used Face ID checks are kept only in memory.
- **L9:** rustfmt indentation in `user_presence.rs` and the `wip:` commit message.
- **L10:** the live chip shows the start's way. With H1's fix a screen can no longer change route, so L10 is now moot.

Status: DONE_WITH_CONCERNS
Summary: H1, M1, M2, M3 and the catalog and expiry items are fixed correctly in the daemon: a screen can no longer run on a route other than its start's, and the monthly budget now ends a screen in progress. What remains is Low: the phone never shows the new `route_changed` text because a refused rebind still reads as "lost for too long" (R1), plus test and comment gaps (R2–R5) and the owner questions M4 and Q3.
