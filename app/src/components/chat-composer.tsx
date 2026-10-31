// Chat view composer: the message field with the agent's own completions — `/` commands at the
// start of a message, `$` skills (Codex) and `@` files anywhere — and pickers for the settings
// the agent exposes (model, effort, permission mode).
import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { MicButton } from "./voice";
import { Icon } from "./icons";
import { AttachedPictures } from "./image-lightbox";
import { store } from "../lib/store";
import { isMac } from "../lib/platform";
import { quickOpen } from "../lib/ipc";
import { confirmDialog } from "../lib/confirm-dialog";
import { draftAfterSend } from "../lib/composer-core";
import { createPictures } from "../lib/composer-pictures";
import { errorText, t } from "../lib/i18n";
import type { AgentRun } from "../chat/agent-runs";
import { type ChatController, type ChatCommand, type ChatItem, type ChatSetting, withImageNames } from "../chat/chat-model";
import type { SessionInfo } from "../lib/types";

type Trigger = "/" | "$" | "@";
interface Suggestion { label: string; detail?: string; hint?: string; insert: string; dim?: boolean }

/** The completion being typed at the caret, if any. */
function tokenAt(text: string, caret: number, skills: boolean): { trigger: Trigger; start: number; query: string } | null {
  const before = text.slice(0, caret);
  const slash = /^\/([\w:.-]*)$/.exec(before);
  if (slash) return { trigger: "/", start: 0, query: slash[1] };
  const m = /(^|\s)([$@])([^\s]*)$/.exec(before);
  if (!m || (m[2] === "$" && !skills)) return null;
  return { trigger: m[2] as Trigger, start: before.length - m[3].length - 1, query: m[3] };
}

/** Name prefix matches first (the shortest, i.e. closest, leading), then name substrings, then
 *  description matches; otherwise the agent's own order. */
function rank(list: ChatCommand[], query: string): ChatCommand[] {
  const q = query.toLowerCase();
  if (!q) return list;
  const score = (c: ChatCommand) => {
    const n = c.name.toLowerCase();
    return n.startsWith(q) ? 0 : n.includes(q) ? 1 : c.description?.toLowerCase().includes(q) ? 2 : 3;
  };
  return list
    .map((c) => [score(c), c] as const)
    .filter(([s]) => s < 3)
    .sort((a, b) => a[0] - b[0] || (a[0] === 0 ? a[1].name.length - b[1].name.length : 0))
    .map(([, c]) => c);
}

const MAX_SUGGESTIONS = 50;

/** Permission modes without approvals: Shift+Tab never steps into them, and their chip warns. */
const UNGUARDED = new Set(["full-access", "bypassPermissions"]);

