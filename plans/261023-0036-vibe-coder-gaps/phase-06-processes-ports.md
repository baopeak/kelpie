# Phase 6 — Processes & ports per session

## Context
Agents start dev servers, watchers and test runners and leave them behind; ports stay busy.
The daemon owns every session's root PID and already walks process trees
([proc_tree.rs](../../crates/kelpied/src/proc_tree.rs), Job Objects on Windows, process groups on macOS).

## Requirements
- Per session: listening TCP ports of its process tree → chip `:5173` on the tab/sidebar;
  click opens the browser pane (existing localhost routing).
- Processes panel (right panel tab): tree per session — name, PID, RAM, CPU, ports; Kill (graceful,
  then force after 3 s); "Kill all children" keeps the agent.
- Orphans: processes that outlive a closed session (detached from the job / group) listed once
  with "Kill" / "Ignore"; inbox item.
- Port in use when a session starts a server → existing owner shown ("held by vite in session 4").

## Design
- Scan every 5 s only while ≥ 1 session has child processes; one OS table read per scan
  (Windows `GetExtendedTcpTable` via `windows` crate; macOS `libproc`/`lsof -iTCP -sTCP:LISTEN -nP` fallback).
- Results pushed as a compact status delta (like the 250 ms status ticker), not polled by the UI.
- RAM/CPU via `sysinfo` only while the panel is open.

## Files
- Create: `crates/kelpied/src/ports.rs`, `app/src/components/processes-panel.tsx`.
- Modify: `crates/kelpied/src/proc_tree.rs` (expose tree with names), `crates/kelpied/src/daemon.rs`,
  `crates/kelpie-proto/src/lib.rs`, `crates/kelpied/Cargo.toml` (windows features), `app/src/components/right-panel.tsx`,
  `app/src/components/sidebar.tsx`, `app/src/lib/browser.ts`, locales, docs.

## Validation
- `cargo test`: port table parse; tree filter by PID.
- e2e `scripts/e2e/processes.py`: shell session runs `python -m http.server 8765` → chip `:8765`
  within 6 s; click opens browser pane; Kill frees the port; close session with a detached child
  → orphan listed.
- Perf: daemon CPU with 20 idle sessions, no children → no scans (0 extra wakeups).

## Risk / rollback
- Killing the wrong process → only PIDs inside a session tree or recorded orphans; confirm on force.
