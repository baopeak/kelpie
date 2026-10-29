# Settings export and import

Branch `phase15/config-export`, stacked on `phase15/git-basics` (PR #6). No daemon change: `API_LEVEL` stays
16. Automations go through the existing `automation.*` RPCs over the app connection.

## What was built

**Native side: `app/src-tauri/src/config_transfer.rs`** (new, registered in `lib.rs`; tests in
`config_transfer/tests.rs`)
- File format: `{ kind: "kelpie-config", version: 1, exportedAt, appVersion, sections }`. The sections are
  `settings`, `quickCommands`, `layouts`, `automations`, `sshHosts`, `plugins` and `agents`.
- Export and import go through the same cleaning, so an export reads back identically:
  - Each section has an allowlist. Unknown fields and sections are dropped, and ignored sections are reported.
  - Unusable items are skipped and counted.
  - Settings never include `profiles`, `quickCommands` or `layouts` (they have their own sections),
    `accounts`, `activeAccounts`, `sshConfig`, `rootYoloOk` or `skippedUpdate`.
  - Every string goes through `kelpie_proto::redact`. Any value it flags is masked; text around the secret
    is kept (`export GITHUB_TOKEN=[redacted] && …`).
  - `is_secret(name, value)` checks the value on its own, as `NAME=value` and as `"name": "value"`.
  - Environment is stored as `[{name, value?}]`, sorted. Values are included only on request, and never
    when flagged as secret. Secret entries of SSH host `SetEnv` are dropped.
  - Automations lose `approval`, `approved_hash`, `command_*`, run history and caps, and are always written
    with `enabled: false`.
  - Hosts are deserialized into the existing `Host` struct and checked with `ssh_hosts::normalize`. Anything
    like `password`, `passphrase` or `privateKey` is dropped; `os` and `lastConnected` are cleared.
  - Layout templates are checked recursively (depth 8, at most 12 children per split).
- Commands:
  - `config_export(path, sections, wanted, includeEnvValues, exportedAt)`: adds the hosts (strict read)
    and the plugin list (id, name, version), then writes atomically (temp file beside the target + rename).
  - `config_import_read(path)`: refuses a file over 2 MB without reading it, then checks kind, version
    and schema. Nothing is changed.
  - `config_backup(appState, automations)`: writes `<data>/backups/config-<ms>/` with
    `app-state.json`, `automations.json` and a byte copy of `ssh-hosts.json`. It is built in a temp folder
    and renamed in; the newest 5 are kept.
  - `config_hosts_current`: reads the hosts strictly.
  - `config_hosts_write(expected, vault)`: compare-before-write under the vault lock.
- `ssh_hosts.rs`:
  - `load_strict` (an unreadable or corrupt file is an error, never an empty vault).
  - `save_checked`.
  - `vault_path` is now `pub(crate)`.
- `plugins.rs`: the `list_installed` body is factored out, plus `export_list` and `valid_plugin_id`.

**Window: `app/src/lib/config-transfer.ts`** (no store import; it takes a `TransferContext`, and tests use a
fake one)
- Settings: an allowlist of the keys of `defaultSettings`, minus the exclusions above. A value is kept only
  when it has the default's JSON shape (same scalar type, string lists, known object keys). Other keys are
  reported as skipped.
- `planImport` drives the preview. Matching is by id first, then by name (label, name or alias), and a
  replaced item keeps its local id:
  - Quick commands: an unknown `projectId` becomes null.
  - Agents: matched by kind. Environment entries with a value replace the local one. A name with no value
    keeps the local value; if there is none it goes to the "needs values" list.
  - Hosts: a replaced host keeps its local `os` and `lastConnected`.
  - Automations: always `enabled: false`, with history zeroed. `project_id` is remapped by id, then by
    `cwd`.
  - Plugins: the preview only reports which ones are missing.
- `applyImport`:
  1. Reads the current configuration again. A failed read stops the import.
  2. Plans again and refuses if the counts differ from the preview.
  3. Refuses if app state cannot be saved (`persistBlocked`).
  4. Takes the backup.
  5. Applies hosts (compare-before-write), then settings (`replaceSettings` + `saveNow`), then automations.
  6. On failure, undoes the applied steps in reverse:
     - added automations are deleted;
     - replaced ones are saved back and, if they were approved, re-approved with their own
       `command_hash`;
     - settings are restored and saved;
     - hosts are written back.
- `store.ts`: `saveNow()` (an immediate save that reports failures), `replaceSettings()` and `canPersist()`.

**UI: `components/settings/config-transfer.tsx` + `.css`**
- It is a lazy chunk (5.41 kB gz) behind a new row, Settings → Session Daemon → "Export and import settings"
  (`system-settings.tsx` via `lazy` + `Suspense`, with a catalog entry so search finds it).
- Export:
  - One checkbox per section. Automations are disabled unless connected with `daemonApi() >= AUTOMATION_API`.
  - "Include values of agent environment variables" is off by default and shows a plain-text warning when
    ticked.
  - Export… uses the native save dialog.
- Import… uses the native open dialog, then shows a preview:
  - per section: a checkbox and its counts (new / replacing; settings say "n of total change" and list the
    changed keys); skipped and ignored entries;
  - notes: automations come in off, hosts arrive without passwords, the plugin list is only a list, and
    which variables have no value.
- The result line gives the backup folder, the variables still to fill in and the plugins to install.
- After an import:
  - open terminals get the new settings;
  - the hosts vault is reloaded;
  - the automations list refreshes.
- E2E hook: following the existing pattern (scripts call `window.__kelpie.plugins.pluginInstallFolder`
  instead of the folder dialog), `main.tsx` exposes the module as `window.__kelpie.configTransfer` in dev
  and `KELPIE_E2E` builds. `exportTo(path)` and `previewImportFile(path)` are what the buttons call after
  their dialog. There is no new test mechanism.

**i18n**: `locales/vi/config.json` has 33 keys, and `backend.json` has 9 Rust error messages (for
`errorText`).

**Docs**:
- `docs/user-guide.md`: the Settings list, plus a new "Export and import settings" section (what is never
  exported, preview, backup, undo, automations off, hosts without passwords, plugins list only).
- `docs/architecture.md`: a row in "Features beyond sessions".

## Numbers
- Rust: `cargo test -p kelpie -j 2`: 196 passed, 5 ignored (pre-existing), including 12 new `config_transfer` tests.
- Clippy: `cargo clippy -p kelpie --all-targets` is clean. `kelpied` was not touched.
- Vitest: 485 passed, 43 files. `config-transfer.test.ts` has 17 tests.
- Frontend: `pnpm typecheck` clean. `pnpm i18n:check`: 0 missing, 0 problems, 0 untranslated-looking.
- `pnpm build` passes. Bundle gate: entry **326.02 kB gz** against a 338.60 limit (baseline 323.6; it was
  ~325.8 before this change).
  - Lazy chunk: 5.41 kB JS + 0.31 kB CSS.
  - Report: `plans/reports/perf-config-export.json`.
- E2E `scripts/e2e/config_export.py`: **45/45 passed twice in a row**.
  - Afterwards only the user's own `kelpie.exe` and `kelpied.exe` were running; every test process and
    scratch folder was removed.

## E2E coverage (`scripts/e2e/config_export.py <shots> <scratch> <app.exe>`)
- Setup:
  - Two phases. Each has its own endpoint, data folder, `kelpied` and `tauri-driver` (free `--port` and
    `--native-port`).
  - `KELPIE_ENDPOINT`, `KELPIE_DATA_DIR` and `KELPIE_SSH_DIR` are set in the script's own process before
    anything starts.
  - Nothing touches the keyring: no `ssh_secret_set`, and no host is removed. No network.
- A (export):
  - Seeds credentials into:
    - quick-command text and a layout;
    - claude env (`GITHUB_TOKEN`, `DB_PASSWORD`);
    - automation prompt and env;
    - host `SetEnv` and notes;
    - an account folder.
  - Seeds an approved command automation (approved with `automation.approve`).
  - Ticks "include values" in the UI and exports with values and without.
  - Checks:
    - the header;
    - no token, password, approval hash, account folder or credential key in either file;
    - names always present, values only for non-secrets;
    - automations off;
    - the host keeps its key path;
    - the plugin list.
- B (fresh data dir and daemon):
  - Corrupt, oversized (>2 MB) and newer-version files are refused, with no change and no backup.
  - The UI preview counts are checked, then the Import button is clicked.
  - Checks:
    - every section arrived;
    - no accounts;
    - both automations are off, and the command has `command_approved: false` with Approve command shown
      on the Automations page;
    - a backup was kept;
    - the missing variables and plugin are reported.
  - It exports again and compares the sections:
    - settings, quick commands, layouts and hosts are identical;
    - agents and automations are identical once name-only variables are removed;
    - B lists the plugin it has.

## Decisions
- **Where the logic lives:**
  - The app core owns the file contract and the secret filter, so the redaction helper and the strict
    hosts read stay in Rust.
  - The window owns what it stores (settings) and the daemon-RPC orchestration, so automations keep using
    the app connection's existing approval rules.
- **Automations come in `enabled: false` (paused) and unapproved:**
  - The file never carries approvals.
  - The daemon's `upsert` ignores an editor's approval anyway.
  - When an import replaces a local automation whose command is byte-for-byte the same, the daemon keeps
    this machine's own approval for it, but the automation is still off.
- **Variables named without a value:** they keep this computer's value, or are reported as needing one.
  They are never created empty, because an empty variable would override one inherited from the system.
  Because of this, a round trip is identical only once those names are removed (the e2e checks exactly
  that).
- **Plugin list is informational:**
  - There is no recorded source to install from, and the task rules out network use.
  - The import says which plugins to install.
  - Approvals are never transferred.
- **Not exported:** SSH port-forward rules (`forwards`) are left out to keep scope tight; an auto-start
  forward from a file would open ports on launch. `registryAgents` and `skippedUpdate` are also out, because
  they are not part of `defaultSettings`.
- **The preview names changed setting keys**, so a file that sets e.g. `agentPermissions: "yolo"` shows it
  before Import.
- Unknown `projectId` / `project_id` values become null (or are matched by folder for automations), because
  project ids are per-machine.

## Follow-ups / concerns
- Undoing a settings save that itself failed reports "not every change could be undone". This is
  conservative: `state_save` writes to a temp file and renames, so the file on disk was never changed.
- Port forwards and `registryAgents` could become their own sections if users ask.
- A code review pass is not yet done (the orchestrator usually runs one per sub-task).

Status: DONE
Summary: Settings export/import is built: a Rust-owned file contract with secret filtering, a lazy settings UI with a per-section preview, backup and undo, and imported automations always off and unapproved. Unit tests (Rust 12, Vitest 17) and the two-phase GUI round trip (45/45 twice) pass, and the entry bundle is 326.02 of 338.60 kB gz.

## Review fixes

All findings in `plans/reports/code-reviewer-261029-phase15c-config-export.md` were addressed. No daemon change was made; `API_LEVEL` stays 16.

**The preview now shows everything that decides what runs.**
- `planImport` builds a list of these values, each shown old → new in full: `{section, item, field, before, after}`.
  - Settings `shellArgs`, `agentPermissions` and `agentsScripting`.
  - For each agent profile: `args`, `permission` and every `env:<NAME>` (name and value).
  - The text of new and replaced quick commands, with where it goes (`$` terminal, `>` agent prompt) and whether Enter follows.
  - Automation `kind`, `command` (program and argv), `args`, `prompt`, `cwd` and `env`.
  - SSH host `startup`.
  - Saved layouts whose panes start on another machine (`kind @ dest:port path`).
  - Unchanged values are not listed.
- When the list is not empty:
  - Import stays disabled until "I've checked these commands; they will run on this computer." is ticked.
  - Changing which sections are ticked clears that tick.
  - `applyImport` refuses without the confirmation (`runsChecked`). It also re-plans and refuses when the sections, the list or the host changes differ from the preview.
- Environment values from the file are imported as they are, and shown in this list.

**A replaced SSH host whose connection target changed no longer keeps this computer's password.**
- The target is user, hostname, port and jump. When any of these differs:
  - the host is marked "Connection target changed: alias, before → after";
  - the password saved for that host id is forgotten. This is the last step of the apply, so undo never needs to restore it;
  - its `os` and `lastConnected` are reset.
- Agent forwarding (`forwardAgent`) never comes from a file:
  - a new host gets false;
  - a replaced host keeps the local value only while its target is unchanged; otherwise false.
- Port forwards bound to a moved host are removed rather than kept, and the preview says so. This is the safer choice: an auto-start forward would otherwise tunnel through the new destination. The removed forwards remain in the backup.

**Secret filter.**
- Names: `secret_name` is case-insensitive. It matches a name part that is, or ends with, PASS, PASSWD, PASSWORD, PWD, SECRET, TOKEN, KEY, AUTH, CREDENTIAL(S), COOKIE or SESSION, ignoring trailing digits.
  - Such a variable's value is never exported, even with values on.
  - Secret-named SSH `SetEnv` entries are dropped.
- Argument lists (agent args, automation args, shell argv, `shellArgs`) are masked by `mask_args`:
  - the value after a secret flag (`--api-key X`);
  - the value in `--flag=value`;
  - `name=value` arguments with a secret name (`-c api_key=X`);
  - every argument also goes through the text filter.
- Free text is masked by `mask_text`. This covers the same flags in text, `mysql -p…`, `curl -u user:…`, and upper-case `NAME=value` whatever the value looks like, followed by `redact`.
- The copy in the catalog, the warning, the user guide and the architecture doc now describes what is filtered and says to check the file before sharing it.
- Every case in the report has a Rust test.

**Settings classification.**
- `app/src/lib/settings-transfer.json` lists `exported` and `excluded` settings. It is the single source: the window imports it, and the app core reads it with `include_str!`. This also removes the duplicated exclusion list.
- A Vitest test fails when a key of `defaultSettings` is in neither list or in both. `defaultSettings` moved to `lib/default-settings.ts`, re-exported from `store.ts`, so the test does not need the store.
- A Rust test checks that only exported keys get through.

**Value checks on import.**
- Imported values are checked against the ranges and choices the settings page offers:
  - font size 8–28, line height 1–2, scrollback 500–100 000;
  - UI scale 0.9 / 1 / 1.1 / 1.2;
  - panel widths, cost cap, hibernate minutes;
  - every enum (theme, language, keep-awake, resume…);
  - `integrations.jira` reduced to `{url, email}`.
- Invalid values and unknown keys are left out and listed in the preview.

**Refused automations.**
- When the daemon refuses an automation (an agent or script that is not on this computer), it is skipped and named in the result; the rest of the import stays.
- If the daemon no longer answers the follow-up `automation.state` check, the whole import is undone as before.

**Low findings:**
- The copy now says commands need approving "unless this computer already approved exactly the same command".
- Settings are re-read and re-planned right before they are replaced, so a change made while the backup was being written is kept.
- The preview reads each section on its own (`loadForPreview`). A section that cannot be read is shown as unavailable with the reason; this includes automations on an older daemon.
- `read_capped` checks the path's metadata before opening it.
- The exclusion list is no longer duplicated (see "Settings classification" above).
- An imported host id that is not alphanumeric gets a new id, so a password can be saved for it.

**E2E** (`config_export.py`):
- Phase A also seeds `shellArgs` and turns agent forwarding on for its host.
- Phase B first saves its own `build-box` with another target, agent forwarding on and a port forward. It then checks:
  - the "connection target changed" note;
  - the old → new values for the terminal program, claude's args, permission and variable, quick-command text, the automation command and the prompt;
  - Import disabled until the tick, then enabled;
  - after import: same host id, new hostname, agent forwarding off, forward removed, and that host's password forgotten;
  - the round-trip comparison ignores host `id` and `forwardAgent`.
- The password forget goes through `TransferContext.forgetPassword`. The e2e replaces it with a recorder via `configTransfer.replacePasswordStore`, which is only reachable through the dev/`KELPIE_E2E`-gated `window.__kelpie`. The script refuses to import if the recorder is not installed.
- **Disclosure:** the first H2 attempt wrapped `__TAURI_INTERNALS__.invoke` in the page. Tauri defines that property read-only, so the wrapper silently did nothing. In that one run, the import's forget call reached `ssh_secret_set` and the OS credential store. It deleted `ssh-password:<id>` under the test endpoint's own service names, `Kelpie (kelpie-config-export-<pid>-b)` and the legacy `Erka (…)` copy, where nothing was stored. No real Kelpie entry was touched. The two green runs below used the recorder.

**Numbers after the fixes**
- Rust `config_transfer`: 18 tests (6 new).
- `cargo test -p kelpie -j 2`: 202 passed, 5 ignored (pre-existing). `cargo clippy -p kelpie --all-targets` is clean.
- Vitest: 492 passed; `config-transfer.test.ts` has 24 tests.
- typecheck clean; `i18n:check` 0 missing.
- `pnpm build` passes. Entry 326.06 kB gz against the 338.60 limit; lazy chunk 8.35 kB JS + 0.46 kB CSS.
- E2E: 56/56, twice in a row. Afterwards only the user's own `kelpie.exe` and `kelpied.exe` were running.

Status: DONE_WITH_CONCERNS
Summary: All blocking, medium and low review findings are fixed and tested. During development, one e2e run reached the OS credential store under the test endpoint's isolated service name; nothing real was affected, and the final runs use a recorder.

## Review fixes, round 2

All round-2 findings in the reviewer report were addressed, plus the optional Lows. No daemon change was made; `API_LEVEL` stays 16.

**Filter false positives, and masked values on import**

Narrower name matching (`secret_name`):
- Names are split at punctuation and at lower→upper case.
- `PASS`, `PWD`, `KEY`, `AUTH`, `OAUTH`, `COOKIE` and `APIKEY` count only as a whole part.
- `PASSWORD`, `PASSWD`, `SECRET`, `TOKEN` and `CREDENTIAL(S)` also count at the end of a part.
- `SESSION` on its own no longer counts, so `--session-id`, `--no-session-persistence` and `XDG_SESSION_TYPE` keep their values. `SESSION_TOKEN` and `--session-token` are still caught by `TOKEN`.
- `BYPASS`, `MONKEY` and `AUTHOR` no longer match.
- A name ending in `env_key` (Codex `env_key=OPENAI_API_KEY`) holds a variable name and is left alone.

The redaction marker is never written into this computer's configuration:
- A masked value takes this computer's value for that field.
- A masked argument (`unmaskArgs`) takes this computer's value for the same flag (`--api-key X`) or name (`--token=X`, `api_key=X`).
- When this computer has no value:
  - the masked argument is dropped together with the flag it belongs to (`-c api_key=X` drops the `-c`), so the remaining arguments still work;
  - other fields are left empty;
  - an item that cannot work without the field is left out (counted as skipped): quick-command text or label, layout name or template, host name, and an automation's name, folder, command, trigger, or prompt for agent actions.
- Every case is listed in a new `ImportPlan.fillIn`, shown in the preview and in the result.
- Masked environment values behave like name-only entries.

Tests:
- Rust: the three report cases plus ordinary names.
- Vitest: a round trip of your own export (local `--api-key realkey` and `--token=tok1` are kept, the file's `--model opus` is taken), arguments with no local value, left-out items, the fill-in list, and that `[redacted]` appears nowhere in the planned settings, hosts or automations.
- E2E: A's Claude has `--api-key <A's key>`. B already has its own `--api-key`, which is kept. B's export equals A's. `[redacted]` is nowhere in B's configuration. The masked quick command and automation are left out and named.

**Forget order**
- Moved hosts' passwords are now forgotten right after the backup, before anything is saved. A crash or a failed hosts undo can therefore never leave a password bound to a new address.
- A forgotten password is never restored. `ImportFailed` carries `forgotten` and says so:
  - "…; everything else was put back. The saved passwords of moved hosts were cleared: … Enter them again on the Hosts page."
  - or "…; nothing else was changed. …" when nothing else had been applied.
  - It never says "nothing was changed".
- If a forget fails part way, the import stops before any save and names the passwords already cleared.
- Tests: forget order; the "daemon gone" undo keeps the password cleared and the message says so; a partial forget failure leaves no write; settings save failure.

**Lows**
- `identityFile` is in the run list for every host, and a new host's connection target (user@host:port via jump) is listed too.
- Network paths (`\\server\…`, `//server/…`) are refused on import (`is_unc`; the item is skipped, or the setting dropped) for:
  - a host key file;
  - an automation folder or program;
  - the terminal program (`shellArgs[0]`).
- Automations now list `action`, `when` (`file *.md ≤10/day` or `weekly 03:00 (0)`) and `deliver`.
- The user guide lists layouts with remote panes and the other new run-list entries, and describes masked-value handling, UNC refusal and the forget order.
- E2E: the recorder is checked again right before the Import click, and the script refuses to import without it.
- Optional (daemon write failure wording): after a refusal the list is read again. If the daemon kept the imported automation in memory although it reported a failure, the import fails and is undone instead of being reported as "refused". Unit-tested.
- Optional (Windows device paths): `\\.\` and `\\?\` (and `//./`, `//?/`) are refused before any open or metadata read, for both import and export.

**Follow-ups**
- Pre-existing, separate ticket: `askpass::answer` falls back to any saved host whose hostname matches the prompt (`crates/kelpied/src/askpass.rs:38`).
  - On an OpenSSH that does not prefix keyboard-interactive prompts with "(user@host)" (check the Windows 10 built-in 8.1), a server could word a prompt to receive another saved host's password.
  - Not changed here.
- Still open from round 1: port forwards and `registryAgents` could become sections of their own.

**Numbers after round 2**
- Rust `config_transfer`: 21 tests.
- `cargo test -p kelpie -j 2`: 205 passed, 5 ignored (pre-existing).
- Clippy on `kelpie`: it first flagged a collapsible `if` in `name_parts`. I collapsed it (the logic is unchanged) and re-ran: clippy is clean and the 21 `config_transfer` tests still pass.
- Vitest: 497 passed; `config-transfer.test.ts` has 29 tests.
- `pnpm typecheck` clean; `i18n:check` 0 missing.
- `pnpm build` passes. Entry 326.07 kB gz against the 338.60 limit; lazy chunk 10.00 kB JS + 0.46 kB CSS.
- E2E: 59/59 twice in a row, with the recorder in place.
  - The run before those failed one check, the quick-command count, which had to be updated for the masked item now left out.
  - Afterwards only the user's own `kelpie.exe` and `kelpied.exe` were running.

Status: DONE
Summary: All round-2 findings and the optional Lows are fixed and tested. Rust, Vitest, typecheck, i18n, build and the bundle gate are green, and the GUI round trip passed 59/59 twice using only the recorder.
## Review fixes, round 3

All round-3 findings in the reviewer report were addressed. No daemon change was made; `API_LEVEL` stays 16.

**R3-M2: network and device paths (security)**
- One classifier, `path_kind(path) -> Local | Network | Device`, handles every form.
  - It first treats `/` and `\` alike (`\/server`, `/\?\…` and `//./pipe` all count).
  - Network: `\\server\…`, `//server/…`, `\\?\UNC\…`, `\\.\UNC\…`, `\??\UNC\…`.
  - Device: `\\.\…`, `\\?\…`, `\??\…`.
  - `foreign_arg` applies it to a whole argument and to the part after `=`, or after `:` in a flag (`-File:\\srv\x.ps1`).
- On export and on import, these are left out before anything reaches the daemon:
  - a host key file;
  - an automation folder, program or any shell argument (the daemon reads scripts given to an interpreter when it saves the automation);
  - the terminal's program or any of its arguments.
- What was left out is named (`foreign_items`, `{section, name}`):
  - in the export result: "Not exported, they use a network or device path: share-key (SSH hosts)";
  - in the import preview: "Left out, a network or device path: …".
- `device_path` for the file itself now uses the same classifier. `\??\` and mixed separators are refused before any open. A network share chosen in the dialog is still allowed: the user picked it.
- Tests:
  - every path form;
  - argument forms;
  - an import with each kind of foreign item, including the names reported;
  - device paths never opened, mixed separators included.
- E2E checks:
  - A's host with a `\\127.0.0.1\…` key is named and absent from the file;
  - a file whose shell automation runs a script on `\\127.0.0.1\…` is previewed with that automation named and left out, and the daemon never sees it.
  - Both use the loopback address and a share that does not exist, so nothing leaves the machine.

**R3-M1: argument repair**

`unmaskArgs` now matches by position only:
- An element that is exactly the marker, after a flag, takes the value after the same occurrence of that flag on this computer. A repeated `--token` takes the 1st and 2nd values in order, and a local flag followed by another flag has no value.
- Without a local value, the marker and that flag are dropped together. This is the only case where a flag goes.
- `name=[redacted]` takes the same occurrence of `name=…` here.
- A header (`-H "Authorization: Bearer [redacted]"`) takes a local value only from the same occurrence of that flag, and only when it carries the same header name; otherwise there is no local value. A header needs a space after its colon, so `https://…` is not one.
- Any other masked element takes the element at the same index, only when both lists have the same shape (same length, same flags in the same places).
- Otherwise only the masked element is dropped. A standalone flag is never dropped, and a non-marker argument is never used up.
- For example, `--read-only https://u:[redacted]@db` with no local value becomes `--read-only`. `-c api_key=[redacted]` keeps the `-c`. That is the rule as given; the preview lists the dropped argument.
- A shell automation whose command would change (any `command:` entry it could not restore) is left out and named.
- Tests cover every example in the review, plus repeated flags and repeated `name=`.

**R3-M3: name matching**
- Whole-part words: PASS, PWD, KEY, AUTH, OAUTH, COOKIE, PAT.
- Whole-part or suffix compounds: PASSWORD, PASSWD, SECRET, TOKEN, CREDENTIAL(S), SSHPASS, DBPASS, PASSPHRASE, PRIVATEKEY, SECRETKEY, ACCESSKEY, APIKEY, AUTHORIZATION, WEBHOOK.
- An explicit list of known false positives: BYPASS, COMPASS, PASSTHROUGH, PASSIVE, PASSPORT, MONKEY, TURNKEY, HOTKEY, AUTHOR, COMPAT, PATH, PATCH.
- Acronym camel-case is split: `HTTPAuth` → `HTTP`, `AUTH`; `OPENAIKey` → `OPENAI`, `KEY`.
- New text masks:
  - `x-…key/token/secret/auth/password:` and `api-key:` / `private-token:` / `(proxy-)authorization:` headers, including the `Basic`/`Bearer`/`Token`/`Digest` scheme;
  - `Bearer` tokens of 8 or more characters;
  - `sshpass -p X`.
  - `SSHPASS=…` is caught again through the name rule.
- In argument lists (`mask_args` now takes the program):
  - `-u user:pass` (not a drive path such as `C:\x.py`);
  - `-pPASS` for mysql, mariadb and sshpass;
  - `-p PASS` for sshpass.
- Tests cover every name and text case in the review, plus non-secrets that must stay unmasked (`cargo -p kelpie`, `python -u C:\x.py`, `Bearer of good news`).

**Lows**
- **R3-L1:** a new `ImportPlan.keptLocal` (part of `planShown`) says when a masked value took this computer's value. It shows in the preview and the result, e.g. "Masked in the file, this computer's value is kept: Claude Code (arguments --api-key)".
- **R3-L2:** after a refused save, the check compares every field the import saves: id, name, enabled, kind, prompt, cwd, project, args, env, schedule, deliver, trigger, action, command, quick label and dry run. A test covers a replaced automation that differs only in those fields; it is put back with its approval.
- **R3-L3:** covered by the shared classifier (mixed separators, `\??\`).
- **R3-L4:** hosts and automations dropped for a network or device path are named on export (see R3-M2).
- **R3-L5:** acronym splitting (see R3-M3).

**Notes**
- The test daemon in the unit tests now refuses a save only once, so an undo can put a refused automation back as a real daemon would after a one-off failure.
- During this round, drive C: (Claude Code's temp folder) ran out of space once, filled by other sessions' temp files and other workstreams' test folders.
  - One background test output was cut short, and that run was simply repeated.
  - Nothing outside this session's own files was deleted.
  - Free space on C: is still low (about 1 GB); builds and e2e scratch stay on D:.

**Numbers after round 3**
- Rust `config_transfer`: 24 tests (`cargo test -p kelpie config_transfer`; an earlier draft of this line said 26).
- `cargo test -p kelpie -j 2`: 208 passed, 5 ignored (pre-existing). `cargo clippy -p kelpie --all-targets` is clean.
- Vitest: 504 passed; `config-transfer.test.ts` has 36 tests.
- `pnpm typecheck` clean; `i18n:check` 0 missing.
- `pnpm build` passes. Entry 326.09 kB gz against the 338.60 limit; lazy chunk 10.59 kB JS + 0.46 kB CSS.
- E2E: 63/63, twice in a row, with the password-store recorder.
  - An invocation before those printed nothing (its output was lost), so it is not counted.
  - Afterwards none of this script's processes were running. The `tauri-driver`, `msedgedriver` and `kelpied` running at that time belong to another workstream (other ports, another worktree) and were left alone.

Status: DONE
Summary: All round-3 findings are fixed and tested: one shared path classifier keeps network and device paths (shell arguments included) away from the daemon, masked arguments are matched by position only, and the name matcher covers compound and acronym names. Every check is green, and the GUI round trip passed 63/63 twice using only the recorder.
## Follow-up: agent arguments with a hidden value that cannot be filled in

The coordinator first asked for the flag before a masked flag value to be dropped as well. They withdrew this before it was implemented: it cannot tell `-c api_key=…` from `--read-only <masked url>`, and silently dropping a safety flag is worse than leaving a dangling `-c`. The rule implemented instead:

- When an agent profile's arguments have a masked element that cannot be restored from this computer, none of the file's arguments are imported for that profile. This computer's arguments for it stay unchanged, or there are none for a new profile.
- The profile's other fields (label, permission, variables) import as before.
- A new `ImportPlan.argsNotImported` lists the profile labels. It is part of `planShown`, so the apply must match the preview.
- The preview (agents row) and the result show "{profile}: arguments not imported, a hidden value couldn't be filled in here (re-enter it in Agent settings)". A Vietnamese translation was added.
- Restored elements of such a profile are not reported as "kept", since the file's list is not used.
- When every masked element is restored, the arguments import as before.
- Shell automations already followed this rule (left out whole when their command would change).
- Not changed (outside the request): terminal arguments (`shellArgs`) and an agent automation's `args` still lose only the masked element. `unmaskArgs` itself is unchanged. The docs say which rule applies where.

Tests (`config-transfer.test.ts`):
- Codex `--search -c api_key=[redacted]` with no local value, for a new profile and over existing local arguments;
- `--read-only https://u:[redacted]@db/x`, with and without local arguments;
- a list where one masked element could be filled in and another could not;
- successful restoration (`-c api_key=` from here) still imports and is reported as kept;
- the earlier position-matching cases, now asserting the whole-list fallback;
- terminal arguments still lose only the masked part.

The e2e did not depend on dropping: its agent case is a successful restore (B has its own `--api-key`).

Numbers:
- `cargo test -p kelpie -j 2 config_transfer`: 24 passed. `cargo clippy -p kelpie --all-targets` is clean. No Rust change was made.
- Vitest: 506 passed; `config-transfer.test.ts` has 38 tests. `pnpm typecheck` is clean; `i18n:check` reports 0 missing.
- `config_export.py`: 63/63 on a fresh debug build. Afterwards none of this script's processes were left; the `tauri-driver`, `msedgedriver` and `D:\Works\Kelpie` daemon belong to another workstream.
## Follow-up: whole-list rule for every argument list

At the coordinator's request, the whole-list fallback now applies to every argument list. The path that dropped only the masked element (and, for an exact marker, its flag) is removed.

- `unmaskArgs` is all-or-nothing. It returns the list with every masked element filled in from this computer (position rules unchanged, named in `kept`), or `null` when any masked element has no value here (named in `missing`). Nothing else drops elements.
- **Agent profile:** this computer's arguments stay as they are (none for a new profile), and the profile is named.
- **Default shell (`shellArgs`):** this computer's value stays; it is not counted as a changed setting, and it is named in the preview and the result: "Default shell: arguments not imported, a hidden value couldn't be filled in here (re-enter it in Terminal settings)".
- **Agent automation:**
  - one that replaces a local automation keeps that automation's arguments, and is named: "{name}: arguments not imported, … (re-enter it on the Automations page)";
  - a new one is left out and named like other left-out items, e.g. "Left out, masked in the file and not set here: Review (arguments api_key)". This matches shell automations.
- **Shell automation command:** unchanged; left out whole.
- `ImportPlan.argsNotImported` is now `{ section, item }[]` and stays part of `planShown`. Vietnamese translations were added for the two new messages.

Tests: for the terminal and for an agent automation, a dangling `-c api_key=[redacted]` case and a successful restore. The agent automation also has a no-local-copy case (left out). The agent-profile cases from the previous follow-up are unchanged.

The e2e needed no change: none of its argument lists has a masked element without a local value.

Numbers:
- `cargo test -p kelpie -j 2 config_transfer`: 24 passed. `cargo clippy -p kelpie --all-targets` is clean. No Rust change was made.
- Vitest: 509 passed; `config-transfer.test.ts` has 41 tests. `pnpm typecheck` is clean; `i18n:check` reports 0 missing.
- `config_export.py`: 63/63 on a fresh debug build. Afterwards only the user's own Kelpie app and daemon were running.