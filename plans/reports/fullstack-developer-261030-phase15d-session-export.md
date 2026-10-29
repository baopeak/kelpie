# Session export (Markdown): Sessions panel → Export as Markdown…

Branch `phase15/session-export` (stacked on `phase15/config-export`). No daemon or API_LEVEL change.

## What was built

- **Rust core** `app/src-tauri/src/session_export.rs`: command `agent_session_export {agent, id, cwd?, home?, title?, path}`
  returns `{path, skipped, truncated}`. It runs on a blocking thread (`fs_ops::blocking`).
  - Agents: `claude`, `codex` and `omp`. Any other agent is refused.
  - **Locating the transcript.** It finds the file by conversation id under the agent's own folder, using `session_usage::resolve_in`, the resolver the usage readers already use. It also finds Codex `.jsonl.zst` rollouts. Ids are checked with `valid_id`. No path inside a transcript is ever opened.
  - **Bounded reading.** It streams the file with `agent_history::open_lines`, which handles zstd, through a new `Lines::next_capped`.
    - A line over 32 MiB is passed over without being kept, and counted as skipped.
    - The Markdown stops at 16 MiB and ends with a note.
  - **Redaction.** Every text piece goes through `kelpie_proto::redact` on its own, before it is cut or formatted. That covers header values, user and assistant text, tool names, the hint shown in each tool summary, tool input and tool output. Each piece is redacted separately so that a key block cut off in one tool output cannot mask the rest of the document.
  - **Format.**
    - Header: title, agent, project folder, session id, started / ended (first and last transcript timestamps), model(s).
    - Then `## User` / `## Assistant` sections.
    - Tool calls and results are `<details>` blocks. Inputs are pretty JSON, or raw text for Codex `apply_patch`. Results are labelled with the tool name and `(error)`.
    - Each tool block is cut to 40 lines or 4,000 characters, with a `… N lines omitted` marker.
    - Code fences are longer than any run of backticks in the content.
    - Images and attachments appear as `[image: image/png]` / `[attachment: …]`.
    - Line endings are LF, and CR in transcripts is normalised.
    - Skipped lines are counted in a closing note.
  - **Writing.** `config_transfer::write_file_atomic` (now `pub(crate)`) writes a temporary file beside the target and renames it over. A relative path, or the transcript itself as the target, is refused.
- **Reuse instead of a new parser.**
  - From `agent_history`: `open_lines`, `codex_injected` (the injected-context list factored out of `codex_prompt`, the same filter the history list uses), `is_claude_command`, `valid_id`, and the folder helpers.
  - From `session_usage`: `resolve_in`, `find_rollout*` and `agent_of`.
  - Claude filtering mirrors `applyClaudeMessage`: meta lines, compaction summaries, sidechains, `<…>` markup and interruption markers are left out. `<synthetic>` notices are dropped too.
- **UI.**
  - `history-panel.tsx` gets a download-icon button "Export as Markdown…" on claude/codex/omp rows, plus `data-agent` / `data-id` on rows.
  - On click it dynamically imports `lib/session-export.ts`, a lazy chunk of 0.71 kB gz. That module opens the save dialog, calls the typed IPC wrapper `ipc.agentSessionExport` and shows a toast.
  - The suggested file name is `<agent>-<id8>-<day>.md`. The title is kept out of the name because the name is not redacted.
  - Test hook: `window.__kelpie.sessionExport.replacePicker`, available in dev / KELPIE_E2E only.
- **Test isolation for OMP.** New `agent_history::omp_sessions_dir()`. With `KELPIE_E2E` and `KELPIE_AGENT_CONFIG_HOME` set, it reads OMP sessions under that fixture home. This follows the agent-setup pattern. The history list, usage and export all use it, so an e2e run never scans the real `~/.omp`.
- **i18n.** Six interface strings were added to `vi/tools.json`, and four backend error messages to `vi/backend.json`. `Exported to {path}` was reused.

## Decisions

- **Export runs in Rust, not TS.**
  - The only existing transcript normaliser in TS is Claude's `applyClaudeMessage`. It builds UI state, and its input (`agent_claude_transcript`) is capped to the last 6,000 lines. Using it would drop the start of long conversations and send whole transcripts over IPC into the webview.
  - Codex and OMP have no on-disk normaliser in TS: their adapters translate the live app-server / ACP protocol.
  - The Rust readers (resolver, streaming, zstd) and the redaction helper already lived in the core. The new code is only a line → event mapping per format.
