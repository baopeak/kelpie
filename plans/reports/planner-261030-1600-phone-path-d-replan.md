# Phone plan: path D re-plan (2026-10-30)

Plan: `plans/261024-1205-phone-remote-control/plan.md`. Plan files only; no code changed, nothing committed. Nothing was built or run. Citations come from the phase-3 working tree, uncommitted; `mobile.rs` moved while being read, so phases name symbols there and lines must be re-grepped.

The relay decision changed twice during this work (Hong Kong VPS, then Cloudflare). No VPS content was ever written. `research-261030-1600-...` section 5 is what the plan follows.

## New execution order
3 → 3a → 3b → 3c → 3d → 3e → 4a → 4 → 7 → 8 → 9. Strictly sequential: all touch `mobile.rs`, `daemon.rs`, and 3a–3e share `mobile-settings.tsx`. Only the 4a soak runs alongside.

| # | Phase | Effort | API | File state |
|---|---|---|---|---|
| 3 | kelpie-link (in progress) | 7d | 19 | Addendum only |
| 3a | Tailscale mode: detection, HTTPS, guided install | 4d | 20 | New |
| 3b | Web Push from kelpied + delivery refactor + outbox | 4d | 21 | New (moved from 6) |
| 3c | App lock + daemon risk labels | 6d | 22, security level 3 | New |
| 3d | Settings switches | 2d | none expected | New |
| 3e | Telegram options | 3d | 23 | New |
| 4a | Validation on real Cloudflare | 1d + 7-day soak | none | New |
| 4 | One Cloudflare relay | 6d (was 5d) | 24 | Reworked, Cloudflare and `workerd` kept |
| 7, 8, 9 | Browser and screen over Tailscale | 5d, 10d, 6d | 25, 26, none | Dependencies fixed, relay/TURN deferred |
| 5 | Native shell | 7d | | Deferred banner, content kept |
| 6 | Native push, sealed details, lost-contact | 3d | | Deferred banner, content kept |

Total ~74d + soak (was ~65d); ~10d deferred, outside the total. Numbers are stable names, not an order: files could not be renamed (no delete tool here) and other documents cite them; 2b set the precedent.

## Decisions amended in `plan.md`
D1, D2, D3, D4, D5, D7, D9, D10 amended; D15–D19 added (Tailscale mode, app lock, plain-HTTP fallback, switches, Telegram). Validation Session 2 records the decisions and a table of what they reverse. Security model and success criteria updated.

