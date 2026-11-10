---
phase: 5
title: "CI deploy, approval, published hash, live verification"
status: pending
priority: P1
effort: "2d"
dependencies: [1, 3]
---

# Phase 5: CI deploy and verification

## Context
- `.github/workflows/release.yml`: on tag `v*`, builds installers (`build` `:16-122`), checks the tag against the manifests (`:52-61`), `publish` makes a **draft** release with notes from the annotated tag (`:124-171`); it downloads every artifact with `merge-multiple` and copies all files into the release (`:133-143`).
- `.github/workflows/relay.yml`: checks only; says deploys are by hand (`:3-4`).
- `docs/relay-self-host.md:78-97`: hand deploy with `wrangler`, room script first.
- `relay/package.json`: `deploy:room`, `deploy:router` scripts.
- Owner decision 3 (2026-11-10): hosted app deployed only by CI from a release tag behind a GitHub environment approval; each release publishes the deployed bundle's hash.

## Overview
Add the hosted pipeline to the release workflow, move Kelpie's relay deploys to CI, publish the bundle's manifest and hash with every release, and give anyone a script to check what the origin serves.

## Decision: relay deploys move to CI, room script on its own trigger
- **Router (and assets)**: static assets are part of the router Worker's version, so deploying the app *is* deploying the router: it moves to CI with the app, every release.
- **Room script**: also CI, from a release tag, behind approval, but by `workflow_dispatch` and only when room code changed, because a room deploy drops every active room's sockets (phase 4a measured). The router binds the room script's classes, so a router needing a new class fails to deploy until the room script has it (safe order).
- Hand deploys of code or assets to Kelpie's account stop. Break-glass is the owner's call [Q6].