- **Thinking is left out**, with no option. OMP "narration" blocks, which are prose to the user, are kept.
- **Header labels are English** in the file, so the golden files stay deterministic. The interface strings are translated.
- **Format details.** OMP sessions are trees; only the active branch is exported (see Review fixes). Claude sub-agent transcripts are not included; they are the sub-agent's internals.

## Agents not covered

- Other ACP agents (Gemini, OpenCode, Copilot, Goose, Qwen, Kimi, Auggie, Cursor) have no transcript Kelpie can read today. They are not listed in the Sessions panel, so there is no export action for them.
- OMP is the ACP agent that is covered.

## Numbers

- **`cargo test -p kelpie -j 2`**: 220 passed, 0 failed, 6 ignored. The new `session_export` module has 10 tests and 1 ignored bless helper:
  - golden files;
  - fixture secrets;
  - secrets in user text, assistant text, tool input and tool output for all 3 formats;
  - truncation;
  - fences;
  - malformed and partial lines;
  - the capped reader;
  - the size cap;
  - thinking;
  - writing (atomic overwrite, no `.tmp` left, refusals, Codex `.zst`).
- **clippy** `-p kelpie --all-targets`: clean. kelpie-proto and kelpied were not changed.
- **Frontend**: `pnpm typecheck` ok; `pnpm test` 510 passed; `pnpm i18n:check` 2201 keys used, 0 missing, 0 problems; `pnpm build` ok.
- **Bundle gate**: `perf_gate.py --metrics bundle`. The entry is 326.26 kB gz against a baseline of 323.6 kB and a limit of 338.60 kB: ok. It was about 326.1 kB before this change. The report is `plans/reports/perf-phase-15d.json`.
- **GUI e2e** `scripts/e2e/session_export.py`: 25/25 checks passed in two consecutive runs. Test daemon, driver and scratch data were stopped and removed afterwards. Only the user's own installed Kelpie stayed running, untouched.
  - The first two runs failed on test bugs, both fixed in the script:
    - removing toast DOM nodes by hand confused Solid's toast list;
    - polling the target file kept a read handle open, which made the rename over it fail.

## Files

- New: `app/src-tauri/src/session_export.rs` and `session_export/tests.rs`; `app/src-tauri/tests/fixtures/session-export/{claude,codex,omp}.{jsonl,md}` (synthetic); `app/src/lib/session-export.ts`; `scripts/e2e/session_export.py`.
- Changed: `agent_history.rs`, `session_usage.rs`, `config_transfer.rs` (visibility only), `lib.rs`, `ipc.ts`, `history-panel.tsx`, `main.tsx`, `locales/vi/{tools,backend}.json`, `docs/user-guide.md`, `docs/architecture.md`.

## Follow-ups

- **Over-masking.** `redact` sometimes masks harmless values (`API_KEY = process.env.API_KEY`, `API_KEY=test-key`). This is the shared helper's trade-off, visible in `codex.md`.
- **Locked target file.** If the chosen file is open in a program that blocks deletes, the rename fails and the user sees an error toast. Retrying could be added.
- **Size limits.** A transcript line over 32 MiB is skipped whole, for example one carrying a large pasted PDF, so that message's text is lost too.
- **Possible additions.** An "include thinking" option and sub-agent transcripts could be added if wanted.

## Review fixes

Review: `plans/reports/code-reviewer-261030-phase15d-session-export.md`.

### Redaction of structured tool text

- **Tool inputs.** Every string in a structured tool input is redacted on its own (`redact_value`) before it is serialised. The serialised text is then redacted again as a second net. This covers Claude Write/Edit/Bash heredocs, Codex shell scripts and OMP edits.
  - The new test shows the escaped form alone still leaks: `redact(to_string_pretty(...))` keeps a key at a line start.
- **JSON outputs.** Tool outputs that are JSON text get the same treatment (`json_text`): parse, redact the leaves, then pretty-print. This covers MCP results, Codex outputs without an `output` key, and OMP results.
- **Tests.**
  - The redaction test now has a multi-line case for each format: a `.env` Write, a Bash heredoc, a Codex multi-line script, an OMP edit, and a JSON output with nested secrets.
  - The golden fixtures have the same cases. Claude has a `.env` Write, a heredoc and an MCP JSON result; Codex has a multi-line script and a JSON output; OMP has a multi-line edit.

### Document structure

