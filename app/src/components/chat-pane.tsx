import { For, Match, Show, Switch, createEffect, createMemo, createResource, createSignal, lazy, on, onCleanup, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import DOMPurify from "dompurify";
import { sessionLabel, store } from "../lib/store";
import { CHECKPOINT_API } from "../lib/checkpoint-api";
import { listTurns, turnsByMessage } from "../lib/checkpoints";
import { findPane } from "../lib/layout";
import { getController, switchToTerminal } from "../chat/mode-switch";
import { CHAT_SANITIZE, chatMarkdownHtml, fileRefInCode, linkifyText } from "../chat/chat-links";
import type { ChatItem } from "../chat/chat-model";
import { openTable, stableEnd } from "../chat/streaming-markdown";
import { describeTool, diffStats, unifiedFiles } from "../chat/tool-view";
import type { Project } from "../lib/types";
import { Icon } from "./icons";
import { ChatComposer } from "./chat-composer";
import { CopyButton, copyText } from "./chat-copy";
import { openChatFile, openChatLink } from "./chat-open-file";
import { type FileChange, PermissionCard, ThinkingRow, ToolRow, ToolRun, TurnFiles } from "./chat-rows";
import { isAgentKind, kindInfo } from "../lib/agents";
import { locale, t } from "../lib/i18n";

/** Vietnamese interface: Vietnamese dates; English: the system's own date format. */
const clockLocale = () => (locale() === "vi" ? "vi-VN" : []);

const RENDER_WINDOW = 300;

/** The turn panel's undo preview, loaded the first time a chat offers an undo. */
const UndoDialog = lazy(() => import("./turn-timeline").then((m) => ({ default: m.UndoDialog })));

// The code block copy button's icons (our own constant markup); `.copied` shows the tick.
const COPY_ICONS =
  '<svg class="i-copy" width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/><path d="M3.5 10.5h-1v-8h8v1"/></svg>' +
  '<svg class="i-done" width="13" height="13" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3.5 8.5l3 3 6-7"/></svg>';

const toHtml = (text: string) => DOMPurify.sanitize(chatMarkdownHtml(text), CHAT_SANITIZE);

/** Dragging over text to select it ends in a click too: that one should not follow anything. */
const selecting = () => window.getSelection()?.isCollapsed === false;

/** Sanitized markdown whose code blocks get a copy button and whose inline code naming a file gets
 *  marked (given a project; nothing is looked up until a click). Both are added to the DOM
 *  afterwards, never spliced into the sanitized markup: some WebKit builds serialize `<` and `>`
 *  unescaped in attribute values, so a string edit could turn an attribute into live markup. */
function MarkdownHtml(props: { text: string; project?: Project; class?: string; onClick?: (e: MouseEvent) => void }) {
  let el!: HTMLDivElement;
  createEffect(() => {
    el.innerHTML = toHtml(props.text);
    for (const pre of el.querySelectorAll("pre")) {
      const block = document.createElement("div");
      block.className = "code-block";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "code-copy";
      button.title = t("Copy");
      button.setAttribute("aria-label", t("Copy"));
      button.innerHTML = COPY_ICONS;
      pre.replaceWith(block);
      block.append(button, pre);
    }
    if (!props.project) return;
    for (const code of el.querySelectorAll<HTMLElement>(":not(pre) > code")) {
      const text = code.textContent ?? "";
      if (code.closest("a") || !fileRefInCode(text)) continue;
      code.dataset.fileRef = text;
      code.classList.add("file-ref");
    }
  });
  return <div ref={el} class={props.class} onClick={props.onClick} />;
}

/** A code block's button copies its code; links and inline code naming a file are followed. All of
 *  it is the agent's text: `data-href` and `data-file-ref` only ever go through the parsers. An
 *  `<a>` the agent wrote as HTML has no `data-href`, only what DOMPurify left of its `href`. No
 *  click navigates the window, even on something the sanitizer should already have removed. */
const onMarkdownClick = (project?: Project) => (e: MouseEvent) => {
  const target = e.target as HTMLElement;
  const copy = target.closest<HTMLElement>(".code-copy");
  if (copy) {
    copyText(copy.parentElement?.querySelector("pre")?.textContent ?? "", copy);
    return;
  }
  if (target.closest("a, area, form, button, input")) e.preventDefault();
  const a = target.closest("a");
  if (a) {
    if (!selecting()) openChatLink(a.dataset.href ?? a.getAttribute("href") ?? "", project);
    return;
  }
  const code = target.closest<HTMLElement>("[data-file-ref]");
  const ref = code && project && !selecting() ? fileRefInCode(code.dataset.fileRef ?? "") : null;
  if (ref) void openChatFile(project!, ref);
};

function Markdown(props: { text: string; project?: Project }) {
  return <MarkdownHtml class="msg-assistant" text={props.text} project={props.project} onClick={onMarkdownClick(props.project)} />;
}

/** A reply that is still arriving: finished blocks as markdown, the open one as plain text (or
 *  as a table once it is one: only that open table is parsed again per delta). */
function StreamingMarkdown(props: { text: string; project?: Project }) {
  const cut = createMemo(() => stableEnd(props.text));
  const done = createMemo(() => props.text.slice(0, cut()));
  const tail = () => props.text.slice(cut());
  return (
    <div class="msg-assistant" onClick={onMarkdownClick(props.project)}>
      <MarkdownHtml text={done()} project={props.project} />
      <Show when={openTable(tail())} fallback={<div style={{ "white-space": "pre-wrap" }}>{tail()}</div>}>
        <MarkdownHtml text={tail()} project={props.project} />
      </Show>
    </div>
  );
}

type Tool = Extract<ChatItem, { kind: "tool" }>;

const isExplore = (it: ChatItem): it is Tool => it.kind === "tool" && ["read", "search"].includes(describeTool(it).category);

/** Runs of 2+ consecutive reads/searches (folded into one row, keyed by their first item), and
 *  the files each turn changed (keyed by its result). Only structure is read here, so streamed
 *  text does not recompute it. */
function transcriptShape(items: ChatItem[]) {
  const runs = new Map<string, Tool[]>();
  const member = new Set<string>();
  const turnFiles = new Map<string, FileChange[]>();
  let run: Tool[] = [];
  const endRun = () => {
    if (run.length >= 2) {
      runs.set(run[0].id, run);
      for (const t of run.slice(1)) member.add(t.id);
    }
    run = [];
  };
  let changed = new Map<string, FileChange>();
  for (const it of items) {
    if (isExplore(it)) run.push(it);
    else endRun();
    if (it.kind === "tool" && it.diff?.length) {
      for (const d of it.diff) {
        const s = diffStats(d.text);
        const f = changed.get(d.path) ?? { path: d.path, adds: 0, dels: 0 };
        changed.set(d.path, { ...f, adds: f.adds + s.adds, dels: f.dels + s.dels });
      }
    } else if (it.kind === "result") {
      const files = it.diff ? unifiedFiles(it.diff) : [...changed.values()];
      if (files.length) turnFiles.set(it.id, files);
      changed = new Map();
    }
  }
  endRun();
  return { runs, member, turnFiles };
}

/** "23/10 16:02" (the full date and time on hover). */
const clock = (at: number) => new Date(at).toLocaleString(clockLocale(), { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
const fullClock = (at: number) => new Date(at).toLocaleString(clockLocale(), { dateStyle: "full", timeStyle: "medium" });

/** The date and time a message was sent, in the top right corner. */
function Stamp(props: { at?: number }) {
  return <Show when={props.at}><span class="msg-time" title={fullClock(props.at!)}>{clock(props.at!)}</span></Show>;
}

export function ChatPane(props: { sessionId: string; project: Project; focused: boolean }) {
  return (
    <Show when={store.sessions[props.sessionId] ? props.sessionId : null} keyed fallback={<div class="empty">{t("Waiting for session…")}</div>}>
      {(id) => <ChatBody sessionId={id} project={props.project} focused={props.focused} />}
    </Show>
  );
}

function ChatBody(props: { sessionId: string; project: Project; focused: boolean }) {
  let scroller!: HTMLDivElement;
  const session = () => store.sessions[props.sessionId];
  const ctrl = getController(session());
  const [showAll, setShowAll] = createSignal(false);
  let stick = true;

  // Attach only when the handle or the daemon connection changes — not on every status tick.
  const target = createMemo(() => (store.connected() ? session()?.handle ?? null : null));
  onMount(() => {
    createEffect(on(target, (handle) => handle != null && void ctrl.attach(handle)));
    // The daemon dropped us for falling behind: re-attach (full replay).
    const onResync = (e: Event) => (e as CustomEvent).detail === props.sessionId && target() != null && void ctrl.attach(target()!);
    window.addEventListener("kelpie:chat-resync", onResync);
    // A phone answered one of the agent's requests first.
    const onResolved = (e: Event) => {
      const d = (e as CustomEvent).detail;
      // `dropped`: an answer sent from here lost to `by`'s, which is what the agent got.
      if (d?.id === props.sessionId && !(d.dropped && d.by === "desktop")) ctrl.resolvedElsewhere(d.request_id, !!d.dropped);
    };
    window.addEventListener("kelpie:chat-resolved", onResolved);
    onCleanup(() => {
      window.removeEventListener("kelpie:chat-resync", onResync);
      window.removeEventListener("kelpie:chat-resolved", onResolved);
      ctrl.detach();
    });
  });

  const items = createMemo(() => (showAll() ? ctrl.state.items : ctrl.state.items.slice(-RENDER_WINDOW)));
  const shape = createMemo(() => transcriptShape(items()));

  // Keep pinned to the bottom while streaming unless the user scrolled up; then offer a way back.
  const [away, setAway] = createSignal(false);
  createEffect(() => {
    const last = ctrl.state.items[ctrl.state.items.length - 1];
    void (last && "text" in last ? (last as any).text : last);
    void ctrl.state.items.length;
    if (stick) queueMicrotask(() => scroller && (scroller.scrollTop = scroller.scrollHeight));
  });
  const toBottom = () => {
    stick = true;
    setAway(false);
    scroller.scrollTop = scroller.scrollHeight;
  };
  /** An approval nobody answered yet: the pill leads to it. */
  const pending = createMemo(() => ctrl.state.items.find((i): i is Extract<ChatItem, { kind: "permission" }> => i.kind === "permission" && !i.resolved));
  const toPending = () => {
    const el = scroller.querySelector(`[data-perm="${CSS.escape(pending()!.id)}"]`);
    if (el) el.scrollIntoView({ block: "center", behavior: "smooth" });
    else toBottom();
  };

  // Elapsed time of the running turn.
  const [now, setNow] = createSignal(Date.now());
  createEffect(() => {
    if (ctrl.state.busySince == null) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(timer));
  });
  const elapsed = () => {
    const since = ctrl.state.busySince;
    if (since == null) return "";
    const s = Math.max(0, Math.floor((now() - since) / 1000));
    return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
  };

  /** Continues the conversation in the agent's terminal UI and runs `command` there. */
  const openInTerminal = (command: string) => {
    const loc = store.paneLocation(props.sessionId);
    const tab = loc && store.workspace(loc.projectId).tabs.find((x) => x.id === loc.tabId);
    const pane = tab && findPane(tab.root, loc.paneId);
    if (tab && pane) void switchToTerminal(props.project, tab, pane, command);
  };

  const meta = () => kindInfo(session()?.kind ?? "custom");

  // The turns each message led to, for its undo button. Read again when this session takes a
  // snapshot or an undo finishes.
  const [turnsVersion, setTurnsVersion] = createSignal(0);
  const recorded = () => {
    const s = session();
    return !!s && !s.remote && isAgentKind(s.kind) && store.daemonApi() >= CHECKPOINT_API;
  };
  const [turns] = createResource(() => recorded() && { id: props.sessionId, v: turnsVersion() }, ({ id }) => listTurns(id).catch(() => null));
  onMount(() => {
    const onCheckpoint = (e: Event) => (e as CustomEvent).detail?.session === props.sessionId && setTurnsVersion((v) => v + 1);
    window.addEventListener("kelpie:checkpoint", onCheckpoint);
    onCleanup(() => window.removeEventListener("kelpie:checkpoint", onCheckpoint));
  });
  const undoable = createMemo(() => {
    const list = turns();
    return list?.records.length ? turnsByMessage(list, ctrl.state.items.filter((i) => i.kind === "user")) : new Map();
  });
  const [undoing, setUndoing] = createSignal<number | null>(null);

  return (
    <div class="chat">
      <div class="chat-scroll" ref={scroller} onScroll={(e) => {
        stick = e.currentTarget.scrollHeight - e.currentTarget.scrollTop - e.currentTarget.clientHeight < 40;
        setAway(!stick);
      }}>
        <div class="chat-inner">
          <Show when={!showAll() && ctrl.state.items.length > RENDER_WINDOW}>
            <button class="btn" style={{ "align-self": "center" }} onClick={() => setShowAll(true)}>{t("Show {n} earlier items", { n: ctrl.state.items.length - RENDER_WINDOW })}</button>
          </Show>
          <Show when={ctrl.state.ready && ctrl.state.items.length === 0}>
            <div class="muted" style={{ "text-align": "center" }}>
              {t("{agent} chat · {project}. Ask anything — tools, diffs and approvals appear inline.", { agent: meta().label, project: props.project.name })}
            </div>
          </Show>
          <For each={items()}>
            {(item) => (
              <Switch>
                <Match when={item.kind === "user" && item}>
                  {(it) => {
                    const u = it() as Extract<ChatItem, { kind: "user" }>;
                    return (
                      <Show when={u.text}>
                        <div class="msg-user-wrap">
                          <div class="msg-meta">
                            <For each={undoable().get(u.id) ?? []}>
                              {(rec) => (
                                <button class="msg-action" title={t("Undo turn #{n}…", { n: rec.n })} aria-label={t("Undo turn #{n}…", { n: rec.n })} onClick={() => setUndoing(rec.n)}>
                                  <Icon name="undo" size={12} />
                                  <Show when={(undoable().get(u.id)?.length ?? 0) > 1}><span>#{rec.n}</span></Show>
                                </button>
                              )}
                            </For>
                            <CopyButton text={u.text} />
                            <Stamp at={u.at} />
                          </div>
                          <div class="msg-user" style={{ opacity: u.pending ? 0.7 : 1 }}>
                            <For each={linkifyText(u.text)}>
                              {(part) => ("url" in part
                                ? <a href={part.url} draggable={false} onClick={(e) => {
                                    e.preventDefault();
                                    if (!selecting()) openChatLink(part.url);
                                  }}>{part.url}</a>
                                : part.text)}
                            </For>
                          </div>
                        </div>
                      </Show>
                    );
                  }}
                </Match>
                <Match when={item.kind === "assistant" && item}>
                  {(it) => (
                    <div class="msg-reply" classList={{ streaming: !!(it() as any).streaming }}>
                      <div class="msg-meta">
                        <Show when={!(it() as any).streaming && (it() as any).text}><CopyButton text={(it() as any).text} /></Show>
                        <Stamp at={it().at} />
                      </div>
                      <Show when={(it() as any).streaming} fallback={<Markdown text={(it() as any).text} project={props.project} />}><StreamingMarkdown text={(it() as any).text} project={props.project} /></Show>
                    </div>
                  )}
                </Match>
                <Match when={item.kind === "thinking" && item}>
                  {(it) => <Show when={(it() as any).text}><ThinkingRow item={it() as any} /></Show>}
                </Match>
                <Match when={item.kind === "tool" && item}>
                  {(it) => (
                    <Show when={!shape().member.has(it().id)}>
                      <Show when={shape().runs.get(it().id)} fallback={<ToolRow item={it() as Tool} project={props.project} />}>
                        {(run) => <ToolRun items={run()} project={props.project} />}
                      </Show>
                    </Show>
                  )}
                </Match>
                <Match when={item.kind === "permission" && item}>
                  {(it) => <PermissionCard item={it() as any} project={props.project} ctrl={ctrl} markdown={(text) => <Markdown text={text} project={props.project} />} />}
                </Match>
                <Match when={item.kind === "system" && item}>
                  {(it) => {
                    const s = it() as Extract<ChatItem, { kind: "system" }>;
                    return (
                      <div class="result-line" style={{ color: s.tone === "error" ? "var(--danger)" : undefined }}>
                        {s.text}
                        <Show when={s.tui}>
                          {" "}<button class="btn" style={{ height: "22px" }} onClick={() => openInTerminal(s.tui!.command)}><Icon name="terminal" size={12} /> {t("Run it in the terminal")}</button>
                        </Show>
                      </div>
                    );
                  }}
                </Match>
                <Match when={item.kind === "result" && item}>
                  {(it) => (
                    <>
                      <Show when={shape().turnFiles.get(it().id)}>{(files) => <TurnFiles files={files()} project={props.project} />}</Show>
                      <Show when={(it() as any).text}><div class="result-line">{(it() as any).text}<Show when={it().at}> · {clock(it().at!)}</Show></div></Show>
                    </>
                  )}
                </Match>
              </Switch>
            )}
          </For>
          <Show when={ctrl.state.busy}>
            <div class="typing">
              <span class="dot working" /> {ctrl.state.status === "requesting" ? t("Thinking…") : t("Working…")}
              <Show when={elapsed()}><span class="elapsed">{elapsed()}</span></Show>
              <span class="hint">{t("Esc to interrupt")}</span>
            </div>
          </Show>
        </div>
        <Show when={away()}>
          <div class="chat-pill-dock">
            <Show when={pending()} fallback={<button class="chat-pill" onClick={toBottom}><Icon name="down" size={12} /> {t("Jump to latest")}</button>}>
              <button class="chat-pill warn" onClick={toPending}><Icon name="alert" size={12} /> {t("Waiting for your approval")}</button>
            </Show>
          </div>
        </Show>
      </div>
      <ChatComposer ctrl={ctrl} session={session()} agentLabel={meta().label} focused={props.focused} onSent={() => (stick = true)} />
      {/* In the body: the work area contains layout, which would hold a fixed overlay inside it. */}
      <Show when={undoing()}>
        {(n) => (
          <Portal>
            <UndoDialog
              sessionId={props.sessionId}
              sessionName={session() ? sessionLabel(session()!) : ""}
              n={n()}
              onClose={(changed) => {
                setUndoing(null);
                if (changed) setTurnsVersion((v) => v + 1);
              }}
            />
          </Portal>
        )}
      </Show>
    </div>
  );
}
