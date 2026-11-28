import { describe, expect, it } from "vitest";
import { boostLeft, projectsByTier, projectTier, reorder, weightOf, type Priorities } from "./priorities";
import { waitingSessions } from "./attention";
import type { SessionInfo } from "./types";

const policy = (projects: Priorities["policy"]["projects"], effective: Priorities["effective"] = {}): Priorities => ({
  policy: { projects, sessions: {}, max_agents: 0, max_agents_per_project: null },
  effective,
  boosted: [],
  reserved: { cores: 0, ram_mb: 0, vram_mb: 0 },
  reservable: { cores: 3, ram_mb: 4850, vram_mb: 0 },
  max_boosts: 2,
});

describe("priorities", () => {
  it("groups projects by tier in their rank, unranked ones after in the sidebar's order", () => {
    const p = policy({ b: { tier: "critical", rank: 1 }, a: { tier: "critical", rank: 0 }, d: { tier: "background", rank: 0 } });
    const projects = [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }, { id: "e" }];
    const tiers = projectsByTier(p, projects);
    expect(tiers.critical.map((x) => x.id)).toEqual(["a", "b"]);
    expect(tiers.normal.map((x) => x.id)).toEqual(["c", "e"]);
    expect(tiers.background.map((x) => x.id)).toEqual(["d"]);
    expect(projectTier(null, "a")).toBe("normal");
    expect(projects[0]).toEqual({ id: "a" });
  });

  it("puts a dropped project before the one it was dropped on, or last", () => {
    expect(reorder(["a", "b", "c"], "c", "a")).toEqual(["c", "a", "b"]);
    expect(reorder(["a", "b", "c"], "a", null)).toEqual(["b", "c", "a"]);
    // A project from another tier joins this one.
    expect(reorder(["a", "b"], "x", "b")).toEqual(["a", "x", "b"]);
  });

  it("says what is left of a boost", () => {
    const now = 1_000_000;
    expect(boostLeft(now + 30_000, now)).toBe("<1 min");
    expect(boostLeft(now + 12 * 60_000, now)).toBe("12 min");
    expect(boostLeft(now + 65 * 60_000, now)).toBe("1 h 5 min");
    expect(boostLeft(now + 120 * 60_000, now)).toBe("2 h");
  });

  it("orders sessions waiting for the person by their priority, then by how long they waited", () => {
    const s = (id: string, waiting_since: number) => ({ id, status: "waiting", alive: true, waiting_since, created_at: 0 }) as unknown as SessionInfo;
    const sessions = [s("old-bg", 1), s("new-critical", 5), s("mid", 3)];
    const p = policy({}, {
      "old-bg": { tier: "background", weight: 1, boosted: false, boost_until: null, source: "project", reserve: null },
      "new-critical": { tier: "critical", weight: 4, boosted: false, boost_until: null, source: "project", reserve: null },
    });
    expect(waitingSessions(sessions).map((x) => x.id)).toEqual(["old-bg", "mid", "new-critical"]);
    expect(waitingSessions(sessions, (id) => weightOf(p, id)).map((x) => x.id)).toEqual(["new-critical", "old-bg", "mid"]);
  });
});
