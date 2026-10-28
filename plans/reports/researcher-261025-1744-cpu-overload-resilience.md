# Host overload resilience for phone remote control (CPU/GPU/mem)

Date 2026-10-25. Scope: plan `261024-1205-phone-remote-control` phases 1,3,4,6,7,8 (+2,5,9 hints). Windows first, macOS parity.
Tags: [V] verified in fetched source; [U] unverified / from memory / inferred. Citations at bottom.

## 0. Verdict (read this)
1. **Measure symptoms, not causes.** Primary overload signals = (a) own event-loop lag, (b) per-sink queue age, (c) encoder frame time / in-flight depth. System CPU%/GPU%/mem/thermal are secondary inputs that only *raise* the level. Counters like Processor Queue Length / GPU Engine PDH are diagnostics, not control inputs (cost + noise).
2. **Biggest silent risk: `kelpied` has no window -> Windows QoS heuristics treat it as non-visible/Low (efficient cores, low freq on battery).** [V] MS QoS doc: only window-owning processes (and their descendants) get High/Medium by focus; unclassified = heuristic. Explicitly opt `kelpied` and `kelpie-screen` out of EcoQoS (`SetProcessInformation(ProcessPowerThrottling)`, ControlMask=EXECUTION_SPEED, StateMask=0) and also IGNORE_TIMER_RESOLUTION off-throttle [U on exact flag semantics]. Cheap, high value, zero risk.
3. **Never solve this with HIGH/REALTIME priority class.** Cap at ABOVE_NORMAL for control threads + MMCSS for capture/encode. Agents' builds must not be able to starve heartbeats, and our heartbeats must not starve the user's builds beyond tens of ms.
4. **Dead-peer detection must be lag-aware** (biggest false-disconnect source under load). Rule in section 2.4.
5. **Screen streaming: no authoritative public doc exists on how Parsec/Sunshine/RustDesk/CRD detect host-encoder overload** (searched; only manual knobs found). Use libwebrtc's overuse detector (85 % / ~42 % encode-usage thresholds) as the reference model; implement in `kelpie-screen` ourselves because str0m has no adaptation.
6. Ship a single `load` module (level L0-L3 + reason + hysteresis) in phase 1; every later phase just subscribes. Do not build per-phase overload logic.

## 1. Detecting overload (APIs, cost, role)
| Signal | Windows | macOS | Cost | Role |
|---|---|---|---|---|
| **Loop lag** (probe task `interval(100ms)`; lag = actual - expected tick; also on the 2nd runtime) | tokio | tokio | ~0 | **PRIMARY**. Directly measures what users feel; catches CPU, runnable-queue, lock, blocking-in-async at once |
| Tokio runtime metrics (`RuntimeMetrics`: global queue depth, busy duration per worker; `tokio-metrics`) | same | same | ~0 | diagnostic; poll-time metrics are unstable API [U] |
| System CPU % | `GetSystemTimes` delta, 1 Hz | `host_statistics`/`host_processor_info` or `getloadavg`/ncpu | us | secondary (EMA over 3 s) |
| Run-queue | PDH `\System\Processor Queue Length` (>2x logical CPUs sustained = saturated) [U] | `getloadavg` [U] | PDH ms-scale, needs 2 samples | **skip in v1**; CPU% + lag already cover it |
| GPU engine util | PDH `\GPU Engine(*)\Utilization Percentage`, per-engine instance names `pid_X_luid_Y_engtype_Z`; Task Manager shows the *busiest* engine, not average [V] | IOKit/`powermetrics` (root) [U] | wildcard enumeration is heavy (hundreds of instances) [U] | diagnostic only, only while screen streaming, expand wildcard once, refresh 30 s. Control GPU contention via encode/capture time instead |
| GPU memory | DXGI `IDXGIAdapter3::QueryVideoMemoryInfo` (Budget/CurrentUsage) | n/a | us | secondary while streaming |
| Memory | `GlobalMemoryStatusEx`, `GetPerformanceInfo` (commit), `CreateMemoryResourceNotification(LowMemoryResourceNotification)` event | `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` (WARN/CRITICAL) | event = free | secondary; low-mem event forces L2 + cache eviction |
| Thermal | no clean API; `CallNtPowerInformation(ProcessorInformation)` CurrentMhz vs MaxMhz < 70 % sustained = throttled [U] | `NSProcessInfo.thermalState` (nominal/fair/serious/critical) + `thermalStateDidChangeNotification` [V-ish: Apple page lists it] | us | secondary; serious/critical -> L2/L3 |
| Battery saver / power mode | `GetSystemPowerStatus` (`SystemStatusFlag`), `PowerRegisterForEffectivePowerModeNotifications` [U] | `NSProcessInfo.isLowPowerModeEnabled` + notification | event | secondary: cap to L1 floor (screen 15 fps) |
| User put kelpied in "Efficiency mode" | `GetProcessInformation(ProcessPowerThrottling)` + `GetPriorityClass==IDLE` on self, check at start + 60 s | n/a | us | surface a warning, do not fight it |
| App Nap | n/a | daemon: `ProcessType Interactive` in LaunchAgent plist and/or `beginActivityWithOptions(NSActivityUserInitiated...)` while any phone is connected; end when none [U-API-details, V-concept] | free | required, else timers coalesce/pause when no UI |
| Sink pressure | per-phone outbound queue age/bytes; encoder in-flight; CDP unacked frames | same | 0 | **PRIMARY**, per-sink not global |

