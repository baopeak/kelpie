# Phase 1 — Auto-update & release channel

## Context
Installers are CI artifacts only (`.github/workflows/ci.yml`); no `tauri-plugin-updater`
(grep: 0 hits). Every later phase only reaches users if updates are one click.

## Requirements
- Signed updates (minisign key in CI secrets) published as GitHub Release + `latest.json`.
- Check on start and every 6 h; status-bar badge "Update ready"; release notes in a dialog.
- Install only on user consent; if any session is *working*, offer "Install when idle".
- Daemon restart on update goes through the existing interrupted → resume path
  (`resumeSessions` setting), never a raw kill.
- Setting: Updates = automatic check / manual only. Channel: stable (beta later).

## Files
- Modify: `app/src-tauri/Cargo.toml`, `app/src-tauri/tauri.conf.json` (updater pubkey, endpoints),
  `app/src-tauri/src/lib.rs` (plugin init), `app/package.json` (`@tauri-apps/plugin-updater`),
  `app/src/components/status-bar.tsx`, `app/src/lib/settings-catalog.ts`, `.github/workflows/ci.yml`
  (release job on tag: build, sign, upload, generate `latest.json`), `scripts/build-release.sh`.
- Create: `app/src/lib/updater.ts` (check / download / install, idle gate).

## Steps
1. Generate signing key; store private key + password as CI secrets; commit only the pubkey.
2. Add plugin + config; release job on `v*` tags for NSIS + DMG (+ AppImage once phase 8 lands).
3. `updater.ts`: check, progress, "install when idle" (watch store session statuses).
4. Before install: ask daemon to persist (`session.list` snapshot already on 30 s timer → force
   a persist call), then let the installer replace binaries; on next start the daemon-version check
   ([store.ts](../../app/src/lib/store.ts) "daemon is older") restarts it.
5. Docs: README Install, user guide "Updates".

## Validation
- e2e `scripts/e2e/updater.py`: local HTTP server serving a fake `latest.json` + signed bundle;
  badge appears, bad signature rejected, install-when-idle waits for a working session.
- Manual: v0.1.0 → v0.1.1 on Windows with 3 running agents; they resume.

## Risk / rollback
- Lost signing key = no updates → back the key up offline. Rollback: disable endpoint in
  `tauri.conf.json`; users keep current version.
