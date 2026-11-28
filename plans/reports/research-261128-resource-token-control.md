# Research: RAM + token control for many agent CLI sessions (8 GB box)
Date 2026-11-28. Tags: [V] = fetched this session from primary source; [S] = search snippet/secondary; [U] = from memory, unverified.

## 0. Bottom line (ranked recommendation)
1. Do not hard-limit or suspend agents by default. Cheapest, safest win = **detect pressure -> demote (lower priority + memory priority + EcoQoS) -> gate NEW heavy commands -> only then pause idle/ finished sessions' child trees**.
2. Command-RAM gate: hook-based (Claude/Codex PreToolUse) + ACP `session/request_permission`, learned per (project, command-fingerprint) peak RSS via EWMA, seeded by a static table. Gate = delay, not deny.
3. Token side: biggest lever is **not letting caches expire needlessly** + model tiering + reading local JSONL for 5h/weekly burn. Daemon cannot change agent model mid-session except via agent commands; treat as advisory/UI.
4. Order of build: (a) pressure monitor + per-OS "available RAM" fn, (b) per-session process-tree RSS sampler (peak tracker), (c) PreToolUse gate hook, (d) demotion actions, (e) JSONL usage reader, (f) suspend (last, opt-in).

## 1. Memory-pressure detection

### Windows
- `CreateMemoryResourceNotification(LowMemoryResourceNotification | HighMemoryResourceNotification)` -> HANDLE; wait with `WaitForSingleObject`/`RegisterWaitForSingleObject`; system-wide, signaled on low/high **available physical** memory; band in between = neither signaled. Thresholds are OS-fixed (not configurable) [V: learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification]. Rust: `windows` crate `Win32::System::Memory`. Good as a wake-up edge; poll `QueryMemoryResourceNotification` for state.
- `GlobalMemoryStatusEx` -> `ullAvailPhys`, `ullTotalPhys`, `ullAvailPageFile` (= commit headroom). Better: `GetPerformanceInfo` -> `CommitTotal`, `CommitLimit`, `PhysicalAvailable` (pages) [U on field list; stable API]. **Commit charge is the real failure signal on Windows** (alloc fails when CommitTotal hits CommitLimit = RAM + pagefile), not "free RAM". Alert at CommitTotal/CommitLimit > ~85% and Available < ~10-12% of RAM (heuristic, [U]).
- Per-process: `K32GetProcessMemoryInfo` -> `PROCESS_MEMORY_COUNTERS_EX2`: `PrivateWorkingSetSize`, `PrivateUsage` (commit), `PeakWorkingSetSize`. Use **private working set**, not WorkingSetSize (shared DLL pages double count over a tree).
- Whole tree cheaply: assign every session's tree to a Job Object; `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.PeakJobMemoryUsed` (commit, peak, tracked constantly) [V: learn.microsoft.com/.../ns-winnt-jobobject_extended_limit_information] and `QueryInformationJobObject(JobObjectBasicAccountingInformation / ...ProcessIdList)`. Gives per-session peak for free = ideal input for the learning in section 3.

### macOS
- `dispatch_source_create(DISPATCH_SOURCE_TYPE_MEMORYPRESSURE, 0, WARN|CRITICAL(|NORMAL), queue)`; flags `DISPATCH_MEMORYPRESSURE_NORMAL/WARN/CRITICAL`; event delivered when level changes [V-ish: Apple doc page returned only title; semantics from [U]/Apple dev docs]. Rust: `dispatch2` crate or a tiny C shim; Tauri already links AppKit.
- Polling alternative: `sysctl kern.memorystatus_vm_pressure_level` = 1 normal / 2 warn / 4 critical [S: newosxbook.com/articles/MemoryPressure.html; kernel enum internally 0-3, mapping unconfirmed]. Also `sysctl vm.memory_pressure` is a different, confusing value (Apple forum 802771) - avoid.
- `host_statistics64(HOST_VM_INFO64)` for free/inactive/compressed; macOS compresses memory, so "free" is meaningless; pressure level is the signal. `memory_pressure` CLI can **simulate** warn/critical for testing [S: man page].
- Per-process: `proc_pid_rusage(RUSAGE_INFO_V4).ri_phys_footprint` (matches Activity Monitor "Memory") [U].

