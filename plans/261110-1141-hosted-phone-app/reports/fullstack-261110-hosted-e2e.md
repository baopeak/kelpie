# E2E on an emulated phone; existing e2e updated — implementation report

Date: 2026-11-10. Branch `test/hosted-e2e` (worktree `D:\kelpie-hosted-p6`), not pushed. Nothing reached Cloudflare or the real relay; one
harness gap let a phone browser and a test daemon reach a real **push service** before it was closed (see "Incident").

## Commits
| Commit | What |
|---|---|
| `1e86250` fix(mobile) | Making a room ignored the test relay's CA (`KELPIE_RELAY_TEST_CA`), which `dial()` and the hosted probe honour: no room could be made at an HTTPS test relay. One `https_agent()` now serves `register()` and `probe()`. Debug/e2e seam only; release builds unchanged. |
| `1194a7d` fix(phone) | The link counted a relay's `desktop_offline` answer as "the computer left my first message unanswered". On the hosted app the reach probe is the relay's own `/v1/health`, which answers while the computer is away, so three quick reconnects during a relay restart showed **Not paired here (reset)** for a minute. Such a message now reached no computer and counts for nothing. Regression test in `link.test.ts` (fails without the fix). |
| `ddd4b3b` test(e2e) | `mobile_hosted.py` (new), `mobile_relay.py`, `mobile.py`, `phone_support.py` (`relay_pair_token`, `status_of`, used by both relay scripts). |

## Results (final runs, Windows 10, Edge/WebView2 157, msedgedriver 157)
| Script | Result |
|---|---|
| `python -X utf8 scripts/e2e/mobile_hosted.py <debug kelpied> --edgedriver <157> --gui <debug kelpie.exe>` | **76/76 PASS**, "processes of this run still running: none" |
| `python -X utf8 scripts/e2e/mobile_relay.py <debug kelpied>` | **49/49 PASS** (run alone; a run concurrent with the hosted one hit 4503 "busy" pacing once, load only) |
| `python -X utf8 scripts/e2e/mobile.py <shots> <scratch>` (via a scratch launcher that starts tauri-driver with the isolated env) | **246 passed, 0 failed**, launcher: "processes left: none" |
| Unit | `cargo test -p kelpied --bin kelpied mobile` 333 passed; `app`: typecheck clean, `pnpm test` 94 files / 1777 passed, `i18n:check` 0 missing; `relay`: `pnpm test` 69 passed; `pyflakes` clean on the four scripts |

