# Desktop Settings → Mobile: hosted default, trust text — implementation report

Date: 2026-11-10. Branch `phone/hosted-settings` (worktree `D:\kelpie-hosted-p4`, on top of `phone/hosted-phone-app`), not pushed. Feature commit
`42ea450`. No daemon or app was started; nothing reached Cloudflare or the real relay.

## Files
`app/src/components/settings/mobile-settings.tsx`, `mobile-relay-settings.tsx`, `app/src/lib/mobile-options.ts` (+ test), `settings-catalog.ts`,
`app/src/locales/vi/settings.json`. `types.ts`, `ipc.ts` and the Tauri bridge untouched (`daemon()` and `mobile_secure_call` pass params through as JSON).

## What changed
- **Types**: `MobileStatus extends Partial<MobileHostedFields>`, `pending: MobilePairRequest[]`, `MobileDevice extends MobileDevicePage`,
  `RelayStatus extends Partial<MobileRelayHosted>`; `pair_start` answers typed `MobilePairStart`.
- **New setup** (`newSetup`, `turnOnAccess`): only when `api ≥ HOSTED_API` and `new_store === true`: `mobile.enable {lan: false}`, then
  `mobile.set_relay {on: true}` (verification, no key, default relay), then the guide opens at Step 1. The enable's expected refusal is passed over when
  status says phone access is on; otherwise it is thrown. Any other store: `mobile.enable {}` as before. A refused room shows its reason and a note with
  *Use Tailscale instead* (`enable {lan: true}` then `set_tailscale {on: true}`).
- **Guide**: one modal, `{via: "hosted" | "tailscale", step}`; hosted Step 1 = `relay.hosted.install_qr` / `origin`, "Already installed: Step 2";
  Step 2 = `pair_start {hosted: true}`. Errors now also show inside the modal. Data attributes kept (`data-tailscale-guide`, `data-guide-next`, …), plus
  `data-pair-guide`, `data-guide-via`, `data-hosted-guide`.
- **Rows**: new `mobile-hosted` (*Hosted phone app*: primary *Pair a phone* while `hostedPairing`, daemon reason, *What this trusts* with the spec's text)
  and `mobile-lan` (*Pair browsers on this network*, under Advanced). *Pair a browser on this network* only while `lan !== false`. Catalog lines for
  `mobile-access` / `mobile-relay` rewritten so they hold with the hosted app (no "serves no page", no "needs Tailscale the first time").
- **Approval** (`approvalChoices`): `origin_kind: hosted` ⇒ Read is the default, Read and type carries the spec's line, "asks to" capped at type; the
  page is named ("It pairs on: …"). Control is never a choice at approval.
- **Devices**: page badge (`pageKindLabel`); `permissionChoices` never includes control for `page_kind: hosted`; the "may take control once…" hint is
  hidden for them; lock row says hosted phones always use the lock; the summary's lock line says so when the lock is off and a hosted phone exists.
- **Relay row**: hosted line (origin + version / checking / not served + reason), key field only after the relay answered it needs one (`asksForKey`),
  moving-relay warning from `hosted_devices` before verification, "What the relay can see" says it serves the phone app, the Tailscale-origin warning
  only when the hosted app is not served, token line mentions pairing on the hosted app.
- **Summary**: names the hosted app as a way in (`relayServesApp`: available, or phones paired on it).

## Tests
`pnpm test`: **94 files, 1777 passed** (before: 94 files, 1750). `pnpm typecheck` clean, `pnpm i18n:check` 0 missing, `vite build` (to a scratch dir) OK.
New in `mobile-options.test.ts`: turning phone access on (new store ⇒ enable `lan:false` then `set_relay {on:true}`, with and without the enable
refusal; existing store, older daemon, unknown status ⇒ `enable {}` only; refused room throws its reason; enable failing with access off stops);
stale error hidden; hosted rows say restart below `HOSTED_API` and offer no pairing; pairing only when served; plain pairing hidden with LAN off; LAN
switch rules; hosted line/reason; key field rule; trust text (both variants, EN + VI, no "verified"/"guarantee", VI no "xác minh"/"đảm bảo");
approval choices for hosted exclude control; device choices for hosted exclude control; moving-relay warning (singular/plural, same address, no
count); summary names the hosted app; catalog search finds "hosted", "trust", "fingerprint"; every row's three lines in EN and VI (existing `it.each`
over `OPTION_IDS`, now with the two new rows).

## Departures from the spec
1. **Rows below `HOSTED_API` are shown with the restart text** (req. 9) rather than hidden (test list); the pairing button and hosted lines are hidden.
2. **Stale daemon error hidden**: after the new-setup flow the daemon keeps "Turn on the relay or Tailscale: nothing else reaches this computer" in
   `mobile.status.error` (autostart does not clear `last_error` on success). The page hides that exact text while `running` (`statusError`).
3. **Tailscale offer turns LAN back on** first: `set_tailscale` needs phone access running, which with LAN off and no relay it is not.
4. **LAN switch cannot be turned off** while neither relay nor Tailscale is on (the daemon would save `lan: false` and stop phone access).
5. **New room without a key**: "Make a new room" and the offer after unpairing need no kept key from a `HOSTED_API` daemon at a v2 relay.
6. **Second trust text** for a relay that is not Kelpie's (names whoever runs it; Kelpie's fingerprints do not describe it).
7. Wording: the row is *Hosted phone app*, badges/approval use "hosted app"; *Pair a phone* label of the plain row became *Pair a browser on this
   network*.

## Open questions
- Daemon: clear `last_error` when `autostart` succeeds (then `statusError` can go).
- Daemon texts new in phase 2 (`relay.hosted.reason`, registration refusals, LAN refusal) are not in `locales/vi/backend.json`, so VI shows them in
  English via `errorText`.
- Stale VI entries for the old `mobile-access` tradeoff (`mobile.json`) and old `mobile-relay` lines (`relay.json`) are now unused; outside this
  phase's files, left in place.
- The hosted reason "not checked yet" is matched as a literal (it is the daemon's tested text); a code field would be sturdier.
