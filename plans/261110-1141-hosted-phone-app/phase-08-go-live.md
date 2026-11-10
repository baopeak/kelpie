---
phase: 8
title: "Go-live (owner and maintainer)"
status: pending
priority: P1
effort: "0.5d + owner time"
dependencies: [1, 2, 3, 4, 5, 6, 7]
---

# Phase 8: Go-live

## Context
Everything here needs the owner: Cloudflare account and plan, GitHub environment settings, secrets, approvals, real phones. Agents prepare commands and checklists and never read or print secrets (`%USERPROFILE%\.kelpie-relay.env`, any registration key).

## Steps (in order)
1. **Decisions**: answer Q1–Q9 in [plan.md](plan.md); phases 2–7 assume the recommended answers and change where an answer differs.
2. **Cloudflare plan [Q5]**: if Workers Paid, upgrade and set usage notifications (requests, DO duration, rows written); confirm the Free/Paid limits on Cloudflare's pricing page and correct phase 1's cost table.
3. **Token and environments**: create the CI token (account-scoped, Workers Scripts: Edit, expiry); create environments `hosted-staging`, `hosted-production`, `relay-room-staging`, `relay-room-production` with the owner as required reviewer and tag rule `v*`; put `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` in each; confirm no repository-level Cloudflare secret exists.
4. **Staging secret**: owner sets `TICKET_SECRET` for `kelpie-link-relay-staging` once (a secret, not a code deploy).
5. **Release tag**: bump and tag as usual (`release.yml` builds the draft; `hosted-build` produces the bundle and hash).
6. **Room script v2**: `deploy-room` from the tag, staging then production (approve each); active phone sockets drop once and reconnect (deploy off-peak).
7. **Router + app**: approve `hosted-staging`; check the staging origin on a phone and with `verify_live.py`; approve `hosted-production`. Registration still needs the key (it is set), so nothing is open yet.
8. **Check from outside CI**: `python scripts/hosted/verify_live.py` ⇒ 0; maintainer `--scripts` with the token in the shell only ⇒ 0; `wrangler versions list` shows the CI message.
9. **Open registration**: owner deletes `REGISTRATION_KEY` from the production router (`OPEN_REGISTRATION = "1"` is in the deployed config). A smoke without a key makes and drops a room.
10. **Publish** the draft release (installers + `hosted-phone-manifest.json` + hash in notes).
11. **Hand access [Q6]**: apply the answer (keep the personal token for break-glass only, revoke it, or leave as is) and write it into `docs/relay-self-host.md` if it changed.
12. **Real phones (owner)**: iPhone (Safari, Home Screen install, Face ID lock, an alert) and Android (Chrome), Tailscale off on both ends: pair, read, type, lock, alert, background/foreground; an existing Tailscale-paired phone keeps working with no action; a new deploy while the app is open reloads into it.
13. **7-day watch**: Cloudflare analytics (requests, DO duration GB-s, rows written, room count) against the plan's limits; registration refusals by reason; adjust Q8 limits by config if needed (CI deploy).

## Validation
- Steps 8, 9, 12 recorded with results (pass/fail, devices, OS versions) in a report under `plans/reports/`.
- Release notes show the bundle hash; the live origin matches it.

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Cost or limit spike after opening | M×H | usage notifications; close by setting `REGISTRATION_KEY`; lower limits by config |
| Production deploy breaks hosted phones | M×H | staging check first; redeploy previous tag; Tailscale users unaffected |
| Room deploy drops sockets at a busy hour | H×L | off-peak; phones reconnect by themselves |
| Real-phone behaviour differs from Edge emulation (WebAuthn, push, SW on iOS) | M×M | step 12 before announcing; fixes go through a new release |

## Rollback
- Close registration: owner sets `REGISTRATION_KEY` again (no code change).
- App/router: run the pipeline on the previous tag (approve), or break-glass rollback if Q6 allows.
- Room script: `deploy-room` from the previous tag (rooms stored with `origins` fail closed for browser phones on v1 code; Tailscale-paired phones on a v1 room need their `origin`, which a new daemon re-sends as v1 when the header is gone).
- Desktop: users stay on or return to the previous Kelpie; hosted devices fail closed there.

## Success criteria
The plan's acceptance criteria, all checked; owner confirms real-phone results.
