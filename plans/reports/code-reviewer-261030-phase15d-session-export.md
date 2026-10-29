# Code review: 15d Session export (`git diff 60f6913 dc3f15d`)

Branch `phase15/session-export`, commit dc3f15d, worktree `D:\kelpie-par`. Review only; no source edited.

## Verdict

**One blocking finding (H1).** Tool inputs are redacted *after* they are turned into pretty JSON. JSON escaping (`\n`, `\t`, `\"`) defeats `redact`'s word-boundary and line-start anchors. As a result, secrets on the second or later line of a Write/Edit/Bash input reach the file unmasked. This was confirmed against the real `kelpie_proto::redact`. Everything else is Medium or Low.

## Checks run

| Check | Result |
|---|---|
| `cargo test -p kelpie -j 2 session_export` (target `D:\kelpie-p9-target`, TMP on D:) | 10 passed, 1 ignored (bless) |
| `pnpm typecheck` | ok |
| `pnpm i18n:check` | 0 missing, 0 problems |
| Scratch probe on D: (a crate depending on `kelpie-proto`) | Ran the real `redact` on `serde_json::to_string_pretty` output. It also checked `Path::join("C:x.jsonl")` on Windows. Kept in `D:\kelpie-review-scratch`: deleting it was blocked, so remove it by hand. |

No e2e and no daemon were run. No real transcripts were read.

---

## High

### H1. Secrets inside multi-line tool inputs escape redaction (BLOCKING)

**Where.** `session_export.rs:415-421` serialises `Event::Call` input with `serde_json::to_string_pretty`. `details()` then calls `redact` on that escaped text (`session_export.rs:445`).

**Why it fails.** Inside a JSON string, a newline becomes the two characters `\n`, so the next line starts with the word character `n`. `redact` cannot match there:

- Every token pattern starts with `\b` (`\bghp_`, `\bsk-`, `\bAKIA`, `\beyJ`…). There is no word boundary between `n` and `g`.
- The env-style rule needs `(?:^|[^A-Za-z0-9_])[A-Z0-9_]*NAME=`, and the `n` blocks it.
- The YAML rule needs a real start of line (`^`).
- The quoted-key rule needs `["']` straight after `=`/`:`, but JSON gives `\"`.

**Confirmed.** The probe used the real `redact` on `to_string_pretty(json!({"file_path": "/w/.env", "content": "# local\nDB_PASSWORD=hunter2hunter2\nghp_0123…012345\npassword: \"Sup3rS3cretPw\"\nAKIAIOSFODNN7EXAMPLE\neyJhbGci…R8U\n"}))`. **Nothing was masked.** Neither was `{"command": "cat <<EOF > .env\nSTRIPE_SECRET=whsec_abcdef123456\nEOF"}`. Redacting the raw `content` string masks all five.

**Who is affected.** This is the most common way secrets enter a transcript:

- Claude `Write`/`Edit`/`MultiEdit` of `.env`, config or key files (`content`, `old_string`, `new_string`).
- Bash heredocs and multi-line scripts.
- Codex `shell` (`command: ["bash","-lc","…multi-line…"]`).
- OMP `edit`/`write` arguments.

The summary hint is safe, because it redacts the raw string (`hint()`, line 378). So is Codex `apply_patch`, because its input is a raw string. The fenced body leaks.

**Scenario.** The agent writes `.env` with `PORT=8080\nDB_PASSWORD=…\nSTRIPE_SECRET_KEY=…`. The user exports and shares the transcript, and lines 2 onward appear in clear text.

**Fix.**
1. Walk the `Value` and redact every string leaf (and keys) before serialising.
2. Keep the post-serialisation `redact` as a second net.
3. Add a regression test with a multi-line Write/Bash input for each format. The existing `secrets_in_every_part_of_a_turn_are_masked` only uses single-line inputs (tests.rs:90, 97, 104).

---

## Medium

### M1. Tool *outputs* that are JSON text have the same escaping gap
**Where.** `blocks_text`/`codex_output` (`session_export.rs:128-144, 206-219`).

