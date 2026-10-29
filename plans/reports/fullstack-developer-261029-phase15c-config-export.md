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