### Linux
- PSI `/proc/pressure/memory` lines `some|full avg10/avg60/avg300 total` (us). Event trigger: open file RDWR, write `some 150000 1000000` (150 ms stall in 1 s window), `epoll` with `EPOLLPRI`; one trigger per fd (EBUSY otherwise); window 500 ms-10 s; **unprivileged triggers need window multiple of 2 s**; also per-cgroup `memory.pressure` [V: docs.kernel.org/accounting/psi.html]. Needs kernel >= 4.20 and PSI enabled (some distros/WSL kernels lack it -> fall back to `/proc/meminfo` MemAvailable + SwapFree).
- cgroup v2 [V: docs.kernel.org/admin-guide/cgroup-v2.html]: `memory.high` = throttle + reclaim, **never OOM-kills** (soft ceiling; ideal for "slow, don't kill"); `memory.max` = hard, OOM inside cgroup; `memory.low`/`memory.min` = protection (protect the important sessions); `memory.oom.group=1` kill whole tree as unit; `memory.swap.max`; `cgroup.freeze` (freezes tree; processes still killable); `cpu.weight` (1-10000, def 100), `io.weight`.
- Unprivileged use: only works in a delegated subtree. `systemd-run --user --scope -p MemoryHigh=2G -p MemoryMax=3G -p CPUWeight=50 -- <cmd>` works when the user manager has memory/cpu controllers delegated (default on systemd >= 240ish, [U]); else no-op -> degrade to nice + oom_score_adj.
- `oom_score_adj`: write `/proc/<pid>/oom_score_adj` (-1000..1000); a user may raise (sacrifice) own processes freely, lowering below current needs CAP_SYS_RESOURCE. So: **raise** adj on build children (cargo/rustc/docker) so kernel kills them before the agent CLI. [U, man proc(5)]
- systemd-oomd: kills on PSI/swap policy via `ManagedOOMMemoryPressure=kill` / `ManagedOOMSwap=kill` per unit; only leaf cgroups or `memory.oom.group=1`; needs cgroup v2, memory accounting, PSI; swap strongly recommended [V: freedesktop.org systemd-oomd man]. Risk: it may kill a whole session scope; set `ManagedOOMPreference=avoid` on the kelpied scope [U]. Do not require it; just make sure session scopes are separate units so oomd (if present) kills a build, not kelpied.

### Cross-platform abstraction (KISS)
`MemSnapshot { total, available, commit_ratio?, level: Normal|Warn|Critical }`. Thresholds map: Win Low-notification/commit>90% = Critical; mac level 2/4 = Warn/Critical; Linux PSI some avg10 >10% = Warn, >25% or full>5% = Critical (numbers [U], tune on box). Hysteresis: require 2 consecutive polls to leave Critical (avoid flapping).

## 2. Enforcing budgets/priorities on process trees

| Mechanism | Windows | Linux | macOS |
|---|---|---|---|
| Group handle | Job Object (nest OK Win8+) | cgroup v2 scope | none; use pgid / `proc_listchildpids` walk |
| Memory cap | `JOB_OBJECT_LIMIT_JOB_MEMORY` = **commit** limit, alloc then fails (process sees ENOMEM-like failure/crash) [V]; notification-only variant via `JobObjectNotificationLimitInformation` | `memory.high` (throttle), `memory.max` | no enforceable cap (rlimits mostly ignored) |
| CPU | `JOBOBJECT_CPU_RATE_CONTROL_INFORMATION`: WEIGHT_BASED (1-9, def 5) or HARD_CAP (CpuRate = pct*100, never 0), Win8+; not under RDS+DFSS [V] | `cpu.weight`, `nice` | `setpriority(PRIO_DARWIN_BG)` / `taskpolicy -b` [U] |
| IO | `SetPriorityClass(PROCESS_MODE_BACKGROUND_BEGIN)` (self only) or `NtSetInformationProcess(ProcessIoPriority)` [U] | `io.weight`, `ionice -c3` | PRIO_DARWIN_BG also throttles IO [U] |
| Priority class | `SetPriorityClass(BELOW_NORMAL/IDLE)` on handle w/ PROCESS_SET_INFORMATION | `setpriority` | `setpriority` |
| Efficiency | **EcoQoS**: `SetProcessInformation(h, ProcessPowerThrottling, {Version=1, ControlMask=EXECUTION_SPEED, StateMask=EXECUTION_SPEED})` -> always efficient cores/lowest freq; Win11-class QoS table [V: learn.microsoft.com/.../procthread/quality-of-service; "Eco" row = Windows 11; API itself exists from 1709 [U]] | n/a (use cpu.weight) | taskpolicy `-c background`/QoS class `QOS_CLASS_BACKGROUND` [U] |
| Mem priority | `SetProcessInformation(ProcessMemoryPriority, MEMORY_PRIORITY_VERY_LOW=1)` -> pages trimmed/standby'd first, **keeps process running** [U, documented API] | `memory.low` on important, `memory.high` on others | none |
| Trim now | `EmptyWorkingSet` / `SetProcessWorkingSetSize(-1,-1)` (pages go to standby, refault cost) | `memory.reclaim` (cgroup v2, kernel 5.19+ [U]) | none |