## Requirements
1. **`release.yml` jobs** (all `runs-on: ubuntu-22.04`; deploy jobs pin third-party actions by commit SHA; `concurrency: hosted-deploy`):
   - `hosted-build` (no secrets, `contents: read`): checkout tag; `cd app && pnpm install --frozen-lockfile`; build hosted twice into clean dirs with `KELPIE_HOSTED_ORIGIN=https://kelpie-link-relay.baopeak.workers.dev`; `bundle_manifest.py` on both with `--tag`/`--commit`; fail unless the two `bundle_sha256` match; upload artifact `hosted-phone` (bundle + manifest).
   - `publish` (existing; needs `build`, `hosted-build`): download installer artifacts by pattern `bundle-*` only (the hosted bundle's files must not become release assets); add `hosted-phone-manifest.json` and `hosted-phone-<version>.tar.gz` (the bundle, for independent comparison); append to `notes.md`: the origin, `bundle sha256`, and the check command.
   - `hosted-staging` (needs `hosted-build`; environment `hosted-staging`): checkout tag; `cd relay && pnpm install --frozen-lockfile`; artifact → `relay/public`; recompute the hash and require it equals the artifact manifest's; `wrangler deploy -c wrangler.toml --env staging --message "<tag> <bundle sha256>"`; `verify_live.py` against the staging origin; `PAGE=1 pnpm smoke` against staging.
   - `hosted-production` (needs `publish`, `hosted-staging`; environment `hosted-production`): same against production (`--message` as above), `verify_live.py`, smoke.
2. **Environments** (owner sets them, phase 8): `hosted-staging`, `hosted-production`, `relay-room-staging`, `relay-room-production`; each requires the owner's review and allows only tags `v*`. Staging needs review too: one Cloudflare token cannot be limited to one script, so an unreviewed staging job holding it could deploy production from a tag whose workflow was edited. `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` exist only as environment secrets, never repository secrets.
3. **Token**: account-scoped, Workers Scripts: Edit (plus whatever `wrangler deploy` with assets proves to need, found in staging and written down), an expiry date, used by no other system.
4. **`relay.yml`**:
   - Checks (PR, master): existing typecheck/tests/workerd smoke, now run in open and key mode; a page job: `cd app && pnpm build:phone:hosted` with a test origin, copy into `relay/public`, `wrangler dev --local`, `PAGE=1 pnpm smoke` (headers from `_headers`, 404 page, `/v1/health`).
   - `deploy-room` (`workflow_dispatch`, inputs `tag` matching `v*`, `target` staging|production; environment `relay-room-<target>`): checkout the tag, `pnpm deploy:room` (`--env staging` for staging) with `--message "<tag>"`, then smoke.
   - Header comment updated: no hand deploys of Kelpie's relay.
5. **Staging config**: `[env.staging]` in `relay/wrangler.toml` (name `kelpie-link-relay-staging`, `ROOM` and `REGISTRAR` bound to `kelpie-link-relay-room-staging`, same assets and vars) and in `relay/wrangler.room.toml` (name `kelpie-link-relay-room-staging`). Staging has its own `TICKET_SECRET` (owner, once).
6. **`scripts/hosted/verify_live.py`** (stdlib; imports `bundle_manifest.py`):
   - `--origin` (default production), `--tag` (default: from live `/version.json`), `--manifest PATH` (default: download `hosted-phone-manifest.json` from the GitHub release of that tag).
   - Fetches every manifest path with `Cache-Control: no-cache`, compares SHA-256; checks `version.json` (tag, commit, `bundle_sha256`); checks response headers per path against the manifest's `_headers` rules; checks the service worker's precache list and the scripts `index.html` loads are all in the manifest.
   - `--rebuild`: checks out the tag in a temporary directory, builds the hosted bundle (pnpm, Node 24, Linux/WSL) and compares manifests.
   - `--scripts` (maintainer): with `CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ACCOUNT_ID` from the environment (never printed), downloads the live router and room script content through the Cloudflare API and compares them with `wrangler deploy --dry-run --outdir` built from the tag — the check the owner did by hand. [API path for module Workers: verify in staging]
   - Prints paths and hashes only; exit 0 on match, 1 on any difference, 2 on usage/network errors. Unit-tested against a local fixture server, including a one-byte tamper and a header change.
   - What it cannot prove is printed with every run: it saw what the origin served to this machine at this moment.
7. **Release order** (runbook text for phase 7): room code changed? → `deploy-room` staging, then production, from the tag → push nothing else → approve `hosted-staging` → check → approve `hosted-production` → `verify_live.py` from a machine outside CI → publish the draft release. A draft whose production deploy failed is not published.
8. **Who deployed what**: every CI version carries `--message "<tag> <hash>"`; `wrangler versions list` showing a version without such a message means a hand deploy — investigate.

## Related code files
- Modify: `.github/workflows/release.yml`, `.github/workflows/relay.yml`, `relay/wrangler.toml` (`[env.staging]`), `relay/wrangler.room.toml` (`[env.staging]`).
- Create: `scripts/hosted/verify_live.py`, `scripts/hosted/test_verify_live.py` (fixture server).
- Docs (commands only; prose in phase 7): none here.

## Implementation steps
1. Staging env blocks in both wrangler files; dry-run them locally (`wrangler deploy --dry-run --env staging`), nothing sent.
2. `verify_live.py` + tests.
3. `release.yml` jobs; fix the `publish` download pattern; notes append.
4. `relay.yml` page job, two smoke modes, `deploy-room`.
5. `actionlint` (if available) on both workflows; a fork/branch dry run of `hosted-build` only (no secrets). First real deploy is phase 8.

## Tests
- `verify_live.py`: match ⇒ 0; changed byte, missing file, extra precache entry, changed CSP ⇒ 1; network error ⇒ 2.
- Workflow review checklist: no secret outside environment jobs; `hosted-production` needs `publish` and `hosted-staging`; installer artifacts and hosted files do not mix; deploy jobs pin actions by SHA; `concurrency` set.

## Validation
`python -I scripts/hosted/test_verify_live.py`; `hosted-build` run on a test tag in a fork or with deploy jobs disabled shows equal hashes; phase 8 runs the real pipeline.

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Token leaks through logs or a modified workflow | L×H | environment-only secrets, review on every environment, tag rule, wrangler output not echoed |
| Notes claim a hash that never went live | M×M | runbook: publish only after production verify; verify job status visible on the release run |
| `wrangler versions rollback` does not restore assets [UNVERIFIED] | M×M | rollback = redeploy the previous tag's bundle through the pipeline until staging proves it |
| Hosted files leak into release assets | M×L | `pattern: bundle-*` download; checked in the first run |
| Staging and production drift | L×M | same artifact, same commands, only `--env` differs |

## Rollback
Disable the hosted jobs (or delete the environments' secrets) to stop deploys; production stays at its last version. Revert the commit to return to checks-only CI. Revert a bad production deploy by running the pipeline on the previous tag (re-approve), or by the break-glass rollback if Q6 allows it.

## Success criteria
- Production cannot be deployed without the owner's approval in `hosted-production` (`gh api repos/<owner>/<repo>/environments/hosted-production` shows the reviewer and the `v*` rule).
- The first release with the pipeline has `hosted-phone-manifest.json` and the hash in its notes, and `verify_live.py` exits 0 against production.
