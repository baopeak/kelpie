// Which browser panes an agent session may use with Kelpie's browser tools, and how they are described to it: plain functions of the
// app's state, tested without a window (the side that answers the daemon and talks to Rust is agent-browser.ts). A session may use the
// panes opened for it or for a session above it (an agent's `kelpie_open_page`, the tab of a server it started), and the panes the person
// shares with agents in the session's project. Text that goes to agents stays in English: it is read by a program, not shown to the person.
import { allPanes } from "./layout";
import type { Project, Workspace } from "./types";

export type BrowserMode = "off" | "read" | "act";

/** What the app can do in a pane on this platform. */
export interface BrowserFeatures {
  /** Input the page cannot tell from the person's (else synthetic DOM events). Only in a pane that is shown. */
  trustedInput: boolean;
  /** Screenshots of the page. Only of a pane that is shown. */
  screenshot: boolean;
  /** The page's network requests. */
  network: boolean;
}

/** The one place that says what this build can do: each is turned on, per platform, once its backend exists. */
export const BROWSER_FEATURES: Readonly<BrowserFeatures> = { trustedInput: false, screenshot: false, network: false };

export interface PaneCapabilities {
  input: "trusted" | "synthetic";
  screenshot: boolean;
  network: boolean;
}

/** What an agent can do in a pane, shown or in a background tab. */
export function paneCapabilities(shown: boolean, features: BrowserFeatures = BROWSER_FEATURES): PaneCapabilities {
  return { input: features.trustedInput && shown ? "trusted" : "synthetic", screenshot: features.screenshot && shown, network: features.network };
}

/** A pane the person shares with agents: the sites it was on when shared (the agent may stay on those, and on this computer's own). */
export interface PaneShare {
  origins: string[];
}

/** The origin (`scheme://host[:port]`) of an http(s) address, or null for anything else. */
export function webOrigin(url: string | undefined | null): string | null {
  if (!url) return null;
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.origin : null;
  } catch {
    return null;
  }
}

/** What a share records when the person turns it on: the site the pane shows now. Nothing for a page that is not on the web. */
export function sharedOrigins(url: string | undefined | null): string[] {
  const o = webOrigin(url);
  return o ? [o] : [];
}

/** The two things this needs of a session: its project and the session that opened it. */
export interface SessionRow {
  project_id: string | null;
  parent?: string | null;
}

/** Whether `opener` is `sessionId` or a session above it (its parent, that one's parent…). */
export function openedForSession(opener: string, sessionId: string, sessions: (id: string) => SessionRow | undefined): boolean {
  const seen = new Set<string>();
  for (let id: string | null | undefined = sessionId; id && !seen.has(id); id = sessions(id)?.parent) {
    if (id === opener) return true;
    seen.add(id);
  }
  return false;
}

/** Whether an http(s) address is on this computer (localhost, 127.x.x.x, ::1): a shared pane may always go there. */
export function isLoopback(url: string | undefined | null): boolean {
  if (!webOrigin(url)) return false;
  const host = new URL(url!).hostname;
  return host === "localhost" || host.endsWith(".localhost") || host === "[::1]" || /^127\.\d+\.\d+\.\d+$/.test(host);
}

/** A pane one session may use, as the browser tools see it. `id` is the Kelpie pane id agents name it by; `label` its webview's. */
export interface UsablePane {
  id: string;
  label: string;
  projectId: string;
  tabId: string;
  title: string;
  url: string;
  kind: "opened" | "shared";
  /** The sites of a shared pane (empty for a pane opened for the session). */
  origins: string[];
  /** A shared pane that is on another site now than the ones it was shared on (nor on this computer): not listed, not used, its address
   *  and title not told to agents, until it is back. */
  offSite: boolean;
  /** On screen now: in the project and tab shown. */
  shown: boolean;
  /** A phone controls it now. */
  held: boolean;
}

