# Hosted phone app: docs report

Date: 2026-11-10. Worktree `D:\kelpie-hosted-p7`, branch `docs/hosted-phone-guide`. Docs only; nothing run, deployed or sent to Cloudflare.
Not committed: the shell tool failed on this machine (`ENAMETOOLONG` on every call, with and without the sandbox), so the files are edited
and unstaged. See "Commit" below.

## Surfaces changed
- `README.md` (privacy paragraph): relay is on in a new setup, serves the phone app, what it sees, the trust limit, Tailscale as the alternative.
- `docs/user-guide.md` "Phone access": new first subsection (pairing with the Kelpie app, no Tailscale), what a hosted phone may do, relay always
  in the path, moving the relay, *What the hosted app trusts*, *Checking what the relay serves*, the LAN switch, options table rows, relay
  paragraph rewritten, Tailscale presented as the option for an app served only by the computer and for screen/takeover, lock, takeover, alerts lines.
- `docs/relay-self-host.md`: registration modes, limit variables, workerd serves no page, own-Cloudflare deployment and trust, Kelpie's own relay
  (CI only, environments, release order, staging, room dispatch, rollback, break-glass, who deployed what), `verify_live.py` flags.
- `docs/link-protocol.md`: `/v1/rooms` modes with 4xx/5xx reasons, static files outside `/v1` (Cloudflare only), `origins`, `o` in `open`,
  `x-kelpie-relay-version`, storage, hosted QR endpoint. Vectors paragraph untouched.
- `docs/architecture.md`: new "Hosted phone app" sub-bullet under Relay (origin, two builds, finding the app, relay versions, device page and code
  binding, ceiling and lock path, token life, LAN optional, trust boundary, amended earlier decision), plus edits to the relay, room, phones, slots,
  permissions, phone-client and process-diagram lines.
- Plan: `phase-07-docs.md` status and its line in `plan.md` set to done.

## Claims and their evidence (all checked by reading source; grep list)
| Claim in docs | Evidence |
| --- | --- |
| `OPEN_REGISTRATION`, `REGISTRATION_KEY` wins, 503 "relay not configured" | `relay/src/worker.ts` `register()`; `relay/workerd/config.capnp` |
| `ROOMS_PER_NETWORK_DAY` 20, `ROOMS_PER_DAY` 1000 | `worker.ts` constants; `wrangler.toml` `[vars]` |
| rate 5/min per network, `ROOM_RATE` | `wrangler.toml` `[[ratelimits]]` |
| reasons `rate`, `network`, `full`, `attempts`; 429/503/401/403/413/400 | `worker.ts` `plain(...)` calls and constants |
| 10 wrong keys/day; 30 refused sockets/10 min; body 1,024 B; key at most 256 chars | `worker.ts` `registrations`, `ADMISSION_REFUSALS`, `MAX_REGISTRATION_BODY`, bearer regex |
| `x-kelpie-relay-version: 2` on the desktop 101; `origins` (4), `origin`, `o` in `open` | `relay/src/room.ts`; `relay.rs` `VERSION_HEADER`, `origins_message` |
| usage flush once a minute or each MB; registrar per day/network | `room.ts` `USAGE_FLUSH_MS`; `registrar.ts` |
| static files outside `/v1`, 404 page, `run_worker_first` | `relay/wrangler.toml` `[assets]` |
| workerd relay serves no page | `config.capnp`, `Dockerfile` comments |
| Settings labels (*Mobile access*, *Hosted phone app*, *Pair a phone*, *Pair browsers on this network*, *Pair a browser on this network*, *Reach this computer through the relay*, *Relay address*, *Registration key*, *What this trusts*, *Step 1: Install*, *Already installed: Step 2*, *Step 2: Pair*, *Allow reading*, *Allow reading and typing*, *It pairs on: hosted app*, *Remove phones not seen for*, *Never remove*) | `settings-catalog.ts`, `mobile-settings.tsx`, `mobile-relay-settings.tsx`, `mobile-options.ts` |
| phone: *Scan the code*, *Continue in this browser* | `app/phone/src/install-guide.tsx` |
| refusal text for control; hosted ceiling `write`; `PasskeyAlways` | `mobile.rs` `record_ceiling`, `ceiling_refusal`; `mobile/lock.rs` |
| token life: dormant days, min 30, max 365, never = 365 | `relay.rs` `token_life` |
| probe: `/version.json`, `app` = `kelpie-phone`, `api_min`, 10 s, 4 KiB, 10 min | `relay.rs` `probe`, `PROBE_*` |
| `code_fits`, `relay_page` binding | `mobile/link.rs` |
| LAN-off message; `lan`, `new_store` in status | `mobile.rs` `start`, status; `mobile-options.ts` |
| `HOSTED_API` 34 | `crates/kelpie-proto/src/lib.rs`, `app/src/lib/attention.ts` |
| `verify_live.py` flags and exit codes; `CANNOT_PROVE` meaning | `scripts/hosted/verify_live.py` argparse and docstring |
| workflow names, environments, dispatch inputs, room-code file list, message formats, owner setup | `release.yml`, `relay.yml`, phase 5 report |
| e2e-backed behaviour (one computer per app, alerts link, replace, hosted lock with the switch off) | phase 6 report (Edge with a virtual authenticator, not a real iPhone; the docs do not claim iOS testing) |

## Validation run
- Stale-statement grep ("serves no page", "registration key", "Tailscale address", "never from CI", "deployed by hand", "never serves", "only from this
  computer", "Only phones paired over Tailscale") across `docs/` and `README.md`: what remains is true. The remaining "serves no page" is the
  workerd statement; "registration key" appears only as "a relay that has one"; "Tailscale address" is the Telegram link, notifications and the Tailscale-installed
  app's origin.
- Grep for plan, decision and finding codes (`D5`, `Q1`..`Q9`, `phase N`, `H1`..`H9`) in the three prose docs: none.
- Links: `relay-self-host.md#checking-what-the-relay-serves` matches the new heading; architecture cross-references ("Hosted phone app", "Phone link",
  "Relay") exist; `link-protocol.md` "Making a room" referenced from the HTTP table exists.
- README and user guide say the same about what the relay sees (connection, addresses, when, how much, room number) and serves (the phone app on Kelpie's relay).

## Docs impact
No new files. The user-guide section is long already; the new text sits in its first subsection and the trust and verify paragraphs.

## Commit
Not made. The shell tool is unusable here. To commit, from the worktree stage by path:
`README.md docs/user-guide.md docs/relay-self-host.md docs/link-protocol.md docs/architecture.md plans/261110-1141-hosted-phone-app/phase-07-docs.md plans/261110-1141-hosted-phone-app/plan.md plans/261110-1141-hosted-phone-app/reports/docs-manager-261110-hosted-docs.md`.

## Unresolved
1. `verify_live.py --scripts` was never run against Cloudflare (phase 5 report: API path unverified); the docs describe it as the maintainers' option without a test claim.
2. `wrangler versions rollback` restoring the app's files is unconfirmed; the relay doc says so.
3. A hosted phone whose relay token expired shows endless "Reconnecting" instead of a pair-again state (phase 6 open question); the guide says it must be paired again after a year unused, which is true, but the app does not say so.
4. Cloudflare's edge `Cache-Control` handling vs `_headers` is unverified until the first staging run (phase 5); not in the docs.
5. The `relay.rs` module header comment still says "30 days" for every token (code comment, outside my files).
