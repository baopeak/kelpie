# CI deploy, approval, published hash, live verification: implementation report

Date: 2026-11-10. Branch `ci/hosted-deploy` (worktree `D:\kelpie-hosted-p5`, stacked on `phone/hosted-settings`), not pushed. Nothing was
deployed or dispatched, nothing reached Cloudflare, no login, no token used.

## What changed
- **`relay/wrangler.toml`, `relay/wrangler.room.toml`**: `[env.staging]` blocks. The router is `kelpie-link-relay-staging` (workers.dev on,
  same assets and vars, its own `ROOM_RATE` namespace `1002`) with `ROOM`/`REGISTRAR` bound to `kelpie-link-relay-room-staging`. The room
  script is `kelpie-link-relay-room-staging`. Vars and bindings are repeated because wrangler does not inherit them; `migrations` and
  `assets` are inherited (checked in wrangler 4.156's config validator and in the dry run).
- **`scripts/hosted/verify_live.py`** (stdlib, imports `bundle_manifest.py`):
  - **Options**: `--env production|staging` (origin and script names), `--origin`, `--tag` (default: live `/version.json`), `--manifest`
    (default: the tag's GitHub release asset).
  - **Live checks**:
    - Every manifest path and `/` are fetched with `Cache-Control: no-cache`; SHA-256 and size are compared. `_headers` is not fetched,
      because wrangler never uploads it.
    - Headers are checked per final path against the manifest's `_headers` rules: splats, placeholders, `! Name`, and values merged.
    - `/version.json` is checked: app, tag, commit, `api_min`, `bundle_sha256`.
    - The service worker's precache list and the scripts/stylesheets `index.html` loads must all be in the manifest.
    - The manifest is checked against itself: the hash covers the file list, and the `_headers` entry covers the headers text.
    - Redirects are followed only within the origin.
  - **`--rebuild`**: clones the tag (`core.autocrlf=false`), runs `pnpm install` and `pnpm build:phone:hosted`, and compares manifests.
  - **`--scripts`**: reads the token and account ID from the environment only. It runs `wrangler deploy --dry-run --outdir` for both
    scripts from the tag clone, fetches `GET /accounts/{id}/workers/scripts/{name}` (multipart split into modules) and compares the
    modules.
  - **`--bundle DIR`**: checks a local folder against the manifest, with no network (CI uses this before a deploy).
  - **Safety**: child builds never receive `CLOUDFLARE_*`. Only paths, hashes and header hashes are printed; server-chosen text is
    escaped. The "cannot prove" line is printed on every live run.
  - **Exit codes**: 0 match, 1 difference, 2 usage/network error (1 if a difference was already found).
- **`release.yml`**:
  - **Daemon binary check**: also fails if `KELPIE_RELAY_TEST_URL` or `KELPIE_RELAY_TEST_CA` appears in a release `kelpied`.
  - **`hosted-build`** (`contents: read`, no secrets):
    - Checks that `app/package.json` matches the tag.
    - Builds twice from clean folders with the production origin and runs `bundle_manifest.py --tag/--commit` on each.
    - `cmp`s the two manifests, outputs `bundle_sha256`, and uploads `hosted-phone` (bundle + manifest).
  - **`publish`**:
    - Downloads installers by `pattern: bundle-*` only, and `hosted-phone` separately into `hosted/`.
    - After `latest.json` is written (so in-app update notes stay the tag's own), it re-checks the bundle against its manifest and the
      build's hash.
    - Adds `hosted-phone-manifest.json` and a reproducible `hosted-phone-<v>.tar.gz`.
    - Appends to `notes.md`: the origin, the hash and the check command.
  - **`hosted-staging` and `hosted-production`**:
    - Run in environments of the same names, with `concurrency: hosted-deploy` and `cancel-in-progress: false`.
    - Before deploying, they check the bundle with `verify_live --bundle` and the hash against `hosted-build`'s output.
    - The deploy is `wrangler deploy -c wrangler.toml --env=staging|--env= --message "<tag> <sha256>"`. Secrets are only in the deploy
      step's `env`.
    - After the deploy, `verify_live.py` retries up to 6 times, 10 s apart (edge propagation), then `PAGE=1 pnpm smoke` runs.
    - Production `needs: [hosted-build, publish, hosted-staging]`.
- **`relay.yml`**:
  - **Header**: rewritten. There are no hand deploys of Kelpie's relay.
  - **Triggers**: `app/phone/**`, `app/vite.phone.config.ts` and `scripts/hosted/**` were added to the paths. `permissions: contents:
    read`.
  - **`test`**: the workerd smoke now runs in open mode, then in key mode.
  - **`page`** (new):
    - Runs both Python test files.
    - `pnpm build:phone:hosted` with a test origin, then `bundle_manifest.py`.
    - `wrangler dev --local` (both configs, throwaway `.dev.vars`), then `PAGE=1 pnpm smoke` and `verify_live.py` against it.
  - **`deploy-room`** (`workflow_dispatch`: `tag`, `target` staging|production):
    - Environment `relay-room-<target>`, the same concurrency group.
    - Refuses unless run from a `refs/tags/v*` ref equal to the `tag` input.
    - `pnpm deploy:room --env=staging|--env= --message "<tag> room <sha>"`, then `pnpm smoke`.

Third-party actions in deploy jobs are pinned (resolved with `git ls-remote` against each action's `v4` tag on 2026-11-10):

| Action | SHA | Version |
| --- | --- | --- |
| `actions/checkout` | `11d5960a326750d5838078e36cf38b85af677262` | v4.4.0 |
| `pnpm/action-setup` | `b906affcce14559ad1aafd4ab0e942779e9f58b1` | v4.3.0 (what `v4` points at; v4.4.0 exists) |
| `actions/setup-node` | `49933ea5288caeca8642d1e84afbd3f7d6820020` | v4.4.0 |
| `actions/upload-artifact` | `ea165f8d65b6e75b540449e92b4886f43607fa02` | v4.6.2 |
| `actions/download-artifact` | `d3f86a106a0bac45b974a628896c90dbdf5c8093` | v4.3.0 |

## Tests and checks
- **Unit tests**:
  - `python -I scripts/hosted/test_verify_live.py`: **29 passed**. The fixture origin on 127.0.0.1 behaves like Cloudflare (`/index.html`
    → 307 `/`, `_headers` not served) and its headers are written out by hand.
  - Exit code cases:
    - **0**: a match, and a tag read from the live `version.json`.
    - **1**: a changed byte, a missing file, an extra precache entry, a changed CSP, a changed `Cache-Control`, a `version.json` with
      another hash, a manifest for another tag, an inconsistent manifest, an off-origin redirect, an off-manifest or off-origin
      script, no precache list.
    - **2**: nothing listening, or a bad tag.
  - Also covered: the `_headers` rules, the precache regex, `--bundle` (match, changed byte, extra file, other tag, no manifest),
    `--rebuild` and `--scripts` with fake tools (see below).
  - `python -I scripts/hosted/test_bundle_manifest.py`: 10 passed, unchanged.
- **Real bundle**: `pnpm build:phone:hosted` on this branch gave `bundle_sha256 5263a297…f46b`, the same as phase 3's two builds.
  `verify_live.py --origin http://127.0.0.1:8791` against `wrangler dev --local` serving it gave **exit 0**: all 15 files, headers from
  the real `_headers` handling, precache 9 paths, the index loads. A one-byte change to `public/icon.svg` gave **exit 1**. The server
  was stopped by PID.
- **Wrangler dry runs** (`WRANGLER_SEND_METRICS=false`, no token in the environment, empty `public/`): `--dry-run --outdir` succeeded for
  both files with `--env staging` and with the top level. Staging shows the `-staging` bindings. `--env=` (empty) selects the top level
  without the "multiple environments" warning; passing it through `pnpm deploy:room` works. No login prompt. The only network contact
  was wrangler's npm update check ("update available 4.159.0").
- **workerd smoke, both modes**: the test job's script was run locally with the Windows workerd binary substituted. Open mode passed
  15/15 and key mode 16/16; no workerd was left running.
- **Workflow validation**: `actionlint` is not installed. Instead:
  - PyYAML 6.0.3 parsed both files.
  - A scratch script listed each job's environment, needs, concurrency, secrets, action pins, the steps holding `CLOUDFLARE_*`, and
    `set -x`.
  - Every `run:` script passed `bash -n` (Git Bash).
  - The publish hosted step ran in a scratch folder: correct notes and a correct tarball, and it fails on a wrong hash.
  - The deploy-room guard passed on a tag and failed on a branch and on a mismatched input.
- **Not run**:
  - Any real deploy.
  - `--scripts` against Cloudflare: the API path and multipart shape for module Workers are **unverified**.
  - A real `--rebuild`: only the clone/install/build/compare sequence with fake tools.
  - The fork dry run of `hosted-build` (no push allowed). Do it before phase 8: push a test tag to a fork, or run with the deploy
    jobs disabled.

## Workflow review checklist (checked against the YAML)
- [x] **Secrets only in environment jobs.** `CLOUDFLARE_API_TOKEN`/`CLOUDFLARE_ACCOUNT_ID` appear only in `hosted-staging`,
  `hosted-production` and `deploy-room`, all with `environment:`, and only in the deploy step's `env`. Install, verify and smoke steps
  never see them. `build` keeps the Tauri secrets as before.
- [x] **`hosted-production` ordering.** It needs `publish` and `hosted-staging` (and `hosted-build`, for its output).
- [x] **Installer and hosted files kept apart.** `publish` downloads `pattern: bundle-*` into `downloaded/`, which matches the
  `bundle-${{ matrix.os }}` names, and `hosted-phone` into `hosted/`. Only the manifest and tarball are copied into `dist/`.
- [x] **Deploy jobs pin actions by SHA.** This holds for `hosted-build`, `hosted-staging`, `hosted-production` and `deploy-room`. The
  existing `build` and `publish`, and the check jobs, keep `@v4`.
- [x] **Concurrency.** `concurrency: {group: hosted-deploy, cancel-in-progress: false}` is set on all three deploy jobs, across both
  workflows.
- [x] **No `set -x`.** Deploy steps check that the secrets are present without printing them. Wrangler does not print the token, and
  GitHub masks environment secrets. `persist-credentials: false` is set on every deploy checkout.

## Owner setup (requirements 2–3, before the first tag with this pipeline)
1. **Environments.** Create `hosted-staging`, `hosted-production`, `relay-room-staging` and `relay-room-production`. For each:
   - Required reviewer: the owner. Leave "prevent self-review" off while the owner is the only reviewer.
   - Deployment branches and tags: *Selected*, with tag rule `v*` and no branches.
2. **Secrets.** Add `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` as **environment** secrets in all four environments. There must
   be no repository or organization secret with these names. Check with `gh secret list` and
   `gh api repos/baopeak/kelpie/environments/hosted-production`.
3. **Token.** Account-scoped, Workers Scripts: Edit, with an expiry, used nowhere else. The first staging deploy reveals any extra
   permission that a deploy with assets needs; write it down then.
4. **Staging `TICKET_SECRET`.** Set it once for the staging router: `wrangler secret put TICKET_SECRET -c wrangler.toml --env staging`
   (the router alone reads it). This is a break-glass-style use of the personal token, allowed under 6a.
5. **First run order.** The staging router binds `kelpie-link-relay-room-staging`, so run `deploy-room` with target staging from that
   tag first. Production's room script needs v2 too (relay-v2 report).
6. **`--scripts` token.** Maintainers running `--scripts` should use a separate read-only token (Workers Scripts: Read), not the deploy
   token.

## Release runbook (requirement 7; text for phase 7)
1. **Room code.** If room code changed since the last release (`relay/src/room.ts`, `registrar.ts`, `entry-room.ts`,
   `wrangler.room.toml`):
   - `gh workflow run relay.yml --ref vX -f tag=vX -f target=staging`, approve `relay-room-staging`, wait for the smoke.
   - Then `-f target=production`, and approve `relay-room-production`. This drops every live room's sockets, so pick a quiet time.
2. **Hold pushes.** Push nothing else to the tag's history while the release runs.
3. **Staging.** The tag's `release` run builds the installers and the hosted app (two builds, one hash), then makes the draft with the
   manifest, bundle and hash. Approve `hosted-staging`, wait for its deploy, `verify_live` and smoke. Open the staging origin if you
   want to look.
4. **Production.** Approve `hosted-production` and wait for its deploy, `verify_live` and smoke.
5. **Outside check.** From a machine outside CI, with a clone at the tag, run
   `python3 -I scripts/hosted/verify_live.py --tag vX --manifest <draft's hosted-phone-manifest.json>`. The draft's asset is not
   public yet. It must exit 0.
6. **Publish.** Publish the draft. Do not publish a draft whose production deploy or check failed. Fix forward with a new tag, or roll
   back by running the pipeline on the previous tag.
7. **Who deployed what.** `wrangler versions list` (or the dashboard) must show every version with a message starting with a tag:
   `vX <bundle sha256>` (router) or `vX room <commit>` (room). A version without one is a hand deploy: investigate.

## Departures from the spec
1. **`--env production|staging`** in `verify_live.py`. It names both the origin and the script names for `--scripts`. CI uses it for
   staging.
2. **`--bundle DIR`** (offline local check). CI checks the artifact with it before every deploy and in `publish`, instead of inline
   Python.
3. **`/` is fetched too** (it must equal `index.html`). `_headers` is not fetched: wrangler ignores it on upload, verified in its
   source.
4. **`deploy-room` must run from the tag itself** (`--ref vX`). The `tag` input must equal it, because environment tag rules check
   the run's ref. Its message is `"<tag> room <commit>"` (spec: `"<tag>"`), so every CI version starts with the tag and carries a hash.
5. **`hosted-build` checks `app/package.json` against the tag.** Otherwise staging could deploy from a mislabelled tag while `build`
   fails.
6. **Notes go after `latest.json`.** The hosted lines are not in the updater notes apps show.
7. **The page job also runs both Python test files** and `verify_live.py` against `wrangler dev`. CI otherwise runs neither.
   `relay.yml`'s paths now include the phone app and `scripts/hosted`.
8. **The `test` job's workerd uses `localhost:8787` as before**, now started twice by a small function.

## Open questions / findings
- **Service worker bug (from phase 3, outside this phase's files).** `vite.phone.config.ts` substitutes only the **first**
  `__PRECACHE__` in `sw.js` (`String.replace` with a string). The built `sw.js` still contains `__PRECACHE__.includes(s.pathname)` in
  its fetch handler. Any non-asset, non-navigation GET there throws `ReferenceError`. The browser then falls back to the network, so
  icons and the web manifest are never answered from the cache, which affects offline use. `master`'s config has the same line, so
  the daemon's phone bundle is affected too. Suggested fix: `replaceAll`, plus a test that the built `sw.js` has no `__PRECACHE__`
  left. Owner: phases 3/6.
- **Header comparison is exact.** Local wrangler replaced `Cache-Control` with the `_headers` value. Whether Cloudflare's edge does the
  same (or appends to its default) is **unverified** until the first staging run. If it appends, `verify_live` will report it there,
  before production.
- **Staging's CSP.** Staging serves the production-built artifact (same bytes, by design), so its CSP names
  `wss://kelpie-link-relay.baopeak.workers.dev`. Real-phone pairing on staging relies on `'self'` covering `wss:` (modern browsers do).
  This does not affect `verify_live` or the smoke.
- **Concurrency queueing.** With one group, GitHub keeps one running and one pending deploy job; a third queued job cancels the pending
  one. This is acceptable for one release at a time.
- **Unpinned existing actions.** `build` and `publish` still use `@v4` tags. Pinning them too, since `publish` writes release assets,
  is the owner's call.
- **Unverified paths.**
  - `--scripts`: the API path and the multipart part names for module Workers, to be checked in staging.
  - `wrangler versions rollback` restoring assets: still unverified (phase 8).
- **Local leftovers.** `app/node_modules` and `relay/node_modules` remain in this worktree (installed offline from the store,
  gitignored). Everything else (`relay/public`, `.wrangler`, `dist-phone-hosted`, dry-run outdirs, workerd `dist`/`rooms`,
  `__pycache__`) was removed.
