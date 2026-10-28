---
phase: 15
title: "Platform & hygiene"
status: pending
priority: P3
effort: "3d"
dependencies: [1, 2]
---

# Phase 15: Platform & hygiene

## Overview
Independent sub-tasks; each ships on its own. (Splitting `store.ts` was removed from this roadmap by user decision.)

## Requirements
- **15a Linux**: data dir `~/.local/share/Kelpie`, login-shell PATH, keep-awake `systemd-inhibit`, keyring secret-service, notifications; CI `ubuntu-22.04` → AppImage + .deb; updater entry (phase 2).
- **15b Git basics**: branch switcher (refuse when dirty, offer worktree), compact log (last 100), stash list apply/drop with "stash is shared across worktrees" warning.
- **15c Config export/import**: settings, quick commands, layouts, automations, SSH hosts (no secrets), plugins list, agent args/env (values opt-in).
- **15d Session export**: History panel → Markdown of Claude/Codex/OMP/ACP conversations, redacted (phase 1 `redact`).

## Related Code Files
- Modify: `crates/kelpie-proto/src/lib.rs`, `crates/kelpied/src/awake.rs`, `app/src-tauri/Cargo.toml`, `.github/workflows/ci.yml`, `app/src-tauri/src/git.rs`,
  `app/src/components/git-panel.tsx`, `app/src/components/settings/system-settings.tsx`, `app/src-tauri/src/agent_history.rs`, `app/src/components/history-panel.tsx`.

## Implementation Steps
1. 15b, 15c, 15d in any order; 15a last (CI time).

## Tests
- 15a: `cargo test` on Linux CI + smoke e2e under Xvfb (`webkit2gtk-driver`). 15b: extend `parity.py` git section (PowerShell-safe). 15c: e2e round trip into fresh data dir.
  15d: export of a fixture conversation equals a golden file; secrets redacted.

## Perf gate
- No idle cost; Linux smoke meets the global budgets relative to its own baseline.

## Success Criteria
- [ ] Linux build installs and passes smoke; git basics, export/import and session export covered by e2e.

## Risk Assessment
- Linux WebKitGTK rendering differences → run `ui_tour.py` screenshots on Linux before release.
