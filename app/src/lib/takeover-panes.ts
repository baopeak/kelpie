// What a phone that takes over a browser pane is shown of this window, and who may take control: plain functions of the
// app's state, so that they are tested without a window (the side that talks to Rust is takeover.ts).
import { allPanes } from "./layout";
import { TAKEOVER_API } from "./attention";
import type { Project, Workspace } from "./types";

/** One browser pane, as the phone lists it. `pane` is its webview's label. */
export interface PaneInfo {
  pane: string;
  title: string;
  host: string;
  project: string;
  project_name: string;
}

/** Where a pane is in the layout. */
export interface PaneLocation {
  projectId: string;
  tabId: string;
  paneId: string;
}

export const browserLabelOf = (paneId: string) => `browser-${paneId}`;

/** The host of a page's address, for http(s) pages only (what the phone shows beside the title). */
export function hostOf(url: string): string {
  try {
    const u = new URL(url);
    return u.protocol === "http:" || u.protocol === "https:" ? u.host : "";
  } catch {
    return "";
  }
}

/** Every browser pane of every project that has a page open, in the projects' order. */
export function listPanes(projects: Project[], workspace: (id: string) => Workspace, pages: Record<string, { title: string; url: string } | undefined>): PaneInfo[] {
  const out: PaneInfo[] = [];
  for (const p of projects) {
    for (const tab of workspace(p.id).tabs) {
      for (const pane of allPanes(tab.root)) {
        if (pane.kind !== "browser") continue;
        const url = pages[pane.id]?.url || pane.url || "";
        const host = hostOf(url);
        if (!host) continue;
        out.push({ pane: browserLabelOf(pane.id), title: pages[pane.id]?.title || host, host, project: p.id, project_name: p.name });
      }
    }
  }
  return out;
}

/** The pane whose webview is `label`, if it is still in a layout. */
export function findPane(label: string, projects: Project[], workspace: (id: string) => Workspace): PaneLocation | null {
  for (const p of projects) {
    for (const tab of workspace(p.id).tabs) {
      const pane = allPanes(tab.root).find((x) => x.kind === "browser" && browserLabelOf(x.id) === label);
      if (pane) return { projectId: p.id, tabId: tab.id, paneId: pane.id };
    }
  }
  return null;
}

/** Whether Settings → Mobile offers "May take control" for a device: the native app, or a browser device the daemon says may have it (paired on a
 *  secure origin, its app lock set up and on), from a daemon that has takeover. One that has it already keeps the choice, to be lowered. */
export function offersControl(d: { kind: "app" | "pwa"; permission: string; may_control?: boolean }, api: number): boolean {
  return d.kind === "app" || d.permission === "control" || (api >= TAKEOVER_API && d.may_control === true);
}
