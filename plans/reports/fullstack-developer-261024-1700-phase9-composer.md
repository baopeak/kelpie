# Phase 9 report: Composer and context attach (after review fixes)

Worktree `D:\kelpie-p9`, branch `phase9/composer` (base b64543f). Not committed; plan.md and phase status untouched.
Builds used `CARGO_TARGET_DIR=D:\kelpie-p9-target`; nothing in `D:\Works\Kelpie` was read, built or run. The reviewer's report is `code-reviewer-261024-1900-phase9-composer.md`.

## What the composer does (unchanged scope)

`Ctrl+Shift+I` / pane pencil opens a floating composer over any session (shell, agent TUI, chat). Enter sends, Shift+Enter newline, Esc closes keeping the draft, Up/Down walk the project's prompts, palette `!` searches them. Mentions: files and folders, `@selection`, `@diff`, `@note`, `@terminal:<session>`, `@issue[:KEY]`; pictures pasted or dropped; "Send selection to agent" from the editor, diff and terminal menus; folders in the search index. Pure logic is in `lib/mentions.ts`; the UI is the lazy `components/composer.tsx`.

## Review fixes

| Finding | Fix |
|---|---|
| C1 names reach the shell and are submitted | A shell or `custom` program never gets content: paths only, written as one word for its shell (`shellQuote`: bare when plain, else single quotes; `'` doubled in pwsh, `'\''` in POSIX; cmd refuses `% ^ ! & \| < > ( ) "`; control, line-separator and bidi characters refused for every shell). Shell style comes from the session (`shellArgs`/program; pwsh default on Windows, POSIX elsewhere). Once the text holds a mention or a picture it is left at the prompt, never submitted. Refused names block the send with a message (no "Enter again" bypass). Pictures and temp-file paths go through the same quoting. The e2e workaround is reverted (`@inject.txt` alone) and hostile fixtures added. |
| H1 inline content without bracketed paste | For every terminal session `term.modes.bracketedPasteMode` decides: without it nothing is inlined (an agent still starting gets temp-file paths) and a text with a line break is refused with a reason. Covers "Send selection -> new session, send before ready". |
| H2 `typeInto` fallback dropped markers | New `pasteInput(raw, bracketed)` in `typed-text.ts` (cleaned text, inside `ESC[200~ ... ESC[201~` when the view says the program asked for it); `typeInto` uses it when the view is detached. Unit-tested. |
| H3 terminal selection not redacted | `Selection.source = "terminal"` (terminal pane and its menu); `@selection` runs `redact` for it, not for editor selections. Unit and e2e checks. |
| M1 per-agent `@path` forms | `pathReference(kind, path, windows)`: Gemini follows its own escaping (Windows: quote when `[\s&()[\]{}^=;!'+,`~%$@#]`; POSIX: backslash-escape, no quotes); Claude/Codex quote on whitespace; the typed `@"..."` form also quotes names the parser would cut (`(`, `;`, trailing `,` ...). |
| M2 `@diff` | New `git_context(kind "worktree")`: `git diff HEAD` (staged and unstaged) plus untracked files, empty when nothing changed; clean tree now warns "no uncommitted changes". |
| M3 remote sessions | No local temp paths: text over the caps goes in as its last <= 200 lines / 64 KB (`(last N lines)` label); for a shell it is refused with a message; `@terminal` reads 200 lines so it inlines. Pictures already refused. |
| M4 fenced content into a shell | Gone: shells never get content (see C1). |
| M5 chat picture size | Chat pictures <= 5 MB each (paste, drop, and again at send, where bytes of a picture dropped on a terminal are read lazily); TUI path references keep 25 MiB. |
| M6 Esc during a slow send | `disposed` flag set on cleanup; the send bails before delivery. The unsent text stays as the draft. |
| L1 | `fs_read_image` refuses `\\`/`//` paths and symlinks and reads at most 25 MiB + 1 through a bounded reader. |
| L2 | Temp folder swept at app start and on each write; README privacy line mentions it. |
| L3 | Workers = half the cores, 1..4; chunks are claimed from a shared counter and the caller works too (numbers below). |
| L4 | Architecture doc says composer text is cleaned by the front end only (daemon `paste:true` covers launch prompts). |
| L5 | Names with control/bidi characters are not offered by completion and are refused for shells. |
| L6 | `@terminal:<ref>`: exact label, else a unique id prefix (>= 6 chars), else a unique label prefix; an ambiguous name names no session (warns). |
| L7 | Not done: `formatNotes` and `notesPrompt` (diff view) differ on purpose and `parity.py` checks the diff-view text. |
| L8 | Doc comment moved back above `QueuedMessage`. |
| L9 | Covered by M5 (lazy read at send). |
| L10 | Prompt history is removed with its project. |
| L11 | Verified by hand with real key events (SendKeys, isolated daemon) on the app with no session: before the fix Ctrl+Shift+I opened WebView2 DevTools. `composer.open` is now always bound (no `when`), with a "Focus a running session..." toast when none is live; DevTools no longer opens (6 runs; one earlier run opened it because the keys arrived before the keymap was installed, so the check waits 25 s). The chord stays, no other shortcut needed. |

## Re-review fixes (R1-R3)

| Finding | Fix |
|---|---|
| R1 dropped name with a line break | `drop()` goes through `droppedFileMention`, which refuses names with control, line-separator or bidi characters (toast). The shell no-submit rule is now `looksLikeMention(text)` (any `(^|\s)@`, parsed or not) or a picture, so text the parser cannot read is still never submitted. Vitest: `parseMentions(pathMention("x $(touch pwned)
.txt"))` is `[]`, the drop helper returns null for it, `looksLikeMention` is true; plus safe-name and non-mention cases. |
| R3 names starting with `-` | `shellQuote` prefixes `./` (`.\` for cmd) before quoting: `./-rf`, `'./-a b'`. |
| R2 remote quoting | A session on another machine is taken for a POSIX host: shell style `posix`, Gemini form non-Windows, for mentions and pictures. The local OS says nothing about the host; a remote Windows host is not detected (no OS is recorded for it), so its names with `'` or `\` would be quoted the POSIX way. |

Re-run on the new build: `pnpm typecheck` clean, `pnpm test` 215 pass (+3), `pnpm i18n:check` 0 missing, `composer.py` 85/85 once. The drop path itself is covered by the pure helper's unit tests; the e2e still emits the drop event only for a picture and a safe file.

## Checks (final build)

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 17 files, 212 tests pass (mentions 54, prompt-history 8, typed-text +2, picture tests +8) |
| `pnpm i18n:check` | 1534 keys, 1598 translated, 0 missing |
| `cargo test -p kelpied -p kelpie -p kelpie-proto` | kelpie 65, kelpied 125, proto 14; 0 failed (search tests re-run after the last rank change: 6 pass) |
| `cargo clippy -p kelpied -p kelpie -p kelpie-proto --all-targets -D warnings` | clean |
| `scripts/e2e/composer.py` (free `--port` and `--native-port`) | 85/85, twice in a row on the final build |
| `scripts/e2e/context_cost.py` | 26/26 |
| `scripts/e2e/attention.py <kelpied.exe>` | 12/12 |
| `perf_gate.py --label phase9 --metrics bundle --runs 1` | entry 328.64 KB gz (limit 338.60; baseline 323.6), 229 lazy chunks 1676.08 KB |

New e2e cases: hostile names (`x$(New-Item ...).txt`, `a;New-Item ...;.txt`, `a&...&.txt`, a backtick, `it's.txt`, `deploy.ps1`, `setup.exe`) typed as mentions in PowerShell: each is one single-quoted word at the prompt, nothing submitted, no marker file anywhere; `@inject.txt` alone not submitted; non-agent programs (bracketed or not) get the path only, unsubmitted; an agent without bracketed paste (stand-in `opencode`) gets a path for `@diff` and a multi-line text is refused with its reason; staged and unstaged changes both in `@diff`; clean repository warns; terminal selection masked; ambiguous `@terminal:fc` warns; 6 MB picture refused for a chat. The Codex check retries up to 3 times because the stand-in judges the pause by arrival time, which shrinks when the machine is starved (it failed twice under the other developer's builds, passed on retry-free runs when quiet; the final runs needed no retry as far as the log shows).
Names with a newline cannot be created on Windows; refusal of control characters is covered in `mentions.test.ts` (`shellQuote`, escape character in a mention). H2 (detached view) is covered by the `pasteInput` unit tests only, M6 (Esc during a slow send) and M3 (remote) by code and unit tests only: no e2e (no SSH host, no slow lookup to hold the send open).

## Perf (fuzzy, 51 025 entries, release build of the real `rank.rs`, 4-core laptop shared with the other developer's builds)

Same 10 queries x 20 rounds per row. The machine was never quiet during these runs (single-worker p95 is 65-89 ms here against 32 ms the reviewer saw when quiet), so only the relative figures mean anything.

| Condition | 1 worker p50 / p95 | shipped (2 workers) p50 / p95 |
|---|---|---|
| background load only (3 runs) | 19.5-23.9 / 65-88 ms | 17.9-28.8 / 56-107 ms |
| four spinning threads added (all cores busy) | 20.4 / 78 ms | 92 / 166 ms (earlier runs 78-170 / 214-389 ms) |
| earlier with 4 workers, quiet moments | 14-18 / 36-65 ms | 6-8 / 16-29 ms |

Findings: the thread cap and shared-counter chunks lowered the saturated tail (390 -> 166-250 ms p95) but fan-out still loses to one thread when every core is busy (new threads wait for a time slice and the scope waits for them); and with only 2 workers a quiet machine gains less than with 4. The 30 ms p95 gate was met only in the quiet 4-worker runs before the cap (16-29 ms) and is not demonstrated with the capped pool on this machine. A persistent pool that the caller need not wait for would remove the saturated penalty; not done. The ignored test `fuzzy_p95_on_fifty_thousand_files` (release only, `KELPIE_BENCH_TREE=<empty dir>`) still guards the gate.

## Files

Created: `composer.tsx`, `composer.css`, `lib/mentions.ts` + test, `lib/prompt-history.ts` + test, `lib/composer-state.ts`, `locales/vi/composer.json`, `src-tauri/src/search/rank.rs`, `search/tests.rs`, `scripts/e2e/composer.py`, `scripts/e2e/fake_tui_agent.py`.
Modified: `pane-view`, `terminal-pane`, `file-viewer`, `diff-view`, `palette`, `icons`, `lib/commands`, `send-to-agent`, `ipc`, `store`, `types`, `typed-text` (+test), `quick-commands`, `chat/chat-model` + claude/codex/acp adapters (+tests), `src-tauri/src/search.rs`, `fs_ops.rs`, `git_sync.rs`, `lib.rs`, `scripts/e2e/fake_acp_agent.py`, `scripts/e2e/context_cost.py` (`--native-port`), docs (`architecture.md`, `user-guide.md`, `README.md`).

## Environment

Before continuing I checked that no edit was half done (typecheck clean, no stray control characters in the scripts, no untracked strays) and that none of my processes ran. The `tauri-driver`/`msedgedriver` seen afterwards started minutes after my last run, with ports I did not pick: left alone. Scratch outputs, `%TEMP%\kelpie-composer`, `D:\kelpie-p9-target\release` and `debug\incremental` removed; D: has about 14 GB free.

## Unresolved

- Fuzzy p95 under saturation (above): needs a persistent pool or a real quiet-machine measurement.
- Whether Claude's stream-json chat or Gemini's ACP expands `@path` in prompt text (reviewer's open question): not checked; chat views get the `@path` text only.
- Real vendor binaries and a real OS drag-and-drop were not exercised.

Status: DONE_WITH_CONCERNS
Summary: R1-R3 fixed on top of: C1, H1-H3, M1-M6, the cheap Lows and the thread cap are fixed and verified: composer.py 85/85 twice, context_cost 26/26, attention 12/12, vitest 212, cargo tests and clippy clean, bundle 328.64 KB gz; Ctrl+Shift+I no longer falls through to DevTools.
Concerns/Blockers: the 30 ms fuzzy p95 is not demonstrated with the capped pool on this shared machine (fan-out still loses to one thread when all cores are busy); M3 remote handling and M6 are covered by unit tests only; L7 deliberately not done.
