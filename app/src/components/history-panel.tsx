// Past agent conversations (Claude / Codex / OMP transcripts on disk), scoped to this folder,
// this project tree, or everything; resumable in a terminal or the chat view, exportable as Markdown.
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "../lib/store";
import { agentHistoryList, isDir, onEvent } from "../lib/ipc";
import { isMac, relTimeShort } from "../lib/platform";
import { newChat, supportsChat } from "../chat/mode-switch";
import { HOME_VAR, accountHomes, accountLabel, isAccountAgent } from "../lib/accounts";

/** A conversation resumes on the account it was made on. */
function historyAccount(it: HistoryItem): { account?: string; env?: Record<string, string> } {
  if (!isAccountAgent(it.agent)) return {};
  return it.home ? { env: { [HOME_VAR[it.agent]]: it.home } } : { account: "default" };
}

export const historyAccountLabel = (it: HistoryItem) => (isAccountAgent(it.agent) && it.home ? accountLabel(it.agent, it.home) : null);
import { Icon, KindGlyph } from "./icons";
import type { HistoryItem, Project } from "../lib/types";
import { formatDate, t } from "../lib/i18n";
import { LoadBar } from "./load-bar";

type Scope = "folder" | "project" | "all";
const PAGE = 150;

// One list for every mount of the panel (it remounts on each project switch): shown at once,
// refreshed in the background, never two scans in flight.
const [items, setItems] = createSignal<HistoryItem[] | null>(null);
const [loadError, setLoadError] = createSignal<string | null>(null);
let inFlight: Promise<void> | null = null;
function refresh(count = true) {
  inFlight ??= agentHistoryList(count, accountHomes())
    .then((list) => {
      setItems(list);
      setLoadError(null);
    })
    .catch((e) => void setLoadError(String(e)))
    .finally(() => (inFlight = null));
  return inFlight;
}

/** Comparable path: forward slashes, no `\\?\` prefix or trailing slash, case-folded (Windows
 *  and default macOS volumes are case-insensitive). */
const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
const within = (path: string, root: string) => path === root || path.startsWith(root + "/");

function dayLabel(ms: number): string {
  const d = new Date(ms);
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  const days = Math.floor((start.getTime() - new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime()) / 86_400_000);
  if (days <= 0) return t("Today");
  if (days === 1) return t("Yesterday");
  if (days < 7) return formatDate(d, { weekday: "long" });
  return formatDate(d);
}

/** A command that resumes the conversation from a fresh terminal (PowerShell on Windows). */
function resumeCommand(it: HistoryItem): string {
  let run = { claude: `claude --resume ${it.id}`, codex: `codex resume ${it.id}`, omp: `omp --resume ${it.id}` }[it.agent];
  const posix = isMac || (!!it.cwd && !/^[a-z]:|^\\\\/i.test(it.cwd));
  // Single quotes: no `$`, backtick or wildcard expansion of folder names.
  const q = (p: string) => (posix ? `'${p.replace(/'/g, "'\\''")}'` : `'${p.replace(/'/g, "''")}'`);
  // A conversation of another account resumes with that account's config home.
  if (it.home && isAccountAgent(it.agent)) run = posix ? `${HOME_VAR[it.agent]}=${q(it.home)} ${run}` : `$env:${HOME_VAR[it.agent]}=${q(it.home)}; ${run}`;
  if (!it.cwd) return run;
  return posix ? `cd ${q(it.cwd)} && ${run}` : `Set-Location -LiteralPath ${q(it.cwd)}; ${run}`;
}

/** The sidebar project owning the folder (deepest match), adding the folder as a project if none. */
function projectFor(it: HistoryItem): string {
  const paths = [norm(it.cwd), norm(it.folder || it.cwd)];
  const owner = store.state.projects
    .filter((p) => paths.some((f) => within(f, norm(p.path))))
    .sort((a, b) => b.path.length - a.path.length)[0];
  if (owner) return owner.id;
  return store.addProject(it.folder || it.cwd);
}

/** Agents whose transcripts the app core can export (Markdown; lib/session-export.ts, loaded on use). */
const EXPORTABLE = new Set(["claude", "codex", "omp"]);

/** Conversations being resumed right now (double clicks must not start a second writer). */
const resuming = new Set<string>();

