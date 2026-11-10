---
phase: 4
title: "Desktop Settings → Mobile: hosted default, trust text"
status: done
priority: P1
effort: "2d"
dependencies: [2, 3]
---

# Phase 4: Desktop Settings → Mobile

## Context
- Page: `app/src/components/settings/mobile-settings.tsx` — phone access row (`:373`), Tailscale row and its "Pair a phone over Tailscale" (`:388-418`), relay row (`:419-423`), two-step guide (`:426-463`, Step 1 install QR from `tailscale().install_qr`, Step 2 `pairOverTailscale` `:239-251`), plain pairing (`:227-238`, `:464-480`), pair requests (`mobile.pair_request` handled in this file).
- Relay form/details: `app/src/components/settings/mobile-relay-settings.tsx` (`createRelayForm` `:31`, `RelayDetails` `:47`).
- Rows and their three lines: `app/src/lib/mobile-options.ts` (`OPTION_IDS` `:10`, Tailscale `:53`, relay `:57`, `mobileSummary` `:120`), tests `mobile-options.test.ts`; search: `app/src/lib/settings-catalog.ts:56-70`; VI: `app/src/locales/vi/settings.json`.
- Daemon contract from phase 2: `mobile.pair_start {hosted}`, `relay.hosted {available, origin, version, reason}`, `relay.hosted_devices`, device `page_kind`, `origin_kind` on pair requests, `mobile.enable {lan}`, registration error texts, `HOSTED_API`.

## Overview
Make "turn on phone access, then pair a phone" the hosted path for new users, keep every existing control for existing users, say plainly what the hosted app trusts, and stop offering what a hosted phone cannot have.

## Requirements
1. **New users** (`mobile.status.new_store`: phone access never on, no devices, relay never set): turning on *Phone access* = `mobile.enable {lan: false}`, then `mobile.set_relay {on: true}` on the default relay (one OS verification, no key), then the pairing guide opens. If room creation is refused (limits, network), say why and offer Tailscale mode. Existing users: the switch does exactly what it does today. [Q4]
2. **Pairing guide, generalized**: one two-step modal for both origins. *Pair a phone* (primary, when `relay.hosted.available`): Step 1 install QR = `https://<hosted host>/` (no secret, the daemon gives `relay.hosted.install_qr`), "Already installed: Step 2"; Step 2 = `mobile.pair_start {hosted: true}`. *Pair a phone over Tailscale* keeps its own entry and text. The plain "Pair a browser on this network" shows only while LAN listening is on.
3. **Approval of a hosted phone** (`origin_kind: hosted`): permissions offered are *Read* (default) and *Read and type*; *Type* carries one line: "Typing from the hosted app lets code served by Kelpie's relay type into your sessions while the phone is unlocked." *May take control* is never offered for hosted devices.
4. **Relay row**: no registration-key field unless the relay answered that it needs one; hosted line ("Phone app at <origin>, version X" / "This relay does not serve the phone app: pair phones over Tailscale"); "What the relay can see" adds "and serves the phone app" when hosted; changing the relay address with `hosted_devices > 0` warns "N phones using the app at <old> lose their way to this computer and must be paired again" before asking to verify.
5. **"What this trusts"** explainer under the hosted pairing row (EN + VI): "The phone app comes from Kelpie's relay at <host>, on Cloudflare. Whoever controls that Cloudflare account or Kelpie's release deploy could send your phone different code that reads your sessions and types into them while the app is unlocked. Each release publishes the app's fingerprint so anyone can check what the relay serves; nobody can check what one phone received. For a phone app served only by this computer, use Tailscale mode."
6. **Device list**: a badge per page kind (*hosted app*, *Tailscale*, *this computer's HTTPS*, *this network*); no control switch for hosted devices; the lock row says hosted phones always use the lock [Q1].
7. **Advanced**: *Pair browsers on this network* switch (`lan`), off for new hosted setups, on for existing ones.
8. **Summary** (`mobileSummary`) names the hosted app as a way in.
9. Rows keep the house pattern (*What it does* / *Trade-off* / *How safe*), are searchable, and are gated by `HOSTED_API` (an older daemon: rows say to restart it, as other gated rows do).

## Related code files
- Modify: `app/src/components/settings/mobile-settings.tsx`, `app/src/components/settings/mobile-relay-settings.tsx`, `app/src/lib/mobile-options.ts`, `app/src/lib/mobile-options.test.ts`, `app/src/lib/settings-catalog.ts`, `app/src/locales/vi/settings.json`.
- Not touched: `app/src/lib/types.ts`, `app/src/lib/attention.ts` (phase 2).

## Implementation steps
1. Option-state logic and texts in `mobile-options.ts` with tests (new-user path, hosted rows, gating).
2. Generalize the guide component; hosted pairing; approval permission choices by `origin_kind`.
3. Relay row changes and the moving-relay warning.
4. Device badges, control switch hidden for hosted, LAN switch.
5. VI strings, catalog entries; `cd app && pnpm typecheck && pnpm test && pnpm i18n:check`.

## Tests (vitest)
- New store: turning phone access on issues enable(lan false) then set_relay on; existing store: enable only, unchanged params.
- Hosted rows hidden below `HOSTED_API`; hosted pairing button only when available; plain pairing hidden with LAN off.
- Approval choices for `origin_kind: hosted` exclude control; device row for hosted has no control switch.
- Moving-relay warning when `hosted_devices > 0`.
- Every new row has its three lines in EN and VI; catalog search finds "hosted", "trust", "fingerprint".

## Validation
Unit tests above; the GUI path in phase 6 (`mobile.py` Settings checks, tauri-driver, isolated daemon).

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Existing users silently moved to the hosted default | M×H | new-user path keyed on daemon's "new store" flag only; test both |
| Trust text overclaims | M×M | reviewed against the plan's trust table; no word "verified" for in-app hash |
| Too many pairing entries confuse | M×L | one primary *Pair a phone*; Tailscale and plain under their own rows |

## Rollback
Revert the commit; the daemon keeps working with the old UI (new params are optional).

## Success criteria
Tests green; a reviewer reading Settings → Mobile can tell, without the docs, which way a phone reaches the computer and what the hosted app trusts.

## Completion note (2026-11-10)
Done on branch `phone/hosted-settings`; report: [reports/fullstack-261110-desktop-settings.md](reports/fullstack-261110-desktop-settings.md). `pnpm typecheck`, `pnpm test` (1777) and `pnpm i18n:check` green; GUI checks are phase 6's. Where the code differed from this spec:
- `types.ts` not edited after all: `MobileStatus`, `RelayStatus`, `MobileDevice` and the pending list extend phase 2's shapes (`Partial<…>`, since older daemons omit them).
- The hosted rows (*Hosted phone app*, *Pair browsers on this network*) are shown below `HOSTED_API` with the restart text, as req. 9 says; what is hidden there is the pairing button and the hosted lines.
- A new setup turns the relay on with `{on: true}` (the daemon's default relay); `mobile.enable {lan: false}` is expected to fail until the relay is on, so its error is passed over when phone access is on all the same, and the daemon's stale "nothing else reaches this computer" is hidden once phone access runs.
- "Offer Tailscale mode" after a refused room turns this network's listeners back on first: Tailscale mode needs phone access running.
- The LAN switch cannot be turned off while neither the relay nor Tailscale is on (the daemon would stop phone access).
- "Make a new room" and the offer after unpairing no longer need a kept key at a v2 relay from a `HOSTED_API` daemon.
- A second trust text for a relay other than Kelpie's; badge/approval wording uses "hosted app"; the row is "Hosted phone app".