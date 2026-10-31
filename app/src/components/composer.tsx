// Floating composer over a pane: one place to write a prompt for any session — a shell, an
// agent's terminal UI or a chat view. `@` mentions files, folders, the selection, the diff,
// review notes, other terminals and issues; pictures can be pasted or dropped. Enter sends,
// Shift+Enter adds a line, ↑ / ↓ walk the project's earlier prompts, Esc closes.
import "./composer.css";
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { store, sessionLabel } from "../lib/store";
import { msg, t, errorText } from "../lib/i18n";
import { Icon } from "./icons";
import { MicButton } from "./voice";
import { isMac, basename, joinPath } from "../lib/platform";
import { closeComposer, currentSelection, type ComposerRequest } from "../lib/composer-state";
import { droppedFileMention, expandMentions, hasUnsafeName, issueMention, looksLikeMention, mentionAt, pathMention, pathReference, shellQuote, terminalMention, MAX_TOTAL_BYTES, type Mention, type MentionContext, type ShellStyle } from "../lib/mentions";
import { stepHistory } from "../lib/prompt-history";
import { gitContext, isDir, listDir, quickOpen, readFile, redact, writeTempFile } from "../lib/ipc";
import { createPictures } from "../lib/composer-pictures";
import { AttachedPictures } from "./image-lightbox";
import { sendToSession } from "../lib/send-to-agent";
import { sessionTranscript } from "../lib/agent-actions";
import { getController } from "../chat/mode-switch";
import { peekView } from "../lib/terminal-host";
import { isAgentKind } from "../lib/agents";
import { gitlabIssues, gitlabProject, jiraIssues, linearIssues, type Issue } from "../lib/integrations";
import type { Project, SessionInfo } from "../lib/types";

interface Suggestion { label: string; detail?: string; insert: string; /** More follows (`terminal:`): no space, menu stays. */ more?: boolean }

const windows = /Win/.test(navigator.platform);
const KEYWORDS: { name: string; detail: string; more?: boolean }[] = [
  { name: "selection", detail: msg("The selection in an editor, diff or terminal") },
  { name: "diff", detail: msg("Uncommitted changes") },
  { name: "note", detail: msg("Review notes on the diff") },
  { name: "terminal:", detail: msg("Recent output of another session"), more: true },
  { name: "issue:", detail: msg("An issue from Linear, Jira or GitLab"), more: true },
];

/** Drafts survive closing the composer until they are sent. */
const drafts = new Map<string, string>();

const isAbsolute = (p: string) => /^([A-Za-z]:[\\/]|\\\\|\/)/.test(p);

function toBase64(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 0x8000) out += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(out);
}

/** How a shell (or another program that is not an agent) wants a path written. */
function shellStyle(s: SessionInfo): ShellStyle {
  // A session on another machine: its host is taken for a POSIX one (the local OS says nothing about it).
  if (!windows || s.remote) return "posix";
  const program = (s.kind === "shell" ? s.args?.[0] ?? store.state.settings.shellArgs?.[0] ?? "" : "").toLowerCase();
  if (/(^|[\\/])(bash|zsh|sh|fish)(\.exe)?$/.test(program)) return "posix";
  return /(^|[\\/])cmd(\.exe)?$/.test(program) ? "cmd" : "pwsh";
}

function problemText(m: Mention): string {
  switch (m.kind) {
    case "selection": return t("@selection: nothing is selected in an editor, a diff or a terminal");
    case "diff": return t("@diff: there are no uncommitted changes");
    case "note": return t("@note: there are no review notes");
    case "terminal": return m.arg ? t("@terminal: no session named {name}", { name: m.arg }) : t("@terminal: no other terminal is running");
    default: return m.arg ? t("@issue: {name} was not found in a connected tracker", { name: m.arg }) : t("@issue: this session was not started on an issue, write @issue:KEY");
  }
}

const issueText = (i: Issue) => [`${i.key}: ${i.title}`, i.url, i.state && `State: ${i.state}`, i.description && `\n${i.description}`].filter(Boolean).join("\n");