Recommended demotion ladder for a non-focused/idle session tree, least to most intrusive: (1) EcoQoS + below-normal + MEMORY_PRIORITY_VERY_LOW (Win) / nice+cpu.weight+memory.high (Linux) / PRIO_DARWIN_BG (mac); (2) trim working set of idle children (browsers, dev servers); (3) terminate regenerable children (dev servers, headless Chrome) with user-visible notice; (4) suspend. Never apply EcoQoS to the agent CLI process while it streams (latency of token rendering) - apply to its **child** build/test trees, which is where RAM+CPU go.

Also apply Job Object `KILL_ON_JOB_CLOSE` only if desired; it conflicts with the daemon-survives-restart goal (daemon dying kills sessions) - leave unset [design note].

### Suspend vs throttle
- Windows `NtSuspendProcess` (ntdll, undocumented but stable for decades) / `DebugActiveProcess` (bad: kills on detach issues) ; Win10+ job freeze via `NtSetInformationJobObject(JobObjectFreezeInformation)` is internal [U]. Linux `SIGSTOP`/`SIGCONT` per pgid or `cgroup.freeze` (preferred: atomic over tree, race-free vs forks) [V]. mac `SIGSTOP`.
- Risks for agent CLIs [S: blog/ML snippets, Cloudflare "when TCP sockets refuse to die"; inference, not documented guarantee]:
  - Stopped process does not ACK app data; kernel still ACKs until recv buffer fills, then zero-window; long pause > server idle/retransmit timeout (Anthropic/OpenAI streams: tens of seconds to minutes) => stream aborted, request lost, tokens billed for partial output. Streaming HTTPS/SSE mid-response is the worst moment.
  - Prompt cache TTL keeps ticking while suspended (see 4): a suspended-then-resumed session likely pays full cache write.
  - Timers/heartbeats (MCP stdio, ACP, language servers) fire late on resume -> spurious timeouts; wall-clock-based logic (token expiry) breaks.
  - Children frozen mid-write (git index.lock, cargo lock files, pnpm store) -> stale locks if later killed.
  - Suspend the *tool subprocess tree*, not the agent CLI, and only while agent is **blocked waiting on that tool** (no open model stream). Safe suspend points: session idle awaiting user input (agent turn finished: Stop hook / ACP `stopReason`), or a long tool running whose stdout is merely buffered.
- Verdict: suspend only (a) idle-at-prompt sessions' leftover children, (b) never the model-streaming CLI. Prefer demotion/memory.high over freeze everywhere.

## 3. Predict RAM before a command runs

