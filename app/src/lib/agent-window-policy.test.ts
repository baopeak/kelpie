import { describe, expect, it } from "vitest";
import { toWindowPolicy, windowPatch, windowToolsShown } from "./agent-window-policy";
import { catalogEntry, entryAvailable } from "./settings-catalog";
import { WINDOW_AGENT_API } from "./attention";

describe("window policy", () => {
  it("reads the two keys of a policy reply", () => {
    expect(toWindowPolicy({ window_agents: "on", window_visible: true, sub_sessions: "ask" })).toEqual({ on: true, visible: true });
    expect(toWindowPolicy({ window_agents: "off", window_visible: false })).toEqual({ on: false, visible: false });
    expect(toWindowPolicy({ window_agents: "on" })).toEqual({ on: true, visible: false });
  });

  it("is null for a reply without the keys or with an unknown mode", () => {
    expect(toWindowPolicy({ browser_agents: "read" })).toBeNull();
    expect(toWindowPolicy({ window_agents: "ask" })).toBeNull();
    expect(toWindowPolicy(null)).toBeNull();
    expect(toWindowPolicy("on")).toBeNull();
  });

  it("sends only what changed, and withdraws the visible desktop with the tools", () => {
    expect(windowPatch({ on: true })).toEqual({ window_agents: "on" });
    expect(windowPatch({ visible: true })).toEqual({ window_visible: true });
    expect(windowPatch({ on: false })).toEqual({ window_agents: "off", window_visible: false });
    expect(windowPatch({ on: false, visible: true })).toEqual({ window_agents: "off", window_visible: true });
  });

  it("is shown on Windows with a daemon that has the tools, nowhere else", () => {
    expect(windowToolsShown(WINDOW_AGENT_API, true)).toBe(true);
    expect(windowToolsShown(WINDOW_AGENT_API - 1, true)).toBe(false);
    expect(windowToolsShown(WINDOW_AGENT_API, false)).toBe(false);
  });

  it("keeps its two catalogue rows out of a search on other systems", () => {
    for (const id of ["window-agents", "window-agents-visible"]) {
      const e = catalogEntry(id)!;
      expect(entryAvailable(e, WINDOW_AGENT_API, true)).toBe(true);
      expect(entryAvailable(e, WINDOW_AGENT_API, false)).toBe(false);
      expect(entryAvailable(e, WINDOW_AGENT_API - 1, true)).toBe(false);
    }
    expect(entryAvailable(catalogEntry("browser-agents")!, WINDOW_AGENT_API, false)).toBe(true);
  });
});
