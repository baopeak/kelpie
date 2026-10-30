# Completion — Phase 15: Platform & hygiene

Date: 2026-10-30 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-15](../261023-0048-multi-agent-ide-roadmap/phase-15-platform-hygiene.md)

Four independent sub-tasks, each merged on its own. No daemon protocol change; `API_LEVEL` stays 16.

## Delivered
- **15b Git basics** (PR #6): branch switcher in Changes (refuses a held branch, a dirty tree incl. untracked, a merge/rebase, and a detached HEAD whose commits no ref reaches; offers a worktree for a dirty tree), last 100 commits, stashes with apply by commit id and drop verified against the list before/after. Inputs validated; keyboard accessible; lazy.
- **15c Config export/import** (PR #7): versioned JSON of settings (explicit allowlist with a classification test), quick commands, layouts, automations, SSH hosts, plugin list, agent args/env (values opt-in). Secrets never written; import preview shows every value that decides what runs behind a separate tick; backup + undo; automations arrive off and unapproved; network/device paths refused; argument lists import whole or not at all; moved SSH hosts lose password, agent forwarding and port forwards.
- **15d Session export** (PR #8): History → Markdown for Claude, Codex, OMP; every string redacted (leaf-first for structured data, escaped/unescaped variants merged by range); containment via `pulldown-cmark` with plain-text fallback; transcript resolved by id under the agent's folder only; atomic write.
- **15a Linux**: XDG data dir (0700), account-shell fallback, `systemd-inhibit --what=idle:sleep` keep-awake, Secret Service keyring, AppImage daemon copy, AppImage/GTK env stripped from every program the app starts, AppImage + .deb via `build-release.sh`, release job and `latest.json` `linux-x86_64`(+`-deb`), CI `ubuntu-22.04` with install checks and Xvfb smoke. Also fixed: Finish checks were stopped with `kill -KILL -<pid>`, which on Linux could kill every process of the user; now `killpg` with a pgid guard.

## Verification
- 15b: cargo kelpie 184, e2e `git_basics.py` 37/37; review 2 rounds.
- 15c: cargo `config_transfer` 26, vitest 510, e2e `config_export.py` 63/63 twice per round (password-store recorder only); review 4 rounds.
- 15d: cargo kelpie 237 (golden files incl. a 93-message adversarial corpus), vitest 515 incl. `marked` render of every golden; e2e `session_export.py` 28/28 twice; review 6 rounds.
- 15a: CI runs 40994932186 and 41001537847 (after review round 2) green on Windows, macOS, ubuntu-22.04 (all Rust suites, frontend, bundle, .deb install, AppImage check, Xvfb smoke 8/8); smoke baseline `scripts/perf/linux-smoke-baseline.json` (634 ms cold start, 4.5 % idle CPU, 506 MB); review 2 rounds, no blocking items.
- Reports: [15b](code-reviewer-261029-phase15b-git-basics.md) · [15c](code-reviewer-261029-phase15c-config-export.md) · [15d](code-reviewer-261030-phase15d-session-export.md) · [15a](code-reviewer-261030-phase15a-linux.md).

## Not done / follow-ups
- Linux: release job only runs on a tag (unexercised); signing key still visible to the bundle step's shell and Tauri's unpinned linuxdeploy download; manual desktop checks (keyring unlock, keep-awake under GNOME/KDE, notifications, AppImage/.deb update, Wayland, arm64); `ui_tour.py` screenshots before the first Linux release; smoke not gated (runner noise ~2×).
- 15c: Low follow-ups in the dev report (mysql `-p` never restorable, header restore without space, filter over/under-matching, glob normalisation in write-failure check).
- 15d: Claude conversations exported in file order; other ACP agents have no readable history; corrupt `.zst` ends silently.
- 15b: remote (SSH) path not covered by e2e.
