import { describe, expect, it } from "vitest";
import { closeAction, createCloseFlow, runningWork, workLines, type CloseDeps, type CloseSettings, type RunningWork } from "./close-daemon";

const nothing: RunningWork = { sessions: [], transfers: 0, runTasks: 0 };
const busy: RunningWork = { sessions: ["Claude · fix tests", "pwsh"], transfers: 1, runTasks: 0 };

function harness(settings: CloseSettings | null, work: RunningWork | null, answer: { dontAsk: boolean } | null = { dontAsk: false }) {
  const calls: string[] = [];
  const deps: CloseDeps = {
    settings: () => settings,
    work: async () => {
      calls.push("work");
      return work;
    },
    ask: async (w) => {
      calls.push(`ask:${w.sessions.length}/${w.transfers}`);
      return answer;
    },
    dontAskAgain: () => calls.push("dont-ask"),
    save: async () => void calls.push("save"),
    quit: async (stop) => void calls.push(stop ? "quit:stop" : "quit:keep"),
  };
  return { flow: createCloseFlow(deps), calls };
}

describe("closeAction", () => {
  it("stops by default, a missing setting included", () => {
    expect(closeAction({}, nothing)).toBe("stop");
    expect(closeAction({ daemonOnClose: "stop", askBeforeStop: true }, nothing)).toBe("stop");
  });

  it("keeps the daemon when the setting says so, whatever runs", () => {
    expect(closeAction({ daemonOnClose: "keep" }, busy)).toBe("keep");
    expect(closeAction({ daemonOnClose: "keep", askBeforeStop: true }, null)).toBe("keep");
  });

  it("asks only when something runs and asking is on", () => {
    expect(closeAction({}, busy)).toBe("ask");
    expect(closeAction({ askBeforeStop: false }, busy)).toBe("stop");
    expect(closeAction({}, { ...nothing, transfers: 2 })).toBe("ask");
    expect(closeAction({}, { ...nothing, runTasks: 1 })).toBe("ask");
  });

  it("stops without asking when the daemon cannot say what runs", () => {
    expect(closeAction({}, null)).toBe("stop");
  });
});

describe("runningWork", () => {
  it("counts live sessions, transfers and the busy tasks of running runs", () => {
    const w = runningWork(
      [{ alive: true, label: "a" }, { alive: false, label: "b" }, { alive: true, label: "c" }],
      3,
      [
        { status: "running", tasks: [{ status: "running" }, { status: "starting" }, { status: "pending" }, { status: "done" }] },
        { status: "done", tasks: [{ status: "running" }] },
      ],
    );
    expect(w).toEqual({ sessions: ["a", "c"], transfers: 3, runTasks: 2 });
  });

  it("names what stops, in the singular and the plural", () => {
    expect(workLines(nothing)).toEqual([]);
    expect(workLines({ sessions: ["a"], transfers: 1, runTasks: 1 })).toEqual(["1 session is running", "1 run task is working", "1 file transfer is copying"]);
    expect(workLines({ sessions: ["a", "b"], transfers: 4, runTasks: 2 })).toEqual(["2 sessions are running", "2 run tasks are working", "4 file transfers are copying"]);
  });
});

describe("closing Kelpie", () => {
  it("keep: exits at once and leaves the daemon, without looking at what runs", async () => {
    const h = harness({ daemonOnClose: "keep" }, busy);
    expect(await h.flow()).toBe("kept");
    expect(h.calls).toEqual(["save", "quit:keep"]);
  });

  it("an app state that never loaded keeps the daemon: the choice is unknown", async () => {
    const h = harness(null, busy);
    expect(await h.flow()).toBe("kept");
    expect(h.calls).toEqual(["save", "quit:keep"]);
  });

  it("stop with nothing running: exits and stops the daemon without asking", async () => {
    const h = harness({ daemonOnClose: "stop", askBeforeStop: true }, nothing);
    expect(await h.flow()).toBe("stopped");
    expect(h.calls).toEqual(["work", "save", "quit:stop"]);
  });

  it("stop with work running: asks, then stops", async () => {
    const h = harness({}, busy);
    expect(await h.flow()).toBe("stopped");
    expect(h.calls).toEqual(["work", "ask:2/1", "save", "quit:stop"]);
  });

  it("asking suppressed: stops without the question", async () => {
    const h = harness({ askBeforeStop: false }, busy);
    expect(await h.flow()).toBe("stopped");
    expect(h.calls).toEqual(["work", "save", "quit:stop"]);
  });

  it("cancel: nothing exits and nothing stops", async () => {
    const h = harness({}, busy, null);
    expect(await h.flow()).toBe("cancelled");
    expect(h.calls).toEqual(["work", "ask:2/1"]);
  });

  it("Don't ask again is remembered before the state is saved and the app exits", async () => {
    const h = harness({}, busy, { dontAsk: true });
    expect(await h.flow()).toBe("stopped");
    expect(h.calls).toEqual(["work", "ask:2/1", "dont-ask", "save", "quit:stop"]);
  });

  it("a second close while the question is up changes nothing, and a later one asks again", async () => {
    let answer!: (a: { dontAsk: boolean } | null) => void;
    const calls: string[] = [];
    const flow = createCloseFlow({
      settings: () => ({}),
      work: async () => busy,
      ask: () => {
        calls.push("ask");
        return new Promise((r) => (answer = r));
      },
      dontAskAgain: () => {},
      save: async () => {},
      quit: async () => void calls.push("quit"),
    });
    const first = flow();
    await new Promise((r) => setTimeout(r, 0));
    expect(await flow()).toBe("busy");
    answer(null);
    expect(await first).toBe("cancelled");
    const again = flow();
    await new Promise((r) => setTimeout(r, 0));
    answer({ dontAsk: false });
    expect(await again).toBe("stopped");
    expect(calls).toEqual(["ask", "ask", "quit"]);
  });

  it("a save that fails does not keep the app open", async () => {
    const calls: string[] = [];
    const flow = createCloseFlow({
      settings: () => ({ askBeforeStop: false }),
      work: async () => busy,
      ask: async () => null,
      dontAskAgain: () => {},
      save: () => Promise.reject(new Error("locked")),
      quit: async (stop) => void calls.push(`quit:${stop}`),
    });
    expect(await flow()).toBe("stopped");
    expect(calls).toEqual(["quit:true"]);
  });
});