/** Issues of the connected trackers that match `search`, the first one's source first. */
async function findIssues(project: Project, search: string): Promise<Issue[]> {
  const cfg = store.state.settings.integrations;
  const sources: Promise<Issue[]>[] = [];
  if (cfg?.linear) sources.push(linearIssues(search || undefined));
  if (cfg?.jira) sources.push(jiraIssues(cfg.jira, search || undefined));
  sources.push(gitlabProject(project.path).then((g) => (g ? gitlabIssues(project.path, search || undefined) : [])));
  const found = await Promise.all(sources.map((p) => p.catch(() => [] as Issue[])));
  return found.flat();
}

/** The sessions of the project a mention can name, with a label that tells them apart. */
function otherSessions(project: Project, self: SessionInfo): { label: string; s: SessionInfo }[] {
  const list = Object.values(store.sessions)
    .filter((s) => s.project_id === project.id && s.id !== self.id)
    .sort((a, b) => b.last_activity - a.last_activity);
  const labels = list.map(sessionLabel);
  return list.map((s, i) => ({ s, label: labels.filter((l) => l === labels[i]).length > 1 ? `${labels[i]} ${s.id.slice(0, 6)}` : labels[i] }));
}

function mentionContext(project: Project, session: SessionInfo, request: ComposerRequest): MentionContext {
  const files = new Map<string, Awaited<ReturnType<typeof readFile>> | null>();
  const file = async (path: string) => {
    if (!files.has(path)) files.set(path, await readFile(project.path, path, MAX_TOTAL_BYTES + 1).catch(() => null));
    return files.get(path)!;
  };
  return {
    stat: async (path) => {
      // Outside the project (a dropped file) a path is only ever passed on, never read.
      if (isAbsolute(path)) return { dir: await isDir(path).catch(() => false) };
      if (await file(path)) return { dir: false };
      return (await listDir(project.path, path.replace(/[\\/]+$/, "")).catch(() => null)) ? { dir: true } : null;
    },
    readText: async (path) => {
      const f = isAbsolute(path) ? null : await file(path);
      return f ? { text: f.text ?? "", binary: f.binary, big: f.truncated } : null;
    },
    selection: async () => request.selection ?? currentSelection(),
    diff: () => gitContext(project.path, "worktree", undefined, 400_000).catch(() => ""),
    terminal: async (ref) => {
      const all = otherSessions(project, session);
      const want = ref.toLowerCase();
      // A name that fits several sessions names none of them: it would be the wrong one's output.
      const only = <T,>(list: T[]) => (list.length === 1 ? list[0] : undefined);
      const hit = ref
        ? only(all.filter((x) => x.label.toLowerCase() === want)) ?? (ref.length >= 6 ? only(all.filter((x) => x.s.id.startsWith(ref))) : undefined) ?? only(all.filter((x) => x.label.toLowerCase().startsWith(want)))
        : all.find((x) => x.s.alive && x.s.kind === "shell");
      // 200 lines is what a snippet may hold: asking for more would only turn it into a file.
      return hit ? sessionTranscript(hit.s, 200).catch(() => null) : null;
    },
    issue: async (ref) => {
      // Bare `@issue`: the issue this session was started on (named by its key).
      const key = ref || (/^[A-Za-z][A-Za-z0-9]*-\d+$/.test(session.name ?? "") ? session.name! : "");
      if (!key) return null;
      const hit = (await findIssues(project, key)).find((i) => i.key.toLowerCase() === key.toLowerCase());
      return hit ? issueText(hit) : null;
    },
    notes: () => store.notesFor(project.id),
    redact,
    stash: (label, text) => writeTempFile(label, "txt", toBase64(new TextEncoder().encode(text))),
  };
}