### `mobile_hosted.py`, by the phase's checks (all PASS in the final run)
1. **Setup**: new store; `mobile.enable {lan:false}` refused with "nothing else reaches this computer" yet enabled; `mobile.set_relay {on:true}` with no key makes the room; relay version 2; hosted app found at `https://localhost:<port>` with version `v0.1.15`; phone access running on the relay alone; daemon's TCP listeners: none on 4817, only loopback.
2. **Pairing**: install step first (`data-install=android`, secure context, no scan yet); "Continue in this browser"; hosted code = hosted page link with the relay as its only endpoint; pasted link → six digits, request `address: the relay`, `origin_kind: hosted`, approved at the daemon; trust sheet with the bundle fingerprint equal to the manifest's `bundle_sha256`; CDP log: the page opened no socket but `wss://localhost:<port>/v1/phone?…`; device `page_kind: hosted`, `page` = hosted origin, may type; IndexedDB record holds a relay token expiring in ~90 days (Q3 a).
3. **Lock**: enrol screen; virtual authenticator passkey (rpId `localhost`) set up in the approval window; reload ⇒ `unlock`, no rows; passkey opens it; the phone's own key copied out of IndexedDB, used by a `link_noise` program through the relay with no passkey: locked hello with no sessions/projects, `list/sub/input/chat_sub` refused `locked` — with the lock switch on **and off** (Q1 a).
4. **Write**: typed `echo …-$((6*7))` in the page, the shell ran it; `mobile.set_permission control` refused ("a phone using the hosted app can read and type…"); a program paired as a hosted page with its own passkey gets `no_control` for `tk_start`, `screens`, `sc_start`.
5. **Alerts**: stand-in push service; needs-you alert's `navigate` = `https://localhost:<port>/#s=<id>&a=1`; the real worker (observed from inside) handled the push and showed it with that link; a synthetic `notificationclick` opened that session on the hosted origin.
6. **Headers/CSP**: `/`, `/sw.js`, `/version.json`, `/manifest.webmanifest`, `/icon.svg`, an asset: headers exactly as `_headers` gives; unknown path = bundle's 404 page; no CSP violation from install through alerts; `verify_live.py --origin … --manifest …` exit 0, exit 1 after one byte of `public/icon.svg` changed.
7. **New version while open**: B built with `KELPIE_APP_VERSION=0.1.15-b`, swapped into the copy's `public/`, wrangler restarted on the same persist folder; A's `claude-adapter-*.js` 404 on the origin; B's worker took page A over (`controllerchange`) keeping A's cache beside B's; a chat opened on page A loaded A's chunk (200, no 404 anywhere) and showed the agent's text; next foreground ⇒ page loaded B's index; a second foreground did not reload; draft typed in A present in B; `verify_live` against B's manifest exit 0.
8. **Replace**: second isolated daemon's hosted code; the page asked once ("This app is paired with … loses this phone"); after yes the browser subscription was ended; paired with B as hosted; with the stand-in answering 410, daemon A's alert test made A drop the phone's subscription.
9. **Limits**: wrangler with `ROOMS_PER_NETWORK_DAY:2`: rooms 1 and 2 made, the third refused "This network made too many rooms today…"; restarted with `REGISTRATION_KEY` set: "This relay needs a registration key".
10. **Relay state**: no marker, typed text, session names, draft, chat text or phone name in either persist folder (files present and searched, UTF-8 and UTF-16).
- **GUI** (`--gui`): app's own isolated daemon confirmed (endpoint, data dir, verification seam); hosted row offered no pairing before; Settings → Mobile switch on ⇒ phone access on, LAN off, relay on, exactly one `relay_on` audit entry; guide opened on the hosted app, Step 1 = `https://localhost:<port>/`; Step 2 code on the hosted app; that link in a fresh Edge paired; approval dialog `data-origin-kind=hosted`, choices read/write only, "It pairs on: hosted app"; device badge "hosted app"; "What this trusts" names Cloudflare.

### `mobile_relay.py` additions (PASS)
Open-registration workerd: room with no key, version 2; a phone pairs; the daemon's control frames (unmasked from the tap) carry `origins` with the Tailscale origin and never `origin`; the relay's `open` carries `o`; a phone with a foreign `Origin` gets 403. Then the existing key-mode flow unchanged. New: a tap that strips `x-kelpie-relay-version` ⇒ status `version: 1`, the daemon sends `origin` and never `origins`, a phone pairs and reconnects with its token. "Off" now measured on that tap.

### `mobile.py` additions (PASS)
Without a relay the hosted row says "Turn on the relay first…" and offers no pairing; `mobile.enable {lan:false}` refused with "nothing else reaches this computer" and Settings drops "Pair a browser on this network"; back with `lan:true`.

