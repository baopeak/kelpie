# Code review: chat view, Claude Code parity (uncommitted)

Date: 2026-10-25. Scope: chat-pane.tsx, chat-composer.tsx, turn-timeline.tsx (export only), chat-model.ts, claude-adapter.ts (+test), checkpoints.ts (+test), vi/tools.json, and the chat hunks of styles.css. Read-only review. tsc, vitest and i18n were already green, so I did not re-run them.

## Overall

Most of it is clean. The Solid reactivity is right, the task lifecycle holds up when the log is replayed or the process exits, and the CSS changes stay inside the chat. Three things need fixing before merge:
- the `<pre>` string injection (an XSS risk on some platforms),
- the rule that links a turn to a message, which picks the wrong message for queued or quick follow-ups,
- the send button turning into Stop under the cursor.

## Critical

### C1. Editing sanitized HTML as a string allows XSS on WebKit builds that leave `<` and `>` unescaped in attributes
`app/src/components/chat-pane.tsx:33-38` (`toHtml`). The comment says "Only tags can spell `<pre>` here: text inside the sanitized HTML is escaped". That holds for text, not for attribute values. Under the older HTML serialization rules, an attribute value keeps a literal `<` and `>`; only `&`, `"` and nbsp are escaped. Chromium 138+ (WebView2 today) escapes them, but WKWebView on macOS 12/13 does not. `tauri.conf.json` sets `minimumSystemVersion: "12.0"`.
- How it fails: the agent outputs (for example, after reading a hostile README) `[x](https://a "<pre><img src=x onerror=...>")`. marked escapes the title, DOMPurify decodes it into the attribute value and keeps it (title is allowed). The old serializer then emits `title="<pre><img src=x onerror=...>"`. `.replace(/<pre>/g, …)` inserts `<div class="code-block"><button type="button" class="code-copy" title="…"…`, and the first `"` of that markup closes the title attribute. Everything after it is parsed as markup, so the `<img onerror>` becomes a real element when Solid sets `innerHTML`. Script in the main window gets Tauri IPC (daemon commands that spawn sessions, clipboard read), so this can lead to running code on the machine. `alt` on images works the same way.
- A smaller problem from the same approach: raw `<pre class=…>` does not match `<pre>`, but its `</pre>` does, so a stray `</div>` can close an enclosing raw `<div>` early.
- Fix: never change markup after sanitizing. Either:
  - sanitize with `RETURN_DOM_FRAGMENT: true`, wrap each `pre` and add a button built with DOM APIs, then serialize, or
  - leave `toHtml` as it was and add the buttons after rendering, in a `createEffect` over the container (`el.querySelectorAll("pre")`).

  Delegated `onMarkdownClick` works with either option.

## High

### H1. Turns are linked to the next message when it was sent soon after the turn ended (always the case for queued messages)
`app/src/lib/checkpoints.ts:163-172`. The rule is `owner = last message with at <= rec.at`. But the daemon sets `rec.at = max(turn end seen by the ticker, time the snapshot finished)` (`crates/kelpied/src/checkpoints.rs:811`). The turn end is detected on the status ticker, and the snapshot needs git work, often 100 ms to over 1 s (`daemon.rs:1211-1220`). `ChatController.flushQueue` sends the next queued message one microtask after `busy` drops (`chat-model.ts:353-366`). That message gets `at = Date.now()`, which is earlier than `rec.at`.
- What happens: message A is sent and a message B is queued. A's turn edits files. B goes out right away and becomes the owner of A's turn. A shows no undo button, while B shows two (`#n`, `#n+1`). A user who replies within about a second gets the same result. The dialog still names `#n` and lists the files, so nothing is undone without the user seeing it, but the button sits on the wrong message.
- Fix: use the turn's start, not its end. Take the last message with `m.at <= rec.started + SLACK`, falling back to `rec.at` when `started` is 0. A small SLACK (about 1 s) covers `kelpie_time` stamps, which are at most once per second and never later than the real time. The daemon sets `started` when it first sees `Working` (`daemon.rs:1202`), which is after the optimistic send and after the echoed user line. Messages steered in mid-turn then also go to the message that started the turn. Add a test where a later message's `at` falls between `rec.started` and `rec.at`.

### H2. Double-clicking Send interrupts the turn that was just started
`app/src/components/chat-composer.tsx:137` and `:280-293`. The Claude adapter's `send` sets `busy=true` synchronously (`claude-adapter.ts:357-360`), and `setText("")` empties the draft, so `stops()` becomes true at once. The Stop button replaces Send at the same spot, and the second click of a double-click (or an impatient repeat click) calls `ctrl.interrupt()`. When the agent is already busy, clicking Queue has the same problem: the second click stops the running turn and pauses the queue. Keyboard focus on the replaced button is also lost, so focus falls to `body`.
- Fix: ignore Stop clicks for about 500 ms after a send (keep a `lastSend` timestamp), or only allow Stop once the pointer has left and re-entered the button. Also move focus back to the textarea after a click-send.

## Medium

### M1. Undo offered on a record that has no base
`app/src/components/chat-pane.tsx:244-247` passes raw `records`. When `first_is_base` is set, the oldest record exists only as the base of the next one, and `visibleTurns` hides it in the Turns panel (`checkpoints.ts:115-118`). In chat it can still get an undo button. Its base refs were pruned (`delete_refs(&dropped)`), so `undo_plan` fails ("no earlier snapshot to restore", or a missing object), and the user sees an error dialog. Fix: drop `records[0]` when `first_is_base` before calling `turnsByMessage`.

