// A chat session on the phone: the conversation as the desktop's adapters read it from the agent's log (in
// observer mode), and the agent's questions answered with a tap. Messages to a chat are written on the
// computer; the phone never starts, configures or talks to the agent.
import { createEffect, createSignal, For, Match, Show, Switch } from "solid-js";
import type { ChatItem } from "../../src/chat/chat-model";
import { describeTool } from "../../src/chat/tool-view";
import { t } from "./i18n";
import { ChatComposer, PendingChips } from "./composer";
import { canType, chat, chatPending, connected, device } from "./phone-state";
import type { SessionRow } from "./link";

type Permission = Extract<ChatItem, { kind: "permission" }>;
type Tool = Extract<ChatItem, { kind: "tool" }>;
type User = Extract<ChatItem, { kind: "user" }>;

const SHOWN = 200;

/** The phone shows no pictures: a message that is only pictures shows their names, not an empty bubble. */
const userText = (u: User) => u.text || (u.images ?? []).map((i) => `[${i.name}]`).join(" ");

/** Text with fenced code as code blocks; everything else as is, line breaks kept. Plain text nodes only: nothing is parsed into markup. */
export function splitFences(text: string): { code: boolean; text: string }[] {
  const out: { code: boolean; text: string }[] = [];
  let code = false;
  let buf: string[] = [];
  const flush = () => {
    if (buf.length) out.push({ code, text: buf.join("\n") });
    buf = [];
  };
  for (const line of text.split("\n")) {
    if (/^\s*```/.test(line)) {
      flush();
      code = !code;
    } else buf.push(line);
  }
  flush();
  return out;
}

function Rich(props: { text: string }) {
  return <For each={splitFences(props.text)}>{(b) => (b.code ? <pre class="code">{b.text}</pre> : <p class="prose">{b.text}</p>)}</For>;
}

function ToolRow(props: { item: Tool }) {
  const v = () => describeTool(props.item);
  return (
    <details class="tool" data-status={props.item.status}>
      <summary>
        <span class="tverb">{v().verb}</span> <span class="ttarget">{v().target}</span>
        <Show when={v().detail}><span class="tdetail"> · {v().detail}</span></Show>
        <Show when={props.item.status === "error"}><span class="terr"> ✕</span></Show>
      </summary>
      <Show when={v().command}><pre class="code">{v().command}</pre></Show>
      <Show when={props.item.output}><pre class="code out">{props.item.output!.length > 1500 ? `${props.item.output!.slice(0, 1500)}…` : props.item.output}</pre></Show>
    </details>
  );
}

function PermissionCard(props: { item: Permission }) {
  const ctrl = () => chat()!;
  const p = () => props.item;
  const [message, setMessage] = createSignal("");
  const [picked, setPicked] = createSignal<Record<string, string[]>>({});
  const open = () => !p().resolved && !p().answering;
  const enabled = () => open() && canType() && connected();
  const answer = (o: { id: string; label: string; tone?: string }) =>
    ctrl().respond(p().id, o.id, o.label, o.tone === "deny" && message().trim() ? { message: message().trim() } : undefined);
  const toggle = (q: string, label: string, multi: boolean) =>
    setPicked((cur) => {
      const now = cur[q] ?? [];
      return { ...cur, [q]: multi ? (now.includes(label) ? now.filter((x) => x !== label) : [...now, label]) : [label] };
    });
  const answers = () => {
    const out: Record<string, string> = {};
    for (const q of p().ask ?? []) if ((picked()[q.question] ?? []).length) out[q.question] = picked()[q.question].join(", ");
    return out;
  };
  const complete = () => (p().ask ?? []).every((q) => answers()[q.question]);
  return (
    <div class="perm" classList={{ resolved: !!p().resolved }} data-perm={p().id}>
      <div class="q">{p().title}</div>
      <Show when={p().detail}><div class="muted">{p().detail}</div></Show>
      <Show when={p().command}><pre class="code">{p().command}</pre></Show>
      <Show when={p().diff?.length}>
        <For each={p().diff}>{(d) => <details><summary>{d.path.split(/[\\/]/).pop()}</summary><pre class="code">{d.text.split("\n").slice(0, 60).join("\n")}</pre></details>}</For>
      </Show>
      <Show when={p().plan}><Rich text={p().plan!} /></Show>
      <Show when={p().resolved}><div class="muted resolved">→ {p().resolved}</div></Show>
      <Show when={p().answering}><div class="muted" role="status">{t("Sending…")}</div></Show>
      <Show when={p().answerError}><div class="muted err" role="status">{p().answerError}</div></Show>
      <Show when={!p().resolved}>
        <Show when={p().ask?.length}>
          <For each={p().ask}>
            {(q) => (
              <div class="ask">
                <div class="ask-q">{q.question}</div>
                <div class="actions">
                  <For each={q.options}>
                    {(o) => (
                      <button class="btn" classList={{ sel: (picked()[q.question] ?? []).includes(o.label) }} disabled={!enabled()} onClick={() => toggle(q.question, o.label, !!q.multiSelect)}>
                        {o.label}
                      </button>
                    )}
                  </For>
                </div>
              </div>
            )}
          </For>
          <div class="actions">
            <Show when={!p().retry}>
              <button class="btn primary" disabled={!enabled() || !complete()} onClick={() => ctrl().respond(p().id, "answer", Object.values(answers()).join(" · "), { answers: answers() })}>{t("Submit answers")}</button>
            </Show>
          </div>
        </Show>
        <Show when={p().feedback}>
          <input class="field" placeholder={t("Tell the agent what to do instead (sent with Deny)…")} value={message()} disabled={!enabled()} onInput={(e) => setMessage(e.currentTarget.value)} />
        </Show>
        <div class="actions">
          <For each={p().options}>
            {(o) => (
              <Show when={!p().retry}>
                <button class="btn" classList={{ primary: o.tone === "allow" && !p().ask?.length, danger: o.tone === "deny" }} disabled={!enabled()} onClick={() => answer(o)}>
                  {o.label}
                </button>
              </Show>
            )}
          </For>
          <Show when={p().retry}>
            <button class="btn primary" disabled={!enabled()} onClick={() => ctrl().retryAnswer(p().id)}>{t("Send again: {answer}", { answer: p().retry! })}</button>
          </Show>
        </div>
        <Show when={!canType()}><div class="muted">{t("This device can only read.")}</div></Show>
      </Show>
    </div>
  );
}

function Row(props: { item: ChatItem }) {
  return (
    <Switch>
      <Match when={props.item.kind === "user"}>
        <div class="msg user">
          <Show when={(props.item as User).from && (props.item as User).from !== device()?.name}>
            <div class="muted from">{t("from {device}", { device: (props.item as User).from! })}</div>
          </Show>
          <Rich text={userText(props.item as User)} />
        </div>
      </Match>
      <Match when={props.item.kind === "assistant"}><div class="msg assistant"><Rich text={(props.item as { text: string }).text} /></div></Match>
      <Match when={props.item.kind === "thinking"}>
        <details class="thinking"><summary>{t("Thinking")}</summary><p class="prose">{(props.item as { text: string }).text}</p></details>
      </Match>
      <Match when={props.item.kind === "tool"}><ToolRow item={props.item as Tool} /></Match>
      <Match when={props.item.kind === "permission"}><PermissionCard item={props.item as Permission} /></Match>
      <Match when={props.item.kind === "system"}><div class="msg system">{(props.item as { text: string }).text}</div></Match>
      <Match when={props.item.kind === "result"}><div class="msg system">{(props.item as { text: string }).text}</div></Match>
    </Switch>
  );
}

/** No box: the chat is not running (after a restart every chat comes back stopped) or this device may only read. What this phone
 *  sent and was not sent stays on screen with its text, above the hint to open the chat on the desktop. */
function NotRunning(props: { alive: boolean }) {
  const [notice, setNotice] = createSignal<string | null>(null);
  return (
    <>
      <Show when={chatPending().some((p) => p.state !== "queued")}>
        <div class="chips kept-chips"><PendingChips setNotice={setNotice} /></div>
      </Show>
      <div class="readonly">
        {!props.alive ? t("This chat is not running. Open it on the desktop to resume it.") : t("This device can only read.")}
        <Show when={notice()}><div class="notice" role="status">{notice()}</div></Show>
      </div>
    </>
  );
}

export function ChatView(props: { row: SessionRow; dim: boolean }) {
  let scroller!: HTMLDivElement;
  let stick = true;
  const items = () => (chat() ? chat()!.state.items.slice(-SHOWN) : []);
  createEffect(() => {
    const list = items();
    void list.length;
    const last = list[list.length - 1];
    void (last && "text" in last ? last.text : last);
    if (stick) queueMicrotask(() => scroller && (scroller.scrollTop = scroller.scrollHeight));
  });
  return (
    <div class="session chat" data-id={props.row.id} classList={{ dim: props.dim }}>
      <div class="chat-log" ref={scroller} onScroll={() => (stick = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 40)}>
        <Show when={chat()?.state.ready} fallback={<div class="muted center">{connected() ? t("Loading the conversation…") : t("This chat is not available offline. It opens when the connection is back.")}</div>}>
          <For each={items()}>{(item) => <Row item={item} />}</For>
          <Show when={chat()?.state.busy}><div class="muted working" role="status">{t("Working…")}</div></Show>
        </Show>
      </div>
      <Show when={canType() && props.row.alive} fallback={<NotRunning alive={props.row.alive} />}>
        <ChatComposer id={props.row.id} alive={props.row.alive} />
      </Show>
    </div>
  );
}
