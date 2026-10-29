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

## Review fixes, round 3

Review: "Round 3" in `plans/reports/code-reviewer-261030-phase15d-session-export.md`.

### Approach

I tracked fences with their indentation, quote depth and an end rule, rather than the plain-text fallback. Most assistant replies have code blocks under numbered steps, and wrapping each such message in an outer fence would have made ordinary exports worse. `contain` is rewritten around one rule: where it is unsure whether something is code or text, it treats it as text. The worst outcome is a visible `\` in code, never raw HTML in the file. This is documented on `contain` and in `docs/architecture.md`.

### Fence inside a list item or blockquote (blocking)

- **What is recorded.** An open fence now records its character, its length, its blockquote depth and its column after the quote markers. The column is where the fence run starts, after any list marker.
- **When it ends.**
  - At a closing line: indented any amount, with the same character, at least as long, and nothing after it. List markers are not stripped from a closing line.
  - When a line has fewer `>` than the opener. A blank line counts, because it ends a quote.
  - When the opener was indented (a list item) and a non-blank line is less indented than it, matching CommonMark's end of the list item. Escaping resumes on that line.
- **Closer added at the end** of a message: the opener's quote markers and indentation, then the run. So `1. Install:` followed by an indented `bash` fence gets a closer at three spaces, which closes the fence and can't open a new one.
- **Tests:** the review's two samples, a closer at the same quote depth, a quote fence ended by a blank line, and a closed list fence followed by text.
- **Golden:** both samples are in `claude.md`.
- **Rendered with the app's `marked`:**
  - every `## ` heading survives: claude 8/8, codex 4/4, omp 2/2;
  - every `<details>` survives: 10/10, 8/8, 6/6;
  - no raw `<textarea|script|style|pre|iframe|title|xmp|noscript|plaintext` and no `<!--` outside code;
  - each document ends with its last paragraph.

### Inline tags

- Every `<` followed by a letter, `/`, `!` or `?` is now escaped anywhere in a text line: at the start, behind quote or list markers, or mid-line (`Why does the <textarea> lose focus?`). A `<` already escaped by an odd number of backslashes is left alone.
- **Code that keeps its text:**
  - **Tracked fences.**
  - **Indented code**, only where it can't be a list item's text: 4 or more columns, outside quotes, no list open, after a blank line or more indented code. A list stays open until a blank line is followed by a line at column 0 with no list marker.
  - **Inline code spans**, only where the pairing is certain. Every line of the paragraph (the lines between blank lines and fences) must close its own backtick runs, using CommonMark same-length matching with backslash-escaped backticks skipped, and the line must have no `|`, since GFM splits table cells before code spans.
- Anything else counts as text. So `x ` + "`" + ` / a ` + "`" + ` <b> ` + "`" + ` c` (a pair the renderer may make across lines) and a table row are escaped.
- Tests cover all of these. The golden has `<textarea>` in prose, and `<script>` in inline code kept as written.

### Redaction union

- `kelpie_proto::redact` gains:
  - `redact_ranges(text)`: the byte ranges the same patterns mask, each pattern run on the text itself, sorted and merged;
  - `mask_ranges`.
  - A kelpie-proto test checks that the two together equal `redact` on its sample corpus.
- `redact_escaped` builds the unescaped text with a per-byte map back to the original. It merges the ranges found in the original and in the unescaped text, mapped back, and masks them on the original. Then it runs `redact` once more, which can only mask more.
- The review's tie example is a test: both `s3cret1` and `abcdef123` are masked, and the text keeps its `\n`.
- This adds two public functions to kelpie-proto. No protocol, daemon or API_LEVEL change.

### Numbers and strings under secret keys

- `redact_value` carries a "under a secret-named key" flag down through arrays and nested objects. Every number and non-empty string under such a key is masked: `{"password": [12345678, "hunter2"]}` and `{"token": {"value": 1234, "list": [{"x": "y"}]}}`. Booleans and nulls are kept.
- `secret_name` now checks a quick word list (secret, token, pass, pwd, key, credential) before probing `redact`, so large JSON stays cheap.

### Optional items, both done

- **OMP root.** The session header's id is recorded, and an entry whose parent is that id is treated as a root. Branch selection works whether real files root at `null` or at the session id. Tested.
- **Toast.** `overflow-wrap: anywhere` added.

### Numbers after round 3

- `session_export`: 22 tests and 1 ignored bless helper.
- `cargo test -p kelpie -j 2`: 232 passed, 0 failed, 6 ignored. `cargo test -p kelpie-proto`: 26 passed, 0 failed, 2 ignored.
- clippy `-p kelpie -p kelpie-proto --all-targets`: clean.
- `scripts/e2e/session_export.py`: 28/28 in two consecutive runs on the rebuilt debug app. Test daemon, driver and scratch data were removed. The python processes still running belong to other sessions (a `fake_acp_agent.py` from the main checkout, an http.server) and were not touched.
- Frontend:
  - `pnpm typecheck`: ok.
  - `pnpm test`: 510 passed.
  - `pnpm i18n:check`: 0 missing, 0 problems.
  - `pnpm build`: ok.
- Bundle gate: entry 326.29 kB gz, limit 338.60: ok.

## Review fixes, round 4

Review: "Round 4" in `plans/reports/code-reviewer-261030-phase15d-session-export.md`. Approach as decided by the controller: a real CommonMark parser replaces the hand-rolled fence and code tracking.

