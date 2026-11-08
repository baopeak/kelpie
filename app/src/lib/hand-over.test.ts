import { describe, expect, it, vi } from "vitest";
import { createHandOver, heldBy, type ShutdownAnswer } from "./hand-over";

/** A hand-over with a manual clock: `tick()` runs the pending check. */
function rig(transfers: number[], answers: ShutdownAnswer[] = []) {
  let pending: (() => void) | null = null;
  const calls = { shutdown: [] as string[], held: [] as number[], idleAgain: 0, handedOver: 0 };
  const h = createHandOver({
    transfers: async () => transfers.shift() ?? 0,
    shutdown: async (when) => {
      calls.shutdown.push(when);
      return answers.shift() ?? true;
    },
    setHeld: (n) => calls.held.push(n),
    idleAgain: () => calls.idleAgain++,
    handedOver: () => calls.handedOver++,
    setTimer: (run) => {
      pending = run;
      return 1;
    },
    clearTimer: () => {
      pending = null;
    },
  });
  const tick = async () => {
    const run = pending;
    pending = null;
    run?.();
    // The check is async (transfers, then the shutdown call): a macrotask lets it finish.
    await new Promise((r) => setTimeout(r, 0));
  };
  const last = () => calls.held[calls.held.length - 1];
  return { h, calls, tick, last, armed: () => pending !== null };
}

describe("waiting for file transfers before a daemon hand-over", () => {
  it("switches once the transfers have finished, and waits again when one started meanwhile", async () => {
    const r = rig([2, 0, 0], [{ waiting_for_transfers: 1 }, true]);
    r.h.switchWhenFinished(3);
    await r.tick(); // 2 still copy
    expect(r.calls.shutdown).toEqual([]);
    await r.tick(); // none: asks, and is refused for a transfer that just started
    expect(r.calls.shutdown).toEqual(["no_transfers"]);
    expect(r.last()).toBe(1);
    expect(r.armed()).toBe(true);
    await r.tick(); // none again: handed over
    expect(r.calls.shutdown).toEqual(["no_transfers", "no_transfers"]);
    expect(r.calls.handedOver).toBe(1);
    expect(r.last()).toBe(0);
    expect(r.h.waiting()).toBe(false);
  });

  it("never acts on a daemon it was not started for", async () => {
    const r = rig([0]);
    r.h.switchWhenFinished(1);
    // The old daemon ended another way (the installer, a crash) and the app connected to a new one.
    r.h.reset();
    await r.tick();
    expect(r.calls.shutdown).toEqual([]);
    expect(r.last()).toBe(0);
    expect(r.h.waiting()).toBe(false);
  });

  it("drops a check already under way when the connection changes", async () => {
    let finish: (n: number) => void = () => {};
    const timers: Array<() => void> = [];
    const shutdown = vi.fn(async () => true as ShutdownAnswer);
    const h = createHandOver({
      transfers: () => new Promise<number>((r) => (finish = r)),
      shutdown,
      setHeld: () => {},
      idleAgain: () => {},
      handedOver: () => {},
      setTimer: (run) => timers.push(run),
      clearTimer: () => {},
    });
    h.switchWhenFinished(1);
    timers.shift()?.();
    h.reset();
    finish(0);
    await new Promise((r) => setTimeout(r, 0));
    expect(shutdown).not.toHaveBeenCalled();
  });

  it("switching anyway ends the wait and its indicator", () => {
    const r = rig([5]);
    r.h.switchWhenFinished(5);
    expect(r.h.waiting()).toBe(true);
    r.h.reset();
    expect(r.armed()).toBe(false);
    expect(r.last()).toBe(0);
  });

  it("an automatic hand-over checks the sessions again once the transfers are done", async () => {
    const r = rig([0]);
    r.h.wait(2);
    await r.tick();
    expect(r.calls.shutdown).toEqual([]);
    expect(r.calls.idleAgain).toBe(1);
    // Asked then, refused for a transfer that started meanwhile: it waits instead of giving up.
    const answer = await r.h.handOver("idle");
    expect(answer).toBe("done");
    const refused = rig([], [{ waiting_for_transfers: 2 }, false]);
    expect(await refused.h.handOver("idle")).toBe("waiting");
    expect(refused.armed()).toBe(true);
    expect(await refused.h.handOver("idle")).toBe("busy");
  });

  it("reads what a refusal names", () => {
    expect(heldBy({ waiting_for_transfers: 3 })).toBe(3);
    expect(heldBy(true)).toBe(0);
    expect(heldBy(false)).toBe(0);
    expect(heldBy(null)).toBe(0);
  });
});