export function HistoryPanel(props: { project: Project }) {
  const [scope, setScope] = createSignal<Scope>("folder");
  const [query, setQuery] = createSignal("");
  const [limit, setLimit] = createSignal(PAGE);

  onMount(() => void refresh());
  // Prompt counts arrive later from a background pass; re-list without starting another one.
  const unlisten = onEvent("agent-history-counted", () => void refresh(false));
  onCleanup(() => void unlisten.then((f) => f()));

  const live = createMemo(() => {
    const m = new Map<string, string>();
    for (const s of Object.values(store.sessions)) if (s.alive && s.agent_session_id) m.set(s.agent_session_id, s.id);
    return m;
  });

  const filtered = createMemo(() => {
    const root = norm(props.project.path);
    const words = query().toLowerCase().split(/\s+/).filter(Boolean);
    return (items() ?? []).filter((it) => {
      // Either spelling may be the one the project was added with (8.3 short vs. long names).
      const paths = [norm(it.cwd), norm(it.folder || it.cwd)];
      if (scope() === "folder" && !paths.includes(root)) return false;
      if (scope() === "project" && !paths.some((f) => within(f, root))) return false;
      if (!words.length) return true;
      const hay = `${it.title ?? ""} ${it.first_prompt ?? ""} ${it.cwd} ${it.agent}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  });

  const groups = createMemo(() => {
    const out: { day: string; items: HistoryItem[] }[] = [];
    for (const it of filtered().slice(0, limit())) {
      const day = dayLabel(it.updated_at);
      if (out[out.length - 1]?.day !== day) out.push({ day, items: [] });
      out[out.length - 1].items.push(it);
    }
    return out;
  });

  async function resume(it: HistoryItem, chat: boolean) {
    const running = live().get(it.id);
    if (running) return store.revealSession(running);
    const key = `${it.agent}:${it.id}`;
    if (resuming.has(key)) return;
    if (!it.cwd) return store.notify(t("This conversation has no folder to resume in"));
    resuming.add(key);
    try {
      // A deleted folder (e.g. a removed worktree) must not turn into a dead project.
      if (!(await isDir(it.cwd).catch(() => false))) return store.notify(t("Folder no longer exists: {path}", { path: it.cwd }));
      const pid = projectFor(it);
      const info = chat
        ? await newChat(pid, it.agent, "tab", { resume: it.id, cwd: it.cwd, home: it.home })
        : await store.newSession(pid, it.agent, { agentSessionId: it.id, cwd: it.cwd, ...historyAccount(it) });
      // The owning project may not be the one on screen.
      if (info) store.revealSession(info.id);
    } finally {
      resuming.delete(key);
    }
  }

  return (
    <div class="history">
      <LoadBar when={!items() && !loadError()} />
      <div class="explorer-tools">
        <input class="text-input" placeholder={t("Search conversations")} value={query()} onInput={(e) => { setQuery(e.currentTarget.value); setLimit(PAGE); }}
          onKeyDown={(e) => e.key === "Escape" && setQuery("")} />
        <div class="history-scope">
          <div class="seg">
            <button class={scope() === "folder" ? "on" : ""} title={props.project.path} onClick={() => setScope("folder")}>{t("This folder")}</button>
            <button class={scope() === "project" ? "on" : ""} title={t("This folder and every folder inside it")} onClick={() => setScope("project")}>{t("Project")}</button>
            <button class={scope() === "all" ? "on" : ""} onClick={() => setScope("all")}>{t("All")}</button>
          </div>
          <button class="icon-btn" title={t("Rescan transcripts")} onClick={() => void refresh()}><Icon name="refresh" size={14} /></button>
        </div>
      </div>
      <div class="list history-list">
        <Show when={items() || loadError()} fallback={<div class="muted">{t("Reading transcripts…")}</div>}>
          <Show when={!loadError() || items()} fallback={<div class="muted">{t("Could not read transcripts: {error}", { error: loadError() ?? "" })}</div>}>
            <For each={groups()} fallback={<div class="muted">{query() ? t("No matching conversations") : t("No conversations here yet")}</div>}>
              {(g) => (
                <>
                  <div class="history-day">{g.day}</div>
                  <For each={g.items}>
                    {(it) => (
                      <div class={`history-row${live().has(it.id) ? " running" : ""}`} data-agent={it.agent} data-id={it.id} title={[it.title, it.first_prompt, it.cwd].filter(Boolean).join("\n")} onDblClick={() => void resume(it, false)}>
                        <KindGlyph kind={it.agent} size={14} />
                        <div class="history-text">
                          <div class="history-title">{it.title ?? it.first_prompt ?? t("(no prompt)")}</div>
                          <div class="history-sub">
                            <Show when={scope() !== "folder" && it.cwd}><span class="history-dir">{it.cwd.split(/[\\/]/).pop()}</span></Show>
                            <span>{relTimeShort(it.updated_at)}</span>
                            <Show when={it.turns}><span>{it.turns === 1 ? t("{n} prompt", { n: it.turns }) : t("{n} prompts", { n: it.turns ?? 0 })}</span></Show>
                            <Show when={live().has(it.id)}><span class="history-live">{t("open")}</span></Show>
                            <Show when={historyAccountLabel(it)}>{(l) => <span class="history-account" title={t("Account")}>{l()}</span>}</Show>
                          </div>
                        </div>
                        <div class="row-actions" onDblClick={(e) => e.stopPropagation()}>
                          <button class="icon-btn" title={live().has(it.id) ? t("Show the running session") : t("Resume in terminal")} onClick={() => void resume(it, false)}><Icon name="terminal" size={13} /></button>
                          <Show when={supportsChat(it.agent) && !live().has(it.id)}>
                            <button class="icon-btn" title={t("Open in chat view")} onClick={() => void resume(it, true)}><Icon name="chat" size={13} /></button>
                          </Show>
                          <Show when={EXPORTABLE.has(it.agent)}>
                            <button class="icon-btn" data-act="export" title={t("Export as Markdown…")} onClick={() => void import("../lib/session-export").then((m) => m.exportConversation(it))}><Icon name="pull" size={13} /></button>
                          </Show>
                          <button class="icon-btn" title={t("Copy resume command")} onClick={() => void writeText(resumeCommand(it)).then(() => store.notify(t("Resume command copied")))}><Icon name="copy" size={13} /></button>
                        </div>
                      </div>
                    )}
                  </For>
                </>
              )}
            </For>
            <Show when={filtered().length > limit()}>
              <button class="btn history-more" onClick={() => setLimit((l) => l + PAGE)}>{t("Show more ({n})", { n: filtered().length - limit() })}</button>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}