### M2. Chips no longer shrink, so in narrow panes Send/Stop can be pushed out of the box
`styles.css` `.setting-btn { white-space: nowrap }`. The chip and `.setting-picker` have no `min-width: 0` or ellipsis, and `.composer-row` does not wrap. Before this change the labels could wrap. At pane widths of about 400 px (a 3-4 way split), the row is roughly model chip + mode chip + meter + mic + send plus gaps, and it overflows to the right. `.send-btn` is `flex: none`, so it is the first thing to go. Fix: `.setting-picker { min-width: 0 }` and `.setting-btn { max-width: 100%; overflow: hidden; text-overflow: ellipsis }`, or `flex-wrap: wrap` on `.composer-row`.

## Low

- L1. Floating `.msg-meta` (`.msg-reply > .msg-meta { float: right }`) narrows block formatting contexts beside it. A reply that starts with a code block (`pre` has `overflow: auto`), a table (`display: block; overflow-x: auto`) or an `hr` loses about 60-100 px of width for the whole block. The copy button keeps its space even at `opacity: 0`. Consider absolute positioning over the reply's top padding instead.
- L2. `.msg-assistant table { display: block }` drops the table semantics for screen readers in Chromium and WebKit. Wrapping it in a scroll container would keep them, but that needs DOM work (see C1).
- L3. Undo buttons on a message with several turns all have the same `aria-label` ("Undo this turn…"). The `#n` is only visible text, so add it to the label.
- L4. `turnsByMessage` does not skip turns already undone (an `undo` record with `undo_of === n`). This matches the Turns panel, but the chat keeps offering an undo that would only skip files. Optional: hide them.
- L5. In the UndoDialog opened from chat, focus stays on the chat button or the composer, so Escape and digit keys go to the composer behind the modal (Escape in a busy composer interrupts). The Turns panel already behaves this way, but chat makes it reachable from the composer. Focus the modal on mount.
- L6. `tasks` survive `conversation_reset` (`ctx.clear()` at `chat-model.ts:208-211` does not touch them). This is only correct if Claude keeps background tasks running across `/clear`. Unverified.
- L7. `TaskStrip` header is a clickable `div`, so it cannot be reached by keyboard. Same as the existing `PlanPanel`; noted only so the gap is known.
- L8. Nothing tests the controller lifecycle of `tasks` (cleared on `kelpie_exit` and on re-attach). The adapter tests cover only the event mapping.

## Verified, no issue

- Task shapes match the installed claude.exe (2.1.310). The emitter sets `is_backgrounded` only when the task has `isBackgrounded`. Claude's own foreground test is `"isBackgrounded" in e && e.isBackgrounded === false`, so treating `!== false` as background matches. Terminal `task_updated` is followed by `task_notification`, and the duplicate drop is harmless.
- Replay: `attach` calls `resetState` (which clears `tasks`) before replaying, `kelpie_exit` is written to the daemon log (`chat.rs:200`), and a duplicate `task_started` is ignored. Tasks that never get a notification are cleared when the process exits.
- Theme variables live on `:root`, so the dialog in the Portal is styled. `turn-timeline.css` loads with the lazy chunk. The light theme has every variable the new CSS uses (`--on-accent`, `--waiting`, `--working`, `--idle`).
- The changed chat classes (`.tool*`, `.trow`, `.msg-*`, `.setting-*`, `.plan`, `.typing`, `.perm`) are only used in chat-pane, chat-rows and chat-composer, so no other view is affected.
- Reactivity: the resource source, the `undoable` memo, the non-keyed mode `Show` accessor, and `CopyButton` (reads `props.text` at click time) are all fine. Sessions are written with `reconcile` or merged, so the resource does not refetch on status ticks.
- The clipboard capability (`clipboard-manager:allow-write-text`) is granted.

## Unresolved questions

1. Which WKWebView versions escape `<` and `>` in attribute serialization? C1 is exploitable on any that do not. macOS 12/13 are in the supported range.
2. Does Claude kill background tasks on `/clear`? This decides L6.
3. Should `awaited` local agents (`awaitedByCaller`, which the turn waits for) appear under "In the background"? Right now they do.

## Resolution (2026-10-25, same session)

- C1 fixed: `toHtml` no longer edits sanitized markup; `MarkdownHtml` sets the sanitized HTML, then wraps each `pre` and adds the copy button with DOM calls.
- H1 fixed: `turnsByMessage(list, messages)` assigns a turn to the last message at or before `rec.started + 1s` (falls back to `rec.at` when `started` is 0); tests cover queued/quick follow-ups, replay slack, unknown start.
- H2 fixed: Stop from the round button is ignored for 600 ms after a send; both Send and Stop clicks return focus to the textarea.
- M1 fixed: base of a full history skipped (`first_is_base`). L4 also done: turns with a later `undo` record are skipped.
- M2 fixed: `.setting-picker { min-width: 0 }`, label in `.setting-label` with ellipsis; verified at a 380 px pane.
- L1 fixed: reply meta is its own line again (no float); rail dot offset fixed at 19px.
- L3 fixed: buttons labelled "Undo turn #{n}…". L5 fixed: UndoDialog focuses itself on open (helps the Turns panel too).
- Q3: `awaited` tasks are not background (`background = is_backgrounded !== false && !awaited`), with a test. L8: controller test that `kelpie_exit` clears tasks.
- Open: L2 (table `display:block` semantics), L6 (`/clear` vs background tasks, unverified), L7 (clickable-div headers, same as PlanPanel).
- Gates after fixes: tsc clean, vitest 227/227, i18n 0 missing.
