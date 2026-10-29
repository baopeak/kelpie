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
