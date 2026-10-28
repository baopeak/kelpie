---
phase: 13
title: "Processes, ports & preview"
status: pending
priority: P2
effort: "3d"
dependencies: [1]
---

# Phase 13: Processes, ports & preview

## Overview
Show what each session runs and which ports it holds, open previews in one click, kill leftovers safely.

## Requirements
- Process tree per session [RT-14]: Windows `QueryInformationJobObject(JobObjectBasicProcessIdList)` on the session's job; macOS/Linux ppid walk reusing
  `sys_stats::descendants` moved into a shared crate (`kelpie-proto` or new `kelpie-sys`). Every tracked process keyed by (pid, start time, exe path).
- Port chips `:5173` on tab/sidebar → browser pane / "Open preview beside". Scan only while the Processes panel is open or a session tab is visible
  and had child processes at last check — triggered by the app, not a daemon timer [RT-14].
- Processes panel: tree (name, PID, RAM, CPU, ports), Kill (graceful → force after 3 s), Kill children (keep agent). Daemon `process.kill {session, pid, start_time}`
  (capability `app`) re-checks membership + identity before acting; Windows kills via an open process handle.
- Orphans [RT-14]: macOS/Linux — processes recorded in a session tree that outlive the session's group; Windows — only processes that broke away from the job (rare; listed if seen).
  Orphan entries keep identity triple; stale identity → entry dropped, never killed.
- "Port in use" → owner shown when known.

## Related Code Files
- Create: `crates/kelpied/src/ports.rs`, `app/src/components/processes-panel.tsx`, shared `descendants` module.
- Modify: `crates/kelpied/src/proc_tree.rs` (job PID list), `app/src-tauri/src/sys_stats.rs` (use shared), `crates/kelpied/src/daemon.rs` (process.*, API_LEVEL),
  `crates/kelpied/Cargo.toml`, `app/src/components/right-panel.tsx`, `app/src/components/sidebar.tsx`, `app/src/components/split-view.tsx`, `app/src/lib/browser.ts`, locales, docs.

## Implementation Steps
1. Shared descendants + job PID list + identity triple. 2. Port table per OS. 3. App-triggered scans + chips. 4. Panel + guarded kill. 5. Orphans (per OS).

## Tests
- `cargo test`: port table parse; identity mismatch refuses kill; job PID list.
- e2e `processes.py`: `python -m http.server 8765` → chip ≤ 6 s → preview → Kill frees port; PID-reuse simulation (stale triple) never kills. macOS-only orphan test in CI.

## Perf gate
- Panel closed and no visible session with children → 0 scans; scan ≤ 5 ms with 5 servers.

## Success Criteria
- [ ] Ports visible and killable; no unrelated process ever killed.

## Risk Assessment
- Job PID list permission issues → fall back to sysinfo ppid walk on Windows too.