Codex shell output is unwrapped (good). Other outputs are JSON text kept as-is with escaped newlines, and H1 applies to them:

- Claude MCP tool results;
- Codex `function_call_output` whose JSON has no `output` key;
- OMP tool results that return JSON.

**Scenario.** An MCP "read secret / list env" tool returns `{"vars":"A=1\nAPI_KEY=abc123def"}`, and the export keeps the key.

**Fix.** When a result text parses as JSON, redact its string leaves, then also redact the text. Non-blocking: the output is JSON by nature, and the shared helper has the same limit elsewhere. Worth doing together with H1.

### M2. Message text is not structurally contained, so one message can swallow the rest of the document
**Where.** `Doc::message` (`session_export.rs:430-439`) appends user and assistant text verbatim. Only tool bodies are fenced.

**Scenarios:**
- A user pastes code with an opening ```` ``` ```` and no closing fence. Or an assistant reply was interrupted mid-code-block. Every later `## User` / `## Assistant` heading and `<details>` block renders as code.
- A message line starting with `<details>` (pasted GitHub issue template) opens an element that folds everything after it.
- `<!--` with no `-->` starts a CommonMark HTML block that runs to the end of the document, so the rest is hidden.
- `<script>`, `<style>`, `<pre>` or `<textarea>` at a line start consume content until their close tag.

The fixture only covers inline fences on one line (`claude.jsonl:13`).

**Fix.**
1. Track fence state per message (opener length and character). If the message ends inside a fence, append a matching closer.
2. Outside fences, backslash-escape a leading `<` on lines that would start an HTML block (`<!--`, `<details`, `<script`, `<style>`, `<pre`, `<textarea`, `</details`).
3. Add golden or unit cases.

Non-blocking, but the spec explicitly asks for structural safety.

### M3. Abandoned branches are exported as if they happened in sequence
**Where.** `omp()` / `claude()` emit in file order.

- OMP sessions are trees (`id`/`parentId`, see `omp.jsonl:3-11`). After a `/tree` jump or fork, the abandoned branch's turns and the new branch appear back to back, with no marker.
- Claude rewinds (`parentUuid`) behave the same. Kelpie's own chat replay of Claude history is also file-order, so Claude is at least consistent with the chat view. OMP history in the chat view comes from OMP itself and follows the active leaf.

**Scenario.** The user tries approach A, rewinds and does B. The export reads "did A, then did B", and a shared transcript misstates what happened.

**Fix.** First pass collects `(id, parentId)` (cheap). Walk from the last message entry to the root, then emit only entries on that path in a second pass. At minimum, mark the branch points. The developer documented this as a follow-up. Non-blocking.

---

## Low

### L1. The target is canonicalised before the device-path guard
**Where.** `export_to` (`session_export.rs:582`) calls `target.canonicalize()` before the `device_path` check, which only runs inside `write_file_atomic` (`config_transfer.rs:704`).

On Windows, `canonicalize` opens the path (`CreateFileW`), so a `\\.\pipe\…` target connects to that pipe before being refused. The comment at `config_transfer.rs:305` says exactly this. The whole transcript is also rendered before the refusal.

Only reachable with an IPC-supplied path: the save dialog does not return device paths.

**Fix.** Check `config_transfer::path_kind(path) == PathKind::Device` first in `export_to`, reusing the shared classifier.

### L2. An id containing `:` escapes the agent folder through `Path::join` (Windows)
**Where.** `valid_id` (`agent_history.rs:417`) allows `:`. `resolve_in` builds `format!("{id}.jsonl")` and joins it (`session_usage.rs:822-828`).

On Windows, joining a path with a drive prefix and no root *replaces* the base. The probe confirmed `root.join("C:x.jsonl")` gives `C:x.jsonl`, which is relative to the process's current directory on drive C.

**Scenario.** An IPC call with id `C:notes` exports `C:<cwd>\notes.jsonl`.

