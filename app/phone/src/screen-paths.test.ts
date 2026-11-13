import { describe, expect, it } from "vitest";
import { chooseVia, isVia, needsProof, pathRows, readVia, relayName, startRefusal, viaKey, viaText, type PathInfo } from "./screen-paths";

const fill = (text: string, vars?: Record<string, string | number>) => (vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text);
const OURS = { ours: true, host: "relay.kelpie.example" };
const MINE = { ours: false, host: "relay.home.example" };
const ALL: PathInfo[] = [{ id: "auto", ok: true }, { id: "tailscale", ok: true }, { id: "relay", ok: true }];

describe("the rows of Connect through", () => {
  it("lists Automatic, Tailscale and the relay by name, Automatic recommended", () => {
    const rows = pathRows(ALL, OURS, { hosted: false }, fill);
    expect(rows.map((r) => [r.id, r.label, r.ok, r.why, r.recommended])).toEqual([
      ["auto", "Automatic", true, null, true],
      ["tailscale", "Tailscale", true, null, false],
      ["relay", "Kelpie's relay", true, null, false],
    ]);
  });

  it("names a relay that is not Kelpie's by its host, and says plainly when the computer did not say", () => {
    expect(relayName(OURS, fill)).toBe("Kelpie's relay");
    expect(relayName(MINE, fill)).toBe("Your relay (relay.home.example)");
    expect(relayName(null, fill)).toBe("The relay");
    expect(pathRows(ALL, MINE, { hosted: false }, fill)[2].label).toBe("Your relay (relay.home.example)");
  });

  it("gives the computer's reason in words for each way it cannot offer", () => {
    const rows = pathRows([
      { id: "auto", ok: false, why: "screen_remote_off" },
      { id: "tailscale", ok: false, why: "no_direct_link" },
      { id: "relay", ok: false, why: "relay_budget_spent" },
    ], OURS, { hosted: false }, fill);
    expect(rows.map((r) => r.ok)).toEqual([false, false, false]);
    expect(rows[0].why).toMatch(/See and control the screen without Tailscale/);
    expect(rows[1].why).toBe("Tailscale is off on this phone or not reachable now.");
    expect(rows[2].why).toMatch(/used up/);
    for (const why of ["method_off", "relay_off", "needs_face_id", "lock_off", "no_passkey", "locked", "not_granted", "not_on_relay"]) {
      expect(pathRows([{ id: "relay", ok: false, why }], OURS, { hosted: false }, fill)[0].why, why).toBeTruthy();
    }
    expect(pathRows([{ id: "relay", ok: false, why: "a_later_reason" }], OURS, { hosted: false }, fill)[0].why).toBe("Not available now.");
    expect(pathRows([{ id: "relay", ok: false }], OURS, { hosted: false }, fill)[0].why).toBe("Not available now.");
  });

  it("on the hosted app Tailscale is never available, with the phone's own reason", () => {
    const tailscale = pathRows(ALL, OURS, { hosted: true }, fill)[1];
    expect(tailscale).toMatchObject({ id: "tailscale", ok: false, why: "This app came from the relay, so it can reach the computer only through the relay." });
    expect(pathRows(ALL, OURS, { hosted: true }, fill).filter((r) => r.ok).map((r) => r.id)).toEqual(["auto", "relay"]);
  });

  it("leaves out a way this app does not know, and lists nothing for a computer that offers no choice", () => {
    expect(pathRows([...ALL, { id: "webrtc", ok: true } as unknown as PathInfo], OURS, { hosted: false }, fill)).toHaveLength(3);
    expect(pathRows(null, null, { hosted: false }, fill)).toEqual([]);
    expect(pathRows(undefined, null, { hosted: true }, fill)).toEqual([]);
  });
});

describe("the way preselected", () => {
  const rows = (hosted: boolean, paths = ALL) => pathRows(paths, OURS, { hosted }, fill);

  it("is the one chosen last when it can be used now, else Automatic", () => {
    expect(chooseVia(rows(false), "relay")).toBe("relay");
    expect(chooseVia(rows(false), "tailscale")).toBe("tailscale");
    expect(chooseVia(rows(false), null)).toBe("auto");
    expect(chooseVia(rows(true), "tailscale"), "Tailscale was chosen on another app of this phone, and cannot be here").toBe("auto");
  });

  it("falls to the first way that can be used when Automatic cannot, and to Automatic when nothing can", () => {
    const paths: PathInfo[] = [{ id: "auto", ok: false, why: "screen_remote_off" }, { id: "tailscale", ok: true }, { id: "relay", ok: false, why: "relay_off" }];
    expect(chooseVia(rows(false, paths), null)).toBe("tailscale");
    expect(chooseVia(rows(false, paths.map((p) => ({ ...p, ok: false }))), "relay")).toBe("auto");
    expect(chooseVia([], null)).toBe("auto");
  });
});

describe("whether the start needs Face ID", () => {
  it("does for the relay, and for Automatic only while this phone is on the relay", () => {
    expect(needsProof("relay", "direct")).toBe(true);
    expect(needsProof("relay", "relay")).toBe(true);
    expect(needsProof("auto", "relay")).toBe(true);
    expect(needsProof("auto", "direct")).toBe(false);
    expect(needsProof("tailscale", "direct")).toBe(false);
    expect(needsProof("tailscale", "relay")).toBe(false);
  });
});

describe("the kept choice", () => {
  it("is kept under the pairing's own key, and read only as a way this app knows", () => {
    expect(viaKey("dev-1")).toBe("kelpie.screen.via.dev-1");
    expect([readVia("auto"), readVia("tailscale"), readVia("relay")]).toEqual(["auto", "tailscale", "relay"]);
    expect([readVia("webrtc"), readVia(""), readVia(null)]).toEqual([null, null, null]);
    expect([isVia("relay"), isVia("p2p"), isVia(3)]).toEqual([true, false, false]);
  });
});

describe("what the live view and a refusal say", () => {
  it("names the way used, and nothing for one this app does not know", () => {
    expect(viaText("tailscale", fill)).toBe("Tailscale");
    expect(viaText("relay", fill)).toBe("the relay");
    expect([viaText("webrtc", fill), viaText(null, fill), viaText(undefined, fill)]).toEqual([null, null, null]);
  });

  it("says Face ID did not get there for needs_face_id, and keeps the computer's words for the rest", () => {
    expect(startRefusal("needs_face_id", "Needs Face ID.", fill)).toMatch(/Face ID did not reach your computer in time/);
    expect(startRefusal("relay_budget_spent", "The relay's data is used up.", fill)).toBe("The relay's data is used up.");
    expect(startRefusal("screen_remote_off", "Turn it on.", fill)).toBe("Turn it on.");
  });
});
