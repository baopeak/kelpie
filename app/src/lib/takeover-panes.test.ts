import { describe, expect, it } from "vitest";
import { findPane, hostOf, listPanes, offersControl } from "./takeover-panes";
import { TAKEOVER_API } from "./attention";
import type { LayoutNode, PaneKind, Project, Workspace } from "./types";

const leaf = (id: string, kind: PaneKind, url?: string): LayoutNode => ({ type: "leaf", id: `l-${id}`, pane: { id, kind, ...(url ? { url } : {}) } });
const projects = [{ id: "p1", name: "Shop" }, { id: "p2", name: "Docs" }] as Project[];
const workspaces: Record<string, Workspace> = {
  p1: { activeTab: "t1", rightPanel: null, tabs: [{ id: "t1", focus: "a", root: { type: "split", id: "s", dir: "row", sizes: [0.5, 0.5], children: [leaf("a", "terminal"), leaf("b", "browser", "https://accounts.example.com/login")] } as LayoutNode }] },
  p2: { activeTab: "t2", rightPanel: null, tabs: [{ id: "t2", focus: "c", root: leaf("c", "browser") }, { id: "t3", focus: "d", root: leaf("d", "browser", "file:///C:/secret.txt") }] },
};
const workspace = (id: string) => workspaces[id] ?? { tabs: [], activeTab: null, rightPanel: null };

describe("the panes a phone may take over", () => {
  it("lists the browser panes with a web page, with the page's title and host", () => {
    const panes = listPanes(projects, workspace, { b: { title: "Sign in", url: "https://accounts.example.com/login?next=/" }, c: { title: "Local", url: "http://localhost:5173/" } });
    expect(panes).toEqual([
      { pane: "browser-b", title: "Sign in", host: "accounts.example.com", project: "p1", project_name: "Shop" },
      { pane: "browser-c", title: "Local", host: "localhost:5173", project: "p2", project_name: "Docs" },
    ]);
  });

  it("falls back to the pane's saved address and to the host for a title, and leaves out what is not a web page", () => {
    const panes = listPanes(projects, workspace, {});
    expect(panes.map((p) => [p.pane, p.title])).toEqual([["browser-b", "accounts.example.com"]]);
    expect(hostOf("about:blank")).toBe("");
    expect(hostOf("not a url")).toBe("");
  });

  it("finds a pane by its webview's label, and nothing for one that is gone", () => {
    expect(findPane("browser-c", projects, workspace)).toEqual({ projectId: "p2", tabId: "t2", paneId: "c" });
    expect(findPane("browser-a", projects, workspace)).toBeNull();
    expect(findPane("browser-zz", projects, workspace)).toBeNull();
  });
});

describe("who Settings offers control to", () => {
  it("offers it to the native app and to a browser device the daemon says may have it", () => {
    expect(offersControl({ kind: "app", permission: "read" }, 0)).toBe(true);
    expect(offersControl({ kind: "pwa", permission: "write", may_control: true }, TAKEOVER_API)).toBe(true);
    expect(offersControl({ kind: "pwa", permission: "write", may_control: false }, TAKEOVER_API)).toBe(false);
    expect(offersControl({ kind: "pwa", permission: "write", may_control: true }, TAKEOVER_API - 1)).toBe(false);
    expect(offersControl({ kind: "pwa", permission: "write" }, TAKEOVER_API)).toBe(false);
  });

  it("no longer offers it to a device whose lock was turned off: its record went back to typing, and the daemon says it may not have control", () => {
    expect(offersControl({ kind: "pwa", permission: "write", may_control: false }, TAKEOVER_API)).toBe(false);
  });

  it("keeps the choice for a device that has control, so that it can be lowered", () => {
    expect(offersControl({ kind: "pwa", permission: "control", may_control: false }, TAKEOVER_API)).toBe(true);
  });
});