Real history ids come from file stems, which cannot contain `:`, so this is only reachable through IPC. Codex and OMP only match on suffixes and are safe.

**Fix.** Pre-existing in the shared resolver. In it, reject `:` or require the joined path to have a single `Normal` component.

### L3. `home` from IPC is not checked against `foreign_path`
**Where.** `locate` → `home_or(home, …)` (`session_export.rs:561-562`).

A `\\host\share` home makes `read_dir` open an SMB connection, sending the user's NTLM credentials. This is the same pre-existing pattern as `session_usage`.

**Fix.** Refuse `config_transfer::foreign_path(home)`.

### L4. Edge cases in `write_file_atomic` (shared helper, now reused)
**Where.** `config_transfer.rs:712-717`.

- A failed `std::fs::write` (disk full) leaves the partial `.{name}.{pid}.tmp` behind. Only a failed rename cleans up.
- `fs::write` follows a symlink pre-planted at that predictable name. This needs local write access to the folder.
- Two different rows exported to the same file at once share one temp name. The frontend's `running` set is per conversation, so one export fails or produces mixed content.

The rename itself is safe: it replaces a symlink rather than following it.

**Fix.** Remove the temp file on any error. Create it with `create_new` and a random suffix.

### L5. `redact` runs over the full tool output before the 4,000-char cut
**Where.** `session_export.rs:445-446`.

Order is correct: redact runs before the cut, and a split PEM or token cannot half-leak. But every tool result (up to a 32 MiB line) passes through about 16 regex passes, each allocating a copy, even though at most 4 KB is kept. A long session with many large `Read` results costs seconds of CPU and several times the line size in memory.

**Fix.** Pre-cut at a **line** boundary (the first `CUT_LINES + 1` whole lines), never mid-line, then redact, then `cut`. This keeps the no-half-secret guarantee.

### L6. Header "Ended" can be misleading
**Where.** `Meta::time`, line 80.

- It is taken from any line with a timestamp, including hidden ones: in the golden file it is the `[Request interrupted…]` marker.
- When `truncated`, it is the last line *read*, not the end of the conversation.

The timestamps are UTC with `Z`, so they are unambiguous. Cosmetic.

### L7. Toast lines run together
**Where.** `session-export.ts:42` joins lines with `"\n"`, but `.toast` (`styles.css:528`) has no `white-space: pre-line`.

The result reads "Exported to X 1 unreadable transcript line was left out." Use `" "` with sentence punctuation, or add `white-space: pre-line`. Cosmetic.

### L8. Header values are not reduced to one line
**Where.** `header()` (`session_export.rs:480-488`): `Started`, `Ended` and the model list are redacted but not `one_line`d.

A transcript value containing a newline could inject header lines. Transcripts are self-authored, so the risk is low. Wrap them with `one_line` like the title and folder.

### L9. Newer Codex injected user items may show as user turns
**Where.** `codex_injected` (`agent_history.rs:498-511`).

The list lacks markers such as `<turn_aborted>`, `<user_shell_command>` and `<skill>` that recent Codex versions record as `role:"user"` items. **Verify against the current Codex rollout format.** If confirmed, add them; this would also fix the history list, which shares the list.

---

## Verified OK

- **Redaction coverage of headers and labels.** Title, project folder, session id, timestamps, models, tool names (call and result summaries), hint, user and assistant text, and tool input and output all pass through `redact`. The file name in the UI (`<agent>-<id8>-<day>.md`) carries no title.
- **Images and attachments** are named by MIME only. Data URLs, http `image_url`s and base64 are never emitted.
- **Redact runs before every cut**: `details()` at 445-446, `hint()` at 378-382, and the title before `one_line`. Per-piece redaction keeps an unterminated PEM block from masking the rest of the document.
- **No transcript-internal path is ever opened.** Claude's `cwd` goes through `claude_project_dir`, whose slug maps every non-alphanumeric character to `-`, so it cannot traverse. Codex and OMP match by suffix inside their root. `..` and separators fail `valid_id`.
- **Save target.**
  - A relative path is refused.
  - The transcript itself is refused (by canonical path, symlink-aware).
  - Device paths are refused, though too late (see L1).
  - Network targets are allowed, consistent with config-transfer (the user picked them).
  - The rename does not follow a symlink at the target.