### 3.1 Getting the command before execution
- **Claude Code `PreToolUse`** [V: code.claude.com/docs/en/hooks]: stdin JSON `{session_id, transcript_path, cwd, permission_mode, hook_event_name, tool_name:"Bash", tool_input:{command, description, timeout, run_in_background}, tool_use_id}`. Output `hookSpecificOutput.permissionDecision` = allow|deny|ask (+`permissionDecisionReason`, `updatedInput` can rewrite); exit 2 = block with stderr reason. **Synchronous hook holds the tool call** until it returns (default timeout 600 s) => a hook that sleeps until RAM is available = the delay gate. Timeout/failure does not block by default; `"onFailure":"block"` exists in newer versions. Hooks in `~/.claude/settings.json`, project `.claude/settings.json`, plugin `hooks/hooks.json`; `if: "Bash(cargo *)"` filter avoids spawning for every call. Parallel tool calls -> parallel hook procs: gate must be a **daemon-side reservation**, not per-hook state (hook = tiny client that calls kelpied over the existing pipe/socket and blocks on the reply).
- **Codex hooks** [V: learn.chatgpt.com/docs/hooks, redirected from developers.openai.com/codex/hooks]: `PreToolUse` covers Bash/`exec_command`, `apply_patch`, MCP, local function tools; stdin has `session_id, transcript_path, cwd, model, turn_id, tool_name, tool_use_id, tool_input, permission_mode`; same `permissionDecision` deny shape or exit 2. Config: `$CODEX_HOME/hooks.json`, `.codex/hooks.json` (project must be trusted), `%ProgramData%\OpenAI\Codex\hooks.json` (system), or `[hooks]` in config.toml; non-managed hooks must be trusted via `/hooks`. Also `PreModelRequest`/`PostModelRequest`, `PreCompact` (useful for token control). Old `notify` setting is post-turn only, cannot gate (not covered by fetched page [U]).
- **ACP** [V: agentclientprotocol.com/protocol/tool-calls]: `session/update` `tool_call{toolCallId, kind:"execute", status:"pending", rawInput}` then `tool_call_update`; agent MAY call `session/request_permission` (client replies selected optionId) before running; spec does not state explicitly that the tool is blocked until reply, but `pending` = "awaiting approval". **A client can delay its reply to request_permission = a gate** for agents that ask. Agents running with auto-approve never ask -> can't gate via ACP; must gate via hooks or forbid auto-approve for heavy kinds. Gemini CLI / Devin: Gemini CLI has hooks (BeforeTool) [U - verify]; Devin CLI hook support [U - unverified, not researched].
- Terminal-only fallback (no hooks): daemon already owns PTY. Parse process creation instead: watch for new child (Win: WMI/ETW process start or periodic Toolhelp snapshot; Linux: netlink proc connector `PROC_EVENT_EXEC` needs CAP_NET_ADMIN, else poll /proc; mac: poll `proc_listchildpids`/ES framework needs entitlement). Detection happens **after** exec -> can only react (demote/suspend-the-new-child via cgroup.freeze or NtSuspendProcess within ms), not prevent. Suspending a just-started compile for a few seconds is safe (no network).

### 3.2 Fingerprint + estimate
Fingerprint = (project_root, normalized argv): strip paths/args, keep `tool + subcommand + flags that change memory` (`--release`, `-j N`, `--workspace`, `--all-targets`). Parse via shell-words; split `&&`, `;`, `|`; take max across segments (sequential) or sum (backgrounded/`&`/parallel e.g. `concurrently`, `turbo`).

Static priors (rough, [U] from experience; confirm on this box before shipping; use as cold-start only):
| Command | Typical peak RSS (tree) |
|---|---|
| `cargo build/check` mid project | 0.5-2 GB; `--release` + LTO link 2-6 GB; rustc ~ jobs x 0.3-1.5 GB; **`-j` default = cores**, so cap `-j`/`CARGO_BUILD_JOBS` |
| `cargo test --workspace` (link many test bins) | 2-6 GB |
| `pnpm install` | 0.5-1.5 GB |
| `pnpm build` (vite/tsc) | 1-3 GB; `next build` 2-6 GB+ (Node default old-space ~ 2-4 GB cap by heap limit) |
| `tsc --noEmit` | 0.5-2.5 GB |
| `docker build/compose up` | VM backend (WSL2/Docker Desktop) 2 GB+ reserved, often grows; check `.wslconfig`/Docker limits |
| Playwright/Chromium headless | 300-600 MB per browser + 100-300 MB per page; x workers |
| vitest/jest | 0.3-1 GB per worker x workers |
| ML / PDF / image render (torch import, pdf2image, ffmpeg, sharp) | 0.5-4 GB; torch import alone 0.5-1 GB |
Gate threshold: estimate > (available - safety reserve [~1 GB + agent baseline sum]) -> queue.

