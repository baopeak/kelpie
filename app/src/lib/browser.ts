// Browser panes (backend: app/src-tauri/src/browser.rs): commands, one event listener routed
// by webview label, element picks, and the prompt an agent gets for a pick.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createStore, produce } from "solid-js/store";
import { createRoot, createSignal } from "solid-js";
import { store } from "./store";
import { allPanes } from "./layout";
import { uid } from "./platform";

export interface Rect { x: number; y: number; width: number; height: number }

export interface Pick {
  page: { url: string; title: string; width: number; height: number; dpr: number; scrollX: number; scrollY: number };
  selector: string;
  tag: string;
  element: string;
  text: string;
  html: string;
  attributes: Record<string, string>;
  ancestors: string[];
  nearby: string[];
  rect: Rect;
  styles: Record<string, string>;
  role: string;
  name: string;
}

/** A picked element (or a whole-page screenshot) with what the user wants done about it. */
export interface Annotation {
  id: string;
  pick: Pick | null;
  pageUrl: string;
  pageTitle: string;
  shot: { path: string; data: string } | null;
  comment: string;
}

export interface BrowserEvent {
  label: string;
  kind: "nav" | "load" | "title" | "pick" | "cancel" | "popup";
  url?: string;
  title?: string;
  data?: Pick | null;
}

export const browserLabel = (paneId: string) => `browser-${paneId}`;

/** Per pane, across remounts (project / tab switches): page title and URL, design mode, marks. */
export const [browserState, setBrowserState] = createStore<Record<string, { title: string; url: string; design: boolean; notes: Annotation[] }>>({});

/** Closes a pane's webview and forgets its state. */
export function closeBrowser(paneId: string) {
  void browserClose(browserLabel(paneId));
  setBrowserState(produce((m) => delete m[paneId]));
}

// Opening, showing, hiding and closing one webview run strictly in the order asked. Commands
// otherwise run concurrently and creating a webview takes around half a second: a hide sent meanwhile
// finds nothing to hide, or is overtaken by the open's show, and the page stays on screen above
// whatever tab or project is shown now (it is native, so it paints over the whole app).
const lifecycle = new Map<string, Promise<unknown>>();
function inOrder<T>(label: string, run: () => Promise<T>): Promise<T> {
  const result = (lifecycle.get(label) ?? Promise.resolve()).then(run);
  const tail = result.catch(() => {});
  lifecycle.set(label, tail);
  void tail.then(() => {
    if (lifecycle.get(label) === tail) lifecycle.delete(label);
  });
  return result;
}

export const browserOpen = (label: string, url: string, rect: Rect) => inOrder(label, () => invoke<void>("browser_open", { label, url, rect }));
export const browserBounds = (label: string, rect: Rect) => invoke<void>("browser_bounds", { label, rect });
export const browserVisible = (label: string, visible: boolean) => inOrder(label, () => invoke<void>("browser_visible", { label, visible }));
export const browserNavigate = (label: string, url: string) => invoke<string>("browser_navigate", { label, url });
export const browserGo = (label: string, delta: -1 | 0 | 1) => invoke<void>("browser_go", { label, delta });
export const browserDesign = (label: string, on: boolean) => invoke<void>("browser_design", { label, on });
export const browserDevtools = (label: string) => invoke<void>("browser_devtools", { label });
export const browserClose = (label: string) => inOrder(label, () => invoke<void>("browser_close", { label })).catch(() => {});
export const browserScreenshot = (label: string, clip?: Rect) => invoke<{ path: string; data: string }>("browser_screenshot", { label, clip });

/** Whether a local port answers with a web page, and its title. */
export const browserProbe = (host: string, port: number) => invoke<{ title: string | null } | null>("browser_probe", { host, port });

/** Panes opened for the user that have not been looked at yet: their tabs stand out. */
export const [freshPages, setFreshPages] = createRoot(() => createSignal<ReadonlySet<string>>(new Set()));
export const markPageFresh = (paneId: string) => setFreshPages((s) => new Set(s).add(paneId));
export const markPageSeen = (paneId: string) =>
  setFreshPages((s) => {
    if (!s.has(paneId)) return s;
    const next = new Set(s);
    next.delete(paneId);
    return next;
  });