- **`KELPIE_AGENT_CONFIG_HOME` gating.** It is gated on `KELPIE_E2E` by environment only, not by build type. That matches `agent_config::Roots::from_vars` (`agent_config.rs:100`). Production already honours `CLAUDE_CONFIG_DIR`/`CODEX_HOME`, so this adds no new way to redirect a production build. Acceptable as is.
- **Bounded reading.**
  - `next_capped` stops buffering at `max+1` and drains the rest without keeping it.
  - It returns an unterminated last line, so a live session's partial line is counted as skipped.
  - The body overshoots by at most one line's events.
  - It runs on `fs_ops::blocking`.
- **Fences.** Tool bodies use a fence longer than the longest backtick run (minimum 3). The `<summary>` text is HTML-escaped.
- **Claude filtering** matches `applyClaudeMessage`, and is stricter: it also drops `isSidechain`, `<synthetic>`, `Caveat:` and the interrupt marker. Codex skips `developer` messages, reasoning and `event_msg` duplicates. OMP skips `synthetic` messages and keeps narration.
- **Frontend.**
  - The export module is a lazy `import()`.
  - The button is shown only for claude/codex/omp.
  - Concurrent clicks on one row are deduped.
  - The picker override is exposed only through `__kelpie` (dev or e2e).
  - The vi strings are present.

## Test gaps

1. Multi-line and nested JSON tool inputs and outputs containing secrets (H1, M1). This is the blocking gap.
2. Unbalanced fences or HTML-block openers in user and assistant text (M2).
3. OMP and Claude branch structure (M3). Codex `compacted` lines, Claude `compact_boundary` lines, newer Codex injected tags (L9).
4. A device-path target (`\\.\pipe\x`) refused before any open (L1). A write failure leaves no temp file (L4).
5. A corrupt or truncated `.zst` rollout. `next_capped` returns `None` on a read error, so the export ends silently without the truncation note.

The golden files are meaningful. Between them they exercise:
- the filtering rules;
- CRLF→LF;
- the 40-line cut;
- the error flag;
- the image placeholder;
- a partial line;
- the `.zst` path (through `export_to`);
- seeded secrets in every part of a turn.

They just don't contain the multi-line cases above.

## Recommended order

H1 (blocking), then M1 alongside it. Then M2 and L1 (both small). M3 and the rest can follow.

---

Status: DONE_WITH_CONCERNS
Summary: The export is well bounded, redacts every text piece before any cut, and finds the transcript safely. However, tool inputs are redacted after JSON pretty-printing, which lets secrets on later lines of multi-line inputs (Write/Edit of `.env`, heredocs, Codex shell scripts) through unmasked; confirmed with the real `redact`.
Concerns/Blockers: H1 blocks the merge. M1 (JSON tool outputs) should be fixed with it. M2 (Markdown structure breakout) and M3 (OMP branch order) are recommended before release.

---

# Round 2: `git diff dc3f15d c756237`

Commit c756237, "fix(history): mask secrets inside structured tool text in Markdown exports".

## Verdict

**No blocking findings.**
- H1 and M1 are fixed for every realistic path.
- L1, L2, L3, L4, L6, L8 and L9 are fixed without regressions in the shared callers.

Some residual issues remain, all non-blocking:
- **R2-M1:** M2 is only partly fixed. An HTML opener behind a blockquote or list marker still breaks the rendered document. Confirmed with `marked`.
- **R2-L1:** M3's branch walk silently drops the start of an OMP conversation when one line in the middle is unreadable.
- **R2-L2 to R2-L4:** a few small redaction residues and a toast height issue.

## Checks run (round 2)

