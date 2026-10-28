# Mobile session protocol + UX (phone as full controller, flaky 4G)

Date 2026-10-24. Scope: streaming protocol, mobile renderer, structured agent UI, ranked recs. ~14 fetches/searches; [U] = unverified / from memory / inference.

## 0. Baseline (read locally)

- `crates/kelpied/mobile/app.js` (258 lines): WS `auth` token -> `hello`/`sessions` push (1 s tick diff) + **poll** `read {lines:200}` every 1.2 s returning **plain text tail** (`text_tail`) rendered in `<pre>`. Replaces whole text on any change. Fixed reconnect 2.5 s, no heartbeat, no seq, no resume.
- Input: `input {text, enter}` (daemon sleeps 120 ms then `\r`) + 11 fixed keys. No idempotency id; a retry after a dropped ack on 4G = duplicate send.
- **Chat sessions are dead-ended on phone** ("open it on your computer"); `mobile.rs` returns a stub for `Entry::Chat`. Biggest gap vs "full controller".
- Daemon already has the hard parts, unused by phone: `session.attach {since_seq}` -> delta / raw reset / ANSI snapshot (architecture.md:117-129); `choices::detect` + `session.answer {waiting_since,key|data}` race-safe prompt answering (architecture.md:156-165); chat JSON-line replay log; Claude `control_request` permission; Codex approvals; ACP `request_permission` with option ids (architecture.md:201-215).
- Bug (real, cheap): app.js:224 submits on Enter with no `isComposing`/`keyCode===229` guard. Android Gboard / Vietnamese keyboards wrap Enter/Backspace in composition events -> mid-composition submit or dup text. [U on exact Vietnamese keyboards; mechanism confirmed in xterm #3600 below].

## 1. Streaming protocol

| Approach | Resume/flaky-link | Bandwidth | Complexity | Fit |
|---|---|---|---|---|
| Raw byte stream (ttyd, gotty, wetty: xterm.js over WS [U from memory]) | Poor: no seq; reconnect = blank/stale; must replay | Highest on TUI redraw floods (Claude Code spinner/diff repaints) | Lowest | Bad alone |
| Mosh SSP (state sync, UDP, 70% speculative echo, jump to newest state) [paper](https://mosh.org/mosh-paper.pdf) | Best: skip intermediate frames, roam IPs | Lowest | Very high (UDP, crypto, own protocol); browser has no UDP | Take the *idea*, not the protocol |
| Screen snapshot / row-diff of alacritty grid | Good: any frame is self-sufficient | Low | Medium (diff + fallback) | Good, later |
| Existing attach: since_seq delta else snapshot | Good | Medium-low | **Already built** | Best first step |
| tmux control mode (`%output`, iTerm2 style) | Needs tmux in loop | Mid | High; Kelpie has own VT state, redundant | Reject |
| VibeTunnel | Sends PTY output to web (Lit + ghostty-web), asciinema for recording, native iOS app WIP "not recommended for production" ([repo](https://github.com/amantus-ai/vibetunnel)); wire format not documented on page -> [U] buffer snapshots | - | - | Evidence the category is immature |
| Blink / Termius | Native apps using Mosh / SSH [U] | - | - | N/A (not web) |

Key insight from mosh: **terminal is state, not a stream**; rate-limit frames and, after a gap, jump to the newest state rather than replaying. Kelpie's daemon already holds the state (alacritty_terminal) and the attach logic implements exactly this (delta if cheap, snapshot otherwise).

Compression: terminal text frames are small (a 24x80 ANSI snapshot is a few KB [U]); win comes from **frame coalescing** (50-100 ms on phone vs 4 ms desktop) and sending snapshots rather than redraw floods, not from codecs. permessage-deflate: axum/tungstenite support is doubtful [U, verify]; browsers lack native zstd decode in JS without wasm [U]. If needed: app-level `DecompressionStream("deflate-raw")` on snapshots > ~1 KB (supported in modern browsers [U]). Defer until measured.

Battery/data: push-on-change beats 1.2 s polling (phone radio wakes constantly; JSON re-sends 200 lines when 1 changed). Pause stream when `document.hidden` (mobile browsers kill background WS anyway), resume with `since_seq` on `visibilitychange`.

## 2. Mobile terminal renderers

Evidence ([xterm #5377](https://github.com/xtermjs/xterm.js/issues/5377), open, removed from 7.0.0 milestone): maintainers say frontend is desktop-oriented, mobile "essentially absent"; touch scroll/selection/long-press/pinch missing; virtual-keyboard IMEs batch characters, which maintainers consider out of scope. Workarounds exist (custom selection handles, `touch-action`, DOM renderer/`screenReaderMode`, custom keyboard bar) but are DIY.
- [xterm #3600](https://github.com/xtermjs/xterm.js/issues/3600) (open since 2022, low priority): Chrome Android + Gboard doesn't reliably fire key events, wraps Enter/Backspace in composition -> duplicated/corrupted text. Workaround: never focus xterm's helper textarea; feed it from your own input.
- [#6012](https://github.com/xtermjs/xterm.js/issues/6012) (open, Jun 2026): hidden textarea keeps IME text until blur; voice IME input corrupted. [#6045](https://github.com/xtermjs/xterm.js/issues/6045) (open, Jul 2026): keyCode 229 rollover dup/drop chars. => Vietnamese Telex typing *into xterm.js* on phone is a known-broken path in 2026. [Direct Telex-in-xterm report not found; inferred from these; Telex failures on mobile reported in other editors e.g. [Wiki.js](https://feedback.js.wiki/wiki/p/vietnamese-telex-input-not-working-in-table-editor-and-on-mobile-devices).]
- ghostty-web ([coder/ghostty-web](https://github.com/coder/ghostty-web)): ~3k stars, xterm-API-compatible goal, relies on patched Ghostty, "works best on Linux and macOS", Chrome the only supported browser (per README summary), no mobile/touch/IME story. Native libghostty mobile apps exist (Echo iOS, Chuchu Android) but are not web. => Not mobile-ready.
- hterm, others: no evidence of better mobile IME [U, not researched].

Conclusion: **never use xterm.js (or any terminal widget) as the input surface on the phone.** Input = native `<textarea>` (platform IME handles Telex/VNI, you get final committed text) + key bar. xterm.js, if used at all, is read-only (`disableStdin`, no focus) for fidelity (colors, TUI layout) with CSS fit-to-width + pinch-zoom/pan; keep the current plain-text "reflow" view as a toggle because a 100+ col PTY at 390 px is unreadable. Resizing the PTY from the phone would fight the desktop; avoid.

## 3. Structured agent UI

- Anthropic ships Claude Code Remote Control (app/claude.ai attach, approve/deny prompts) but needs a claude.ai Pro/Max plan, and 2026 issues report approvals not reaching the host, hangs, plan-mode prompts not rendering ([search summary](https://github.com/anthropics/claude-code/issues/32493), [hang issue](https://claudeissues.com/issue/61177-remotecontrol-permission-approval-from-mobile-app-hangs-session-indefinitely)). Third-party: Forge Remote (iOS, risk labels per command). => Demand is real; reliability is the differentiator, and it only covers Claude. Kelpie's multi-agent (Claude/Codex/ACP) is the edge.
- ACP: `session/request_permission` is a baseline agent->client method with agent-supplied option ids; clients "can be other UIs" ([spec](https://agentclientprotocol.com/protocol/overview)). No ACP mobile client found in research [U: none exists / not found]. Kelpie already normalizes Claude/Codex/ACP into one item model in `app/src/chat` -> phone should consume that model, not re-implement three dialects.
- Permission card (native buttons): title = tool + one-line intent; collapsed monospace command/path/diff-stat; buttons = the agent's own option ids (Allow once / Always / Deny; Claude can_use_tool, Codex approval, ACP options); big touch targets (>=48 px), destructive-styled Deny never adjacent to Allow-always; stale-guard like `waiting_since` so a late tap on an already-answered prompt is a no-op (idempotency key).
- Diff review on 390 px: unified only (no side-by-side), per-file collapsed with +/- counts, per-hunk expand, no-wrap + horizontal scroll with sticky line gutter off, syntax color minimal. Real code review stays on desktop; phone job = approve/deny + skim. Offer "Open on desktop" for >N lines.
- Composer: multiline `<textarea>` with explicit Send (Enter = newline on touch, Send button submits; guard `isComposing`/229), per-session draft persisted in localStorage, `/` and `@` menus fed from the adapter's command list/file index (exists in `chat-composer.tsx`), model/mode pickers collapsed in a sheet, queue-while-offline with client message id (server dedupes), Stop/interrupt button always visible while working.
- Voice: Web Speech API is partial on both Chrome Android 147 and iOS Safari 14.5-26.5 per [caniuse](https://caniuse.com/speech-recognition), prefixed on Safari, cloud-backed, reports of dead `result` events and no support in installed PWAs. Keyboard dictation mic (Gboard / iOS) works with any textarea, handles Vietnamese, zero code. => Don't build Web Speech; tell users to use keyboard mic. (Voice also corrupts xterm hidden textarea per #6012; another reason for native textarea.)
- Notifications: `navigator.vibrate` (current) only fires while the page is foreground; mobile browsers suspend background WS. "Needs you" is the killer phone feature, so it needs Web Push (service worker, VAPID, HTTPS; iOS only for installed PWA >=16.4 [U]) or an ntfy-style relay. Not researched deeply; flag as design question.

## 4. Recommendation (ranked)

Cost = rough effort. Risk to Kelpie: low unless noted.

1. **Fix IME submit guard now** (app.js:224: ignore Enter when `e.isComposing || e.keyCode===229`; on touch default Enter=newline, Send button submits). 5 lines. Needed for Telex/VNI.
2. **Chat-session parity on phone** (highest value). Push chat items over WS as they arrive: `chat.attach {since_seq}` -> replayed items then live, each with `seq`; client renders items (user/assistant/tool card/permission/result) + composer + interrupt. Open decision: (a) bundle the existing TS adapters (`app/src/chat`) into a mobile build (reuse, adds a build step to a no-build vanilla page) vs (b) normalize to items in Rust (duplicate logic, drift). Prefer (a) if adapters are DOM-free [U, check]; else send raw relay lines + `kelpie_time` and run adapters client-side. Permission = native buttons (section 3). Medium-large.
3. **Native prompt cards for terminal sessions**: when `status==waiting`, WS pushes `session.read`-style `{keys, question, line, waiting_since}` (from `choices::detect`) and the phone answers via `session.answer`, not blind `y/n/1/2/3` keys. Covers Codex/Gemini/Aider TUI approvals with real buttons and race safety. Small.
4. **Protocol hardening** (small-medium): replace polling with server push of `screen` frames coalesced 100 ms, each with `seq`; on (re)connect client sends `{attach, since_seq}` and server reuses delta/snapshot logic; app-level ping 20 s + 2 missed pongs => reconnect (4G half-open sockets), exponential backoff with jitter (not fixed 2.5 s), pause when hidden; `client_id` on every `input`/`answer` for server-side dedupe; ack cursor so server can drop buffered frames for slow clients (send latest state only, mosh-style). Keep JSON text for control, binary frame for screen payloads optional.
5. **Screen rendering**: keep text view as default; add "Terminal" toggle = read-only xterm.js fed the ANSI snapshot (fit width + pinch/pan), own key bar below. Expand key bar: Left/Right, Shift+Tab, Ctrl+D, Ctrl+L, PgUp/PgDn, Home/End, paste. Medium.
6. **Web Push for needs-you** (separate design; biggest battery/UX win off-foreground). Medium-large, needs infra decision.
7. **Defer / reject**: mosh SSP/UDP (reject), tmux control mode (reject), ghostty-web on phone (reject until mobile + Windows support documented), grid row-diff + zstd/deflate (only after measuring 4G bytes; coalescing first), local speculative echo (unneeded: composer sends whole messages), Web Speech (reject).

Adoption risk notes: xterm.js is mature/ubiquitous but mobile is officially unsupported (low abandonment risk, high mobile-bug risk); ghostty-web is young, patch-dependent, Chrome-only (high); Remote Control is vendor-controlled and Claude-only (dependency risk if Kelpie relied on it). Phone page stays no-build today; item 2(a) breaks that, weigh it.

## Pick (modern + performance on 4G + user-friendly)

**Stack: state-sync over the existing WebSocket + structured items + native input.**
- Protocol: server-push, seq-numbered, 100 ms-coalesced state frames (mosh idea on the existing attach delta/snapshot logic; latest-state-only for slow clients); chat sessions as seq-numbered structured items with `since_seq` resume; app-level ping + jittered backoff; idempotent `client_id` on all writes; pause when hidden. Optional later: WebTransport/HTTP3 [U, no evidence gathered; Safari support unverified], skip for now.
- Renderer: chat items (native cards) for agent sessions; terminal sessions = text/reflow view default, read-only xterm.js toggle (fit + pinch) for TUI fidelity.
- Input: native `<textarea>` composer (OS IME = Telex/VNI safe, keyboard dictation for voice, `isComposing` guard) + prompt-card buttons from `choices::detect`/permission options + extended key bar.

| Criterion | Pick | Runner-up |
|---|---|---|
| Modern | 4 | 3 |
| Performance (latency/bandwidth/battery) | 5 (push, coalesced, no polling, resume without replay) | 4 |
| User-friendliness | 5 (buttons, native IME, chat view) | 3 |
| Effort (5 = cheap) | 3 (chat parity + protocol push are the cost) | 4 |

Runner-up: **pure terminal mirror** = full-screen xterm.js (or ghostty-web) fed raw bytes with its own keyboard handling (VibeTunnel/ttyd style). Lost because: xterm mobile/IME unsupported and broken for Telex-class input (#3600, #6012, #6045, #5377), raw redraw floods cost bandwidth/battery, no structured permission buttons, ghostty-web Chrome-only/immature. Cheaper (effort 4) but fails the Vietnamese-IME and 4G goals.

Sequencing if effort-limited: items 1, 3, 4 of the ranking (guard fix, prompt cards, push+resume) deliver most of the gain at low cost; chat parity (item 2) next.

## Limitations

- No direct xterm.js+Telex phone repro found; conclusion inferred from open IME issues. Test on real Android (Gboard + Unikey) and iOS Telex before finalizing.
- Did not benchmark bytes/frame or battery; compression recommendations are reasoned, not measured. Wire formats of VibeTunnel/ttyd/Termius/Blink from memory or undocumented [U].
- Web Push on iOS/Android and axum permessage-deflate support not verified.
- Search tool summaries were used for several pages (caniuse, ACP, Remote Control issues); primary pages not read in full.

## Unresolved questions

1. Are `app/src/chat` adapters DOM/Solid-free so they can ship in a mobile bundle? Is a build step acceptable for the phone page?
2. Should the phone ever resize the PTY, or stay a passive viewport (recommended)?
3. Push notifications: Web Push via internet push service vs relay; what is the phone->daemon reachability (LAN only, Tailscale, tunnel)? Drives Web Push feasibility and flaky-4G scope.
4. Read-only devices (`can_write=false`): should they see chat items and prompt cards (view) while buttons are disabled?
5. Expected peak bytes/s from Claude Code TUI on 4G; measure before choosing compression.

Sources: [mosh paper](https://mosh.org/mosh-paper.pdf), [xterm #5377](https://github.com/xtermjs/xterm.js/issues/5377), [#3600](https://github.com/xtermjs/xterm.js/issues/3600), [#6012](https://github.com/xtermjs/xterm.js/issues/6012), [#6045](https://github.com/xtermjs/xterm.js/issues/6045), [ghostty-web](https://github.com/coder/ghostty-web), [VibeTunnel](https://github.com/amantus-ai/vibetunnel), [ACP overview](https://agentclientprotocol.com/protocol/overview), [caniuse speech](https://caniuse.com/speech-recognition), [Claude Code RC issues](https://github.com/anthropics/claude-code/issues/32493).