/** How the window sees panes now (which are on screen, which a phone controls), and whether the calling session runs restricted. */
export interface PaneView {
  shown?: (projectId: string, tabId: string, paneId: string) => boolean;
  held?: Readonly<Record<string, unknown>>;
  /** A restricted session uses only the panes opened for it or a session above it, never a shared one. */
  restricted?: boolean;
}

/** The panes `sessionId` may use, in the projects' order: every browser pane with a web page that was opened for the session or a session
 *  above it, or that the person shares and that is in the session's project (not for a restricted session). A pane the person stopped
 *  the agents in is left out until it is shared again. */
export function usablePanes(
  sessionId: string,
  projects: Project[],
  workspace: (id: string) => Workspace,
  sessions: (id: string) => SessionRow | undefined,
  pages: Readonly<Record<string, { title: string; url: string } | undefined>>,
  shared: Readonly<Record<string, PaneShare | undefined>>,
  stopped: ReadonlySet<string>,
  view: PaneView = {},
): UsablePane[] {
  const own = sessions(sessionId)?.project_id ?? null;
  const out: UsablePane[] = [];
  for (const p of projects) {
    for (const tab of workspace(p.id).tabs) {
      for (const pane of allPanes(tab.root)) {
        if (pane.kind !== "browser" || stopped.has(pane.id)) continue;
        const url = pages[pane.id]?.url || pane.url || "";
        const origin = webOrigin(url);
        if (!origin) continue;
        const share = shared[pane.id];
        const opened = !!pane.openedBy && openedForSession(pane.openedBy, sessionId, sessions);
        if (!opened && (view.restricted || !(share && own === p.id))) continue;
        const origins = opened ? [] : [...(share?.origins ?? [])];
        out.push({
          id: pane.id,
          label: `browser-${pane.id}`,
          projectId: p.id,
          tabId: tab.id,
          title: pages[pane.id]?.title ?? "",
          url,
          kind: opened ? "opened" : "shared",
          origins,
          offSite: !opened && !origins.includes(origin) && !isLoopback(url),
          shown: view.shown?.(p.id, tab.id, pane.id) ?? false,
          held: !!view.held?.[pane.id],
        });
      }
    }
  }
  return out;
}

/** What an agent is told when it has no pane to use. */
export const NO_PANE = 'No browser pane is usable by this session. Open one with kelpie_open_page, or ask the user to share one ("Let agents use this page").';

/** The ids an error names, a few at most: the provider cuts an error to 300 characters. */
const listIds = (usable: UsablePane[]) => {
  const ids = usable.slice(0, 8).map((p) => p.id);
  return usable.length > ids.length ? `${ids.join(", ")}, …` : ids.join(", ");
};

const offSiteError = (id: string) => `Pane ${id} is shared, but its page is now on another site; it is usable again once it is back.`;

/** The pane a tool call is about: the one it names (its id, or its webview's label), else the only usable one. `exists` tells a pane
 *  that is not this session's from one that is not there at all. A shared pane on another site counts as usable for nothing. */
export function resolvePane(arg: unknown, all: UsablePane[], exists?: (paneId: string) => boolean): { pane: UsablePane } | { error: string } {
  const usable = all.filter((p) => !p.offSite);
  const named = typeof arg === "string" ? arg.trim() : "";
  if (!named) {
    if (usable.length === 1) return { pane: usable[0] };
    if (!usable.length) return { error: NO_PANE };
    return { error: `Several panes are usable; pass pane: ${listIds(usable)}.` };
  }
  const id = named.startsWith("browser-") && !all.some((p) => p.id === named) ? named.slice("browser-".length) : named;
  const pane = all.find((p) => p.id === id);
  if (pane) return pane.offSite ? { error: offSiteError(id) } : { pane };
  const why = exists?.(id) ? `Pane ${id} is neither opened for this session nor shared in its project.` : `There is no browser pane ${id}.`;
  return { error: usable.length ? `${why} Usable: ${listIds(usable)}.` : `${why} ${NO_PANE}` };
}

