# Research: takeover patterns, macOS capture permissions, OSS streaming reuse

Date 2026-10-24. ~13 searches/fetches. Search tool returns thin summaries; many facts are [UNVERIFIED] or [KT]. Complements `research-261024-1820-remote-desktop-view-control.md`.

## A. Human-in-the-loop takeover patterns

| Product | Transport | "Human needed" signal | Credential handling | Source quality |
|---|---|---|---|---|
| Browserbase Live View | iframe embed of `debuggerFullscreenUrl` (from `sessions.debug`); transport not documented [UNVERIFIED; likely CDP screencast+input, not VNC]. R/W iframe = click/type/scroll; read-only = `pointer-events:none`; `browserbase-disconnected` postMessage | Developer-built (your app decides when to show iframe) | Docs say "delegate credentials" to the human; recording of typed input not addressed | Official docs, thin |
| Steel | Same pattern (debug/session viewer URL). Steel docs warn debug URLs are NOT authenticated: anyone with URL sees session | Dev-built | Not verified (HITL guide 404'd) | Vendor docs via search snippet |
| OpenAI Operator | Hosted browser | Agent itself asks user to "take over" for login/payment | **Operator does not collect or screenshot anything typed in takeover mode** (OpenAI blog) | Official, 2025 preview; current status unverified |
| Anthropic computer-use-demo | Docker X11 + VNC; noVNC on :6080, VNC :5900, combined UI :8080 | None; human just shares same desktop. No pause/takeover protocol | Advice: don't give model logins; allowlist; human confirms consequential actions; VNC ports unauthenticated, bind localhost | Official README. Xvfb/x11vnc/noVNC inferred from ports, not named |
| vercel-labs/agent-browser (Apache-2.0) | WebSocket stream of JPEG frames (dashboard live view); `stream enable --port`; `--headed` = plain headed window "for debugging" | None | n/a | Repo README; input-back path not confirmed; a branch `stream-input-latency` suggests input exists |
| Cursor/Devin/Manus/E2B | NOT researched (budget). [KT] E2B Desktop = Xvfb + noVNC-like stream; Devin/Manus = VNC-ish browser view. Unverified |

**Patterns that matter (cross-source, 3 independent: Operator, Browserbase, Anthropic):**
1. Takeover is app-level UX, not transport. Agent emits "needs human" state -> UI shows live view in R/W mode -> human hands back via explicit "Done" button. Kelpie already has the "needs you" state; just attach the viewer to it.
2. Credential isolation = the agent must not see the pixels/keystrokes during takeover. Only Operator states it (no screenshots while in takeover). For Kelpie: while takeover active, (a) pause the agent's own screenshot/DevTools reads, (b) never log input events or the text-box content, (c) don't route human keystrokes through PTY/transcript. Cheap, high value. [Design inference]
3. Viewer URLs are bearer secrets (Steel warning). Kelpie's paired-token + Origin check is already stronger.

**Lighter alternative for browser login: CDP screencast. Verdict: yes, do it first.**
- `Page.startScreencast` emits JPEG/PNG frames only on page change; `Input.dispatchMouseEvent/KeyEvent` (+ `Input.insertText` [KT] for OTP/password, layout-independent) feed back. agent-browser uses exactly this (WS + JPEG). Must keep `Page.screencastFrameAck` flowing or you get backpressure/low FPS (macmyths troubleshooting article, low-grade source).
- Gotchas: CDP `maxWidth/maxHeight` are signed ints (agent-browser bug: whole call rejected, zero frames); only captures page content, so OAuth popups = new targets (must attach), native dialogs/extensions/passkey prompts/OS file pickers are invisible; captcha/anti-bot may detect... [UNVERIFIED].
- Trade: ~zero new native deps, works on Win/mac/Linux, tiny bandwidth, no OS permissions (no TCC, no UAC issue), reuses Kelpie's existing DevTools browser pane + phone WS. Cost: JPEG over WS (TCP) is OK on LAN/Tailscale for a login form. Does NOT cover native-app testing or IDE.
- Recommendation: Tier 1 = CDP screencast takeover for browser panes (covers OAuth/2FA/captcha, the stated pain). Tier 2 = full desktop WebRTC only if native-GUI takeover is truly needed.

## B. macOS host

**Verified-ish (3 sources: 9to5mac, TidBITS, Apple forum):**
- Sequoia 15 screen-recording consent: started weekly in betas, shipped monthly ("Allow For One Month"); applies to ScreenCaptureKit apps too. 15.1 notes: apps on deprecated capture APIs get "enhanced user awareness".
- `com.apple.developer.persistent-content-capture`: obtained via Apple's **request form** (Apple forum OP, vendor of remote-control product, targets macOS 14.4+). Apple gave no public docs in 2024 (Hockenberry). Intended for VNC/remote-desktop-style apps. Eligibility criteria unpublished -> assume Apple discretion, likely requires a signed, notarized, Developer-ID app with a real remote-desktop purpose. [UNVERIFIED]
- New TCC category: **"Remote Desktop"** separate from "Screen & System Audio Recording" (shown on macOS 15-26 per N-able/Take Control docs; forum thread observed on macOS 27 beta). Granting one does not grant the other. Remote Desktop grant does NOT expire monthly (N-able). Internal name `kTCCServiceRemoteDesktop`; no public API to query it silently; `CGPreflightScreenCaptureAccess` reports only the Screen Recording grant; `SCShareableContent` shows a consent alert each call (Apple DTS Quinn confirmed no API; FB24263354). Exactly how entitlement maps to this category: unanswered in thread.
- macOS 26.1 forum report: unbundled executables still capture once granted but do NOT appear in the Screen Recording list; only .app bundles show. **Implication for Kelpie: a bare `kelpie-screen` helper binary would be invisible in Settings; ship it inside the .app bundle (or have the main app bundle ID own capture).** (single forum source)
- MDM cannot silently grant Screen Recording (Level docs); Accessibility can be MDM-granted via PPPC [KT].
- Accessibility: `CGEventPost` needs Accessibility TCC [KT, standard]. N-able/ManageEngine docs list both Accessibility + Screen Recording for remote control (consistent).
- Login window / lock screen: user-session agent cannot capture (confirmed in main report from Apple forum). RustDesk/Jump/Screens deal with it by installing a LaunchDaemon/privileged helper [KT, not verified this session]; no search evidence on those three.

**Cost-benefit for Kelpie:** monthly re-prompt is a nuisance, not a blocker, for an occasional-takeover feature. Applying for the entitlement = optional later polish, not a gate. Not researched: Jump Desktop/Screens internals (nothing returned).

**VideoToolbox low-latency H.264 [KT, not re-verified]:** `kVTCompressionPropertyKey_RealTime=true`; `kVTVideoEncoderSpecification_EnableLowLatencyRateControl=true` (macOS 11.3+, forces hw, no B-frames); `AllowFrameReordering=false`; `ProfileLevel` Baseline/Main AutoLevel; `MaxKeyFrameInterval` long + force keyframe on PLI; `AverageBitRate` + `DataRateLimits` to cap bursts; `PrioritizeEncodingSpeedOverQuality=true` [KT]; output AVCC -> convert to Annex-B for RTP. Need to verify against Apple docs/WWDC21 "Low-latency video encoding" before coding.

## C. Open-source reuse candidates

| Project | License | Stack | Status | Fit |
|---|---|---|---|---|
| m1k1o/neko | Apache-2.0 | Go server + Vue client, WebRTC, Docker, ~22.5k stars, multi-participant control | Active-ish: fork continued after original archived; "last commit 1mo ago" per one listing; page gave no dates | Linux/container-oriented; capture via GStreamer/Xorg. Not for Win/mac host. Learn from its control-host/handover UX only |
| Selkies | MPL-2.0 (+ third-party GPL bits per its licensing docs) | Python, HTML5 client, WS default, WebRTC opt-in; Linux X11/Wayland only | ~2.4k stars, README says "needs maintainers"; low open issues | Linux only. Reject for reuse |
| MrCreativ3001/moonlight-web-stream | **GPL-3.0** | Rust server + TS client, Moonlight protocol -> WebRTC, WS fallback | ~731 stars, 1,279 commits, migration guides v2/v3 suggest active; no dates seen | GPL = cannot embed in MIT Kelpie; needs Sunshine (GPL) host anyway. Reference only |
| vercel-labs/agent-browser | Apache-2.0 | CDP screencast over WS | active (many branches) | Best template for Tier 1; reading its stream code is permitted reuse |
| str0m / windows-capture / etc. | MIT (str0m MIT/Apache [KT]) | see main report | | already in main report |

Finding: **no permissively-licensed, maintained, cross-platform (Win+mac host) browser-desktop-streaming project found.** All good ones are Linux-hosted or GPL. Conclusion in main report stands: build thin (str0m + OS APIs) or recommend RustDesk/Tailscale as external. Not found: any 2025-26 MIT/Apache Rust "WebRTC desktop capture" crate with traction (search budget; GitHub topic search not done).

## Ranked recommendation
1. **Tier 1 now: CDP screencast + Input dispatch in Kelpie's browser pane**, "Take over" button on needs-you card, agent screenshots/logging paused during takeover, `Input.insertText` for secrets not logged. Cheapest, cross-platform, no TCC/UAC. Risk: only in-browser content (OAuth in same browser is fine; passkey/native prompts not).
2. Tier 2 (only if native GUI takeover really needed): WebRTC desktop helper per main report; on macOS ship helper inside .app, request Screen Recording + Accessibility, treat monthly prompt as acceptable, consider entitlement form later.
3. External fallback: Tailscale + RustDesk docs for login window/UAC cases.

## Limitations
Search tool gave summaries not primary docs; Steel HITL guide 404; Browserbase transport undocumented; Cursor/Devin/Manus/E2B not covered; macOS permission details rest on vendor docs + one Apple forum thread (macOS 27 beta); RustDesk/Jump/Screens behavior on macOS not verified; VideoToolbox params unverified; project dates/commit recency not seen.

## Unresolved questions
1. Does Kelpie's DevTools browser pane run headed Chrome on the host desktop or a separate profile? (affects whether CDP screencast sees OAuth popups/targets)
2. Is native-GUI takeover (not browser) actually required for v1, or is browser login the real pain?
3. Which cases need passkeys/WebAuthn/native pickers (invisible to CDP)?
4. Entitlement eligibility criteria and whether it maps to the "Remote Desktop" TCC category -- need Apple docs/request form.
5. Does a bundled helper inside Kelpie.app inherit the app's TCC entry? Test on macOS 26.
6. Steel/Browserbase live-view transport and credential recording policy -- ask vendors or inspect network traffic.

Status: DONE_WITH_CONCERNS
Summary: Report written to D:/Works/Kelpie/plans/reports/researcher-261024-1820-takeover-patterns-macos.md. Recommends CDP-screencast takeover for browser panes first (agent-browser/Operator pattern), WebRTC desktop only if needed; no permissive cross-platform OSS to reuse (Neko/Selkies Linux-only, moonlight-web GPL-3.0).
Concerns/Blockers: Search tool returned thin summaries; macOS entitlement/TCC details rest on one Apple forum thread and vendor docs; VideoToolbox, Cursor/Devin/Manus/E2B, RustDesk/Jump/Screens macOS behavior unverified.