export function ChatComposer(props: { ctrl: ChatController; session: SessionInfo | undefined; agentLabel: string; focused: boolean; onSent: () => void; agents: AgentRun[]; onAgentMap: () => void }) {
  const ctrl = props.ctrl;
  let root!: HTMLDivElement;
  let textarea!: HTMLTextAreaElement;
  const pictures = createPictures(() => props.session);
  const [dropping, setDropping] = createSignal(false);
  const [draft, setDraft] = createSignal("");
  const [caret, setCaret] = createSignal(0);
  const [selected, setSelected] = createSignal(0);
  const [dismissed, setDismissed] = createSignal<string | null>(null);
  const [files, setFiles] = createSignal<Suggestion[]>([]);

  const token = createMemo(() => tokenAt(draft(), caret(), ctrl.state.skills.length > 0));
  const tokenKey = () => {
    const tk = token();
    return tk ? `${tk.trigger}${tk.start}` : null;
  };

  // `@` asks Kelpie's file index, a little after the last keystroke.
  let fileTimer: ReturnType<typeof setTimeout> | undefined;
  // Only the newest search may fill the list (a slow answer for `@a` must not win over `@ab`).
  let fileSearch = 0;
  createEffect(on(token, (tk) => {
    clearTimeout(fileTimer);
    const gen = ++fileSearch;
    if (tk?.trigger !== "@") return setFiles([]);
    fileTimer = setTimeout(() => {
      void quickOpen(ctrl.cwd, tk.query, 30).then(
        (hits) => gen === fileSearch && setFiles(hits.map((h) => ({ label: h.path, insert: ctrl.mention(h.path) }))),
        () => gen === fileSearch && setFiles([]),
      );
    }, 80);
  }));
  onCleanup(() => clearTimeout(fileTimer));

  const suggestions = createMemo<Suggestion[]>(() => {
    const tk = token();
    if (!tk || dismissed() === tokenKey()) return [];
    if (tk.trigger === "@") return files();
    const list = tk.trigger === "/" ? ctrl.state.commands : ctrl.state.skills;
    return rank(list, tk.query).slice(0, MAX_SUGGESTIONS).map((c) => ({
      label: `${tk.trigger}${c.name}`, detail: c.tuiOnly ? t("terminal UI only") : c.description, hint: c.hint, insert: `${tk.trigger}${c.name}`, dim: c.tuiOnly,
    }));
  });
  createEffect(on(suggestions, () => setSelected(0)));

  const syncCaret = () => setCaret(textarea.selectionStart ?? 0);

  function setText(text: string, at = text.length) {
    setDraft(text);
    textarea.value = text;
    textarea.setSelectionRange(at, at);
    setCaret(at);
    textarea.style.height = "auto";
    textarea.style.height = Math.min(240, textarea.scrollHeight) + "px";
  }

  function accept(s: Suggestion) {
    const tk = token();
    if (!tk) return;
    const text = draft();
    const end = caret();
    const insert = `${s.insert} `;
    setText(text.slice(0, tk.start) + insert + text.slice(end), tk.start + insert.length);
    textarea.focus();
  }

  /** When the last message went: the send button turns into Stop under the pointer right away,
   *  and the second click of a double-click must not stop the turn it just started. */
  let sentAt = 0;
  /** Getting a message's pictures ready takes a moment: a second Enter must not send it twice. */
  let sending = false;
  const send = async () => {
    const text = draft();
    const attached = pictures.images();
    if (sending || (!text.trim() && !attached.length)) return;
    sending = true;
    try {
      const withPictures = attached.length > 0 && ctrl.takesPicturesWith(text);
      if (attached.length && !withPictures) store.notify(t("A command takes no pictures: they stay here for your next message"), undefined, 6000);
      ctrl.send(text, true, withPictures ? await pictures.forChat() : undefined);
      // Only what went: something added while the pictures were being got ready stays.
      if (withPictures) pictures.forget(attached);
      sentAt = Date.now();
      setText(draftAfterSend(draft(), text));
      props.onSent();
    } catch (e) {
      store.notify(errorText(e), undefined, 7000);
    } finally {
      sending = false;
    }
  };
  // A click on the round button keeps the caret in the message field: the button it clicked
  // is replaced (Send ↔ Stop), which would drop the focus.
  const stopFromButton = () => {
    if (Date.now() - sentAt > 600) ctrl.interrupt();
    textarea.focus();
  };
  const sendFromButton = () => {
    void send();
    textarea.focus();
  };

  createEffect(() => props.focused && store.overlay() === null && queueMicrotask(() => textarea?.focus()));

  // A picture dropped here. What is on top under the pointer decides, so a floating composer over
  // this chat takes its own drops; anything but a picture goes nowhere, as it always did here.
  onMount(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const { type } = event.payload;
      if (type === "leave") return setDropping(false);
      const ratio = window.devicePixelRatio || 1;
      const { x, y } = (event.payload as { position: { x: number; y: number } }).position;
      const inside = !ctrl.state.exited && root.contains(document.elementFromPoint(x / ratio, y / ratio));
      if (type === "drop") {
        setDropping(false);
        if (inside) void pictures.drop((event.payload as { paths: string[] }).paths).then(() => textarea.focus());
      } else setDropping(inside);
    });
    onCleanup(() => void unlisten.then((f) => f()));
  });

  /** The newest approval the running turn is waiting on (asked live — not one from a replayed
   *  log — and with options rather than questions to fill in). */
  const waiting = () =>
    ctrl.state.busy
      ? ctrl.state.items.findLast((i): i is Extract<ChatItem, { kind: "permission" }> => i.kind === "permission" && !i.resolved && i.at != null && !i.ask?.length)
      : undefined;

  /** Shift+Tab steps through the permission modes, as in the agents' terminal UIs — but never
   *  into one without approvals: that takes a deliberate pick. */
  const modeSetting = () => ctrl.state.settings.find((s) => s.id === "mode");
  /** Model and effort share one chip ("Opus 5.5 · high"); other settings get their own. */
  const modelGroup = () => ["model", "effort"].map((id) => ctrl.state.settings.find((s) => s.id === id)).filter((s): s is ChatSetting => !!s);
  const otherSettings = () => ctrl.state.settings.filter((s) => !["model", "effort", "mode"].includes(s.id));
  /** Busy with nothing to send: the send button stops the turn instead, as in Claude Code. */
  const stops = () => ctrl.state.busy && !draft().trim() && !pictures.images().length;
  const sendKey = isMac ? "↩" : "⏎";
  const cycleMode = () => {
    const mode = modeSetting()!;
    const options = mode.options.filter((o) => !UNGUARDED.has(o.value));
    if (!options.length) return;
    const i = options.findIndex((o) => o.value === mode.value);
    ctrl.setSetting(mode.id, options[(i + 1) % options.length].value);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing) return;
    // With nothing typed, a digit answers the waiting approval (1 = its first option).
    const perm = !draft() && /^[1-9]$/.test(e.key) && !e.ctrlKey && !e.altKey && !e.metaKey ? waiting() : undefined;
    const option = perm?.options[Number(e.key) - 1];
    if (perm && option) {
      e.preventDefault();
      ctrl.respond(perm.id, option.id, option.label);
      return;
    }
    if (e.key === "Tab" && e.shiftKey && !suggestions().length && modeSetting()?.options.length && !ctrl.state.exited) {
      e.preventDefault();
      cycleMode();
      return;
    }
    const list = suggestions();
    if (list.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        return;
      }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) {
        e.preventDefault();
        const tk = token();
        // Enter sends when the token is already complete (a command, skill or file typed out in
        // full, as in the agents' terminal UIs) or a bare `@` / `$` was typed; Tab always picks.
        const typedOut = tk && list.some((s) => s.insert === draft().slice(tk.start, caret()));
        if (e.key === "Enter" && tk && (typedOut || (tk.trigger !== "/" && !tk.query))) void send();
        else accept(list[selected()]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(tokenKey());
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    } else if (e.key === "Escape" && ctrl.state.busy) {
      e.preventDefault();
      ctrl.interrupt();
    }
  };

  return (
    <div class="chat-composer" classList={{ dropping: dropping() }} ref={root}>
      <TaskStrip ctrl={ctrl} />
      <PlanPanel ctrl={ctrl} />
      <Show when={ctrl.state.queue.length}>
        <div class="queue">
          <Show when={ctrl.state.queuePaused}>
            <div class="queue-note">
              {t("Stopped: these wait until you send again.")}
              <button class="link-btn" onClick={() => ctrl.resumeQueue()}>{t("Send them now")}</button>
            </div>
          </Show>
          <For each={ctrl.state.queue}>
            {(m) => (
              <div class="queue-chip" title={withImageNames(m.text, m.images)}>
                <Icon name="clock" size={11} />
                <span class="queue-text">{withImageNames(m.text, m.images)}</span>
                <Show when={m.from}><span class="msg-time">{t("from {device}", { device: m.from! })}</span></Show>
                <Show when={m.needsWindow}><span class="msg-time" title={t("This message has a picture or a command that only the Kelpie window it was typed in can prepare. It waits there; remove it to let the others go.")}>{t("needs the desktop window")}</span></Show>
                <button class="link-btn" title={t("Give it to the agent now, during its current turn")} onClick={() => ctrl.sendNow(m.id)}>{t("Send now")}</button>
                <button class="icon-btn mini" title={t("Remove from the queue")} onClick={() => ctrl.unqueue(m.id)}><Icon name="close" size={11} /></button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <div class="composer-box">
        <Show when={suggestions().length}>
          <div class="composer-menu" role="listbox">
            <For each={suggestions()}>
              {(s, i) => (
                <div
                  class={`composer-item${i() === selected() ? " sel" : ""}${s.dim ? " dim" : ""}`}
                  role="option"
                  aria-selected={i() === selected()}
                  onPointerDown={(e) => {
                    e.preventDefault();
                    accept(s);
                  }}
                  onPointerEnter={() => setSelected(i())}
                >
                  <span class="name">{s.label}</span>
                  <Show when={s.hint}><span class="hint">{s.hint}</span></Show>
                  <Show when={s.detail}><span class="detail">{s.detail}</span></Show>
                </div>
              )}
            </For>
          </div>
        </Show>
        <AttachedPictures images={pictures.images()} onRemove={pictures.remove} />
        <Show when={pictures.images().length && !ctrl.takesPictures()}>
          <div class="composer-attached-note">{t("{agent} is given where the picture is, not the picture itself: it may not be able to read it.", { agent: props.agentLabel })}</div>
        </Show>
        <textarea
          ref={textarea}
          rows={2}
          placeholder={
            ctrl.state.exited ? t("Agent exited — switch to terminal to resume")
            : waiting() ? t("Press 1–{n} to answer, or type a message", { n: waiting()!.options.length })
            : ctrl.state.busy ? t("Queue another message… (it goes when {agent} finishes)", { agent: props.agentLabel })
            : t("Message {agent}…  / commands, @ files", { agent: props.agentLabel })
          }
          value={draft()}
          disabled={ctrl.state.exited}
          onInput={(e) => {
            setDraft(e.currentTarget.value);
            syncCaret();
            e.currentTarget.style.height = "auto";
            e.currentTarget.style.height = Math.min(240, e.currentTarget.scrollHeight) + "px";
          }}
          onKeyUp={(e) => (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "Home" || e.key === "End") && syncCaret()}
          onClick={syncCaret}
          onBlur={() => setDismissed(tokenKey())}
          onFocus={() => setDismissed(null)}
          onKeyDown={onKeyDown}
          onPaste={pictures.paste}
        />
        <div class="composer-row">
          <Show when={modelGroup().length} fallback={<span class="composer-agent">{props.agentLabel}<Show when={ctrl.state.model}> · {ctrl.state.model}</Show></span>}>
            <SettingPicker settings={modelGroup()} disabled={ctrl.state.exited} onPick={(id, v) => ctrl.setSetting(id, v)} />
          </Show>
          <For each={otherSettings()}>{(s) => <SettingPicker settings={[s]} disabled={ctrl.state.exited} onPick={(id, v) => ctrl.setSetting(id, v)} />}</For>
          <Show when={ctrl.state.conversationId && ctrl.state.conversationId !== "latest"}>
            <span class="composer-id" title={t("Agent conversation id")}>{ctrl.state.conversationId!.slice(0, 8)}</span>
          </Show>
          <span class="spacer" />
          <Show when={props.agents.length}>
            {/* The caret stays in the message field, for the dialog to give the keyboard back to. */}
            <button class="setting-btn agent-chip" title={t("Show Agent Map")} onMouseDown={(e) => e.preventDefault()} onClick={() => props.onAgentMap()}>
              <Show when={props.agents.some((r) => r.status === "running")}><span class="tdot running" /></Show>
              <span class="setting-label">{t("Agents {n}", { n: props.agents.length })}</span>
            </button>
          </Show>
          <ContextMeter ctrl={ctrl} />
          <Show when={modeSetting()}>
            {(mode) => <SettingPicker settings={[mode()]} right warn={UNGUARDED.has(mode().value ?? "")} disabled={ctrl.state.exited} onPick={(id, v) => ctrl.setSetting(id, v)} />}
          </Show>
          <Show when={props.session && !props.session.alive}>
            <button class="btn" style={{ height: "26px" }} title={t("Start the agent again, continuing this conversation")} onClick={() => void store.resumeSession(props.session!.id)}>
              <Icon name="restart" size={12} /> {t("Resume")}
            </button>
          </Show>
          <Show when={!ctrl.state.exited}><MicButton field={() => textarea} /></Show>
          <Show
            when={stops()}
            fallback={
              <button class="send-btn" disabled={(!draft().trim() && !pictures.images().length) || ctrl.state.exited} onClick={sendFromButton}
                title={ctrl.state.busy ? t("Queue {key}", { key: sendKey }) : t("Send {key}", { key: sendKey })} aria-label={ctrl.state.busy ? t("Queue {key}", { key: sendKey }) : t("Send {key}", { key: sendKey })}>
                <Icon name="up" size={15} />
              </button>
            }
          >
            <button class="send-btn stop" onClick={stopFromButton} title={`${t("Stop")} (Esc)`} aria-label={t("Stop")}><Icon name="stop" size={13} /></button>
          </Show>
        </div>
      </div>
    </div>
  );
}

/** Work left running in the background (shells, subagents): the first one's status, the rest on click. */
function TaskStrip(props: { ctrl: ChatController }) {
  const tasks = () => props.ctrl.state.tasks.filter((x) => x.background);
  const [open, setOpen] = createSignal(false);
  return (
    <Show when={tasks().length}>
      <div class="plan tasks">
        <div class="plan-head" onClick={() => setOpen(!open())}>
          <span class="tdot running" />
          <span class="plan-title">{t("In the background")}</span>
          <Show when={tasks().length > 1}><span class="plan-count">{tasks().length}</span></Show>
          <Show when={!open()}><span class="plan-current">{tasks()[0].summary || tasks()[0].description}</span></Show>
          <span class="spacer" />
          <Icon name={open() ? "chevronDown" : "chevronRight"} size={12} />
        </div>
        <Show when={open()}>
          <For each={tasks()}>
            {(x) => (
              <div class="plan-entry">
                <span class="task-text">{x.description}<Show when={x.summary}><span class="task-summary">{x.summary}</span></Show></span>
              </div>
            )}
          </For>
        </Show>
      </div>
    </Show>
  );
}

/** The agent's task list, pinned above the message field; folds itself once everything is done. */
function PlanPanel(props: { ctrl: ChatController }) {
  const plan = () => props.ctrl.state.plan;
  const done = () => plan()?.entries.filter((e) => e.status === "done").length ?? 0;
  const total = () => plan()?.entries.length ?? 0;
  const [open, setOpen] = createSignal<boolean | null>(null);
  const isOpen = () => open() ?? done() < total();
  const current = () => plan()?.entries.find((e) => e.status === "active");
  return (
    <Show when={total()}>
      <div class={`plan${isOpen() ? " open" : ""}`}>
        <div class="plan-head" onClick={() => setOpen(!isOpen())}>
          <Icon name="check" size={12} />
          <span class="plan-title">{t("Plan")}</span>
          <span class="plan-count">{done()}/{total()}</span>
          <Show when={!isOpen() && current()}><span class="plan-current">{current()!.text}</span></Show>
          <span class="spacer" />
          <Icon name={isOpen() ? "chevronDown" : "chevronRight"} size={12} />
        </div>
        <Show when={isOpen()}>
          <Show when={plan()!.explanation}><div class="plan-why">{plan()!.explanation}</div></Show>
          <For each={plan()!.entries}>
            {(e) => <div class={`plan-entry ${e.status}`}><span class="plan-box">{e.status === "done" ? "✓" : e.status === "active" ? "◐" : "○"}</span>{e.text}</div>}
          </For>
        </Show>
      </div>
    </Show>
  );
}

const tokens = (n: number) => (n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1000 ? `${Math.round(n / 1000)}k` : String(n));

/** How full the context is (a ring when the window is known), and the cost; click to compact. */
function ContextMeter(props: { ctrl: ChatController }) {
  const usage = () => props.ctrl.state.usage;
  const pct = () => {
    const u = usage();
    return u?.size ? Math.min(100, Math.round((u.used / u.size) * 100)) : null;
  };
  const canCompact = () => props.ctrl.state.commands.some((c) => c.name === "compact") && !props.ctrl.state.exited;
  const title = () => {
    const u = usage()!;
    return [
      u.size ? t("Context: {used} of {size} tokens ({pct}%)", { used: u.used.toLocaleString(), size: u.size.toLocaleString(), pct: pct()! }) : t("Context: {used} tokens", { used: u.used.toLocaleString() }),
      u.cost != null ? t("Cost so far: ${cost}", { cost: u.cost.toFixed(2) }) : "",
      canCompact() ? t("Click to compact the conversation") : "",
    ].filter(Boolean).join("\n");
  };
  const compact = async () => {
    if (canCompact() && (await confirmDialog(t("Compact the conversation now? The agent summarizes it to free up context.")))) props.ctrl.send("/compact");
  };
  const R = 6;
  const C = 2 * Math.PI * R;
  return (
    <Show when={usage() && usage()!.used > 0}>
      <button class={`ctx-meter${(pct() ?? 0) >= 80 ? " high" : ""}`} title={title()} onClick={compact}>
        <Show when={pct() != null} fallback={<span>{tokens(usage()!.used)}</span>}>
          <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
            <circle cx="8" cy="8" r={R} class="ring-bg" />
            <circle cx="8" cy="8" r={R} class="ring" stroke-dasharray={`${(C * pct()!) / 100} ${C}`} transform="rotate(-90 8 8)" />
          </svg>
          <span>{pct()}%</span>
        </Show>
      </button>
    </Show>
  );
}

/** Long lists (OMP offers hundreds of models) get a filter field. */
const FILTER_FROM = 12;

const currentLabel = (s: ChatSetting) => s.options.find((o) => o.value === s.value)?.label ?? s.value ?? t("Default");

/** A chip for one or more settings (model and effort share one), opening a list per setting.
 *  `right`: the list opens towards the left (a chip at the right end); `warn`: a risky value. */
function SettingPicker(props: { settings: ChatSetting[]; disabled: boolean; right?: boolean; warn?: boolean; onPick: (id: string, value: string) => void }) {
  const [open, setOpen] = createSignal(false);
  const [filter, setFilter] = createSignal("");
  const filterable = () => props.settings.some((s) => s.options.length > FILTER_FROM);
  /** A model or a mode names itself; a lone level ("high") needs its setting's name. */
  const label = () => props.settings.map((s) => (s.id === "model" || s.id === "mode" || props.settings.length > 1 ? currentLabel(s) : `${s.label}: ${currentLabel(s)}`)).join(" · ");
  /** The filter narrows only the long lists (models), never a short one shown beside them. */
  const shown = (s: ChatSetting) => {
    const q = filter().toLowerCase();
    return q && s.options.length > FILTER_FROM ? s.options.filter((o) => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q)) : s.options;
  };
  let root!: HTMLSpanElement;
  const close = (e: PointerEvent) => !root.contains(e.target as Node) && setOpen(false);
  createEffect(() => {
    if (!open()) return;
    setFilter("");
    window.addEventListener("pointerdown", close);
    onCleanup(() => window.removeEventListener("pointerdown", close));
  });
  return (
    <span class="setting-picker" ref={root}>
      <button class={`setting-btn${props.warn ? " warn" : ""}`} disabled={props.disabled} title={props.settings.map((s) => s.label).join(" · ")} onClick={() => setOpen(!open())}>
        <span class="setting-label">{label()}</span><Icon name="chevronDown" size={10} />
      </button>
      <Show when={open()}>
        <div class={`setting-pop${props.right ? " right" : ""}`} onKeyDown={(e) => e.key === "Escape" && (e.stopPropagation(), setOpen(false))}>
          <Show when={filterable()}>
            <input class="text-input" ref={(el) => queueMicrotask(() => el.focus())} placeholder={t("Filter…")} value={filter()} onInput={(e) => setFilter(e.currentTarget.value)} />
          </Show>
          <For each={props.settings}>
            {(s) => (
              <>
                <div class="setting-title">{s.label}</div>
                <div class="setting-list">
                  <For each={shown(s)}>
                    {(o) => (
                      <div class={`setting-opt${o.value === s.value ? " sel" : ""}`} title={o.description} onClick={() => { props.onPick(s.id, o.value); setOpen(false); }}>
                        {o.label}
                        <Show when={o.description && o.description !== o.label}><span class="detail">{o.description}</span></Show>
                      </div>
                    )}
                  </For>
                </div>
              </>
            )}
          </For>
        </div>
      </Show>
    </span>
  );
}