/** Why the window refuses a tool in a pane it may otherwise use, or null. */
export function refusal(pane: UsablePane, state: { design?: boolean }): string | null {
  if (pane.offSite) return offSiteError(pane.id);
  if (pane.held) return `A phone controls pane ${pane.id} now; try again once it lets go.`;
  if (state.design) return `The user is picking elements in pane ${pane.id} (design mode); try again once they finish.`;
  return null;
}

/** What the window tells Rust about an agent's call in a pane (`browser_agent_run`'s `ctx`). */
export interface AgentCallContext {
  session: string;
  sessionName: string;
  kind: "opened" | "shared";
  /** A shared pane's sites (empty for a pane opened for the session). */
  origins: string[];
  /** The policy's allowed sites, for a pane opened for the session without a profile of its own. */
  sites: string[];
  mayEval: boolean;
  /** The pane's project. */
  project: string;
  /** The pane's page has a browser profile of its own. Always false until panes record the profile their page was created with: a
   *  build setting cannot tell, since a page the person opened uses the person's profile whatever the build can do. */
  isolated: boolean;
}

/** The policy as the daemon forwards it with a call. */
export interface CallPolicy {
  mode?: string;
  eval?: boolean;
  sites?: unknown;
}

/** `ctx` for a call in `pane`: scripts only in a pane opened for the session and only when the policy allows them. */
export function callContext(pane: UsablePane, policy: CallPolicy | null | undefined, session: { id: string; name: string }): AgentCallContext {
  const sites = Array.isArray(policy?.sites) ? policy.sites.filter((s): s is string => typeof s === "string") : [];
  return {
    session: session.id,
    sessionName: session.name,
    kind: pane.kind,
    origins: pane.kind === "shared" ? [...pane.origins] : [],
    sites,
    mayEval: policy?.eval === true && pane.kind === "opened",
    project: pane.projectId,
    isolated: false,
  };
}

/** What the window does with one call: answer it (`tabs`), refuse it, or run it in a pane. */
export type CallPlan = { answer: ReturnType<typeof tabsReply> } | { error: string } | { pane: UsablePane };

export function planCall(tool: string, args: Readonly<Record<string, unknown>>, usable: UsablePane[], state: { exists?: (paneId: string) => boolean; design?: (paneId: string) => boolean } = {}): CallPlan {
  if (tool === "tabs") return { answer: tabsReply(usable) };
  const picked = resolvePane(args.pane, usable, state.exists);
  if ("error" in picked) return picked;
  const why = refusal(picked.pane, { design: state.design?.(picked.pane.id) });
  return why ? { error: why } : picked;
}

/** One pane as `kelpie_browser_tabs` lists it. */
export interface TabEntry {
  pane: string;
  title: string;
  url: string;
  kind: "opened" | "shared";
  shown: boolean;
  input: "trusted" | "synthetic";
  screenshot: boolean;
  network: boolean;
  held: boolean;
}

const yesNo = (b: boolean) => (b ? "yes" : "no");

/** One line of `kelpie_browser_tabs`: `<id> "<title>" <url> — opened by you|shared, shown|background tab, input: …, screenshot: …, network: …[, a phone controls it]`. */
export function tabLine(e: TabEntry): string {
  const title = JSON.stringify(e.title.replace(/\s+/g, " ").trim());
  const parts = [
    e.kind === "opened" ? "opened by you" : "shared",
    e.shown ? "shown" : "background tab",
    `input: ${e.input}`,
    `screenshot: ${yesNo(e.screenshot)}`,
    `network: ${yesNo(e.network)}`,
    ...(e.held ? ["a phone controls it"] : []),
  ];
  return `${e.pane} ${title} ${e.url} — ${parts.join(", ")}`;
}