const handlers = new Map<string, (e: BrowserEvent) => void>();
let listening = false;
/** Routes `browser-event`s of one webview to `fn` (returns the unsubscribe). */
export function onBrowserEvent(label: string, fn: (e: BrowserEvent) => void) {
  if (!listening) {
    listening = true;
    void listen<BrowserEvent>("browser-event", (e) => {
      handlers.get(e.payload.label)?.(e.payload);
      // Observable for tests and anything else interested (the page itself is out of reach).
      window.dispatchEvent(new CustomEvent("kelpie:browser-event", { detail: e.payload }));
    });
  }
  handlers.set(label, fn);
  return () => {
    if (handlers.get(label) === fn) handlers.delete(label);
  };
}

/** A local dev server's address (shown in a browser pane rather than the system browser). */
export function isLocalUrl(uri: string): boolean {
  try {
    const u = new URL(uri);
    return (u.protocol === "http:" || u.protocol === "https:") && ["localhost", "127.0.0.1", "[::1]", "0.0.0.0"].includes(u.hostname);
  } catch {
    return false;
  }
}

/** The browser pane of a project that shows a local port, if any. */
export function browserPaneOnPort(projectId: string, port: number): { tabId: string; paneId: string } | null {
  for (const tab of store.workspace(projectId).tabs) {
    for (const pane of allPanes(tab.root)) {
      if (pane.kind === "browser" && pane.url && isLocalUrl(pane.url) && new URL(pane.url).port === String(port)) return { tabId: tab.id, paneId: pane.id };
    }
  }
  return null;
}

/** Opens a browser pane in the active project: a new tab, or split next to the focused pane. */
export function openBrowser(url = "", where: "tab" | "right" | "down" = "tab", projectId = store.state.activeProject) {
  if (!projectId) return;
  store.placePane(projectId, { id: uid(), kind: "browser", url }, where);
}

const fence = (s: string) => (s.includes("```") ? s.replace(/```/g, "ˋˋˋ") : s);

/** The prompt for one or more annotations: what was picked on which page, and the request. */
export function annotationPrompt(items: Annotation[]): string {
  const out: string[] = [items.length > 1 ? `I marked ${items.length} things in the browser:` : "I marked this in the browser:"];
  items.forEach((a, i) => {
    out.push("", items.length > 1 ? `## ${i + 1}. ${a.pick ? a.pick.element : "Page screenshot"}` : "");
    out.push(`Page: ${a.pageTitle ? `${a.pageTitle} — ` : ""}${a.pageUrl}`);
    if (a.comment.trim()) out.push(`Request: ${a.comment.trim()}`);
    if (a.pick) {
      const p = a.pick;
      out.push(`Element: \`${p.selector}\` (${p.role}${p.name ? `, "${p.name}"` : ""}), ${Math.round(p.rect.width)}×${Math.round(p.rect.height)} px at (${Math.round(p.rect.x)}, ${Math.round(p.rect.y)}) in a ${p.page.width}×${p.page.height} viewport`);
      if (p.text) out.push(`Text: ${p.text.length > 300 ? p.text.slice(0, 300) + "…" : p.text}`);
      if (p.ancestors.length) out.push(`Inside: ${p.ancestors.slice(0, 5).join(" < ")}`);
      const styles = Object.entries(p.styles).filter(([, v]) => v && v !== "none" && v !== "normal" && v !== "0px" && v !== "auto");
      if (styles.length) out.push(`Styles: ${styles.map(([k, v]) => `${k}: ${v}`).join("; ")}`);
      out.push("HTML:", "```html", fence(p.html), "```");
    }
    if (a.shot) out.push(`Screenshot: ${a.shot.path}`);
  });
  return out.filter((l, i) => l !== "" || i > 0).join("\n");
}
