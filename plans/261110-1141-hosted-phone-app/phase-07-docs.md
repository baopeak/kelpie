---
phase: 7
title: "Docs"
status: pending
priority: P1
effort: "1d"
dependencies: [1, 2, 3, 4, 5, 6]
---

# Phase 7: Docs

## Context
Docs that state the old contract, re-grepped on master:
- `docs/user-guide.md` "Phone access" (`:1071-1253`): relay needs a registration key (`:1157`), only Tailscale-paired phones use the relay (`:1159`), "installing the app … still needs Tailscale … the relay serves no page" and "the relay also needs Tailscale mode" (`:1163`), lock text names Tailscale (`:1169-1173`).
- `docs/relay-self-host.md`: secrets incl. `REGISTRATION_KEY` (`:14-17`), using it (`:72-76`), "Kelpie's own relay … deployed by hand … never from CI" (`:78-97`), limits (`:99-102`).
- `docs/link-protocol.md`: rooms need the key (`:50`), "the relay never serves a page" (`:53`), single origin (`:65`, `:83`), storage (`:115`).
- `docs/architecture.md`: phone link and relay (`:559-660`, room made with the operator's key `:653`), takeover/screen never over the relay (`:729`, `:803`), phone client (`:946`).
- `README.md:236`: "it is not a public cloud service", the relay "serves no page … used only by phones paired over Tailscale".

## Requirements
- **User guide** (smallest owning surface: the "Phone access" section):
  - New first subsection *Pair a phone with the Kelpie app (no Tailscale)*: turning phone access on, the two steps, approving, what a hosted phone may do (read, type; never control or screen), the lock (always on for these phones [Q1]), one computer per app [Q2], the relay always in the path (no direct route, even on the same Wi-Fi), alerts, moving relay cuts these phones off.
  - *What the hosted app trusts* paragraph with the exact limits from the plan's trust table, and *Checking what the relay serves* (`python scripts/hosted/verify_live.py`, what it can and cannot show).
  - Relay paragraph rewritten: no key for Kelpie's relay (a self-hosted relay may need one); origins instead of "only from this computer's Tailscale address"; the two Tailscale-only sentences of `:1163` removed or reworded; what the relay sees now includes "serves the phone app".
  - Tailscale mode presented as the option for a page served only by your computer and for screen/takeover; existing text kept.
  - *Pair browsers on this network* switch (LAN) [Q4].
- **relay-self-host.md**: registration modes (`OPEN_REGISTRATION`, `REGISTRATION_KEY` wins), new limits and their vars, a self-hosted workerd relay serves no page (phones use Tailscale), deploying router + assets to your own Cloudflare account and becoming your phones' trust root; *Kelpie's own relay* rewritten: CI only, environments, release order, staging, verification, rollback, break-glass per Q6, room deploys by dispatch.
- **link-protocol.md**: `/v1/rooms` modes and 429/503 reasons, static page on non-`/v1` paths (Cloudflare only), `origins`, `o` in `open`, `x-kelpie-relay-version`, storage of `origins`; vectors unchanged unless tests changed them.
- **architecture.md**: hosted origin and the two phone builds, device `page`/`page_kind` and the code binding, relay v2 negotiation, hosted ceiling and lock path, LAN listener optional, the trust boundary (who can change hosted code), D5 amended 2026-11-10.
- **README.md:236**: privacy paragraph: phone access still off by default; with it on, new setups use Kelpie's relay on Cloudflare, which also serves the phone app; what that means; Tailscale mode for an app served only by your computer. Remove "serves no page" and "used only by phones paired over Tailscale".
- No plan IDs, phase numbers or finding codes in docs prose.

## Related code files
- Modify: `docs/user-guide.md`, `docs/relay-self-host.md`, `docs/link-protocol.md`, `docs/architecture.md`, `README.md`.

## Implementation steps
1. Read each section; write; then check every claim against code, tests and the e2e results of phase 6 (status names, error texts, limits, header names, script flags).
2. Check links and anchors (`#phone-access`, the relay-self-host link).

## Validation
- Every number and name in the docs greps to the code (limits in `relay/src/*.ts` and `wrangler*.toml`, error texts in `relay.rs`, flags in `verify_live.py`).
- `README.md` and the user guide agree on what the relay sees and serves.

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Docs promise protection the browser cannot give | M×H | trust table copied, reviewed against phase 5's "cannot prove" line |
| Old Tailscale-only statements left behind | M×M | grep for "serves no page", "registration key", "Tailscale address" across `docs/` and `README.md` |

## Rollback
Revert with the code it describes.

## Success criteria
A new user can follow the guide to a paired phone without Tailscale; a security reader finds what the hosted app trusts and how to check it in one place.