/** The answer to `kelpie_browser_tabs`: one line per usable pane (or how to get one), and the same as data. A shared pane on another site
 *  is left out: its address and title are the person's business. */
export function tabsReply(usable: UsablePane[], features: BrowserFeatures = BROWSER_FEATURES): { text: string; data: { tabs: TabEntry[] } } {
  const tabs = usable.filter((p) => !p.offSite).map((p): TabEntry => ({ pane: p.id, title: p.title, url: p.url, kind: p.kind, shown: p.shown, held: p.held, ...paneCapabilities(p.shown, features) }));
  return { text: tabs.length ? tabs.map(tabLine).join("\n") : NO_PANE, data: { tabs } };
}

// ---------- the daemon's policy (Settings → Agents → Browser panes) ----------

export interface BrowserPolicy {
  mode: BrowserMode;
  /** Agents may run scripts in the panes opened for them (with mode act). */
  eval: boolean;
  /** Sites agents may open in their panes besides this computer's own (origins). */
  sites: string[];
}

/** The policy from a `daemon.policy` / `daemon.set_policy` reply, or null when the reply has no browser keys (an older daemon). */
export function toBrowserPolicy(raw: unknown): BrowserPolicy | null {
  const r = raw as Record<string, unknown> | null;
  if (!r || typeof r !== "object") return null;
  const mode = r.browser_agents;
  if (mode !== "off" && mode !== "read" && mode !== "act") return null;
  const sites = Array.isArray(r.browser_sites) ? r.browser_sites.filter((s): s is string => typeof s === "string") : [];
  return { mode, eval: r.browser_eval === true, sites };
}

/** The most sites the list takes (the daemon's limit). */
export const MAX_SITES = 50;

export type SiteError = "scheme" | "invalid" | "path" | "duplicate" | "full";

/** A site typed into the allowed list, as an origin: `http://` or `https://`, a host and maybe a port, nothing after it. */
export function parseSite(input: string): { origin: string } | { error: SiteError } {
  const s = input.trim();
  if (!/^https?:\/\//i.test(s)) return { error: "scheme" };
  let u: URL;
  try {
    u = new URL(s);
  } catch {
    return { error: "invalid" };
  }
  if (!u.hostname || u.username || u.password) return { error: "invalid" };
  if (u.pathname !== "/" || u.search || u.hash) return { error: "path" };
  return { origin: u.origin };
}

/** The list with one more site, or why it cannot take it. */
export function addSite(sites: string[], input: string): { sites: string[] } | { error: SiteError } {
  const parsed = parseSite(input);
  if ("error" in parsed) return parsed;
  if (sites.includes(parsed.origin)) return { error: "duplicate" };
  if (sites.length >= MAX_SITES) return { error: "full" };
  return { sites: [...sites, parsed.origin] };
}

/** One line of the daemon's log of what agents did in browser panes. */
export interface AuditEntry {
  /** Wall-clock ms. */
  at: number;
  session: string;
  label: string;
  tool: string;
  host: string;
  target: string;
  ok: boolean;
  error: string;
}

/** The entries of a `browser.audit` reply, newest first; malformed ones are left out. */
export function toAuditEntries(raw: unknown): AuditEntry[] {
  const list = (raw as { entries?: unknown } | null)?.entries;
  if (!Array.isArray(list)) return [];
  const str = (v: unknown) => (typeof v === "string" ? v : "");
  const out: AuditEntry[] = [];
  for (const e of list) {
    const r = e as Record<string, unknown> | null;
    if (!r || typeof r !== "object" || typeof r.at !== "number" || !Number.isFinite(r.at) || typeof r.tool !== "string") continue;
    out.push({ at: r.at, session: str(r.session), label: str(r.label), tool: r.tool, host: str(r.host), target: str(r.target), ok: r.ok === true, error: str(r.error) });
  }
  return out.sort((a, b) => b.at - a.at);
}