### Parser

- **Dependency.** `pulldown-cmark = { version = "=0.13.5", default-features = false }`, in `app/src-tauri` only. It is used for parsing only: no HTML writer and no getopts. `Cargo.lock` gains two packages, pulldown-cmark and unicase; bitflags 2 and memchr were already present.
- **`contain`** parses each message with `Parser::new_ext` and the options the app's `marked` renders (GitHub tables, task lists, strikethrough), using `into_offset_iter`.
  - The byte ranges of fenced and indented code blocks and inline `Code` become the "code" set.
  - Every tag-like `<` (followed by a letter, `/`, `!` or `?`) outside that set is escaped, unless backslashes already escape it.
  - Raw `Html` / `InlineHtml` always starts with such a `<`, so those ranges are escaped by the same rule.
- **Second check.** The result is parsed again. It must have no raw HTML, and it must leave a following `## ` heading a heading.
  - Escaping can change the parse (an HTML block becomes a paragraph), so escaping is redone from the new code ranges at most three times.
  - If the result still doesn't pass, the plain-text form is used.
- **Message left inside an open fence.** This applies when the last fenced block runs to the end of the message and no fence run follows its content inside the block (the parser's own block range shows this). The whole message is then written as plain text in an outer backtick fence longer than any backtick or tilde run in it, with no attempt to make up a closer.
- **Removed:** the old tracking code (`after_quotes`, `lead`, `after_containers`, `code_spans`, `OpenFence`, `flush` and the old `contain`/`escape_tags`) and its tests. The `|` heuristic is gone: the parser knows table cells, and a code span in a cell keeps its text.

### Message boundaries

- Every message now gets its own `## User` / `## Assistant` heading, not only on a role change, so a list or code block can't continue into the next message.
- A message is the text of one transcript entry: consecutive text pieces of one role in one entry are joined. Tool blocks stay under the current heading, getting a new one only after a user message, as before.
- **Tests:**
  - two consecutive user messages (`- item`, then `    <!-- hidden`) parse with no raw HTML, and the second one's indented line is an indented code block, not the list's text;
  - two text pieces of one entry share one heading.

### Regression tests

- **`ADVERSARIAL`** holds 25 messages: every sample from rounds 1 to 4.
  - All 12 round-4 probes: the R3-M1 list fences, the lazy quote and nested quote fences, `para` with an 8-space fence, the two R4-M1a fences, the 2-space top-level fence, `` a `x\` <script> `y` ``, the `|<script>` table cell, and the backslash cases.
  - The R2 container cases, the R3 inline-tag, pairing and table cases, an HTML block containing a code span, and the earlier golden cases.
- **`assert_contained`** places each output as in an export: a heading before it, then the next heading, a tool block and another message. It parses the result and asserts two things:
  1. no raw HTML starts inside the message;
  2. all 3 headings and the `<details>` block after it survive.
- **Expected outputs** are pinned for:
  - escaped prose tags;
  - kept inline, fenced and indented code (including the closed 2-space fence and the nested-fence sample);
  - a table cell's code;
  - a list fence ended by its item;
  - backslash escapes;
  - the plain-text fallback for three open-fence messages;
  - an outer fence longer than a 6-tilde run.
- **Golden.** `claude.md` now also holds:
  - the README sample left inside a fence (`` ```md `` / `> ```` `), written as plain text;
  - the nested and indented fences, kept as written;
  - a list followed by a message starting `    <!-- …`, which is indented code under its own heading;
  - a table with code and a tag.
  - The earlier open-fence messages ("My notes…", "Here is the change…", "Two steps…") are now written as plain text, as specified.

### Render check with `marked` (scripted)

- `app/src/lib/session-export-goldens.test.ts` is part of `pnpm test`. To run it alone: `cd app && pnpm exec vitest run src/lib/session-export-goldens.test.ts`.
- It renders every golden `.md` with the app's `marked` and asserts:
  - every `## User` / `## Assistant` heading line is an `<h2>`;
  - every tool block (`<details>` followed by its `Tool call` / `Result` summary) is a `<details>` element, opened and closed;
  - outside `<pre><code>` and `<code>` there is no `<!--` and no tag outside an allow-list (the export's own and Markdown's);
  - the document ends with a paragraph.
- The goldens load through Vite's `?raw` import, so the test needs no Node typings.
- The documentation is in the test file's header and in `docs/architecture.md`.

### Numbers after round 4

- `cargo test -p kelpie -j 2`: 233 passed, 0 failed, 6 ignored. `session_export` has 23 tests and 1 ignored bless helper.
- clippy `-p kelpie -p kelpie-proto --all-targets`: clean.
- The first combined clippy + test run ran out of memory while `pnpm build` ran beside it. The tests were re-run alone and passed.
- Frontend:
  - `pnpm typecheck`: ok.
  - `pnpm test`: 514 passed, including the 4 new golden render checks.
  - `pnpm i18n:check`: 0 missing, 0 problems.
  - `pnpm build`: ok.
- Bundle gate: entry 326.29 kB gz, limit 338.60: ok. pulldown-cmark is Rust-side only.
- `scripts/e2e/session_export.py`: 28/28 in two consecutive runs on the rebuilt debug app. Test daemon, driver and scratch data were removed; only the user's own installed Kelpie stayed running, untouched.