**Level machine** (single `load.rs`, tokio watch<Level> + reason bitset). Sample 1 Hz (lag p95 over a rolling 5 s).
| Level | Enter (2 consecutive samples = 2 s) | Exit (must hold 10 s, and min dwell 5 s) |
|---|---|---|
| L0 normal | - | - |
| L1 busy | lag p95 > 30 ms OR CPU EMA > 85 % OR battery saver OR thermal fair | lag p95 < 15 ms AND CPU < 70 % |
| L2 heavy | lag p95 > 75 ms OR CPU > 95 % (3 s) OR avail mem < 15 % OR thermal serious OR low-mem event | lag p95 < 35 ms AND CPU < 85 % AND mem > 20 % |
| L3 critical | lag p95 > 200 ms OR avail mem < 8 % OR thermal critical | lag p95 < 100 ms AND mem > 12 % |
Rise fast (immediate jump to the highest matching level), fall one level at a time. Max 2 transitions/min at steady stress (flap gate). Numbers are starting points [U], tune with section 8 harness. Per-sink overrides: a single slow phone gets latest-only without raising global level.

## 2. Keeping the control plane responsive
### 2.1 Priorities (Windows)
- `kelpied`: process class stays NORMAL (must not be inherited by agent children; MS: children inherit only IDLE/BELOW_NORMAL classes from parent, otherwise NORMAL [U, from CreateProcess docs memory - verify with a 5-line test]). Put **link/sync I/O threads** at `THREAD_PRIORITY_ABOVE_NORMAL`; PTY-ingest stays normal.
- Existing "Prioritize visible terminals" stays. Order must be: link/capture/encode (ABOVE_NORMAL/MMCSS) >= visible terminal > rest. Never HIGH/REALTIME.
- `kelpie-screen`: capture + encode threads call `AvSetMmThreadCharacteristics("Capture")` [task exists by default: Audio, Capture, Distribution, Games, Playback, Pro Audio, Window Manager, V]. Caveats [V]: MMCSS boosts by category (Medium 16-22 for foreground app; Low 8-15 guaranteed min share); once a thread exhausts its CPU quota it drops to 1-7 if others are ready; `SystemResponsiveness` (default 20 %) reserves CPU for low-priority threads, so MMCSS cannot starve builds entirely (good). "GPU Priority" registry value is "not yet used" [V]. Do not call it a GPU guarantee.
- Media Foundation work queues: use `MFLockSharedWorkQueue("Capture")` so MF threads are MMCSS'd [U].
- Do NOT set the *process* to EcoQoS anywhere except explicitly background, non-urgent helpers (e.g. a future resume-cache compactor, `THREAD_POWER_THROTTLING_EXECUTION_SPEED` on that thread only) [V: MS says EcoQoS not for performance-critical/foreground].
- Win11 inactivity QoS: foreground app drops to Medium after user inactivity on battery [V, `DisableUserPresenceQos` is registry-only, test only]. Phone-driven sessions = user inactive at the PC -> kelpie-screen should hold explicit High (`ThreadPowerThrottling` off) [U-effect].
- macOS: daemon threads `pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED)` for link/sync (user-interactive is for main-thread UI; avoid for daemons) [V classes listed; choice U]; capture/encode `USER_INTERACTIVE` in helper (phase 9). Background indexing/caches `UTILITY`. Apple: "run at utility or lower 90 % of the time when user not active" [V].

