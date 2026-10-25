# Completion — Phase 9: Composer & context attach

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-09](../261023-0048-multi-agent-ide-roadmap/phase-09-composer-context.md)

## Delivered
- **Composer** (`composer.tsx`, lazy): `Ctrl+Shift+I` / pane pencil over any session (shell, agent TUI, chat view); Enter sends, Shift+Enter newline, Esc keeps the draft; per-project prompt history (Up / Down, palette `!`). The shortcut is always bound, so it never opens DevTools.
- **Mentions** (`mentions.ts`): files and folders (folder entries added to the search index), `@selection` (editor, diff, terminal; also *Send Selection to Agent…* in their menus), `@diff` (staged + unstaged, warns on a clean tree), `@terminal:<session>` (≤ 200 lines), `@issue[:key]` (Linear / Jira / GitLab; GitHub integration has no issues), `@note`. Terminal and issue text and terminal selections pass `redact`. Snippets > 200 lines or > 64 KB total become a path reference.
- **Delivery safety**: through `sendToSession` / `typeInto` and the daemon sanitiser; Claude, Codex and Gemini get `@path` in their own form; other programs get fenced, labelled content. Shells get paths only, quoted for pwsh / bash / cmd (`./` before a leading `-`), and nothing holding a mention or picture is submitted; names with control or bidi characters are refused (typed and dropped). Terminals that have not enabled bracketed paste take no multi-line text; paste markers kept while the view is detached. Remote sessions: POSIX quoting, no local temp paths.
- **Pictures**: paste / drop → temp file path for a TUI (temp folder swept after three days), image block (≤ 5 MB) for a chat; network paths and symlinks refused.
- **Search**: nucleo ranking on a capped worker pool (half the cores, ≤ 4).

## Verification
- Merged with phases 7, 12 and the chat view (rebased onto 67446a5): cargo kelpied 183, kelpie 114, kelpie-proto 14; clippy `-D warnings` clean; vitest, typecheck, i18n 0 missing.
- GUI e2e `composer.py` (hostile names `$(…)`, `;`, `&`, backtick, quotes, `.ps1`, `.exe` inert and unsubmitted; `\x1b[201~` file inert; pictures; Codex submits; send-selection menus; history): 85/85 twice by the developer and again on the merged code, plus regressions (notify, finish, checkpoints, context_cost, attention). Bundle entry ≈ 318 kB gz.
- Fuzzy p95 on 51k entries, quiet machine: 16.8–21.4 ms (target 30 ms); under full CPU load 166–250 ms.
- [Review](code-reviewer-261024-1900-phase9-composer.md): C1, H1–H3, M1–M6, R1–R3 and the cheap Lows fixed; L7 left on purpose.

## Not done / follow-ups
- Remote Windows hosts get POSIX quoting (the host OS is not recorded).
- M3 (remote) and M6 (Esc during a slow send) are unit-tested only; no real OS drag-and-drop or real vendor binaries in e2e.
- Search under full CPU load exceeds 30 ms (a persistent pool would help).
