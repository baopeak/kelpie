// What each session runs and the ports it holds (daemon `process.*`). The daemon never looks on its
// own: this module asks, and only for what is on screen — the sessions of the visible tabs, once when
// they appear, again shortly after their terminal prints something (at most every 2 s, so a session
// that keeps printing is looked at that often), and every few seconds for as long as the last look
// found programs running under them. A session that is quiet and had nothing under it is not looked at
// again until it prints something. A session that leaves the screen keeps its chips, marked stale.
import { createEffect, createMemo, createRoot } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { daemon } from "./ipc";
import { store } from "./store";
import { openBrowser } from "./browser";
import { PROCESS_API } from "./process-api";
import { onTerminalOutput } from "./terminal-host";
import { errorText, t } from "./i18n";

export interface Port {
  port: number;
  /** What a browser on this machine reaches it by (`localhost`, or the one address it listens on). */
  host: string;
  addr: string;
  pid?: number;
  name?: string;
}

export interface ProcInfo {
  pid: number;
  ppid: number | null;
  name: string;
  start_time: number;
  exe: string | null;
  mem: number;
  cpu: number;
  ports: Port[];
  orphan?: boolean;
}

export interface ProcessList {
  session: string;
  root: ProcInfo | null;
  children: ProcInfo[];
  orphans: ProcInfo[];
  /** Pids "Kill children" leaves: the session's own process (and the agent under a launcher). */
  keep: number[];
  dropped: number;
  took_us: number;
}

export interface KillReport {
  closed: number[];
  forced: number[];
  gone: number[];
  refused: { pid: number; reason: string }[];
}

export interface PortOwner {
  port: number;
  pid: number;
  name: string | null;
  exe: string | null;
  session: string | null;
}

export const listProcesses = (session: string) => daemon<ProcessList>("process.list", { session });
/** The full identity goes with the request: the daemon refuses unless the process is still the one listed. */
export const killProcess = (session: string, p: ProcInfo) => daemon<KillReport>("process.kill", { session, pid: p.pid, start_time: p.start_time, exe: p.exe });
export const killChildren = (session: string) => daemon<KillReport>("process.kill_children", { session });
export const portOwner = (port: number) => daemon<PortOwner | null>("process.port_owner", { port });

export const portUrl = (p: Pick<Port, "host" | "port">) => `http://${p.host}:${p.port}/`;

/** The ports held by a session's processes, as of the last look. `stale`: the session left the screen since, so it
 *  is no longer looked at and the ports may have closed. */
export const [chips, setChips] = createStore<Record<string, { children: number; ports: Port[]; stale?: boolean }>>({});

/** One entry per port: a server that listens on both IPv4 and IPv6 is one port. */
export const uniquePorts = (ports: Port[]): Port[] => ports.filter((p, i) => ports.findIndex((q) => q.port === p.port) === i);

/** Opens a port in a browser pane: a tab, or split beside the focused pane. */
export function openPort(p: Port, sessionId: string, where: "tab" | "right" = "tab") {
  const projectId = store.sessions[sessionId]?.project_id ?? store.state.activeProject ?? undefined;
  openBrowser(portUrl(p), where, projectId ?? undefined);
}

/** "node.exe (pid 4242)" for the tooltip of a port. */
export const ownerLabel = (p: Port) => (p.name ? `${p.name} (pid ${p.pid})` : p.pid ? `pid ${p.pid}` : "");

export function formatBytes(n: number): string {
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
  if (n >= 1024 ** 2) return `${Math.round(n / 1024 ** 2)} MB`;
  return `${Math.max(1, Math.round(n / 1024))} KB`;
}

/** After a stop: what to tell the user about processes that were not stopped, in one line. */
export function killSummary(r: KillReport): string | null {
  if (!r.refused.length) return null;
  return t("Not stopped: {reasons}", { reasons: [...new Set(r.refused.map((x) => errorText(x.reason)))].join("; ") });
}

// ---------- the looks ----------

/** At least this long between two looks. */
const MIN_GAP_MS = 2000;
/** Output of a session that was quiet waits this long: the program it started needs a moment to be running. */
const SETTLE_MS = 800;
/** While programs run under a visible session. */
const POLL_MS = 3000;

let timer: ReturnType<typeof setTimeout> | undefined;
/** When the planned look runs. */
let due = 0;
let lastLook = 0;

const supported = () => store.connected() && store.daemonApi() >= PROCESS_API;
const visibleIds = () => Object.values(store.sessions).filter((s) => s.alive && !s.remote && store.isSessionVisible(s.id)).map((s) => s.id);

async function look() {
  timer = undefined;
  if (!supported() || document.hidden) return;
  const ids = visibleIds();
  if (!ids.length) return;
  lastLook = Date.now();
  try {
    const r = await daemon<{ sessions: Record<string, { children: number; ports: Port[] }> }>("process.ports", { sessions: ids });
    setChips(
      produce((m) => {
        for (const id of Object.keys(m)) {
          if (!store.sessions[id]?.alive) delete m[id];
          else if (!ids.includes(id)) m[id].stale = true;
        }
        for (const id of ids) {
          const s = r.sessions[id];
          if (s) m[id] = { children: s.children, ports: s.ports };
          else delete m[id];
        }
      }),
    );
    if (ids.some((id) => chips[id]?.children)) wake(POLL_MS);
  } catch {
    // The next output or tab switch looks again.
  }
}

/** Looks in `delay` ms, never sooner than `MIN_GAP_MS` after the previous look; a look already planned for
 *  sooner stands, one planned for later is brought forward (a server says it listens a moment after it starts). */
function wake(delay: number) {
  const now = Date.now();
  const at = Math.max(now + delay, lastLook + MIN_GAP_MS);
  if (timer && due <= at) return;
  clearTimeout(timer);
  due = at;
  timer = setTimeout(() => void look(), at - now);
}

/** A visible session's terminal printed something: a program may have started. */
export function noteOutput(sessionId: string) {
  if (!supported() || !store.isSessionVisible(sessionId)) return;
  wake(SETTLE_MS);
}

if (typeof window !== "undefined") {
  onTerminalOutput(noteOutput);
  createRoot(() => {
    // Sessions coming on screen (tab, project or window switch) are looked at once.
    const onScreen = createMemo(() => visibleIds().join(","));
    createEffect(() => {
      if (onScreen() && supported()) wake(300);
    });
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) wake(300);
  });
}
