import { describe, expect, it } from "vitest";
import { deviceQueryGate } from "./device-query-gate";

function gateAt(start = 1000) {
  const clock = { t: start };
  return { clock, gate: deviceQueryGate(() => clock.t) };
}

describe("deviceQueryGate", () => {
  it("answers a block once when it arrives three times, 0.5 s apart", () => {
    const { clock, gate } = gateAt();
    expect([gate.primaryRepeats(), gate.secondaryRepeats()]).toEqual([false, false]);
    clock.t += 500;
    expect([gate.primaryRepeats(), gate.secondaryRepeats()]).toEqual([true, true]);
    clock.t += 500;
    expect([gate.primaryRepeats(), gate.secondaryRepeats()]).toEqual([true, true]);
  });

  it("still answers a program that asks right after ConPTY's own primary query", () => {
    const { clock, gate } = gateAt();
    expect(gate.primaryRepeats()).toBe(false);
    clock.t += 600;
    expect([gate.primaryRepeats(), gate.secondaryRepeats()]).toEqual([false, false]);
  });

  it("never holds back primary queries while no secondary one came", () => {
    const { clock, gate } = gateAt();
    for (let i = 0; i < 4; i++) {
      expect(gate.primaryRepeats()).toBe(false);
      clock.t += 10;
    }
  });

  it("answers again once the window has passed", () => {
    const { clock, gate } = gateAt();
    gate.secondaryRepeats();
    clock.t += 1500;
    expect([gate.primaryRepeats(), gate.secondaryRepeats()]).toEqual([false, false]);
  });

  it("does not stretch the window with each repeat, so asking stays answerable", () => {
    const { clock, gate } = gateAt();
    gate.secondaryRepeats();
    clock.t += 1400;
    expect(gate.secondaryRepeats()).toBe(true);
    clock.t += 100;
    expect(gate.secondaryRepeats()).toBe(false);
  });
});