- User and assistant text goes through `contain`, so a message cannot break the document:
  - an unclosed code fence (backtick or tilde, any length) is closed at the end of the message;
  - outside fences, a line starting with `<` followed by a letter, `/`, `!` or `?` gets a backslash;
  - indented code, inline backticks and a `<` inside a line are left alone.
- Golden case in `claude.md`: pasted `<details>` / `<!--` / `</details>` lines with an unclosed `yaml` fence, and an assistant reply cut off inside a `diff` fence.

### Branches

- **OMP.** A first pass (`active_branch`, reading only the type, id, parentId and activeLeafId fields of each line) finds the leaf: the last entry, or the `activeLeafId` an off-branch entry names. It then walks `parentId` back to the root. The second pass exports only entries on that path; lines without an id are always read.
  - Sessions without parent links are exported in file order.
  - A parent loop ends the walk.
  - `omp.jsonl` now has a branch the user left (`a1`, `a2`) and an off-branch late reply (`a3`, with `activeLeafId: e9`). None of them are exported.
- **Claude stays in file order**, documented in code and docs. Its `parentUuid` chain also forks for parallel tool results and progress lines, so a walk from the leaf could drop real turns. Kelpie's chat view replays Claude history in file order too. I could not check real transcripts under the no-real-data rule.
- **Codex** rollouts are linear.

### Lows

- **Device-path target.** `path_kind` is checked on the target's text before anything opens it; device targets are refused. Network targets chosen in the dialog stay allowed, as in config transfer.
- **Conversation ids.** New `agent_history::file_id_ok`: a valid id with no `:` or `..`; separators were already refused.
  - It is used by the shared resolver (`session_usage::resolve_in`, so usage gets it too), the export, and `agent_claude_transcript`. The last one previously only refused `/` and `\`, so it also accepted `C:x`.
  - Test: `C:notes` resolves to nothing.
- **Network account home.** New `agent_history::local_home` refuses a network home from IPC:
  - the export returns an error;
  - `session_usage`, `agent_latest_session` and `agent_claude_transcript` return nothing or an error.
- **Atomic writer.** `config_transfer::write_file_atomic` now creates its temporary file with a name of its own (pid, a counter and the clock) using create-new semantics. It never writes through an existing file or link at that name, retries on a name clash, syncs, and removes the temporary file on any failure, not only a failed rename.
  - Its other caller, `config_export`, keeps its behaviour, and the config-transfer tests pass.
  - New test: four concurrent writers to one target leave one writer's whole text and no `.tmp` files.
- **Windowed redaction.** A tool text is now redacted only over what its block can keep plus a 4 KiB margin (`window`): at most 41 lines and 8,096 characters. The lines after the window are counted into the "… N lines omitted" marker.
  - A secret that starts in the kept part is masked whole while it is shorter than the margin: token and `NAME=` patterns match a prefix, and a cut key block is masked to its end.
  - Test: a token straddling the 4,000-character cut is masked; a 5,000-line output is counted, not redacted whole.
- **"Ended"** is now the time of the last line that produced exported content. In the Claude golden it moved from the interrupt marker to the last reply. "Started" is unchanged.
- **Toast line breaks.** `.toast > span { white-space: pre-line; }`. Every toast that already put `\n` in its text now shows those breaks. The e2e checks the export toast's second line.
- **Header values** (`Started`, `Ended`, models) are reduced to one line.
- **Codex injected items.** `<turn_aborted`, `<user_shell_command` and `<skill>` were added to `codex_injected`, which the history list shares.
  - The repo has no Codex rollout samples, so I checked the installed Codex binary (`codex.exe`, string search only). These markers sit in the same marker list as `# AGENTS.md instructions` and `<environment_context>`.
  - `<subagent_notification>` is defined the same way but was not requested; it is left out.
  - Fixture: `codex.jsonl` has a `<turn_aborted>` and a `<skill>` user item, and both are hidden.
- **Not changed.** If a `.zst` rollout is corrupt, the export still ends silently at the read error, without a note. This was a test gap in the review, not a requested fix.

### Numbers after the fixes

- `cargo test -p kelpie -j 2`: 226 passed, 0 failed, 6 ignored. `session_export` has 16 tests, up from 10.
- clippy `-p kelpie --all-targets`: clean.
- Frontend:
  - `pnpm typecheck`: ok.
  - `pnpm test`: 510 passed.
  - `pnpm i18n:check`: 0 missing, 0 problems.
  - `pnpm build`: ok.