## Incident: a real push service was reached (fixed in the harness)
In runs before the final ones, the page reloaded into version B ran with Edge's real `PushManager` (the stand-in had only been injected into the old page), so the app re-subscribed for real: headless Edge registered with its own push service, and daemon A got a real endpoint. In one run A's alert test then POSTed an encrypted, fixed-text test alert there and got a 2xx. No session content, nothing to Cloudflare or the relay. Fixed in `mobile_hosted.py`: the stand-in is installed with `Page.addScriptToEvaluateOnNewDocument` (every load, before the app's scripts), a check asserts the reloaded page still holds the stand-in's subscription, and the phone's Edge runs with `--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1` so it reaches nothing off this machine. The daemon side is not fenced: see Q2.

## Harness findings (not product bugs)
- **wrangler dev rewrites `Origin`**: its ProxyWorker rewrites every header naming the served host to the Worker's inner URL, plain `http` unless the config's `[dev] upstream_protocol` says `https`; the room then refused the page (`403 origin`). The run's copy of both configs gets `[dev] local_protocol/upstream_protocol = "https"`. No TLS front was needed: `--local-protocol https --https-key-path --https-cert-path` exist in wrangler 4.156.
- Python 3.14's strict verification needs Authority/Subject Key Identifiers: the run's CA + `localhost` leaf carry them.
- wrangler reloads the Worker when a served file changes, dropping every socket; the script waits for the daemon and page to reconnect.
- CDP's virtual authenticator can vanish from the page's DevTools session (the ceremony then hangs at "Asking…"); the credential (private key included) is kept and re-added to a new authenticator.
- `KELPIE_LOAD_FORCE=L0:calm` for the test daemons (as `mobile.py`): under host load the page showed `busy`.

## Departures from the spec
1. Wrangler started with `node relay/node_modules/wrangler/bin/wrangler.js` from a copy of `relay/` (no `node_modules` copied), secrets via `--var`; `[dev]` https appended to the copy's configs (above).
2. Push stand-in installed per document via CDP, plus browser resolver rules (Incident).
3. Check 8's 410 is triggered by daemon A's alert test (`notify.test webpush`, the Settings button), not by a needs-you event: an event's alert also depends on this machine's window focus and the person's idleness, which a test cannot hold still; after the replacement a needs-you event on A queued nothing within 30 s.
4. "One verification" is checked as exactly one `relay_on` audit entry: the daemon has no verification counter.
5. Check 7's worker update is triggered with `registration.update()`, standing for the browser's own periodic check.
6. GUI part: the script starts tauri-driver itself (the relay port and CA are made at run time), with the app's own endpoint, data folder and **`WEBVIEW2_USER_DATA_FOLDER`**: the debug app has the installed app's identifier and would otherwise share its WebView2 profile with the owner's running Kelpie. `mobile.py` was run the same way through a scratch launcher (not committed); its docstring does not mention the WebView2 profile.
7. The GUI test waits for the relay connection before pressing "Already installed: Step 2" (Q3).
8. Ports per PID: relay `47000+pid%1000`, limits relay `46000+`, phone driver `45000+`, inspector `44000+`/`+1`, tauri-driver `43000+`, native `42000+`; the script refuses if one is taken.
9. The phone's WebDriver client (`EdgePhone`) and `Cdp` live in `mobile_hosted.py`; `mobile.py`'s own `Phone`/`Cdp` were left as they are.
10. Not run on macOS; Windows specifics: `taskkill /T` for process trees (POSIX path uses process groups), msedgedriver, tauri-driver's GUI part.

## Leftover processes and files
Every run ended with "no process of this run is left" (recorded PIDs and their children, matched by start time; one early false alarm was a reused PID). The owner's Kelpie (PID 8648) and daemons (4012, 10964) were never touched. Run folders are removed by the script; `mobile_relay.py` leaves its workerd/Tailscale/cwd folders by design (pre-existing): today's were deleted by hand, along with `relay/workerd/dist`. `app/node_modules`, `relay/node_modules` and `app/dist-phone` stay (gitignored).

## Open questions
1. **Expired relay token on the hosted app**: with `relayExpires` set in the past by hand, five runs showed "Reconnecting (try N)" with no end, one run showed `live`; not a stable e2e signal, so it is not a check. A "pair again" state would need a product change (left open, as phase 3 said).
2. **Daemon push seam**: with `KELPIE_PUSH_TEST_BASE` set, real push hosts stay allowed (`web_push.rs` test: "the list still holds"). Fencing them under `KELPIE_E2E`, as `e2e_guard` does for relays, would make the Incident impossible from the daemon side. Owner's call.
3. **Guide timing**: the guide opens once the hosted app is found, which can be a moment before the relay connection is up; pressing Step 2 at once says "the relay is not connected yet: try again in a moment" (pressing again works).
4. Why a needs-you event on daemon A after the replacement sent no alert within 30 s was not pinned down (likely the focus/idle rule).