Learned estimate: after each command exits, record `peak_tree_rss` (Win: `PeakJobMemoryUsed` [V] for commit or sampled private WS; Linux: cgroup `memory.peak` (kernel 5.19+ [U]) else sample `memory.current`; mac: sample sum of `ri_phys_footprint` every 250-500 ms; or `wait4` `rusage.ru_maxrss` per child only - misses grandchildren [U]). Update `est = α*peak + (1-α)*est`, α≈0.3, plus track variance; **plan with `est + 1.5·sd` or max of last 5** (peaks are heavy-tailed; asymmetric loss: under-predict = thrash, over-predict = just delay). Cold start: prior table x repo-size factor (Cargo.lock dependency count, node_modules size). Key per (project, fingerprint, `-j`). Store in daemon's existing atomic state (compare-before-write per AGENTS.md).
- Prior art on same idea [U unless noted; not fetched]: **Kubernetes VPA** recommender = decaying histogram of usage samples, recommends a high percentile (target P90, upper P95-ish) with safety margin, half-life ~24 h decay, OOM bumps +20%/min +100 MB; **Slurm** `sacct MaxRSS` vs `ReqMem` and `--mem` mandatory declaration, cgroup enforced; **Bazel** `--local_ram_resources` / per-action `resource_set`, default estimates per action mnemonic and `--experimental_...` dynamic scheduling with memory tracking (schedules by *declared* cost, not measured; same concept); **Buck2** action `weight`/`resource_hints`; **Nix** `--cores`/`--max-jobs` only, no RAM model; CI (GitHub Actions) just fixed runner sizes. Takeaway: serious schedulers = declared/estimated cost + semaphore of "RAM tokens" + percentile-of-history. Implement a **weighted semaphore in kelpied** (capacity = available - reserve), commands acquire estimated MB, release on exit, correct estimate on exit. Starvation guard: FIFO + max wait (e.g. 120 s) then allow with warning/ask user; always allow if nothing else is running.
- Gate UX: hook blocks, prints to stderr-free reason via `permissionDecisionReason` only if denying; for delay just hold (agent sees slow tool). Hold timeout well under agent tool timeout (Claude default Bash 120 s; `tool_input.timeout` is available - queue wait must not eat it: either keep wait < ~30-60 s then `ask`/allow, or rewrite via `updatedInput` adding `CARGO_BUILD_JOBS=2` / `NODE_OPTIONS=--max-old-space-size=...` prefix [U feasibility]). **Rewriting to cap parallelism is better than waiting**: `cargo build` -> `cargo build -j2`.
- Caveat: hooks only cover shell commands the agent itself issues; builds triggered by `pnpm test` scripts, watch-mode, or IDE aren't predicted - the reactive path (section 1/2) stays mandatory.

## 4. Token / cost optimization

### Prompt caching [V: platform.claude.com/docs/en/build-with-claude/prompt-caching]
- TTL default **5 min**, refreshed (free) on each hit; 1 h via `"ttl":"1h"`. Writes: 5m = 1.25x base input, 1h = 2x; reads = **0.1x** (0.05x for Opus 5.5 / Sonnet 5.5; 0.025x for Fable/Mythos 5.1-5.2). TTL clock starts at request start. 1h entries must precede 5m ones in prompt. Min cacheable 512 tokens on current models.
- Idle consequence: a session idle > 5 min re-sends its whole context as a cache **write** (1.25x) instead of read (0.05-0.1x) = 12-25x cost on the context for that turn. Long contexts (100-200k) make a "come back after coffee" turn cost ~ 125-250k x 1.25 x base. Strategy for daemon: (a) don't suspend/kill idle sessions that will be resumed within the TTL - nothing to gain, (b) surface "cache cold" badge after 5 min (or 1 h if Claude Code uses 1h on subscription - [U], check JSONL `cache_creation.ephemeral_1h_input_tokens`), (c) before a user resumes a cold large session, suggest `/compact` or `/clear` (cheaper than paying a full 200k write) - decision rule: if context tokens x (write_mult - read_mult) > cost of compaction call, compact first. (d) Do not send keep-alive pings: ping every 4 min on 200k ctx costs 0.05-0.1x x 200k each; worth it only for gaps shorter than ~(1.25-0.1)/0.1 ~ 11 pings equivalent... i.e. keep-alive for <~45 min idle breaks even vs one cold write at 0.1x reads [derived, [U]], and it burns subscription rate-limit quota anyway - **not recommended**; adding it needs agent cooperation anyway.
- Cache writes/reads show in usage as `cache_creation_input_tokens`/`cache_read_input_tokens`; if both 0, prompt too short or cache missed.

