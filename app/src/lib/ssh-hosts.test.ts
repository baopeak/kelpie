import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./store", () => ({ store: { state: { projects: [], settings: {} }, sessions: {}, notify: vi.fn() } }));
vi.mock("./terminal-host", () => ({ setSessionThemeResolver: vi.fn() }));

const { agentFolder } = await import("./ssh-hosts");
import type { Project } from "./types";
import type { SshHost } from "./ssh-hosts";

const host = { id: "h1", alias: "vps", label: "vps", folder: "/srv", tmux: true } as SshHost;
const project = (p: Partial<Project> = {}) => ({ id: "p", name: "app", path: "D:\\app", ...p }) as Project;

describe("agentFolder", () => {
  it("is the folder typed for the pane, else the one linked to the project", () => {
    expect(agentFolder(host, project(), " /srv/app ")).toBe("/srv/app");
    expect(agentFolder(host, project({ hosts: [{ hostId: "h1", path: "/srv/linked" }] }))).toBe("/srv/linked");
    expect(agentFolder(host, project({ hosts: [{ hostId: "h1", path: "/srv/linked" }] }), "/tmp/x")).toBe("/tmp/x");
  });
  it("never falls back to the host's start or login folder", () => {
    expect(agentFolder(host, project())).toBe("");
    expect(agentFolder(host, project({ hosts: [{ hostId: "h1", path: "" }] }))).toBe("");
    expect(agentFolder(host, project({ hosts: [{ hostId: "other", path: "/srv/x" }] }))).toBe("");
    expect(agentFolder(host, undefined)).toBe("");
  });
  it("is a remote project's own folder on its host", () => {
    const remote = project({ remote: { dest: "vps", path: "/srv/remote", tmux: true } } as Partial<Project>);
    expect(agentFolder(host, remote)).toBe("/srv/remote");
    expect(agentFolder({ ...host, alias: "elsewhere" }, remote)).toBe("");
  });
});