| Check | Result |
|---|---|
| `cargo test -p kelpie -j 2 --lib` (target `D:\kelpie-p9-target`, `CARGO_INCREMENTAL=0`, TMP on D:) | 226 passed, 0 failed, 6 ignored. Includes config_transfer, session_usage, agent_history and the 16 session_export tests. |
| `node -e` with `marked` (the app's own Markdown dependency) on crafted samples | See R2-M1 and the fence notes. |

No e2e, no daemon, no real transcripts.

The worktree was clean at c756237. Note that c756237 also committed this report file. The round-2 section below is an uncommitted working-tree change.

## 1. H1 / M1: leaf redaction (fixed)

**Inputs.** `redact_value` (`session_export.rs:349-356`) redacts every string and every key on its own. The result is serialised and the text redacted again by `tool_text`, as a second net. Specific cases:

- **Key/value secrets** such as `{"password":"hunter2…"}`: a leaf alone has no name context, but the second net's quoted-key rule catches it after serialisation. The round-1 test covers this, and it still passes.
- **Nested arrays and objects** are handled by the recursion.
- **A secret used as a key** is masked by `redact(k)`. If two keys mask to the same text, one is dropped: data loss, not a leak.
- **String-typed tool input** (`Value::String`) goes through `json_text` when it is JSON. Otherwise it stays raw text, which `redact` handles natively (Codex `apply_patch`, raw arguments).
- **The hint** still redacts the raw string.

**Outputs.** `json_text` (`session_export.rs:358-366`) parses a JSON object or array, redacts its leaves and pretty-prints it. It is applied to every `Event::Result`, including Codex output after the `output` field is unwrapped. So `cat secrets.json`, MCP results and OMP JSON results are covered.

**The new test** (`secrets_on_later_lines_of_structured_tool_text_are_masked`) is meaningful: it asserts that the escaped form alone leaks, then that the leaf pass masks it.

### Residues (R2-L2, Low, non-blocking)

- **Numbers as secrets.** `{"password": 12345678}` keeps its number. `redact_value` leaves numbers alone, and the second net misses it: the quoted-key rule needs a quote before the value, and the YAML rule needs the line to start with the key, not `"`. Numeric PINs and passwords are rare. To fix, when serialising, redact a number whose parent key matches the secret-name list, or stringify numbers under such keys.
- **JSON-like output that does not parse whole** stays raw, so the JSON-escape gap from round 1 still applies to it:
  - NDJSON (`jq -c`, JSON log lines);
  - JSON after a text prefix (`Response:\n{…}`);
  - two text blocks each holding JSON, which `blocks_text` joins with `\n`;
  - nesting deeper than serde's 128-level limit.

  A cheap improvement is to also try `json_text` on each line of a multi-line output that starts with `{`/`[`.
- **Number rewriting.** Pretty-printing re-serialises numbers: integers above u64 become f64 (`1.2345678901234568e22`) and keys are sorted. This is cosmetic but alters displayed data.
- **Windowing.** `json_text`/`redact_value` redact the whole structure before the L5 window applies. This is CPU and memory only, and is bounded by `MAX_LINE`.

## 2. M2: `contain` (partly fixed)

**What works.**
- Top-level fences of either character, any length, with up to 3 spaces of indent, are tracked and closed.
  - Openers and closers follow CommonMark: a closer must be at least as long, with no info string; a backtick info string may not contain a backtick.
  - With `marked`, `~~~~ / ~~~~~ / ~~~` (re-opened, then closed by `contain`) and an indented closer `` ```` `` both behave.
- Fences inside list items and blockquotes (`- ```…`, `> ```…`, `1. x` with a 4-space indented fence) are not tracked by `contain`. That is fine: CommonMark and `marked` close them when the container ends, so they never reach the next heading or `<details>`.
- `contain` runs after `redact`. Each message is contained on its own.

**R2-M1 (Medium, non-blocking): HTML openers inside containers are not escaped.**

`contain` only escapes a line whose first non-space character (at most 3 spaces) is `<` (`session_export.rs:419-424`). An HTML block can also start:
- after a blockquote marker: `> <!--`;
- after a list marker: `- <textarea>`;
- in a list-item continuation indented 4 or more spaces: `- x\n\n    <!--`. Its relative indent is 2, so it is an HTML block, not indented code.

The renderer emits these verbatim.

Confirmed with the app's `marked`: `"> quoted mail\n> <!-- tracking pixel"` followed by the usual `## Assistant` / `<details>` block renders as `<blockquote><p>quoted mail</p>\n<!-- tracking pixel</blockquote>\n<h2>Assistant</h2><details>…`. A browser, or GitHub's HTML sanitiser, then treats everything after `<!--` as a comment, so the rest of the export disappears. `- item\n  <textarea>` similarly makes the rest of the document textarea content (RCDATA).

`<details>`/`</details>` inside a container is harmless: the browser closes it at `</blockquote>`/`</li>`.

**Scenario.** A user pastes a quoted email or HTML snippet in a `>` quote. Or a crafted message does so on purpose. The shared file then shows nothing after that message.

**Fix.** Outside fences, before the existing check, strip any run of container prefixes: `>`, list markers (`-`, `*`, `+`, `\d{1,9}[.)]`) and any amount of spaces or tabs. Apply the `<` test to what remains and insert `\` there. Escaping inside an indented code block only adds a visible `\`, which is acceptable. Alternatively, escape `<!--` and the type-1 tags (`<script`, `<pre`, `<style`, `<textarea`) wherever they appear outside fences and inline code.

**Cosmetic.** An autolink or inline tag at a line start (`<https://…>`, `<kbd>…`) is now shown literally with its `<`. Acceptable.

## 3. M3: OMP active branch (fixed, one gap)

- **Cycles.** `while path.insert(at.clone())` stops on the first revisit (`session_export.rs:627-632`). Tested.
- **Memory.** A map of id → parent id for every entry, a few dozen bytes each. A second full read for OMP only. Fine.
- **`activeLeafId`.** It could not be checked against a real OMP session under the no-real-data rule. If OMP never writes it, the leaf is the last entry, which is how OMP reloads a session. Harmless either way.
- **Missing parent: R2-L1 (Low, non-blocking).**
  - **What happens.** When the walk reaches a parent id the first pass did not record, it stops, and every earlier entry is excluded from the export (`render_lines` skips ids not in `keep`). That happens when a line in the middle is:
    - over 32 MiB (an inline base64 image in an OMP user message);
    - malformed;
    - an entry whose `Link` fails to deserialise (e.g. a non-string `parentId`).
  - **Result.** The export silently begins mid-conversation. The only sign is the "1 line could not be read" note.
  - **Fix.** If the walk ends at `Some(parent)` that is not in the map, return `None` (file order), or keep every entry up to that line. Add a test.
- **Claude in file order: sound.**
  - A leaf walk over `parentUuid` would stop at a compaction boundary: the line after compaction has `parentUuid: null` and only `logicalParentUuid`, so all pre-compaction history would be lost.
  - It would also drop sibling tool results and progress lines.
  - File order matches the chat view's replay.
  - The cost is that rewound branches appear back to back. This is documented. Accept as is.

## 4. Lows

- **L1 (fixed).** `path_kind(path) == Device` is now checked first in `export_to` (`session_export.rs:727-730`), before `canonicalize` or rendering. Tested with `\\.\pipe\…` and `//./pipe/…`.
- **L2 (fixed).** `file_id_ok` means `valid_id` with no `:` and no `..`.
  - **Legitimate ids still work:** Claude and Codex ids are UUIDs and OMP ids have no `:` or `..`. The existing session_usage resolver tests (UUIDs, `o1`, `latest`) pass.
  - **`agent_claude_transcript` is tighter.** It used to refuse only separators; it now needs `valid_id`. Claude file stems are UUIDs, so nothing legitimate is lost.
  - **`accounts.rs:277`** also goes through `resolve_in` and gets the same check. Fine.
- **L3 (fixed).** `local_home` is applied to export (an error), `session_usage`, `agent_latest_session` (`Ok(None)`) and `agent_claude_transcript` (an error).
  - Local and mapped-drive homes (`Z:\…`) are classified as Local by text and still work.
  - An account home configured as a UNC path now silently stops showing usage and latest-session information. This is the intended trade-off, but worth a line in the account settings docs.
- **L4 (fixed, no regression).** The only other caller is the config export command (`config_transfer.rs:799`), plus the config_transfer tests, which pass.
  - The changes are: create-new temp file with a unique name, `sync_all`, a close before the rename, and removal on every failure. Default permissions are the same as `fs::write`, and the rename semantics are unchanged.
  - **Test gap:** `a_failed_write_leaves_no_temporary_file` does not cover a failure *after* the temp file exists (write or rename failure). Its four-thread race relies on `MoveFileExW` concurrency on Windows: it passed here, but watch it for flakiness in CI.
- **L5 (fixed).**
  - `window` keeps at most 41 whole lines and 8,096 characters. A secret that starts inside the 4,000 characters `cut` keeps is redacted whole while it is under 4,096 characters, and an unterminated PEM block is masked to the window's end.
  - The "omitted" count adds the lines after the window.
  - Tested, including the straddling token. Residue is under R2-L2 (structured content is redacted whole).
- **L6, L8 (fixed).** "Ended" now comes only from lines that exported something, and header values are reduced to one line.
- **L7: R2-L3 (Low, non-blocking).** `.toast > span { white-space: pre-line; }` applies to every toast.
  - **Who is affected.** Toasts that pass raw multi-line error text:
    - `git-panel.tsx:75` (push/pull/sync/fetch through `git_sync::git`, which returns the *whole* stdout+stderr, `git_sync.rs:36-38`);
    - `git-panel.tsx:285/301`;
    - `automation-events.ts:44/54`, `file-tree.tsx`, `sidebar.tsx` and others using `String(e)`.
  - **Result.** A failed push with a verbose pre-push hook can print hundreds of short lines. Before, these collapsed into a wrapped paragraph; now each is its own row. The toast has no `max-height` (`styles.css:528`), so it grows upward past the top of the window. Short multi-line errors (rejected push, hints) read better than before.
  - **Fix.** Add `max-height` (e.g. 40vh) and `overflow: auto` to `.toast > span`, or keep only the last few lines at the git call site, as `git-panel.tsx:96` already does.
- **L9 (fixed).** Effect on the history list: a typed prompt that really starts with `<skill>` would be hidden. Negligible.

## Round 2 findings

| ID | Severity | Blocking | Summary |
|---|---|---|---|
| R2-M1 | Medium | No | `contain` misses HTML openers after `>`, list markers or deeper list indentation. `> <!--` hides the rest of the rendered export (confirmed with `marked`). |
| R2-L1 | Low | No | The OMP branch walk drops all earlier entries when a middle line is unreadable or unparsable (broken chain). Fall back to file order. |
| R2-L2 | Low | No | Redaction residues: numeric secret values, and JSON that does not parse whole (NDJSON, text-prefixed JSON, multiple JSON blocks, depth over 128). Number rewriting in pretty-printed outputs is cosmetic. |
| R2-L3 | Low | No | `pre-line` toasts with long multi-line git or hook errors grow off-screen. Add `max-height` and `overflow`. |
| R2-L4 | Low | No | Test gaps: write failure after temp creation; container-prefixed HTML; OMP broken-chain fallback. Possible Windows flakiness in the concurrent-writer test. |

---

Status: DONE_WITH_CONCERNS
Summary: Round 2 fixes H1/M1 (leaf redaction plus a second net, covering inputs and JSON outputs) and every round-1 Low without regressions in the shared writer, resolvers or callers; 226 kelpie lib tests pass. The M2 fix still misses HTML openers inside blockquotes and lists, so `> <!--` can hide the rest of the rendered export.
Concerns/Blockers: No blocking findings. R2-M1 is recommended before release. R2-L1 to R2-L4 can follow.
