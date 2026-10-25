# Windows host: capture, encode, input

2026-10-24 · [V] = verified this session · [U] = background knowledge, unverified. ~14 searches/fetches.

## 1. Secure desktop, lock screen, login screen

**RustDesk** (`src/platform/windows.rs`, ~100k/171k chars read) [V]
- SYSTEM service, not `uiAccess`. No `uiAccess`/`OpenInputDesktop` in read part; nearest is separate broker exe (`INJECTED_PROCESS_EXE`).
- Service: `start_os_service`, `service_main`, `run_service`; tracks active console session (`get_current_session`), launches server into it (`launch_server`, `LaunchProcessWin` `as_user=FALSE` → child runs SYSTEM).
- Desktop switch: `desktop_changed` → `inputDesktopSelected`; `try_change_desktop` → `selectInputDesktop` (C helpers). Capture/input threads re-attach to current desktop (Default/Winlogon/Screen-saver).
- Probes: `is_prelogin`, `is_logon_ui` (`LogonUI.exe`), `is_logon_screen`, `is_session_locked_flag`, `lock_screen`.
- Ctrl+Alt+Del: `send_sas` → `sas.dll` `SendSAS`, toggling `SoftwareSASGeneration` policy. SYSTEM only.
- Elevation: `is_elevated`, `run_uac` (`runas`), `run_as_system`, `is_foreground_window_elevated`.
- Install `install_me`: HKLM uninstall, firewall rules, service → per-machine.
- Portable mode dies at logout/UAC prompt; service mode needed for unattended/login ([guide](https://rustdesk.com/blog/rustdesk-unattended-access-setup)).

**Sunshine** [V partial]
- `display_base.cpp` calls `syncThreadDesktop()` before each duplication attempt (body not seen; presumably `OpenInputDesktop`+`SetThreadDesktop`).
- `DXGI_ERROR_ACCESS_LOST`/`ACCESS_DENIED` → `capture_e::reinit`, rebuild on new desktop.
- Runs as Windows service ([troubleshooting](https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2troubleshooting.html)) → sees UAC.

**What needs what** ([MS UIAccess overview](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview), 2026-02) [V]
- Medium-IL app can't inject into elevated windows (UIPI).
- `uiAccess=true` needs Authenticode + UAC-protected location (Program Files) + manifest flag.
- `uiAccess` from non-admin launch → "medium+" only (not elevated); admin launch → "high". **Never** reaches SYSTEM-IL secure desktop ("setting UIAccess has no effect").
- → UAC, lock, login, Ctrl+Alt+Del need SYSTEM process on Winlogon desktop. `uiAccess` only for elevated windows on normal desktop. (Correction to main report §4.4: `uiAccess` is not a secure-desktop route.)

**Per-user `currentUser` NSIS app can:** capture + input on user's Default desktop. **Cannot:** UAC, lock, login, Ctrl+Alt+Del, elevated windows (view ok, clicks dropped).

**Minimum optional SYSTEM service** (design [U], modelled on RustDesk/Sunshine)
1. Separate elevated opt-in installer → `kelpie-screen-svc` in Program Files; app stays per-user.
2. Service only: watch sessions (`WTSRegisterSessionNotification`/`WTSGetActiveConsoleSessionId`); launch `kelpie-screen` into active session as SYSTEM (`CreateProcessAsUser`, duplicated SYSTEM token, `SetTokenInformation(TokenSessionId)`); optional `SendSAS`.
3. `kelpie-screen` calls `OpenInputDesktop`/`SetThreadDesktop` per loop and on desktop-change error, reinit capture.
4. IPC kelpied↔helper: named pipe, ACL same user + peer-exe check (cf. RustDesk `authorize_service_scoped_ipc_connection`).
5. Cost: code signing, admin install, SYSTEM binary accepting remote input → big attack surface, own threat model. Not v1.

## 2. Capture: WGC vs DXGI Desktop Duplication

| | WGC | DXGI DD |
|---|---|---|
| Min OS | 1803 monitor/window [U]; `CreateFromHwnd` interop 1903 [U] | Win8+ |
| Per-window HWND | Yes, unaffected by occlusion [U] | No, monitor only |
| Minimized window | Frames stop/freeze [U] | N/A |
| Border | see below | None |
| Cursor | `IsCursorCaptureEnabled`, SDK 19041 [V] | Shape+pos separate, composite yourself |
| Secure desktop | Fails non-SYSTEM [U] | ACCESS_LOST/DENIED; works from SYSTEM after `SetThreadDesktop` [V partial] |
| HDR | FP16 format [U] | `DuplicateOutput1`; Sunshine converts scRGB→Rec.2020, checks `IDXGIOutput6` G2084 [V] |
| Multi-monitor | 1 item/monitor | 1 duplication/output (4-per-process limit [U]) |

**Yellow border** ([IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired)) [V]
- Needs 10.0.20348 ("2104") — not 19045. Also `GraphicsCaptureAccess.RequestAccessAsync(Borderless)` (user prompt) + `graphicsCaptureWithoutBorder` capability (MSIX-style; unpackaged NSIS app likely can't [U]). Denied → border shows.
- Assume border always on for WGC window capture. Arguably good: marks shared window.

**Who uses what**
- Sunshine: DDUP first, WGC fallback [V]. DDUP works from SYSTEM across desktop switches.
- RustDesk `libs/scrap/src/dxgi`: `mod.rs`, `gdi.rs`, `mag.rs`, `hdr.rs` [V filenames] → DXGI + GDI + Magnification fallbacks, no WGC seen. No per-window need.
- Kelpie needs per-window (IDE) → WGC; DXGI DD for monitor + future service.

**Occlusion:** DD shows composited top window; WGC HWND reads window surface (not hidden by overlap), freezes when minimized [U].

## 3. Encode from Rust

**Media Foundation H.264 MFT** ([MS Learn](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)) [V]
- Input NV12/I420/YUY2; set output type before input type.
- `AVEncCommonRateControlMode`: CBR/PeakConstrainedVBR Win8+; default unconstrained VBR.
- `AVEncCommonMeanBitRate` changeable live (Win8+); `CODECAPI_AVEncVideoForceKeyFrame` → BWE bitrate + PLI.
- B-frames off by default; `AVEncMPVGOPSize=0` = encoder picks → set large GOP.
- `CODECAPI_AVLowLatencyMode=VARIANT_TRUE` = low-latency switch (optional in HCK).
- HW MFTs (NVENC/AMF/QSV) via `MFTEnumEx` HW flag, D3D11 texture input via `IMFDXGIDeviceManager` [U]. Inbox SW = `Mfh264enc.dll`.
- Intra refresh not in property list; LTR/temporal layers optional [V]. Plan: on-demand keyframes + large GOP [U].

**Latency numbers:** none verified. "Encode 3–8 ms" NVENC 1080p60 ULL [U]; inbox SW 10–30+ ms 1080p [U]. Measure.

| Option | License | Notes |
|---|---|---|
| MF HW MFT via `windows` crate | MIT/Apache | Vendor-neutral; per-GPU quirks (AMD/Intel low-latency/rate props) [U] |
| NVENC direct | SDK license | NVIDIA only, fastest |
| AMF / QSV direct | MIT | Vendor-specific |
| ffmpeg bindings | LGPL (no GPL parts) | Heavy Windows build, DLL/static, compliance |
| openh264 | BSD | Cisco patent grant only for Cisco binary downloaded at runtime [U] |

- Check `rustdesk-org/hwcodec` license before reuse [U]. Match SDP `profile-level-id` to mobile decoders (constrained baseline/high).

## 4. str0m vs webrtc-rs

**str0m** [V] ([repo](https://github.com/algesten/str0m), [CHANGELOG](https://raw.githubusercontent.com/algesten/str0m/main/CHANGELOG.md), [crates.io](https://crates.io/crates/str0m))
- 0.24.1 (2026-10-03), ~1.16M recent downloads, 639 stars, 22 open issues.
- Crypto features: `aws-lc-rs` (default), `rust-crypto`, `openssl`, `openssl-dimpl`, `apple-crypto`, `wincrypto`, `wincrypto-dimpl`. 0.24.0 removed SChannel → Windows DTLS via dimpl + CNG.
- Avoid aws-lc (NASM) and OpenSSL: `default-features=false` + `wincrypto` or `rust-crypto`. Build unverified → spike.
- TWCC + BWE supported; 0.24.0 SSRC-0 probing + TWCC probing state in BWE events (breaking); 0.24.1 caps paced bursts ~63 kB.
- H.264: frame-level `Writer::write`; single-NAL/STAP-A/FU-A detection + filler NAL preserve in **unreleased** section. Safari SDP handling unverified.
- SCTP data channels (configurable buffer/msg size) → input channel.
- Users: BitWHIP, Lookback. No desktop-streaming user found → Kelpie would be first (risk).

**webrtc-rs** [V snippets] ([0.20](https://docs.rs/crate/webrtc/0.20.0)): 0.17.x bug-fix only; 0.20.x stable sans-I/O rewrite, Tokio/smol, breaks 0.17. BWE/H.264/crypto maturity unverified.

→ str0m first; webrtc-rs 0.20 fallback if spike fails. Not 0.17.

## 5. Input injection (all [U], verify in spike)
- Text/passwords: `SendInput` `KEYEVENTF_UNICODE` (`wScan`=UTF-16 unit, `wVk=0`; surrogates = 2 events). Layout-independent, bypasses IME. Some apps mishandle `VK_PACKET`; browser login fields fine.
- Clipboard paste: set `CF_UNICODETEXT`, Ctrl+V, restore clipboard. Better for long text; clobbers clipboard, clipboard managers may log → not for secrets. Offer both.
- Mouse: `MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK`, 0..65535 over virtual desktop; from monitor rect (`GetMonitorInfo`) + `SM_XVIRTUALSCREEN/SM_YVIRTUALSCREEN`. Per-monitor DPI v2 in helper manifest. Watch rounding on negative-origin monitors.
- Per-window: map to `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`; overlap → click hits top window; optionally bring to front first.
- UIPI: elevated targets silently dropped; check `GetTokenInformation(TokenElevation)` on foreground PID, warn on phone.
- Secure desktop: `SendInput` only on input desktop (SYSTEM case).
- Moves: unordered DataChannel `maxRetransmits=0`; keys reliable.

## Ranked recommendation
1. v1 per-user: `kelpie-screen` helper; WGC (window) + DXGI DD (monitor); MF HW H.264 (`AVLowLatencyMode`, CBR, no B, on-demand IDR, live bitrate from BWE); str0m `wincrypto`/`rust-crypto`; `SendInput` UNICODE + paste mode; detect+report secure desktop, locked, elevated foreground, minimized.
2. Later optional: SYSTEM service, separate elevated installer, only if login/UAC is hard requirement.
3. Fallback: webrtc-rs 0.20.

## Limitations
No measured numbers; §5 and MF/GPU quirks [U]; `syncThreadDesktop` body and last 72k chars of RustDesk `windows.rs` unread; MF Rust crates unverified; `wincrypto` untested on this toolchain.

## Unresolved questions
1. `IsBorderRequired=false` for unpackaged NSIS app on 19045/Win11?
2. str0m `wincrypto`/`rust-crypto` builds on MSVC without NASM/cmake?
3. iOS Safari / Android Chrome H.264 SDP needs (`packetization-mode=1`, `profile-level-id`)?
4. WGC on minimized target — "window gone" UX?
5. AMD/Intel MFT honour `AVLowLatencyMode` + live bitrate? Test ≥3 GPUs.
6. `hwcodec` license?
7. Legal/threat model for SYSTEM service + remote input.

## Sources
[RustDesk windows.rs](https://github.com/rustdesk/rustdesk/blob/master/src/platform/windows.rs) · [RustDesk scrap dxgi](https://github.com/rustdesk/rustdesk/tree/master/libs/scrap/src/dxgi) · [RustDesk unattended](https://rustdesk.com/blog/rustdesk-unattended-access-setup) · [Sunshine display_base.cpp](https://github.com/LizardByte/Sunshine/blob/master/src/platform/windows/display_base.cpp) · [Sunshine troubleshooting](https://docs.lizardbyte.dev/projects/sunshine/latest/md_docs_2troubleshooting.html) · [IsBorderRequired](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired) · [GraphicsCaptureSession](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession) · [UIAccess](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-securityoverview) · [MF H.264 encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder) · [str0m](https://github.com/algesten/str0m) · [str0m CHANGELOG](https://raw.githubusercontent.com/algesten/str0m/main/CHANGELOG.md) · [str0m crates.io](https://crates.io/crates/str0m) · [webrtc 0.20](https://docs.rs/crate/webrtc/0.20.0) · [webrtc 0.20.3](https://docs.rs/crate/webrtc/0.20.3)
