---
name: windows-command-spawn-facts
description: Verified Rust 1.97 std::process::Command behaviour on Windows - .cmd args escaped safely, but bare npm not found, relative program resolves against parent cwd, powershell -Command evaluates $() in args
metadata:
  type: feedback
---

Probed 2026-10-25 (phase 11 automations review) with a scratch crate on Rust 1.97:
- `Command::new("x.cmd")` (absolute path) escapes args safely: `&`, `"`, `%COMSPEC%` arrive literally (BatBadBut fixed in std >= 1.77.2; Kelpie pins no rust-version).
- Bare `npm` / `show` (only `show.cmd` exists) -> NotFound: std appends `.exe` only, no PATHEXT search.
- `./show.cmd` with `current_dir(work)` resolves against the PARENT process cwd, not `current_dir`.
- `cmd.exe /c echo <arg>`: Rust quotes metachar args (no `&` injection) but `%VAR%` still expands.
- `powershell -Command ... "$(New-Item f).md"` executes the subexpression: interpreter eval flags re-open injection.
- `powershell.exe ./x.ps1 <arg>` WITHOUT -File also executes `$()` in args (5.1 defaults to -Command); `pwsh` defaults to -File (literal). `-File` is literal in both.
- `cmd /c echo %KELPIE_FILE%` with `&` in the env value injects (cmd expands %VAR% before parsing) - env vars are not a safe channel on a cmd line.
- `npm.cmd run <script> <args>` (npm 11, script `node show.js`) passed `&`, `%PATH%`, `$()`, `^&`, `!X!` literally in MY probe, but the phase 11 developer's probe saw cmd.exe expand `%PATH%` inside `npm run`/`exec`. Treat npm/pnpm/yarn as shell runners; do not cite "npm run is literal" as safe.
- `std::fs::canonicalize` on Windows expands 8.3 short names (`POWERS~1.EXE`) and returns `\\?\` paths; classify programs by canonical name.
- Shared CARGO_TARGET_DIR can hold a stale test binary that cargo reports "Fresh" (210 tests instead of 250+); `cargo clean -p kelpied` fixed it. Sanity-check the test count.

**Why:** "spawned without a shell" claims hold only if users are not pushed to wrap programs in cmd/powershell; on Windows they are (npm.cmd unresolvable).
**How to apply:** For any feature spawning user-configured programs with untrusted data, check PATHEXT/relative resolution and interpreter-eval-flag handling; probe with a tiny scratch crate (build dir on C: is tiny, delete after; C: was at 0.5 GB free). Related: [[shell-hook-blocked-words]] (heredocs strip backslashes; use Write for Rust sources).