export function Composer(props: { session: SessionInfo; project: Project; request: ComposerRequest }) {
  let root!: HTMLDivElement;
  let textarea!: HTMLTextAreaElement;
  const session = () => store.sessions[props.session.id] ?? props.session;
  /** The composer went away (Esc) while a send was still looking things up: that send is dropped. */
  let disposed = false;
  const [draft, setDraft] = createSignal([drafts.get(props.session.id), props.request.draft].filter(Boolean).join("\n"));
  const [caret, setCaret] = createSignal(0);
  const [selected, setSelected] = createSignal(0);
  const [dismissed, setDismissed] = createSignal<number | null>(null);
  const [found, setFound] = createSignal<Suggestion[]>([]);
  const [sending, setSending] = createSignal(false);
  const [dropping, setDropping] = createSignal(false);
  /** An earlier prompt is on screen (↑ / ↓): its mentions are not completions until the user types. */
  const [walking, setWalking] = createSignal(false);

  const kindDetail = (s: SessionInfo) => (s.alive ? s.kind : t("exited"));
  const token = () => (walking() ? null : mentionAt(draft(), caret()));
  const suggestions = () => {
    const tk = token();
    return tk && dismissed() !== tk.start ? found() : [];
  };

  // Completions come a little after the last keystroke; only the newest answer is shown.
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  createEffect(on(token, (tk) => {
    clearTimeout(timer);
    const gen = ++generation;
    if (!tk) return setFound([]);
    const q = tk.query;
    const show = (list: Suggestion[]) => gen === generation && setFound(list);
    const lower = q.toLowerCase();
    if (lower.startsWith("terminal:")) {
      const part = lower.slice(9).replace(/^"/, "");
      return show(otherSessions(props.project, session()).filter((x) => x.label.toLowerCase().includes(part)).slice(0, 12).map((x) => ({ label: x.label, detail: kindDetail(x.s), insert: terminalMention(x.label) })));
    }
    timer = setTimeout(() => {
      if (lower.startsWith("issue:")) {
        void findIssues(props.project, q.slice(6)).then((list) => show(list.slice(0, 12).map((i) => ({ label: i.key, detail: i.title, insert: issueMention(i.key) }))));
        return;
      }
      const words = KEYWORDS.filter((k) => k.name.startsWith(lower)).map((k): Suggestion => ({ label: `@${k.name}`, detail: t(k.detail), insert: `@${k.name}`, more: k.more }));
      void quickOpen(props.project.path, q, 30, true).then(
        // A name with control or bidi characters is not offered: it would look like another file.
        (hits) => show([...words, ...hits.filter((h) => !hasUnsafeName(h.path)).map((h) => ({ label: h.path, detail: h.is_dir ? t("folder") : undefined, insert: pathMention(h.path) }))]),
        () => show(words),
      );
    }, 80);
  }));
  onCleanup(() => clearTimeout(timer));
  createEffect(on(found, () => setSelected(0)));

  function setText(text: string, at = text.length) {
    setDraft(text);
    textarea.value = text;
    textarea.setSelectionRange(at, at);
    setCaret(at);
    grow();
  }
  const grow = () => {
    textarea.style.height = "auto";
    textarea.style.height = Math.min(260, textarea.scrollHeight) + "px";
  };

  function accept(s: Suggestion) {
    const tk = token();
    if (!tk) return;
    const insert = s.more ? s.insert : `${s.insert} `;
    const text = draft();
    setText(text.slice(0, tk.start) + insert + text.slice(caret()), tk.start + insert.length);
    textarea.focus();
  }

  // ---------- history (↑ / ↓) ----------
  const history = () => store.promptsFor(props.project.id);
  let position = -1;
  let typed = "";
  function walk(dir: 1 | -1) {
    const next = stepHistory(history().length, position, dir);
    if (next === position) return;
    if (position === -1) typed = draft();
    position = next;
    setWalking(position !== -1);
    setText(position === -1 ? typed : history()[position]);
  }

  // ---------- pictures ----------
  /** A dropped file that is not a picture becomes a mention. */
  const { images, drop, paste, forChat, remove } = createPictures(session, (others) => {
    const rels: string[] = [];
    for (const p of others) {
      const mention = droppedFileMention(relativeToProject(p));
      if (mention === null) store.notify(t("{name}: this name cannot be written safely for a terminal", { name: JSON.stringify(basename(p)) }), undefined, 6000);
      else rels.push(mention);
    }
    if (rels.length) {
      const text = draft();
      const at = textarea.selectionStart ?? text.length;
      const insert = rels.join(" ") + " ";
      setText(text.slice(0, at) + insert + text.slice(at), at + insert.length);
    }
    textarea.focus();
  });

  const relativeToProject = (p: string) => {
    const root = joinPath(props.project.path, "").replace(/[\\/]+$/, "");
    const norm = (x: string) => x.replace(/\\/g, "/").toLowerCase();
    return norm(p).startsWith(norm(root) + "/") ? p.slice(root.length + 1).replace(/\\/g, "/") : p;
  };

  onMount(() => {
    textarea.value = draft();
    textarea.focus();
    textarea.setSelectionRange(draft().length, draft().length);
    setCaret(draft().length);
    grow();
    const unlisten = getCurrentWebview().onDragDropEvent((event) => {
      const { type } = event.payload;
      if (type === "leave") return setDropping(false);
      const ratio = window.devicePixelRatio || 1;
      const { x, y } = (event.payload as { position: { x: number; y: number } }).position;
      // What is on top under the pointer: nothing goes into a composer that a dialog or a lightbox covers.
      const inside = root.contains(document.elementFromPoint(x / ratio, y / ratio));
      if (type === "drop") {
        setDropping(false);
        if (inside) void drop((event.payload as { paths: string[] }).paths);
      } else setDropping(inside);
    });
    onCleanup(() => void unlisten.then((f) => f()));
  });

  // ---------- sending ----------
  /** A draft is kept for the next time unless it was sent (however the composer goes away:
   *  closed, another request for the same session, the pane unmounted). */
  let sent = false;
  let warned = "";
  onCleanup(() => {
    disposed = true;
    if (!sent && draft().trim()) drafts.set(props.session.id, draft());
    else drafts.delete(props.session.id);
  });

  function close() {
    const pane = root.closest<HTMLElement>(".pane");
    closeComposer();
    // The pane takes the keyboard back: its terminal, or the chat's message field.
    queueMicrotask(() => {
      peekView(props.session.id)?.focus();
      pane?.querySelector<HTMLTextAreaElement>(".chat-composer textarea")?.focus();
    });
  }

  async function send() {
    const text = draft();
    const s = session();
    if (sending() || (!text.trim() && !images().length)) return;
    if (!s.alive) return store.notify(t("This session has ended"));
    setSending(true);
    try {
      const pictures = images();
      // A terminal that has not asked for bracketed paste would take every line break as Enter: an
      // agent still starting gets paths instead of content, and nothing multi-line goes in.
      const bracketed = !s.chat && !!peekView(s.id)?.term.modes.bracketedPasteMode;
      // A shell, or any program that is not an agent, only gets paths, quoted for it.
      const shell = !s.chat && !isAgentKind(s.kind) ? shellStyle(s) : undefined;
      const out = await expandMentions(text, mentionContext(props.project, s, props.request), s.kind, { inline: s.chat || bracketed, shell, remote: !!s.remote, windows: windows && !s.remote });
      if (disposed) return;
      if (out.refused.length) {
        return store.notify(out.refused.map((r) => (r.reason === "remote" ? t("{name}: text too long to inline cannot be sent to a session on another machine", { name: r.mention.arg || r.mention.kind }) : t("{name}: this name cannot be written safely for a terminal", { name: JSON.stringify(r.mention.arg) }))).join("\n"), undefined, 8000);
      }
      // A mention that found nothing is told once; sending the same text again sends it as written
      // (it may not have been a mention: `@note` in a sentence).
      if (out.problems.length && warned !== text) {
        warned = text;
        return store.notify([...out.problems.map(problemText), t("Press Enter again to send it as written")].join("\n"), undefined, 7000);
      }
      if (s.chat) {
        const attachments = await forChat();
        if (disposed) return;
        getController(s).send(out.text, true, attachments);
        store.revealSession(s.id);
      } else {
        const words = pictures.map((i) => (shell ? shellQuote(i.path, shell) : pathReference(s.kind, i.path, windows && !s.remote)));
        if (words.includes(null)) return store.notify(t("{name}: this name cannot be written safely for a terminal", { name: "picture" }), undefined, 8000);
        const full = [out.text, ...words].filter(Boolean).join(" ");
        if (full.includes("\n") && !bracketed) {
          return store.notify(t("This terminal does not take a multi-line paste yet (every line would be sent as its own command): write one line, or wait until the program has started"), undefined, 8000);
        }
        // What is put in a shell by a mention (a file name, a dropped file) stays at the prompt for
        // the user to read and run: a name can be a script, and nothing here knows what it does.
        await sendToSession(s, full, isAgentKind(s.kind) || (!looksLikeMention(text) && !pictures.length));
      }
      store.rememberPrompt(props.project.id, text);
      sent = true;
      close();
    } catch (e) {
      store.notify(errorText(e), undefined, 7000);
    } finally {
      setSending(false);
    }
  }

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing) return;
    const list = suggestions();
    if (list.length) {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        setSelected((i) => (i + (e.key === "ArrowDown" ? 1 : list.length - 1)) % list.length);
        return;
      }
      // Enter sends when the mention is already typed out in full; Tab always picks.
      const tk = token();
      const typedOut = !!tk && list.some((s) => s.insert === draft().slice(tk.start, caret()));
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && !typedOut)) {
        e.preventDefault();
        accept(list[selected()]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        setDismissed(token()?.start ?? null);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey) {
      // ↑ on the first line and ↓ on the last walk the history; elsewhere they move the caret.
      const before = draft().slice(0, textarea.selectionStart ?? 0);
      const after = draft().slice(textarea.selectionEnd ?? 0);
      if (e.key === "ArrowUp" && !before.includes("\n") && textarea.selectionStart === textarea.selectionEnd) (e.preventDefault(), walk(1));
      else if (e.key === "ArrowDown" && position !== -1 && !after.includes("\n")) (e.preventDefault(), walk(-1));
    }
  };

  const target = () => {
    const s = session();
    return `${sessionLabel(s)}${s.chat ? ` · ${t("chat")}` : ""}`;
  };

  return (
    <div class={`composer-float${dropping() ? " dropping" : ""}`} ref={root} data-composer={props.session.id} onPointerDown={(e) => e.stopPropagation()}>
      <div class="composer-box">
        <Show when={suggestions().length}>
          <div class="composer-menu" role="listbox">
            <For each={suggestions()}>
              {(s, i) => (
                <div
                  class={`composer-item${i() === selected() ? " sel" : ""}`}
                  role="option"
                  aria-selected={i() === selected()}
                  onPointerDown={(e) => {
                    e.preventDefault();
                    accept(s);
                  }}
                  onPointerEnter={() => setSelected(i())}
                >
                  <span class="name">{s.label}</span>
                  <Show when={s.detail}><span class="detail">{s.detail}</span></Show>
                </div>
              )}
            </For>
          </div>
        </Show>
        <AttachedPictures images={images()} onRemove={remove} />
        <textarea
          ref={textarea}
          rows={2}
          data-composer-input
          placeholder={t("Message {target}…  (@ files, folders, selection, diff, terminals, issues · paste or drop a picture · Enter to send, Shift+Enter for a new line, ↑ for earlier prompts)", { target: target() })}
          value={draft()}
          onInput={(e) => {
            setDraft(e.currentTarget.value);
            setCaret(e.currentTarget.selectionStart ?? 0);
            position = -1;
            setWalking(false);
            grow();
          }}
          onKeyUp={(e) => (e.key === "ArrowLeft" || e.key === "ArrowRight" || e.key === "Home" || e.key === "End") && setCaret(textarea.selectionStart ?? 0)}
          onClick={() => setCaret(textarea.selectionStart ?? 0)}
          onFocus={() => setDismissed(null)}
          onKeyDown={onKeyDown}
          onPaste={paste}
        />
        <div class="composer-row">
          <Icon name="compose" size={12} />
          <span class="composer-target">{target()}</span>
          <span class="spacer" />
          <MicButton field={() => textarea} />
          <button class="btn" style={{ height: "26px" }} onClick={() => close()}>{t("Close")}</button>
          <button class="btn primary" style={{ height: "26px" }} disabled={sending() || (!draft().trim() && !images().length)} onClick={() => void send()}>{t("Send {key}", { key: isMac ? "↩" : "⏎" })}</button>
        </div>
      </div>
    </div>
  );
}