### Reading local usage
- Claude Code: `~/.claude/projects/<encoded-cwd>/<session>.jsonl`; assistant records have `message.usage` {input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens, nested `cache_creation{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}`}, `message.model`, `timestamp`; per-message additive. Gotchas: streaming placeholder `input_tokens` (reported =1) and duplicated records across resumed/forked sessions -> dedupe by `message.id`+`requestId` (what ccusage does [U]) [S: github.com/anthropics/claude-code/issues/28197; danyuchn.github.io claude-code-jsonl-internals; docs.rs llm-tokei].
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` (+ `archived_sessions/`); `event_msg` with payload `token_count`, `info.total_token_usage` {input, cached_input, output, reasoning_output} is **cumulative per session** -> take last/ diff; also `rate_limits` blocks appear in token_count events [U - verify]; pre-2025-09-06 sessions lack it [S: dev.to/newtorob; codex.danielvaughan.com; docs.rs codexusage].
- ccusage (github.com/ryoppippi/ccusage) [V]: commands `daily|weekly|monthly|session|blocks|statusline`, `--json`, `--since/--until`, handles Claude + Codex; `blocks` = 5-hour billing windows with active-block monitoring. For kelpied: **reimplement the small parser in Rust** (tail with offsets, dedupe) rather than shelling to a Node tool (extra 100+ MB RSS on 8 GB box); or shell to `ccusage --json` only as optional fallback. Gemini/Devin local logs: not researched.
- Cost = tokens x per-model price table (prices change; fetch LiteLLM `model_prices_and_context_window.json` [U] rather than hardcoding).

### Rate-limit windows [S: datacamp, allthings.how, alexcloudstar.com 2026, support.claude.com/en/articles/15910845; all secondary; official help only gives structure]
- Subscription: rolling **5-hour** window starting at first request + separate **weekly** cap; shared pool with claude.ai; sizes unpublished (Max 5x/20x = x Pro per session). `/usage` shows status. Reported 2026 changes (doubled 5h limit, temporary +50% weekly through ~Aug 31/Sep 13) - sources disagree, treat as unverified.
- Scheduling around them: parse block start from first JSONL record timestamp in last 5 h (matches ccusage blocks logic); estimate burn rate (tokens or cost / min) -> predict exhaustion time; actions: warn, route to cheaper tier, defer low-priority/background sessions (batch/automations) to after reset, stagger heavy parallel sessions (N parallel sessions drain the same pool N x faster, a small box already limits N). Weekly: pace = used/(elapsed of week) vs 1.0. Server returns 429 + `anthropic-ratelimit-*`/ `retry-after` headers on API-key mode [U]; subscription mode surfaces as in-CLI limit message -> detect by PTY text pattern is brittle [U].
- API-key users: **LiteLLM proxy** budgets (`max_budget`, `budget_duration`, per-key/team/user, `tpm/rpm` limits) and **OpenRouter** key credit limits [U, not fetched this session]; Claude Code supports `ANTHROPIC_BASE_URL` to point at a proxy, Codex via `OPENAI_BASE_URL`/provider config [U]. Local proxy in daemon = real enforcement point (budget, model rewrite, cache_control injection) but is new attack surface/maintenance: **not recommended for v1** (YAGNI), JSONL read-only first.

### Model routing & compaction
- Tiering: cheap model for read/grep/summarize/test-fix loops, strong model for design/debug; Claude Code supports `/model`, `--model`, `ANTHROPIC_DEFAULT_*_MODEL`, subagent per-agent model in frontmatter, `opusplan`-style plan/exec split [U]. Daemon can only set env/flags **at session spawn** (default model per session "role") - cheap, safe, no mid-session control needed.
- Compaction: Claude Code auto-compacts near limit (`/compact`, `PreCompact` hook exists [V for Codex page; Claude [U]]); cost: summarizing = one large call (cache read of full ctx 0.05-0.1x + output), then later turns re-write the new smaller prefix once (1.25x of small ctx). Compact **before** an idle gap > TTL, not after (after = pay full cold write of the big context first). `/clear` between unrelated tasks is free savings; kelpied can nudge when context > X tokens and cache cold.
- Reduce tokens at source: smaller tool outputs (limit test/build log verbosity via env, `--quiet`), `.claudeignore`-equivalent, short CLAUDE.md/AGENTS.md (re-sent every turn but cached), avoid MCP servers with huge tool schemas (each schema is in every prompt prefix) [U].

## 5. Trade-off matrix (daemon features)
| Feature | RAM benefit | Risk | Effort | Verdict |
|---|---|---|---|---|
| Pressure monitor + UI | indirect | none | S | do first |
| Per-session tree RSS/peak sampler (Job Obj/cgroup/pid walk) | enables all | sampling CPU | M | do |
| PreToolUse gate (delay + `-j` rewrite) via hooks | high (prevents OOM) | hook install/trust UX; parallel races; tool timeout | M | do, delay>deny |
| EcoQoS/nice/memory-priority on children | medium | low | S | do |
| memory.high / Job commit limit on build children | high | commit-limit alloc failures crash builds (Win) | M | Linux yes; Win only soft/notify |
| Trim idle browsers/dev servers | medium | lose state | S | ask user |
| Suspend idle tree | medium | stale locks, TCP, cache TTL | M | opt-in, last |
| JSONL usage reader + 5h block estimator | tokens | format drift (undocumented) | M | do; tolerate unknown fields |
| Local LLM proxy w/ budgets | tokens | high | L | skip v1 |

## 6. Adoption/stability notes
- Windows APIs (Job Objects, memory notifications, EcoQoS) stable, documented. NtSuspendProcess/JobObjectFreeze undocumented -> pin behind a feature flag.
- Claude/Codex hook schemas are young and changing (`onFailure`, trust model); version-sniff, fail-open (a broken gate must never wedge the agent).
- JSONL formats undocumented; ccusage/llm-tokei maintain compat - watch them as canaries.
- macOS has the weakest controls (no memory caps): rely on pressure level + terminating regenerable children.

## 7. Limitations / unverified
- Peak-RSS table, PSI thresholds, VPA/Slurm/Bazel/Buck details, `memory.peak`/`memory.reclaim` kernel versions, mac sysctl mapping, taskpolicy/App Nap, Gemini/Devin hook support, LiteLLM/OpenRouter budget fields: from memory or snippets, not fetched. Measure on the target 8 GB box before fixing numbers.
- Rate-limit numbers: third-party only.
- Did not test any API on this machine.

## Sources
- https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification
- https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information
- https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_cpu_rate_control_information
- https://learn.microsoft.com/en-us/windows/win32/procthread/quality-of-service
- https://docs.kernel.org/accounting/psi.html
- https://docs.kernel.org/admin-guide/cgroup-v2.html
- https://www.freedesktop.org/software/systemd/man/latest/systemd-oomd.service.html
- https://developer.apple.com/documentation/dispatch/dispatch_source_type_memorypressure
- https://newosxbook.com/articles/MemoryPressure.html
- https://code.claude.com/docs/en/hooks
- https://learn.chatgpt.com/docs/hooks (ex developers.openai.com/codex/hooks)
- https://agentclientprotocol.com/protocol/tool-calls
- https://platform.claude.com/docs/en/build-with-claude/prompt-caching
- https://github.com/ryoppippi/ccusage
- https://github.com/anthropics/claude-code/issues/28197
- https://danyuchn.github.io/blog/posts/en/claude-code-jsonl-internals
- https://dev.to/newtorob/claude-code-and-codex-are-logging-your-token-usage-locally-here-is-how-to-read-it-580
- https://codex.danielvaughan.com/2026/03/30/codex-cli-session-analytics-jsonl-rollout
- https://blog.cloudflare.com/when-tcp-sockets-refuse-to-die/
- https://support.claude.com/en/articles/15910845 ; https://alexcloudstar.com/blog/claude-code-usage-limits-2026/

## Unresolved questions
1. Does Claude Code subscription traffic use 5m or 1h cache TTL (check `ephemeral_1h_input_tokens` in real JSONL)?
2. Gemini CLI and Devin CLI hook/pre-exec interception support?
3. Is `PeakJobMemoryUsed` (commit) a good enough proxy for RSS peak, or is private-WS sampling needed?
4. Should gate fail-open after N s (recommended) or ask the user?

Status: DONE_WITH_CONCERNS
Summary: Report written to D:/Works/Kelpie/plans/reports/research-261128-resource-token-control.md; core OS/hook/cache facts verified from primary docs, but peak-RSS figures, scheduler prior art, and rate-limit numbers are unverified and flagged [U]/[S].
