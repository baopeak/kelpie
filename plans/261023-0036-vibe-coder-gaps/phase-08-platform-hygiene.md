# Phase 8 — Platform & hygiene

Four independent sub-tasks; each ships on its own.

## 8a Linux build (G11)
- Unix PTY, Unix socket and process groups already exist for macOS; add Linux cfg paths
  (data dir `~/.local/share/Kelpie`, login shell PATH, keep-awake via `systemd-inhibit`,
  keyring `linux-native`/secret-service feature, notification via plugin).
- CI: `ubuntu-22.04` job → AppImage + .deb; updater entry (phase 1).
- Files: `crates/kelpie-proto/src/lib.rs` (paths), `crates/kelpied/src/awake.rs`,
  `app/src-tauri/Cargo.toml`, `.github/workflows/ci.yml`, README Install.
- Validation: `cargo test` on Linux CI; smoke e2e under Xvfb (`webkit2gtk-driver`).

## 8b Git basics (G10)
- Changes panel: branch switcher (refuse when dirty, offer worktree instead), compact log (last 100,
  graph-less), stash list with "apply" / "drop" and a warning that stash is shared across worktrees.
- Files: `app/src-tauri/src/git.rs`, `app/src/components/git-panel.tsx`, locales.
- Validation: extend `scripts/e2e/parity.py` git section.

## 8c Config export / import (G12)
- Export JSON: settings, quick commands, layouts, automations, SSH hosts (no passwords/keys),
  plugin list, agent args/env (env values opt-in). Import shows a summary and merges.
- Files: `app/src-tauri/src/lib.rs` (`config_export`/`config_import`), `app/src/components/settings/system-settings.tsx`.
- Validation: e2e round trip into a fresh `KELPIE_DATA_DIR`.

## 8d Store split + unit tests (G13)
- Split `app/src/lib/store.ts` (1407 lines) by domain (inbox, sessions, layout state, settings)
  without changing its public API (`store.*` re-exports).
- Add `vitest` for pure modules: `layout.ts`, chat adapters, settings catalog search.
- Validation: `pnpm typecheck`, `pnpm test`, full e2e smoke + layouts + tabs_panes unchanged.

## Risk / rollback
- 8d is a refactor of a hot file → do it when no other phase has open edits to `store.ts`.
