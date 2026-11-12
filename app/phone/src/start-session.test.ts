import { describe, expect, it, vi } from "vitest";
import type { Outcome } from "./link";
import { newSessionRequest, notReadyText, optionsOutcome, projectChoices, startOutcome } from "./start-session";

vi.mock("./i18n", () => ({ t: (s: string) => s }));

describe("starting a session from the phone", () => {
  it("asks for the project, the agent and a trimmed first prompt, leaving a blank one out, under one cid", () => {
    expect(newSessionRequest("p1", "claude", "  fix the build \n", "c1")).toEqual({ project_id: "p1", kind: "claude", prompt: "fix the build", cid: "c1" });
    expect(newSessionRequest("p1", "codex", "   ", "c2")).toEqual({ project_id: "p1", kind: "codex", cid: "c2" });
  });

  it("opens the session the computer started, also when the start was asked for before", () => {
    expect(startOutcome({ t: "ack", result: "ok", id: "s1" })).toEqual({ ok: true, id: "s1" });
    expect(startOutcome({ t: "ack", result: "ok", id: "s1", dup: true })).toEqual({ ok: true, id: "s1" });
  });

  it("says each refusal plainly, and the computer's own words for one it does not know", () => {
    const refused = (code: string, message = "English from the computer") => startOutcome({ t: "error", code, message } as Outcome);
    const said = (code: string) => (refused(code) as { message: string }).message;
    expect(said("busy")).toBe("Your computer is too busy to start a session now. Try again in a minute.");
    expect(said("not_installed")).toBe("That agent is not installed on your computer.");
    expect(said("not_ready")).toBe("Open Kelpie on your computer once so this phone can start sessions.");
    expect(said("rate_limited")).toBe("Starting sessions too fast: wait a few seconds and try again.");
    expect(said("no_project")).toContain("runs over SSH");
    expect(said("no_agent")).toContain("chat-only");
    expect(said("no_folder")).toBe("That project's folder is not on your computer any more.");
    expect(said("read_only")).toContain("cannot start sessions");
    expect(said("not_started")).toBe("English from the computer");
    for (const code of ["busy", "not_installed", "not_ready", "rate_limited", "no_project"]) expect(refused(code).ok).toBe(false);
  });

  it("says nothing started when it cannot know, and never keeps a start to send later", () => {
    expect(startOutcome({ t: "offline" })).toEqual({ ok: false, message: "You are offline. Start it when the connection is back." });
    expect((startOutcome({ t: "unknown" }) as { message: string }).message).toContain("it may have started");
    expect((startOutcome({ t: "ack" }) as { ok: boolean }).ok, "an answer without a session").toBe(false);
  });
});

describe("what the phone may start", () => {
  it("reads the projects and agents, or says to open Kelpie on the computer while it has none", () => {
    const projects = [{ id: "p1", name: "Shop" }];
    const agents = [{ kind: "claude", label: "Claude Code", skips: true }];
    expect(optionsOutcome({ t: "ack", ready: true, projects, agents })).toEqual({ ok: true, options: { ready: true, projects, agents } });
    expect(optionsOutcome({ t: "ack", ready: false, projects: [], agents: [], why: "Open Kelpie on your computer once so this phone can start sessions." })).toEqual({ ok: false, message: notReadyText() });
    expect(optionsOutcome({ t: "error", code: "read_only", message: "x" } as Outcome).ok).toBe(false);
    expect(optionsOutcome({ t: "offline" }).ok).toBe(false);
  });

  it("offers the projects with sessions first, the most recently active first, then the rest in the computer's order", () => {
    const projects = [{ id: "a", name: "A" }, { id: "b", name: "B" }, { id: "c", name: "C" }, { id: "d", name: "D" }];
    const sessions = [{ project_id: "c", last_activity: 100 }, { project_id: "b", last_activity: 300 }, { project_id: "c", last_activity: 50 }, { project_id: null, last_activity: 900 }, { project_id: "gone", last_activity: 999 }];
    expect(projectChoices(projects, sessions).map((p) => p.id)).toEqual(["b", "c", "a", "d"]);
    expect(projectChoices(projects, []).map((p) => p.id)).toEqual(["a", "b", "c", "d"]);
  });
});
