# Completion — Phase 10: Spec workspace

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-10](../261023-0048-multi-agent-ide-roadmap/phase-10-spec-workspace.md)

## Delivered
- **Plan tab** (lazy `plan-panel.tsx`, `spec-index.ts`): detects `plans/*/plan.md` + `phase-*.md`, `specs/**`, `.kiro/specs/**`, `docs/plans/**` (roots created later picked up live; no symlinked or junctioned root or parent followed), phases and tasks with checkbox state and progress. Whole-file `marked.lexer` parse; files over 8 KB parse in a Web Worker with results identical to the main thread (timeout → main thread up to 64 KB). Watcher-driven, debounced 300 ms, nothing while the tab is closed.
- **Manual tick**: changes exactly one byte (CRLF / CR / BOM kept), refused and re-read when the file changed (mtime + checksum), never recreates a deleted file.
- **Run with…** a task: a one-task Run (agent; here / worktree / host) whose prompt references the file and section through the composer path; heading and task text scrubbed (control, bidi, invisible characters, mentions); worktree runs bring only an approved `materialize`; the row shows the run's status.
- **Host runs** (daemon `runs::TaskHost`, API level 14): validated destination / port / config / absolute folder, started through the normal remote launch path, no tmux (the task ends with its agent; non-zero exit or a dropped connection fails it); a session caller can't name a host whatever the param shape; after a daemon restart an interrupted host task is failed and its session released before any client connects; Retry on a host asks first.
- **Remote projects**: Plan tab over ssh (list, read, tick, New spec). Remote writes: constant script with values as positional parameters, exclusive `mktemp` temp file (0600) in the target's folder, size + `cksum` check before the rename, links written through (confined for the Plan tab), owner never changed.
- **New spec** quick command: `requirements.md`, `design.md`, `tasks.md` into `specs/<name>/`, never overwriting, refusing dangling links.

## Verification
- cargo: kelpied 263, kelpie 116, kelpie-proto 19 (host validation and scoping for every param shape, launch argv, no-tmux end rules, restart settling, remote write script under Git Bash incl. planted temp names and placeholder-like file names); clippy `-D warnings` clean.
- vitest 433 (parser fixtures incl. HTML comment / `<details>` / nested fence / reference links, worker equivalence, tick byte-exactness, timeout), typecheck, i18n 0 missing; staged commit built and tested alone in a clean worktree (other sessions' uncommitted work excluded).
- GUI e2e `spec_panel.py` (local + remote project over an ssh stand-in, Run with… here / worktree / host, live tick, refused tick, New spec, symlinked files on the host, worker vs main-thread parse, failed host task + retry): 124/124; regression `best_of_n`, `finish`, `remote` 26/26, `ssh_panes` 43/43.
- [Review](code-reviewer-261025-1730-phase10-spec-workspace.md): H1, H2, M2–M6, N1–N5, F1–F3 and cheap Lows fixed over four rounds; final: no blocking items.

## Not done / follow-ups
- `session.create` still lets a program inside a session open ssh to any saved host (pre-existing; needs a product decision).
- A half-open connection can leave a host agent running for minutes (retry confirmation and wording only); no-tmux and the write script verified under Git Bash, not a real sshd.
- `ssh_hosts.py` keeps one stale expectation (agents no longer start in a host's login folder); run links follow a task's text (no store).
