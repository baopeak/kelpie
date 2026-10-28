// Transcript rows of the chat view: tool calls as "verb target · detail" rows with IN/OUT and
// diffs, folded runs of reads and searches, collapsed thinking, approval cards (with questions,
// plans and a message for the agent) and the files a turn changed.
import { For, Show, createMemo, createSignal } from "solid-js";
import type { ChatController, ChatItem } from "../chat/chat-model";
import { type ToolView, basename, describeTool, diffStats } from "../chat/tool-view";
import { store } from "../lib/store";
import type { Project } from "../lib/types";
import { Icon } from "./icons";
import { t } from "../lib/i18n";

type Tool = Extract<ChatItem, { kind: "tool" }>;
type Permission = Extract<ChatItem, { kind: "permission" }>;
export interface FileChange { path: string; adds: number; dels: number }

/** `path` relative to the project folder, or null when it lies outside. */
export function projectRelative(project: Project, path: string): string | null {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const root = norm(project.path);
  const p = norm(path).replace(/^\.\//, "");
  if (p.split("/").includes("..")) return null;
  if (!/^([a-zA-Z]:)?\//.test(p)) return p;
  const win = /^[a-zA-Z]:/.test(root);
  const same = (a: string, b: string) => (win ? a.toLowerCase() === b.toLowerCase() : a === b);
  return same(p.slice(0, root.length), root) && p[root.length] === "/" ? p.slice(root.length + 1) : null;
}

/** Long text cut to `lines` (the first ones, or the last for output), with a way to see it all. */
function Clamped(props: { text: string; lines?: number; tail?: boolean; class?: string }) {
  const [all, setAll] = createSignal(false);
  const rows = createMemo(() => props.text.replace(/\n+$/, "").split("\n"));
  const max = () => props.lines ?? 12;
  const hidden = () => (all() ? 0 : Math.max(0, rows().length - max()));
  const shown = () => (hidden() ? (props.tail ? rows().slice(-max()) : rows().slice(0, max())) : rows()).join("\n");
  const more = () => (
    <button class="link-btn" onClick={() => setAll(true)}>{t("Show {n} more lines", { n: hidden() })}</button>
  );
  return (
    <div class={props.class}>
      <Show when={hidden() && props.tail}>{more()}</Show>
      <div class="clamp-text">{shown()}</div>
      <Show when={hidden() && !props.tail}>{more()}</Show>
    </div>
  );
}

export function DiffLines(props: { text: string; lines?: number }) {
  const [all, setAll] = createSignal(false);
  const rows = createMemo(() => props.text.split("\n"));
  const max = () => props.lines ?? 14;
  return (
    <>
      <For each={all() ? rows().slice(0, 2000) : rows().slice(0, max())}>
        {(l) => <div class={`diff-line${l.startsWith("+") && !l.startsWith("+++") ? " add" : l.startsWith("-") && !l.startsWith("---") ? " del" : l === "@@" ? " gap" : ""}`}>{l === "@@" ? "⋯" : l || " "}</div>}
      </For>
      <Show when={!all() && rows().length > max()}>
        <button class="link-btn" onClick={() => setAll(true)}>{t("Show {n} more lines", { n: rows().length - max() })}</button>
      </Show>
    </>
  );
}

function Stats(props: { adds: number; dels: number }) {
  return (
    <span class="tstats">
      <Show when={props.adds}><span class="add">+{props.adds}</span></Show>
      <Show when={props.dels}><span class="del">−{props.dels}</span></Show>
    </span>
  );
}

const dotClass = (status: Tool["status"]) => (status === "running" ? " running" : status === "error" ? " error" : "");

export function ToolRow(props: { item: Tool; project: Project; nested?: boolean }) {
  const view = createMemo<ToolView>(() => describeTool(props.item));
  const stats = createMemo(() => (props.item.diff ?? []).reduce((acc, d) => {
    const s = diffStats(d.text);
    return { adds: acc.adds + s.adds, dels: acc.dels + s.dels };
  }, { adds: 0, dels: 0 }));
  // Edits and commands show their content; reads, searches and the rest only when asked.
  const [open, setOpen] = createSignal<boolean | null>(null);
  const isOpen = () => open() ?? (view().category === "edit" || view().category === "command" || props.item.status === "error");
  const hasBody = () => !!(props.item.diff?.length || view().command || props.item.output || props.item.input);
  return (
    <div class={`trow tool${dotClass(props.item.status)}${props.nested ? " nested" : ""}${props.item.status === "error" ? " error" : ""}`}>
      <div class="tool-head" onClick={() => hasBody() && setOpen(!isOpen())}>
        <span class="tdot" />
        <span class="tname">{view().verb}</span>
        <span class="tsum" title={view().target}>{view().target}</span>
        <Show when={view().detail}><span class="tdetail">{view().detail}</span></Show>
        <Show when={stats().adds || stats().dels}><Stats {...stats()} /></Show>
        <Show when={hasBody()}><Icon name={isOpen() ? "chevronDown" : "chevronRight"} size={12} /></Show>
      </div>
      <Show when={isOpen() && hasBody()}>
        <div class="tool-body">
          <Show when={props.item.diff?.length}>
            <For each={props.item.diff}>
              {(d) => {
                const rel = projectRelative(props.project, d.path);
                return (
                  <>
                    <div class="diff-file">
                      <Show when={rel} fallback={d.path}>
                        <button class="link-btn" title={t("Open the file's changes")} onClick={() => store.openFile(props.project.id, rel!, { diff: true })}>{rel}</button>
                      </Show>
                    </div>
                    <DiffLines text={d.text} />
                  </>
                );
              }}
            </For>
          </Show>
          <Show when={view().command}>
            <div class="io"><span class="io-tag">IN</span><Clamped class="io-text" text={view().command!} lines={8} /></div>
          </Show>
          <Show when={props.item.output}>
            <div class="io"><span class="io-tag">OUT</span><Clamped class="io-text" text={props.item.output!.length > 20000 ? props.item.output!.slice(-20000) : props.item.output!} lines={12} tail /></div>
          </Show>
          <Show when={!props.item.diff?.length && !view().command && !props.item.output && props.item.input}>
            <Clamped class="io-text" text={JSON.stringify(props.item.input, null, 2)} lines={12} />
          </Show>
        </div>
      </Show>
    </div>
  );
}

/** Consecutive reads and searches, folded into one row ("Explored 4 files, 2 searches"). */
export function ToolRun(props: { items: Tool[]; project: Project }) {
  const [open, setOpen] = createSignal(false);
  const running = () => props.items.some((i) => i.status === "running");
  const failed = () => props.items.some((i) => i.status === "error");
  const label = createMemo(() => {
    const reads = props.items.filter((i) => describeTool(i).category === "read").length;
    const searches = props.items.length - reads;
    const parts = [reads ? (reads === 1 ? t("1 file") : t("{n} files", { n: reads })) : "", searches ? (searches === 1 ? t("1 search") : t("{n} searches", { n: searches })) : ""].filter(Boolean);
    return (running() ? t("Exploring {what}", { what: parts.join(", ") }) : t("Explored {what}", { what: parts.join(", ") }));
  });
  return (
    <div class={`trow tool run${running() ? " running" : failed() ? " error" : ""}`}>
      <div class="tool-head" onClick={() => setOpen(!open())}>
        <span class="tdot" />
        <span class="tname">{label()}</span>
        <span class="tsum">{props.items.map((i) => describeTool(i).target).join(" · ")}</span>
        <Icon name={open() ? "chevronDown" : "chevronRight"} size={12} />
      </div>
      <Show when={open()}>
        <div class="run-items">
          <For each={props.items}>{(i) => <ToolRow item={i} project={props.project} nested />}</For>
        </div>
      </Show>
    </div>
  );
}

export function ThinkingRow(props: { item: Extract<ChatItem, { kind: "thinking" }> }) {
  const [open, setOpen] = createSignal(false);
  const secs = () => (props.item.at != null && props.item.doneAt != null ? Math.max(1, Math.round((props.item.doneAt - props.item.at) / 1000)) : null);
  return (
    <Show when={!props.item.streaming} fallback={<div class="trow thinking running"><div class="msg-thinking live">{props.item.text.split("\n").slice(-6).join("\n")}</div></div>}>
      <div class="trow thinking">
        <div class="tool-head plain" onClick={() => setOpen(!open())}>
          <span class="tdot" />
          <span class="thought">{secs() ? t("Thought for {n}s", { n: secs()! }) : t("Thought")}</span>
          <Icon name={open() ? "chevronDown" : "chevronRight"} size={12} />
        </div>
        <Show when={open()}><div class="msg-thinking">{props.item.text}</div></Show>
      </div>
    </Show>
  );
}

/** An approval, with what it is about: the command, the edit, the plan, or questions to answer. */
export function PermissionCard(props: { item: Permission; ctrl: ChatController; markdown: (text: string) => any }) {
  const p = () => props.item;
  const [message, setMessage] = createSignal("");
  const [picked, setPicked] = createSignal<Record<string, string[]>>({});
  const [other, setOther] = createSignal<Record<string, string>>({});
  const answer = (o: { id: string; label: string; tone?: string }) =>
    props.ctrl.respond(p().id, o.id, o.label, o.tone === "deny" && message().trim() ? { message: message().trim() } : undefined);
  const toggle = (q: string, label: string, multi: boolean) =>
    setPicked((cur) => {
      const now = cur[q] ?? [];
      return { ...cur, [q]: multi ? (now.includes(label) ? now.filter((x) => x !== label) : [...now, label]) : [label] };
    });
  const answers = () => {
    const out: Record<string, string> = {};
    for (const q of p().ask ?? []) {
      const list = [...(picked()[q.question] ?? []), ...(other()[q.question]?.trim() ? [other()[q.question].trim()] : [])];
      if (list.length) out[q.question] = list.join(", ");
    }
    return out;
  };
  const complete = () => (p().ask ?? []).every((q) => answers()[q.question]);
  return (
    <div class="perm" data-perm={p().id}>
      <div class="q">{p().title}</div>
      <Show when={p().detail}><div class="perm-detail">{p().detail}</div></Show>
      <Show when={p().command}><pre class="perm-command">{p().command}</pre></Show>
      <Show when={p().diff?.length}>
        <div class="tool-body perm-diff">
          <For each={p().diff}>{(d) => <><div class="diff-file">{basename(d.path)}</div><DiffLines text={d.text} /></>}</For>
        </div>
      </Show>
      <Show when={p().plan}><div class="perm-plan">{props.markdown(p().plan!)}</div></Show>
      <Show when={!p().resolved} fallback={<div class="muted" style={{ padding: 0 }}>→ {p().resolved}</div>}>
        <Show when={p().ask?.length}>
          <For each={p().ask}>
            {(q) => (
              <div class="ask">
                <div class="ask-q"><Show when={q.header}><span class="ask-tag">{q.header}</span></Show>{q.question}</div>
                <div class="ask-opts">
                  <For each={q.options}>
                    {(o) => (
                      <button class={`btn ask-opt${(picked()[q.question] ?? []).includes(o.label) ? " sel" : ""}`} title={o.description} onClick={() => toggle(q.question, o.label, !!q.multiSelect)}>
                        {o.label}<Show when={o.description}><span class="detail">{o.description}</span></Show>
                      </button>
                    )}
                  </For>
                </div>
                <input class="text-input" placeholder={t("Other…")} value={other()[q.question] ?? ""} onInput={(e) => setOther((cur) => ({ ...cur, [q.question]: e.currentTarget.value }))} />
              </div>
            )}
          </For>
          <div class="actions">
            <button class="btn primary" disabled={!complete()} onClick={() => props.ctrl.respond(p().id, "answer", Object.values(answers()).join(" · "), { answers: answers() })}>{t("Submit answers")}</button>
          </div>
        </Show>
        <Show when={p().feedback}>
          <input class="text-input perm-msg" placeholder={t("Tell the agent what to do instead (sent with Deny)…")} value={message()} onInput={(e) => setMessage(e.currentTarget.value)}
            onKeyDown={(e) => {
              const deny = p().options.find((o) => o.tone === "deny");
              if (e.key === "Enter" && message().trim() && deny) answer(deny);
            }} />
        </Show>
        <div class="actions">
          <For each={p().options}>
            {(o, i) => (
              <button class={`btn${o.tone === "allow" && !p().ask?.length ? " primary" : ""}`} title={t("Key {n}", { n: i() + 1 })} onClick={() => answer(o)}>
                <span class="key">{i() + 1}</span>{o.label}
              </button>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

/** "3 files changed +42 −7", each file opening its changes. */
export function TurnFiles(props: { files: FileChange[]; project: Project }) {
  const [open, setOpen] = createSignal(props.files.length <= 4);
  const total = () => props.files.reduce((acc, f) => ({ adds: acc.adds + f.adds, dels: acc.dels + f.dels }), { adds: 0, dels: 0 });
  return (
    <div class="turn-files">
      <div class="tool-head" onClick={() => setOpen(!open())}>
        <Icon name="git" size={12} />
        <span class="tname">{props.files.length === 1 ? t("1 file changed") : t("{n} files changed", { n: props.files.length })}</span>
        <Stats {...total()} />
        <span class="spacer" />
        <Icon name={open() ? "chevronDown" : "chevronRight"} size={12} />
      </div>
      <Show when={open()}>
        <For each={props.files}>
          {(f) => {
            const rel = projectRelative(props.project, f.path);
            return (
              <div class="turn-file">
                <Show when={rel} fallback={<span class="tsum">{f.path}</span>}>
                  <button class="link-btn tsum" title={t("Open the file's changes")} onClick={() => store.openFile(props.project.id, rel!, { diff: true })}>{rel}</button>
                </Show>
                <Stats adds={f.adds} dels={f.dels} />
              </div>
            );
          }}
        </For>
      </Show>
    </div>
  );
}
