---
phase: 8
title: "Docs and final verification"
status: pending
priority: P2
effort: "3h"
dependencies: [1, 2, 3, 4, 5, 6, 7]
---

# Phase 8: Docs and final verification

## Overview

Update the owning docs, run the full checks, and walk through every goal once more in the real app.

## Requirements

- Functional: docs describe what users can now do, and the log contract.
- Non-functional: no baseline edits to hide a regression; no installer build unless the user asks.

## Related Code Files

- Modify: `docs/user-guide.md` (chat section, near line 83, where background work is described), `docs/architecture.md` (chat log, near the transport and replay sections: `kelpie_answer` record and its fields; `chat-bookmarks.json`; API level note)

Check `README.md` only if its feature tour names these features.

## Implementation Steps

1. Read both docs' existing chat sections. Update the smallest place that owns each fact. Link to code rather than copying schemas.
2. User guide: pinned prompt, clicking files and code paths and what happens outside the project, answered cards, pasting and enlarging pictures, Saved tab, Agent map.
3. Architecture: `kelpie_answer` in the replay log (sanitized fields, 16 KiB cap, why `updatedInput` is excluded), replay boundary, `chat-bookmarks.json` (atomic, failed-read rule), that the agent map derives from the stream.
4. Verify every claim in docs against the code; check links resolve.
5. Run in order, from the stated directory:

   ```bash
   cd app && pnpm typecheck && pnpm test && pnpm i18n:check && pnpm build
   CARGO_INCREMENTAL=0 cargo test -p kelpie-proto -p kelpied -p kelpie   # root; add -p kelpie-stt only if disk allows
   python scripts/perf/perf_gate.py --label chat-view-parity --compare scripts/perf/baseline.json   # root
   ```

   Report any failure as it is. Fix causes, not tests. Do not edit `baseline.json`.
6. Check `app/phone` still builds and its tests pass (part of `pnpm test`; confirm with the phone's build command in `app/package.json`).
7. Real-app walkthrough on the isolated daemon, dark and light themes, one narrow pane: each "Success Criteria" list from phases 1–7. Take screenshots only into the scratchpad unless the user wants them in `docs/screenshots`.
8. Stop every process you started (dev daemon, `tauri dev`, drivers). Exit Claude test sessions cleanly.
9. Report to the user: what passed, what was not tested and why. Mark the phases completed using `ak plan` (see `ak plan --help`). No push or PR unless asked.

## Success Criteria

- [ ] All commands in step 5 pass, or the failures are reported with output.
- [ ] Every goal in `plan.md` is checked in the real app.
- [ ] Docs match the code.
- [ ] No processes left running.

## Risk Assessment

- **Disk.** On 2026-10-30, C: had 14 GB free and D: 6.9 GB. A full `cargo test --workspace` links a large native library. Check free space first, and skip `kelpie-stt` (no change there) if tight, saying so in the report.
- **Perf gate needs a desktop/WebDriver.** Full GUI metrics may not run in every environment. If only partial metrics run, say which.
