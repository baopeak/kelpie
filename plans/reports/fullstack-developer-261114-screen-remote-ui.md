# Screen without Tailscale: desktop and phone UI (worktree D:\kelpie-rd-0, branch screen/remote-policy)

Checks run in `D:\kelpie-rd-0\app`: `pnpm typecheck`, `pnpm test` (105 files, 2010 tests) and `pnpm i18n:check` all pass. No Rust touched, no commit, no daemon or app started.

## Files changed (all under D:\kelpie-rd-0\app unless noted)

Desktop
- `src/lib/mobile-options.ts`: option `mobile-screen-remote`; `screenRemoteShown`, `screenDoubtedNote`, `screenMethodRows`, `screenUsageLine`, `permissionChoices(d, api, screenRemote)`, `permissionLabel`; relay pairing line says the screen is allowed once the switch is on.
- `src/lib/mobile-options.test.ts`: new block for the row, doubted note, ways, usage, search, picker; test API now `SCREEN_REMOTE_API`, default status has `screen`.
- `src/lib/types.ts` (`MobileScreenStatus`), `src/lib/ipc.ts` (`mobile.set_screen` in `mobileSecure`), `src/lib/settings-catalog.ts` (entries `mobile-screen-remote`, `mobile-screen-ways`, both `api: SCREEN_REMOTE_API`).
- `src/components/settings/mobile-settings.tsx`: the row, "Ways the screen may connect" checkboxes plus "x MB of y MB this month" under Advanced, permission select label "May take control (screen only)", `setScreen` through `mobileSecure`, refresh via the existing `mobile.updated` path.
- `src/locales/vi/mobile.json`, `src/locales/vi/phone.json`: Vietnamese for every new string.

Phone (`phone/src`)
- New `screen-paths.ts` (+ `screen-paths.test.ts`): path rows and reasons, relay naming, hosted Tailscale override, preselected way, `needsProof`, kept-choice key, `viaText`, `startRefusal`.
- `conn-path.ts` (+ test): `screenReasonText` for every `device.screen` and path code, `knownScreenReason`, `screenReady`, `screenOffered`; `screenBlock`/`screenWhy` use `device.screen` first.
- `lock.ts` (+ test): `screenChallenge(target, expiry)` = `recheckChallenge("screen:"+target, "relay", expiry)`, `LockModel.screenProof(target)`.
- `screen.ts` (+ test): ack `via/paths/relay`, `ScreenPort.start(target, via, recheck?)` plus `route/hosted/remembered/remember/proof`, session state `paths/relay/via/usedVia`, `choosePath`, Face ID before `sc_start` when the way needs it, `detailsText(..., via)`.
- `link.ts`: `DeviceInfo.screen`, ack mapping of `via/paths/relay`, `scStart(target, via?, recheck?)`.
- `prefs.ts` (+ test): `rememberedVia`/`rememberVia` (own `store`, already try/catch; key `kelpie.screen.via.<pairing deviceId>`).
- `phone-state.ts`: `canSeeScreen`/`screenWhyNow` go by hello `device.screen` from API 41; port wiring (proof, remembered way, route, hosted).
- `screen-view.tsx`: "Connect through" section in the picker, Face ID note, details line shows the way used. `phone.css`: four small rules.

## Decisions
- Entry visibility: from API 41 the daemon's `device.screen` decides for every phone (ok or needs_face_id shows the entry, any other known code hides it and Settings shows the reason text). Unknown code or older daemon: old permission rule, unchanged.
- "Connect through" is a section at the top of the picker (radio rows), not a separate sheet; choice is saved on change and preselected next time if still usable.
- Face ID is asked when the way is `relay`, or `auto` while the phone's live route is the relay; it runs straight after the tap on "See it" so the browser keeps the user gesture. A daemon that sends no `paths` gets the old flow (no via, no Face ID).
- `p2p` and `relay_turn` checkboxes are rendered, wired to `mobile.set_screen`, and say "Not available yet".
- Hosted phone control label and the control option only while `screen.remote` is on and `api >= 41`.

## Not done / needs attention
- **Blocking for the real app:** `app/src-tauri/src/user_presence.rs` `SECURE_CALLS` (and `APP_ONLY` + its pinned test) do not list `mobile.set_screen`, so `mobile_secure_call` will refuse it ("is not a phone-access decision"). It is Rust, which I was told not to touch; one-line addition needed.
- No real-app or phone check (no dev server by instruction); only vitest, tsc and i18n.
- Docs (`user-guide`, architecture) not touched; another agent has them modified in this worktree.

Unresolved questions: none.