### 2.2 Tokio design
- [V] Tokio: tasks yield only at `.await`; no numeric limit documented; CPU-heavy -> `spawn_blocking`; `block_in_place` only multithread; `spawn_blocking` tasks cannot be aborted once running.
- Rule of thumb ~100 us between awaits [U single-source guide]; gate by measurement instead (lag probe).
- **Decision**: do NOT pre-emptively split runtimes. Phase 1 adds the lag probe; split if, at the phase-1 perf-gate load (3 sessions x 1 MB/s, 8 phones) *without* external stress, lag p99 > 30 ms. Fallback design: second `Builder::new_multi_thread().worker_threads(2).thread_name("kelpie-link")` for WS accept, Noise, heartbeats, sync send; PTY ingest, VT parse and row render stay on main runtime, hand-off via bounded channel / `watch`. Rationale: Noise ChaCha20-Poly1305 and row diffs are cheap; the real starvation risk is OS-level (whole process starved by 20 compiles) which a second runtime does not fix, only thread priority does.
- Row rendering: per session per tick **once**, shared via `Arc` to all subscribed phones; per-device diff against per-device base on row hashes (not strings). Otherwise 8 phones = 8x render. Add to phase 1.
- Locks: snapshot under lock < 200 us budget (assert in debug, histogram in release); never hold std/parking_lot guard across `.await`; use `try_lock` + skip-this-tick on contention for the sync path (stale-by-100 ms is fine, blocking the PTY ingest is not). [V in plan: snapshot under lock, diff outside]
- Use `tokio::time::interval` with `MissedTickBehavior::Delay` (or `Skip`) for sync ticks: `Burst` (default) causes catch-up storms after a stall [V in tokio docs semantics, U on which fits better].
- Watchdog: a plain OS thread (not tokio) checks a heartbeat atomic written by the lag probe every 1 s; if runtime silent > 5 s: dump all-thread backtrace/log "runtime stalled"; > 30 s: surface to app as `daemon_unresponsive`. Never auto-kill agents' PTYs.

### 2.3 What must never sit behind bulk work
Separate **control lane**: small bounded queue (64) per connection, written *before* bulk (`biased` `select!`), items: ping/pong, ack, revoke, stop, error, answer results, `load.status`. Bulk lane: screen/diff frames, latest-only slot (size 1 per session) not a queue. Control lane full => close connection with reason (do not drop silently).
- Revoke/downgrade/Stop: check `Arc<AtomicBool> grant_alive` in the SendInput / `Input.dispatch*` path **per event** (no dependency on async scheduler) -> input stops < 1 s even at L3 [maps to plan RT-12 targets].
- Stop hotkey: `RegisterHotKey` on a dedicated message-loop OS thread in the app (not tokio, not WebView thread) -> sets atomic, sends helper `stop`; if no ack in 500 ms `TerminateProcess` the helper. [U design, standard pattern]

### 2.4 Lag-aware liveness (no false dead-peer)
- Peer deadline = `last_rx + max(3 x ping_interval, 3 x ping_interval + lag_p99_5s)`; timeout only counts if **our own** lag probe was healthy in the window; otherwise reset deadline (grace).
- Monotonic gap check: if tick gap > 2 x interval (stall, sleep, debugger) -> reset all peer deadlines, do not declare any dead (same pattern as suspend/resume).
- Under L2+ double the ping interval (LAN 10 s -> 20 s; relay keepalive stays 30 s, dead at 90 s -> 180 s at L2+). Pings are tiny; their *processing* is what suffers, so prefer WebSocket-protocol pings answered by the lowest layer, and app-level liveness counted by *any* received frame.
- Cloudflare DO WebSocket hibernation can auto-respond to pings without waking [U, verify in phase 4]: use it so the relay never false-kills the host side either.
- Gate: 0 false disconnects in 30 min at L3 stress (section 8).
- Same logic for phone-side: do not show "disconnected" until 2 x deadline; show "host not responding" first (UX section 7).

## 3. Shedding ladder (consumers of `Level`)
Rates = min interval between frames per session, per phone. "Focused" = the one session the phone is currently viewing (phone sends `focus`).
| Item | L0 | L1 | L2 | L3 |
|---|---|---|---|---|
| Focused session diff | 100 ms | 100 ms | 250 ms | 1000 ms latest-only |
| Other subscribed sessions | 100 ms | 250 ms | 1000 ms | paused (list/attention `rev` only) |
| Max diff frame | 64 KiB | 64 KiB | 32 KiB | 16 KiB then force full-redraw TUI cap 1 fps (plan: 5 fps cap -> 1 fps) |
| Resume cache | write-through | write-through | evict LRU > 8 MiB | clear all (clients re-snapshot) |
| Push/notify | normal | coalesce 1 s | digest 5 s, attention events still immediate | attention events only, ≤ 1 s delay |
| Chat reset-on-attach (2 MB) | any | any | max 2 concurrent | 1 concurrent, ≤ 500 lines |
| CDP screencast (phase 7) | q70 w1280 ≤ 10 fps | q60 w1280 ≤ 8 | q50 w960 everyNth=3 ≤ 4 | q40 w800 ≤ 2 fps (relay already ≤ 2) |
| Desktop screen (phase 8) | 1080p30 | 1080p20 | 720p15 | 540p8 + bitrate floor, banner |
| New screen/takeover session | admit | admit | **refuse** `host_busy` (retry-after 10 s) | refuse |
| Existing screen at L3 > 60 s | - | - | - | offer user "pause stream" on phone (auto-pause after 5 min) |
| Pairing handshakes | normal | normal | rate-limit 1/s | 1/5 s (never starve existing control) |
**Never shed**: `input`, `key`, `answer`, `chat_answer`, revoke/downgrade, `Stop` (verb and hotkey), ping/pong/acks, attention-needed event delivery, the `load.status` message itself, desktop toasts for device changes (security). Shedding is by lowering *rate/size*, never by dropping *commands*; if the control queue is full, reject with an explicit error the phone shows.
**Admission control** also on memory: refuse new screen session if avail mem < 20 % or GPU mem budget used > 90 % [U thresholds].
**Phone-side unsubscribe is the best shed**: client sends `sub`/`unsub` + `focus` + `visibility` (hidden tab / app backgrounded / screen off => unsub all except attention). Phase 2/5 requirement.
**Latest-state-only** is already in plan (phase 1); extend: bulk slot size 1 per (phone, session), overwritten by newer full-rev; skipped revs are fine because `base` diff recomputed vs last *acked* rev (needs per-frame ack seq from phone, every 500 ms piggyback).