- Bundle gate: entry 326.31 kB gz, limit 338.60: ok.
- `scripts/e2e/session_export.py`: 28/28 in two consecutive runs. It now also checks that each toast puts its facts on separate lines. Test daemon, driver and scratch data were removed afterwards; only the user's own installed Kelpie stayed running, untouched.

### Over-masking seen in the new golden

In `omp.md`, `DB_PASSWORD=\nLOG_LEVEL=debug` became `DB_PASSWORD=\n[redacted]=debug`. The helper's `\s*` after `=` spans a newline. This is the shared helper's behaviour; it over-masks rather than leaks.

## Review fixes, round 2

Review: "Round 2" in `plans/reports/code-reviewer-261030-phase15d-session-export.md`. It had no blocking findings.

### HTML openers inside containers

- `contain` now looks past any indentation, blockquote markers (`>`, nested `>>`) and list markers (`-`, `*`, `+`, `1.`, `1)`; 1 to 9 digits, followed by a space, a tab or the line end). This uses the new `after_containers`. When what remains starts with `<` followed by a letter, `/`, `!` or `?`, a backslash is inserted before that `<`.
  - This covers `> <!--`, `- item` followed by `  <textarea>`, nested list items, `>>`, `1. > <style>`, and a list item's continuation indented 4 or more spaces.
  - Only fences at the top level are tracked, as before. A fence inside a quote or list, or an indented code block, can show a visible `\`; the review accepted this.
- I picked this over "escape every tag-like `<` anywhere". Inline HTML inside a line cannot open a block, and escaping it would mangle inline text and code spans.
- **Golden.** `claude.md` has a forwarded-mail message with these cases.
- **Check.** Rendering all three goldens with the app's `marked` gives no raw `<!--`, `<script>`, `<textarea>`, `<style>` or `<pre>` outside code. Every `## User` / `## Assistant` heading is present, and each document ends with its last message.

### Redaction residues

- **Numbers under secret-named keys.** These are masked: `{"password": 12345678}` becomes `"[redacted]"`. The key is judged by the credential filter's own rule (`secret_name` probes `redact` with a quoted value), so the list of secret names is not copied.
- **JSON lines.** Tool text that is not one JSON value is tried as JSON lines (`json_lines`): each line that is a JSON object or array has its strings redacted and is kept on one line. Other lines are left alone.
- **Escaped JSON fallback.** The windowed second net is now `redact_escaped`. It also redacts the text with `\n`, `\t`, `\"` and `\\` undone, and uses that form only when it masks more. This covers JSON after a text prefix and JSON in several blocks, while source code with `\n` in a string literal keeps its escapes.
- Tests cover numbers, the probe, NDJSON, text-prefixed JSON, code that stays unchanged, and a whole export with an NDJSON output and a numeric PIN.

### OMP chain broken by an unreadable line

- When the walk from the leaf reaches a parent id that no readable line defines, `active_branch` now returns `None` and the session is exported in file order. Before, the start was dropped silently.
- The same applies when an off-branch entry names a leaf that is not in the file.
- Tests: a middle line that is cut off; a leaf that is not in the file; and a full `render` that keeps the first message.

### Toasts

`.toast > span` now also has `max-height: 40vh; overflow: auto`, so long multi-line git or hook output scrolls instead of growing past the window.

### Atomic writer and its tests

- **Testable rename.** `write_file_atomic` now goes through `write_atomic_with(path, bytes, rename)`, so a test can make the rename fail after the temporary file exists. New test: the temporary file existed, is gone afterwards, and the target is untouched.
- **Retries.** The real rename (`rename_retrying`) is tried again up to 5 times, with 20 to 100 ms back-off, when Windows answers `PermissionDenied`. That happens with concurrent renames to one target or a scanner holding the file. This makes the four-writer test and real exports robust against that contention.

### Numbers after round 2

- `cargo test -p kelpie -j 2`: 229 passed, 0 failed, 6 ignored. clippy `-p kelpie --all-targets`: clean.
- `session_export` now has 19 tests and 1 ignored bless helper.
- Frontend:
  - `pnpm typecheck`: ok.
  - `pnpm test`: 510 passed.
  - `pnpm i18n:check`: 0 missing, 0 problems.
  - `pnpm build`: ok.
- Bundle gate: entry 326.31 kB gz, limit 338.60: ok.
- `scripts/e2e/session_export.py`: 28/28 in two consecutive runs on the rebuilt debug app. Test daemon, driver and scratch data were removed; only the user's own installed Kelpie stayed running, untouched.