## Things the brief did not state that the re-plan found
1. **`web-push` likely pulls OpenSSL.** Its manifest takes `ece` 2.2 with default features; `ece`'s default and only real backend is OpenSSL. This repo keeps OpenSSL/NASM out of Windows builds. 3b has a `cargo tree` gate and a RustCrypto module (~150 lines, RFC 8291 vector) as the fallback. Not checked by building; Bash is broken. See Q5.
2. **`tailscale serve` only proxies to `http://127.0.0.1`** (Tailscale's docs), and the daemon never binds loopback in normal use. 3a adds a loopback listener, a fixed port 8443 (the origin holds keys, passkey and push subscription, so it must not change), and never touches the user's other serve entries. The peer address, `Host` handling and WebSocket pass-through are unverified; the 3a spike tests them, with `tailscale cert` + the existing TLS listener as the kill-rule fallback.
3. **An HTTPS page cannot dial a raw LAN address** (mixed content, and the certificate name would not match). So the installed app's race is `ts` + relay, not LAN. "LAN stays in the race" holds only for a plain `http://` page, a separate read-only pairing. Practical effect: **a relay outage with Tailscale off and only home Wi-Fi leaves the installed app without a path.** Written into D3, 3a and 4. See Q8.
4. **The relay's client must now be the PWA.** Old D5 gave relay tokens to the native app only. Web page storage is per origin and the relay "serves no code", so the plan uses the daemon-origin PWA: install over Tailscale once, then its kept shell can dial the relay with Tailscale off. Needs a per-config CSP (`secure_headers` is static), a `transport_cap` rule for slots, a device `secure_origin` field, and shell-first loading in `sw.ts`. See Q6.
5. **Section 3's "reject `Origin`" cannot apply to a browser client.** Refined: desktop path rejects any `Origin`; phone path accepts only the origin registered for the room, with a valid token.
6. **iOS install order.** Safari tab and installed-app storage are separate [U], and a camera scan opens Safari. So pairing has two QR steps and an in-app scanner, with paste as a fallback.
7. **The daemon may not be able to build chat answers.** The phone's adapters build the line today, so Telegram chat approvals need a per-dialect builder, or ship only where one exists (3e step 1). Terminal approvals are unaffected.
8. **Lock is enforced at the daemon**: `hello` withholds sessions while locked, which adds a choke point to the permission check. Cache at rest on the phone stays unencrypted (documented residual).
9. mac1 and the `Endpoint` list are already in the phase-3 tree (`kelpie-link/src/lib.rs:186`, `:209`), so phase 4 reuses them instead of adding them.

## Consistency sweep
Searched for: `Hong Kong`, `VPS`, `locationHint`, `≤ 30 ms`, `relay push`, `no Web Push`, `kelpie-link::client`, `native only`, and references to phases 5 and 6. Remaining hits are the deferred phases' kept text, history, or statements of the change. Older red-team tables stay as history, overridden by Validation Session 2. The Session 1 action item naming Apple and Firebase accounts is replaced by the Session 2 one.

## Not verified
- All 3a spike items; WebAuthn UV in iOS standalone (3c has a kill rule that stops and asks); iOS Web Push reliability (7-day soak in 3b); Apple's acceptance of the VAPID `sub`; the push-service host list beyond Apple and Google; Telegram `callback_data` 64 B; per-dialect chat request fields for risk labels.
- `ak plan --help` could not be run (Bash broken), so status cells were written by hand against the hook's guidance: new rows `pending`, phases 5 and 6 `pending` under a Deferred heading, phase 3 left `pending` though it is in progress. Run `ak plan reindex` and set statuses through the CLI.

## Open questions for the user
1. **Plain `http://` LAN fallback** (no WebAuthn there). Proposed: read-only until HTTPS, no PIN, with a switch to turn reading off too. A PIN typed into a page that can be swapped on the network protects little. Alternatives: PIN unlock; block plain HTTP once the lock is on.
2. **Idle-timeout default, and grace on leaving.** Proposed: 5 min idle, and re-lock immediately on leaving as you specified. A 30 s grace would cut Face ID prompts when switching apps briefly, at some cost if the phone is grabbed. Which default?
3. **`control` for the PWA, needed before phase 7.** You decided on 2026-10-24 "`control` native only". With the native app deferred, phases 7–9 cannot work without changing that. Proposed: a device paired on HTTPS with an enrolled lock may hold `control`, granted on the desktop with OS verification. Otherwise screen phases wait for a native app.
4. **Lost-contact push** (your 2026-10-25 decision). A dead desktop cannot send Web Push and the relay holds no push secret. Options: stay deferred (planned); or the desktop gives the relay a pre-encrypted push to send when the lease runs out, which reveals the push endpoint to the relay and adds alarm logic.
5. **`web-push` crate vs own module.** If the crate brings OpenSSL, use the ~150-line RustCrypto module (planned), or accept OpenSSL in the Windows build?
6. **Non-Tailscale first install.** As planned, the first install needs Tailscale, so the relay helps when Tailscale is off, not for people who never had it. A PWA hosted on the internet would remove that, but its origin becomes a code-trust point. Planned: leave this to the going-public decision. Agree?
7. **4a pass thresholds**: echo RTT p50 ≤ 200 ms and p95 ≤ 400 ms per carrier; ≤ 1 unplanned drop per socket per day; ≤ $10/month projected. A fail stops for your decision. Are these right? Can you get SIMs on Viettel, VinaPhone and MobiFone?
8. **Relay outage on home Wi-Fi with Tailscale off**: acceptable as-is (turn Tailscale on)? If not, the installed app needs a LAN certificate, which is a separate piece of work not in this plan.
9. **Relay account details**: relay domain, and which Cloudflare account. Room cap lowered from 20 GB to ~2 GB/month per the synthesis; acceptable after 4a measures it?
10. **Raw-terminal chaff** (hides typing rhythm, adds ~10 ms and ~7 KB/s while typing): planned as an off-by-default switch. Keep it, or drop it for personal use?

Status: DONE_WITH_CONCERNS
Summary: Path D is encoded in plan.md and the phase files, with six new phases, phase 4 reworked around one Cloudflare relay, phases 5 and 6 deferred with content kept, and dependencies fixed on 7 to 9. Statuses were hand-written because `ak plan` could not run, and ten questions await the user, of which `control` for the PWA blocks phase 7.