## 4. Screen streaming under host load
### 4.1 What peers do (honest)
- Parsec: manual caps (max bitrate default 10 Mbps, lower res / 30 fps for low bandwidth); no documented automatic host-CPU adaptation [V support articles; adaptation in third-party copy not trusted].
- Sunshine/Moonlight: docs have latency tuning only; real-world: silent fallback to software H.264 when HW path missing (issue 2141), GPU near-full when streaming concurrently with heavy GPU work, 40 fps vs 120 fps reports [V issues, root causes unconfirmed].
- OBS (reference for encoder overload): remedy = lower res/fps, lighter preset, different encoder; "skipped" = encoder lag, "lagged" = GPU render overload [V].
- RustDesk / Chrome Remote Desktop: nothing authoritative found. [gap]
- libwebrtc (best public model) [V]: encode usage = avg encode time / avg inter-frame interval; **overuse > 85 %**, underuse < (85-1)/2 = **42 %** (interval 2x wide on purpose to avoid toggling), estimates need >= 120 frames, 1.5 s frame timeout, capturer max-fps considered so quiet periods don't read as underuse. Stats expose `qualityLimitationReason` = none|cpu|bandwidth|other (priority bandwidth>cpu>other when ambiguous) [V MDN].
- NVIDIA MFT forum reports [V, anecdotal]: H.264 MFT latency ~3 ms -> ~10 ms under a concurrent 3D app, and drift upward when fed at a steady low rate; NVIDIA driver may not even expose an H.264 HW MFT on some hosts (benchmarks project) -> **spike must enumerate with `MFTEnumEx(HARDWARE|SORTANDFILTER)` and log which MFT, per vendor**; Steam streaming: Intel MFX slowed by other Direct3D apps contention [V anecdotal].

### 4.2 Requirements for `kelpie-screen` (phase 8)
1. **Pipeline shape**: capture -> 1-slot mailbox (drop-oldest) -> encoder; MFT in-flight <= 2 frames. If `ProcessInput` returns `MF_E_NOTACCEPTING` or in-flight == 2 for > 3 consecutive frames: drop incoming frames, count `dropped_encoder`.
2. **Overuse detector** (libwebrtc style): `encode_usage = EMA(encode_ms) / EMA(frame_interval_ms)`. Overuse > 0.85 for 2 s -> step down; underuse < 0.42 for 10 s -> step up one rung. Use capture's *target* interval not achieved. Min 60 frames of data (30 fps x 2 s).
3. **Rung order (down)**: fps 30->20->15->10 (terminal/IDE content tolerates low fps) then scale 100->75->50 % (keep text legible: min 540p for windows) then bitrate cap. Never first lower QP-only; frame-time is the problem, not bits. Step-down dwell 2 s, step-up dwell 10 s.
4. **HW<->SW**: stay on HW and reduce load. Switch to SW MFT only if (a) HW MFT absent/failed init, or (b) HW encode p95 > 33 ms for 5 s *at the lowest rung*. SW on a pegged CPU worsens everything (cap SW to 720p15). Never switch silently: emit `encoder_changed` -> UI badge (Sunshine #2141 is the failure to avoid).
5. **Capture rate**: WGC `MinUpdateInterval`/frame-pool of 2-3 buffers; DXGI `AcquireNextFrame` timeout 33 ms; frames only on change (plan). Under 3D saturation DXGI/WGC wait for present: if acquire gives no frame for > 500 ms while desktop is "active", send a keepalive repeat frame at 1 fps (prevents "frozen" look and the NVENC low-rate drift) [U].
6. **Keyframes**: IDR = several x a P-frame (≈5-10x [U]); rate-limit PLI/FIR responses to 1 IDR / 1 s per peer (coalesce), no periodic IDR (GOP infinite), skip IDR when level >= L2 unless peer truly lost (NACK exhaust). Cap keyframe bytes by temporary lower QP only if CBR VBV permits [U].
7. **Latency budget (1080p30 HW, LAN)** [U guide numbers, targets from plan 150 ms p95]: capture <= 8 ms, convert/scale <= 2, encode <= 10 (alert at 15 per plan risk), packetize+send <= 2, network <= 10, decode+render phone <= 40, input return <= 20. Report per-stage histograms via helper stdio (1 Hz) -> debug overlay.
8. **Queue bounds**: str0m outbound RTP queue <= 200 ms of media or 512 KiB; overflow -> drop to next IDR boundary and request IDR once. DataChannel input queue <= 256 events; mouse moves coalesced (latest only); keys/clicks reliable and never dropped.
9. **GPU contention**: D3D11 device created with `IDXGIDevice::SetGPUThreadPriority(+?)`: range -7..7 [U]; only helps within our process queue. Do not rely. `D3DKMTSetProcessSchedulingPriorityClass(HIGH)` is available without privilege in some cases but REALTIME needs it [U]; skip. Use the measured-encode-time loop instead.
10. **Helper process**: NORMAL class + EcoQoS opt-out + MMCSS threads (2.1), above-normal as plan says is acceptable; exits 30 s after last viewer (plan). Idle <= 50 kbps (plan) -> at idle also stop encode thread timers.
11. **Report limits**: DataChannel `stats` 1 Hz: `{fps, w, h, bitrate, encode_ms_p95, dropped, reason: cpu|gpu|bandwidth|thermal|battery|other, level}` so the phone can show it (mirrors `qualityLimitationReason`).

### 4.3 WebView2 CDP screencast (phase 7)
- [V] Events fire per rendered frame, not on a timer: static page = near-zero frames; slow ack = backpressure; JPEG/PNG/WebP cost differs; `quality` JPEG only; params `maxWidth`, `everyNthFrame`; measure achieved fps as frames/time, don't trust `everyNthFrame` [V sources: macmyths guide, Ferrum, browser-use PR]. No measured CPU numbers exist publicly -> **measure in phase 7 spike** (CPU of msedgewebview2 GPU+renderer processes, with/without screencast, 1080p video page worst-case).
- Requirements: 1 frame in flight (ack on phone ack, not on receipt); stop/pause screencast when no phone ack for 2 s; ladder per table; main-thread cost <= 2 ms/frame (plan) measured under L2 stress; hidden pane keeps frames only if pane brought to front (plan).
- Cost is in the browser process, outside our control: only reduce by parameters, so ladder values must be tested to show CPU drop (>= 50 % lower at L2 vs L0 on a busy page) [U target].

## 5. Memory pressure
| Item | Cap |
|---|---|
| Per-phone outbound bulk | latest-state slot per session + <= 256 KiB bytes pending; over -> replace/skip; over for 10 s -> close with `slow_consumer` |
| Per-phone control lane | 64 msgs; full -> close |
| Frame size | 64 KiB (L0) ... 16 KiB (L3) (plan 64 KiB) |
| Resume cache `(device, session)` | <= 64 KiB each; TTL 5 min (plan); global LRU 16 MiB; cleared at L3 / low-mem event |
| Chat reset buffer | 2 MB (plan) per attach, <= 2 concurrent at L2 |
| Phones | <= 8 per room/daemon (plan) |
| Screen helper | fixed texture pool (4), MFT samples <= 2, RTP queue <= 512 KiB; RSS alarm > 300 MB [U] -> restart stream once |
| CDP | 1 JPEG in flight (<= ~300 KB) |
| Notify queue per destination | bounded (plan) 256 events, drop-oldest for non-attention |
- Budget: kelpied additions <= 32 MiB RSS at 8 phones x 20 sessions [U guess, verify in perf gate]; alarm at +10 % growth over 30 min soak.
- Low-memory signals: Windows `LowMemoryResourceNotification` event; macOS memorypressure dispatch source; on trigger: force L2, evict caches, refuse new screen sessions, `load.status` reason `memory`.
- Avoid OOM by design: no unbounded `mpsc::unbounded_channel` on any phone path (lint/grep gate in CI: forbid `unbounded_channel` under `mobile/`). `broadcast` channel `Lagged` already handled (plan RT-12) - also treat Lagged as "send full snapshot".
- Test OOM with a Windows **Job Object** `ProcessMemoryLimit` on the test daemon (reproducible; no host crash) [U].

## 6. Phone side
- Hints phone -> host every 2 s and on change (`client.hint`): `{decode_ms_avg, frames_dropped_rate, render_fps, thermal: nominal|fair|serious|critical, battery_pct, low_power, visible, focus_session, max_res, max_fps}`. Sources: WebRTC `getStats` inbound-rtp (`framesDecoded`, `totalDecodeTime`, `framesDropped`, jitter buffer delay) [U field names exact], iOS `ProcessInfo.thermalState` + `isLowPowerModeEnabled`, Android `PowerManager.getCurrentThermalStatus()` (API 29+) + `isPowerSaveMode` [U], via native shell (phase 5); PWA has no thermal API (use decode time + dropped frames only).
- Host rule: `effective = min(host_level_caps, phone_caps)`. Phone caps: `decode_ms_avg > 25 ms` or dropped > 10 % for 3 s -> step down one rung (720p15 floor 540p10); thermal >= serious or battery < 15 % w/o charging -> L2 caps; `visible=false` -> pause video entirely, keep control channel.
- First 3 s of any stream: probe at 720p30 Baseline 42e01f (plan) and only go 1080p if decode_ms < 8 and the phone says capable.
- Term sheet for terminal diffs: render cost is tiny; but cap DOM updates to rAF-batched, max 10 Hz when phone `low_power`/hidden; virtualised row list (phase 2 requirement). [U]
- Battery: LAN direct, no relay keepalive spam; ping 30 s relay; unsub on background (section 3). Phone shows own "device is hot/battery saver: quality reduced" reason.

## 7. UX: never silently freeze
- Every `screen`/stream frame carries `server_ts` + `level`; phone shows **data age** (now - server_ts, skew-corrected via ping RTT). Header chip: normal = hidden; age > 2 s amber "Updating slowly"; > 5 s red + greyed content "Computer not responding · retrying" (distinct from "Disconnected"/"Phone offline"/"Computer offline").
- `load.status {level, reason[], since}` over control lane at change and every 1 s while level > 0. Phone banner text by reason: "Computer is busy - updates slowed" (cpu), "Low memory on computer" (memory), "Computer is hot / battery saver - reduced" (thermal/battery), "Your connection is slow" (bandwidth, not host).
- Screen overlay (getStats-style, always visible at L1+, collapsible): `720p · 15 fps · 180 ms · reduced: computer busy`. Latency = RTT + known stage ms.
- Desktop: status-bar badge already planned (phase 8); add "Phone control: reduced quality (busy)" tooltip + log line. Refused admission shows desktop toast only if user is present; phone shows `host_busy` with auto-retry countdown (10 s).
- Never auto-stop a stream without a visible reason; auto-pause at L3 > 5 min shows "Paused to protect your computer - tap to resume".
- EN+VI strings for all reason codes (plan constraint).

## 8. Testing and perf gates
### 8.1 Stress tooling
- CPU: custom `scripts/e2e/stress.py` / tiny Rust bin: N threads busy loop at configurable duty (e.g. `duty=100 %` all cores at NORMAL; second profile BELOW_NORMAL competing builds; third = 20 real `cargo build`-like child procs). Reproducible, cross-platform, no install. External: Sysinternals CPUSTRES (Windows GUI, per-thread activity + priority) [U], Prime95 torture (thermal), `stress-ng` on macOS/Linux (`brew install stress-ng`) [U], `yes >/dev/null` x ncpu.
- GPU: no GPU on `windows-latest` / macOS runners [U] -> GPU gates are manual / self-hosted. Tool: wgpu/D3D11 heavy-fragment-shader loop bin (own, deterministic), FurMark/GpuTest as manual cross-check [U]. In CI only the software MFT path + synthetic "slow encoder" test hook (`KELPIE_SCREEN_TEST_ENCODE_DELAY_MS`) to exercise the ladder deterministically.
- Memory: Job Object limit (Windows), `ulimit`/`taskpolicy` or memory-hog bin (macOS).
- Thermal/battery: inject via test hooks on `load.rs` signals (`KELPIE_LOAD_FORCE=L2:thermal`) since can't heat CI.
- Always isolated daemon (`KELPIE_ENDPOINT` set in the script's own process; plan + user memory rule). Stress must never touch the real daemon.

### 8.2 Measuring p95 under load
- **Open-loop load generation** (fixed schedule, measure from *intended* send time; avoids coordinated omission) [U, wrk2 insight]. >= 500 samples per percentile, >= 5 min per profile.
- Terminal: harness phone sends `input` marker char to a fake agent that echoes; latency = marker visible in `screen` frame. Record p50/p95/p99 per Level.
- Screen: test window flips colour on click; in-process str0m receiver (phase 8 integration) measures click->first decoded frame with new colour (glass-to-glass proxy); real-phone camera test manual once per release.
- Loop lag + encode_ms + queue age histograms (hdrhistogram) exposed via debug RPC in release (cheap), dumped by harness.
- CI noise: gate as **ratio vs same-job unstressed baseline** (e.g. p95_L2 <= 3 x p95_L0 and absolute caps below), 3 runs median.

### 8.3 Gates (all phases' e2e include these; numbers are initial [U])
| Gate | Profile | Pass |
|---|---|---|
| False disconnects | all-core 100 % NORMAL, 20 sessions, 8 phones, 30 min | 0 |
| Control-plane latency | same | `input` echo p95 <= 300 ms, p99 <= 800 ms |
| Level machine | stress on/off | reaches L2 ≤ 5 s after start, back to L0 ≤ 25 s after stop, ≤ 2 transitions/min steady |
| Revoke / Stop | during L3 | input stops < 1 s (p100), hotkey Stop < 1 s |
| Never-shed | L3 | 0 dropped `answer/input/key`; any rejection is explicit error |
| Memory | 30 min soak | RSS growth ≤ 10 %, caches ≤ caps, no unbounded queue growth |
| Screen (self-hosted GPU) | 3D shader load 100 % + all-core | stream stays up, fps ≥ 10, encode_ms p95 ≤ 33 at lowest rung, no silent HW->SW switch, glass-to-glass p95 ≤ 400 ms LAN |
| Feature off | idle | no sockets/threads/timers (plan) + `load.rs` probe not running |
| Probe overhead | on, idle | ≤ 0.1 % of one core; 1 Hz sampler allocation-free |

## 9. Mapping to phases
| Phase | Concrete requirement (testable) |
|---|---|
| **1** | Create `crates/kelpied/src/load.rs`: lag probe (100 ms), 1 Hz CPU (`GetSystemTimes`/`host_statistics`), mem, level machine with hysteresis (section 1), `watch<Level>`, started only when phone access on. `sync.rs`: render once/session/tick shared across phones; per-frame ack; latest-only slot; level-driven intervals/caps (section 3); `try_lock` skip; `MissedTickBehavior::Delay`. EcoQoS opt-out + App Nap activity for kelpied (idempotent, only when phone access on). Lag-aware liveness helper. CI grep: no `unbounded_channel` in `mobile/`. Perf gate extension: lag p99 <= 30 ms at gate load; run section 8 profiles. Decision point: second runtime iff lag gate fails. |
| **2** | Phone: `focus`/`visibility` -> sub/unsub; data-age chip; load banner; rAF-batched render; EN+VI reason strings; hints message (decode proxy only on PWA). |
| **3** | Control lane vs bulk lane in `kelpie-link` connection; control queue 64; `grant_alive` atomic checked per event; lag-aware dead-peer rule (2.4); revoke < 1 s at L3 test; pairing rate-limit by level. Noise cost budget test: 8 phones x 10 frames/s x 16 KiB < 1 % core [U]. |
| **4** | Relay keepalive 30 s, host dead-peer 90 s (180 s at L2+); DO auto-response ping if feasible; `desktop_offline` only after lag-aware timeout; slow-consumer close; mapping `host_busy` code; takeover budget unaffected by level (only reduced). |
| **5** | Native hints (thermal, low power, battery, app lifecycle -> unsub); Tauri mobile background -> `visible=false`. |
| **6** | Notify: attention events never delayed > 1 s at any level; digest/coalesce others at L1+/L2+; per-destination bounded queue; no work in loop thread (spawn on blocking-safe worker). |
| **7** | CDP ladder (section 3), 1 frame in flight, pause on no ack 2 s, spike measures msedgewebview2 CPU with/without; CPU drop >= 50 % at L2 vs L0 on busy page; `Stop`/revoke path independent of frame path. |
| **8** | Section 4.2 in full: mailbox, overuse detector 0.85/0.42, rung order, HW/SW rule + visible badge, MMCSS "Capture", EcoQoS off, PLI rate-limit, queue bounds, stats at 1 Hz, test encode-delay hook, admission refusal `host_busy`, spike adds: run under 3D load on >= 2 vendors and record encode_ms p95 with/without load (go/no-go: p95 ≤ 33 ms at 720p15 under load). |
| **9** | macOS: QoS classes (4.1/2.1), `thermalState` + `isLowPowerModeEnabled` + memory pressure source feed `load.rs`; SCK frame interval via `minimumFrameInterval`; VideoToolbox `kVTCompressionPropertyKey_RealTime`, `MaxKeyFrameInterval`; App Nap `beginActivity` while viewer; same ladder & gates [U on API names]. |
| Cross-cutting | Windows: confirm child priority inheritance with a test (daemon ABOVE_NORMAL thread vs process class); do not raise process class. |

## 10. Trade-offs / adoption risk
| Option | Pros | Cons | Verdict |
|---|---|---|---|
| Single level machine + sink-local overrides | DRY, testable, one UX | thresholds need tuning | **Adopt (rank 1)** |
| Second tokio runtime upfront | isolates network | doesn't fix OS starvation; complexity | Only if gate fails (rank 3) |
| Thread priority ABOVE_NORMAL on link threads + MMCSS capture/encode | cheap, no admin | MMCSS quota drop (1-7) when over budget | Adopt (rank 2) |
| HIGH/REALTIME class | strong | can starve system, children inherit risks, needs privilege (REALTIME) | Reject |
| PDH GPU/queue counters as control inputs | "cause" visibility | heavy wildcard enumeration, noisy | Diagnostics only |
| Auto HW->SW encoder switch | resilience | worsens CPU pegging; silent regressions | Only when HW absent/failing |
| Lowering agent build priority (BELOW_NORMAL for hidden terminals) at L2 | frees CPU for control | intrusive to user's builds, touches existing prioritize feature | **User decision needed** (not recommended default) |

## 11. Limitations
Searches returned mostly secondary sources; Parsec/RustDesk/CRD internals not found; no public WebView2 screencast CPU numbers; no GPU-contention MFT benchmarks beyond anecdotes; all numeric thresholds are starting values to be tuned by the harness (marked [U]). Did not read phases 2,5,9 in detail nor the kelpied source (daemon.rs/session.rs); lock/hold-time claims rely on plan text. Windows 10 19045 lacks Win11-only Eco QoS level; behaviour differs between 10 and 11 (verify on both).

## Unresolved questions
1. Does a detached per-user `kelpied` without window really get Low QoS on battery on Win10 19045 / Win11? Needs a 30-min `xperf`/Process Explorer check before relying on the opt-out being impactful.
2. Child priority inheritance: confirm ABOVE_NORMAL parent -> NORMAL child (CreateProcess docs from memory).
3. Should hidden/background agent processes be demoted (BELOW_NORMAL) at L2? Product decision; interacts with existing "Prioritize visible terminals".
4. Is an H.264 HW MFT actually exposed on NVIDIA/AMD/Intel target hosts (driver-dependent)? Decides whether phase 8 hits the SW fallback often.
5. Does CF Durable Object hibernation auto-response pong keep rooms alive without waking? Verify phase 4.
6. Allowed phone thermal/battery data on PWA = none; accept decode-time-only adaptation there?
7. Hosted relay budget interaction: should level reduce relay-session byte quota use (not only CPU)?

## Sources
- MS Quality of Service (EcoQoS, QoS levels, MMCSS tags): https://learn.microsoft.com/en-us/windows/win32/procthread/quality-of-service
- MS MMCSS (tasks, priorities, SystemResponsiveness, GPU priority unused): https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service
- MS SetProcessInformation / SetThreadInformation: https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-setprocessinformation , https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadinformation
- Tokio task docs: https://docs.rs/tokio/latest/tokio/task/index.html
- Apple QoS classes: https://developer.apple.com/library/archive/documentation/Performance/Conceptual/power_efficiency_guidelines_osx/PrioritizeWorkAtTheTaskLevel.html
- libwebrtc overuse detector: https://webrtc.googlesource.com/src/+/HEAD/video/adaptation/overuse_frame_detector.h
- MDN qualityLimitationReason: https://developer.mozilla.org/en-US/docs/Web/API/RTCOutboundRtpStreamStats/qualityLimitationReason
- Task Manager GPU engine semantics (busiest engine): https://devblogs.microsoft.com/directx/gpus-in-the-task-manager/
- GPU Engine PDH naming Q&A: https://learn.microsoft.com/en-us/answers/questions/5641645/how-to-get-the-special-process-gpu-usage-with-the
- NVENC MFT latency drift: https://forums.developer.nvidia.com/t/nvenc-h-264-encoder-mft-latency-increases-when-framerate-is-limited/63930 ; https://forums.developer.nvidia.com/t/h-264-video-encoder-latency/69872
- GStreamer MF encoder async latency: https://gitlab.freedesktop.org/gstreamer/gst-plugins-bad/-/merge_requests/1520
- Sunshine silent SW fallback: https://github.com/LizardByte/Sunshine/issues/2141 ; fps mismatch: https://github.com/LizardByte/Sunshine/issues/1167 ; tuning: https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2performance__tuning.html
- Parsec bitrate/low-bandwidth: https://support.parsec.app/hc/en-us/articles/32381776828820-Setting-The-Maximum-Bitrate-Of-The-Stream , https://support.parsec.app/hc/en-us/articles/32361399881620-How-To-Use-Parsec-On-Low-Bandwidth-Connections
- OBS encoder overload: https://streamvi.io/en/docs/diagnosis/encoder-overload ; https://obsproject.com/forum/threads/encoding-overload-error-and-frame-drops.104918/latest
- CDP screencast behaviour: https://macmyths.com/how-to-fix-low-fps-chrome-devtools-protocol-screencasts/ ; https://rubydoc.info/gems/ferrum/Ferrum/Page/Screencast ; https://github.com/browser-use/browsercode/pull/124
- Tokio starvation discussion: https://github.com/rust-lang/wg-async/pull/144
