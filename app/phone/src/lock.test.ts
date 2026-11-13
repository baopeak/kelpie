import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it, vi } from "vitest";
import { commandParts, escapeCommand, levelClass, levelText, tagText } from "./command-text";
import { fromB64, LockModel, recheckChallenge, screenChallenge, toB64, type Asserted, type Enrolled, type LockEnv } from "./lock";
import type { Outcome } from "./link";

vi.mock("./i18n", () => ({ t: (s: string) => s }));

const UNLOCK = { state: "unlock", challenge: toB64(new Uint8Array(32).fill(7)), rp_id: "desk.example", cred_id: toB64(new Uint8Array([1, 2, 3])) };
const ENROL = { state: "enrol", challenge: toB64(new Uint8Array(32).fill(9)), rp_id: "desk.example", user: toB64(new Uint8Array([4])), name: "Phone" };

function rig(seen?: string) {
  const kept = new Map<string, string>(seen ? [["kelpie-lock-seen", seen]] : []);
  const asked: Record<string, unknown>[] = [];
  const state = { grace: 0, connected: true, now: 1000, refusal: null as Outcome | null, denied: null as string | null, spki: new Uint8Array(91) as Uint8Array | null };
  const lockNow = vi.fn();
  const touch = vi.fn();
  const setResume = vi.fn();
  const changed = vi.fn();
  const denied = () => {
    if (state.denied) {
      const e = new Error("no");
      e.name = state.denied;
      throw e;
    }
  };
  const create = vi.fn(async (): Promise<Enrolled> => {
    denied();
    return { credentialId: new Uint8Array([1]), spki: state.spki, algorithm: -7, authData: new Uint8Array([2]), clientData: new Uint8Array([3]) };
  });
  const get = vi.fn(async (): Promise<Asserted> => {
    denied();
    return { authData: new Uint8Array([2]), clientData: new Uint8Array([3]), signature: new Uint8Array([4]) };
  });
  const env: LockEnv = {
    credentials: { available: () => true, create, get },
    async ask(m) {
      asked.push(m);
      if (m.type === "lock_challenge") return { t: "ack", lock: { ...UNLOCK, challenge: toB64(new Uint8Array(32).fill(8)) } };
      return state.refusal ?? { t: "ack", resume: "the-secret" };
    },
    lockNow,
    touch,
    grace: () => state.grace,
    setResume,
    connected: () => state.connected,
    now: () => state.now,
    store: { get: (k) => kept.get(k) ?? null, set: (k, v) => void kept.set(k, v), del: (k) => void kept.delete(k) },
  };
  return { lock: new LockModel(env, changed), asked, state, lockNow, touch, setResume, create, get, kept };
}

describe("the window's secret", () => {
  it("is handed on when a ceremony opens the phone, and never kept", async () => {
    for (const info of [UNLOCK, ENROL]) {
      const r = rig();
      r.lock.told(info);
      await r.lock.go();
      expect(r.setResume).toHaveBeenLastCalledWith("the-secret");
      expect([...r.kept.values()].join()).not.toContain("the-secret");
    }
  });

  it("goes when the person leaves, even if the computer never hears", () => {
    const r = rig();
    r.lock.told({ state: "open" });
    r.setResume.mockClear();
    r.lock.left();
    expect(r.setResume).toHaveBeenCalledWith(null);
  });

  it("goes when the computer says the phone is locked, stays while it is open, and goes with the pairing", () => {
    const r = rig();
    r.lock.told({ state: "open" });
    expect(r.setResume).not.toHaveBeenCalled();
    r.lock.told(UNLOCK);
    expect(r.setResume).toHaveBeenCalledWith(null);
    r.setResume.mockClear();
    r.lock.forget();
    expect(r.setResume).toHaveBeenCalledWith(null);
  });

  it("a refused ceremony gives none", async () => {
    const r = rig();
    r.lock.told(UNLOCK);
    r.state.refusal = { t: "error", code: "lock_refused", reason: null, message: "no" };
    r.setResume.mockClear();
    await r.lock.go();
    expect(r.setResume).not.toHaveBeenCalledWith("the-secret");
  });
});

describe("before the computer has spoken", () => {
  it("shows the lock screen, and nothing kept is painted", () => {
    const { lock } = rig();
    expect(lock.face).toBe("unknown");
    expect(lock.clear()).toBe(false);
  });

  it("a page the computer said has no lock paints what it kept, as before", () => {
    expect(rig("off").lock.clear()).toBe(true);
  });

  it("a page last seen locked or open starts behind the lock", () => {
    expect(rig("on").lock.clear()).toBe(false);
  });

  it("remembers what it was told, for the next start", () => {
    const a = rig();
    a.lock.told({ state: "none" });
    expect(a.kept.get("kelpie-lock-seen")).toBe("off");
    a.lock.told({ state: "open" });
    expect(a.kept.get("kelpie-lock-seen")).toBe("on");
    a.lock.told({ state: "read_only" });
    expect(a.kept.get("kelpie-lock-seen")).toBe("off");
    a.lock.told(UNLOCK);
    expect(a.kept.get("kelpie-lock-seen")).toBe("on");
    a.lock.forget();
    expect(a.kept.has("kelpie-lock-seen")).toBe(false);
    expect(a.lock.clear()).toBe(false);
  });
});

describe("what the computer says", () => {
  it("only none, open and read-only let anything through", () => {
    const { lock } = rig();
    for (const [state, clear] of [["none", true], ["open", true], ["read_only", true], ["unlock", false], ["enrol", false], ["desktop", false], ["closed", false], ["no_name", false], ["broken", false]] as const) {
      lock.told({ state });
      expect([state, lock.clear()]).toEqual([state, clear]);
    }
  });
});

describe("leaving the app", () => {
  it("covers the page at once and tells the computer", () => {
    const { lock, lockNow } = rig();
    lock.told({ state: "open" });
    lock.left();
    expect(lock.covered).toBe(true);
    expect(lock.clear()).toBe(false);
    expect(lockNow).toHaveBeenCalledTimes(1);
  });

  it("coming back opens nothing until the computer has answered", () => {
    const { lock } = rig();
    lock.told({ state: "open" });
    lock.left();
    lock.back();
    expect(lock.covered).toBe(false);
    expect(lock.face).toBe("unknown");
    expect(lock.clear()).toBe(false);
    lock.told(UNLOCK);
    expect(lock.face).toBe("unlock");
  });

  it("the computer that never heard, because the network was gone, hears on the reconnect: the person did leave", () => {
    const { lock, lockNow } = rig();
    lock.told({ state: "open" });
    lock.left();
    lock.back();
    lockNow.mockClear();
    lock.told({ state: "open" });
    expect(lockNow).toHaveBeenCalledTimes(1);
    expect(lock.clear()).toBe(false);
    lock.told(UNLOCK);
    expect(lock.face).toBe("unlock");
  });

  it("a network flap in the foreground asks nothing", () => {
    const { lock, lockNow } = rig();
    lock.told({ state: "open" });
    lock.told({ state: "open" });
    expect(lockNow).not.toHaveBeenCalled();
    expect(lock.clear()).toBe(true);
  });

  it("a ceremony's own system sheet is not leaving", async () => {
    const { lock, lockNow, get } = rig();
    lock.told(UNLOCK);
    let finish = () => {};
    get.mockImplementationOnce(() => new Promise<Asserted>((resolve) => (finish = () => resolve({ authData: new Uint8Array(1), clientData: new Uint8Array(1), signature: new Uint8Array(1) }))));
    const going = lock.go();
    lock.left();
    expect(lock.covered).toBe(false);
    lock.back();
    finish();
    await going;
    expect(lockNow).not.toHaveBeenCalled();
  });

  it("a page with no lock is left alone", () => {
    const { lock, lockNow } = rig();
    lock.told({ state: "none" });
    lock.left();
    lock.told({ state: "read_only" });
    lock.left();
    expect(lockNow).not.toHaveBeenCalled();
    expect(lock.clear()).toBe(true);
  });
});

describe("the grace after leaving", () => {
  const GRACE = 300_000;
  function open() {
    const r = rig();
    r.state.grace = GRACE;
    r.lock.told({ state: "open", idle_s: 900 });
    return r;
  }

  it("coming back within it shows everything again, with no lock and the secret kept", () => {
    const { lock, lockNow, setResume, state } = open();
    lock.left();
    expect(lock.covered).toBe(true);
    expect(lock.clear()).toBe(false);
    state.now += GRACE - 1;
    lock.back();
    expect(lock.covered).toBe(false);
    expect(lock.face).toBe("open");
    expect(lock.clear()).toBe(true);
    expect(lockNow).not.toHaveBeenCalled();
    expect(setResume).not.toHaveBeenCalled();
  });

  it("coming back after it locks as leaving did before", () => {
    const { lock, lockNow, setResume, state } = open();
    lock.left();
    state.now += GRACE;
    lock.back();
    expect(lockNow).toHaveBeenCalledTimes(1);
    expect(setResume).toHaveBeenCalledWith(null);
    expect(lock.face).toBe("unknown");
    expect(lock.clear()).toBe(false);
    lock.told(UNLOCK);
    expect(lock.face).toBe("unlock");
  });

  it("is never longer than the computer's idle time", () => {
    const r = rig();
    r.state.grace = GRACE;
    r.lock.told({ state: "open", idle_s: 60 });
    r.lock.left();
    r.state.now += 60_000;
    r.lock.back();
    expect(r.lockNow).toHaveBeenCalledTimes(1);
    expect(r.lock.face).toBe("unknown");
  });

  it("counts the computer's idle time from the last touch it heard of", () => {
    const r = rig();
    r.state.grace = GRACE;
    r.lock.told({ state: "open", idle_s: 120 });
    r.state.now += 100_000;
    r.lock.touched();
    expect(r.touch).toHaveBeenCalledTimes(1);
    r.lock.left();
    r.state.now += 119_000;
    r.lock.back();
    expect(r.lockNow).not.toHaveBeenCalled();
    expect(r.lock.clear()).toBe(true);
  });

  it("Immediately is leaving as it always was", () => {
    const { lock, lockNow, state } = open();
    state.grace = 0;
    lock.left();
    expect(lockNow).toHaveBeenCalledTimes(1);
    lock.back();
    expect(lock.face).toBe("unknown");
  });

  it("tells the computer once, on leaving, of what the person did that it has not heard about", () => {
    const { lock, touch, state } = open();
    lock.touched();
    expect(touch).toHaveBeenCalledTimes(1);
    state.now += 5_000;
    lock.touched();
    expect(touch).toHaveBeenCalledTimes(1);
    lock.left();
    expect(touch).toHaveBeenCalledTimes(2);
    lock.back();
    lock.left();
    expect(touch).toHaveBeenCalledTimes(2);
  });

  it("sends no touch on leaving when nothing happened since the last one", () => {
    const { lock, touch } = open();
    lock.left();
    expect(touch).not.toHaveBeenCalled();
  });

  it("a lock that came while away is on the screen when the person is back", () => {
    const { lock, setResume } = open();
    lock.left();
    lock.told(UNLOCK);
    expect(setResume).toHaveBeenCalledWith(null);
    lock.back();
    expect(lock.face).toBe("unlock");
    expect(lock.clear()).toBe(false);
  });

  it("a ceremony's own system sheet is still not leaving", async () => {
    const { lock, get, lockNow } = rig();
    lock.told(UNLOCK);
    let finish = () => {};
    get.mockImplementationOnce(() => new Promise<Asserted>((resolve) => (finish = () => resolve({ authData: new Uint8Array(1), clientData: new Uint8Array(1), signature: new Uint8Array(1) }))));
    const going = lock.go();
    lock.left();
    finish();
    await going;
    lock.told({ state: "open" });
    expect(lock.fresh()).toBe(true);
    expect(lockNow).not.toHaveBeenCalled();
  });

  it("a reconnect while out of sight keeps the cover up until the page is shown", () => {
    const { lock } = open();
    lock.left();
    lock.told({ state: "open", idle_s: 900 });
    expect(lock.covered).toBe(true);
    expect(lock.clear()).toBe(false);
    lock.back();
    expect(lock.clear()).toBe(true);
  });

  it("what needs a fresh unlock waits for the passkey, then runs once", async () => {
    const { lock, lockNow, state } = rig();
    state.grace = GRACE;
    lock.told(UNLOCK);
    await lock.go();
    lock.told({ state: "open" });
    const go = vi.fn();
    lock.whenFresh(go);
    await Promise.resolve();
    expect(go, "fresh from the passkey: at once").toHaveBeenCalledTimes(1);
    lock.left();
    lock.back();
    lock.whenFresh(go);
    expect(lockNow).toHaveBeenCalledTimes(1);
    expect(lock.face).toBe("unknown");
    await Promise.resolve();
    expect(go).toHaveBeenCalledTimes(1);
    lock.told({ state: "open" });
    await Promise.resolve();
    expect(go, "a reconnect that says open is not the passkey").toHaveBeenCalledTimes(1);
    lock.told(UNLOCK);
    await lock.go();
    lock.told({ state: "open" });
    await Promise.resolve();
    expect(go).toHaveBeenCalledTimes(2);
    lock.told({ state: "open" });
    await Promise.resolve();
    expect(go, "once only").toHaveBeenCalledTimes(2);
  });

  it("what waits on a fresh unlock is dropped by leaving, forgetting the pairing or the lock going away", async () => {
    for (const end of ["left", "forget", "none"] as const) {
      const { lock, state } = rig();
      state.grace = GRACE;
      lock.told(UNLOCK);
      await lock.go();
      lock.told({ state: "open" });
      lock.left();
      lock.back();
      const go = vi.fn();
      lock.whenFresh(go);
      if (end === "left") lock.left();
      if (end === "forget") lock.forget();
      if (end === "none") lock.told({ state: "none" });
      lock.back();
      lock.told(UNLOCK);
      await lock.go();
      lock.told({ state: "open" });
      await Promise.resolve();
      expect(go, end).not.toHaveBeenCalled();
    }
  });

  it("with no passkey lock here, what asks for a fresh unlock runs at once", () => {
    const { lock } = rig();
    lock.told({ state: "none" });
    const go = vi.fn();
    lock.whenFresh(go);
    expect(go).toHaveBeenCalledTimes(1);
  });

  it("seeing the computer after any time away asks for the passkey again, even within the grace", async () => {
    const { lock, lockNow, setResume, state } = rig();
    state.grace = GRACE;
    lock.told(UNLOCK);
    await lock.go();
    lock.told({ state: "open" });
    expect(lock.fresh()).toBe(true);
    lock.left();
    lock.back();
    expect(lock.clear()).toBe(true);
    expect(lock.fresh()).toBe(false);
    lock.relock();
    expect(lockNow).toHaveBeenCalledTimes(1);
    expect(setResume).toHaveBeenLastCalledWith(null);
    expect(lock.face).toBe("unknown");
    lock.told(UNLOCK);
    await lock.go();
    lock.told({ state: "open" });
    expect(lock.fresh()).toBe(true);
  });
});

describe("the ceremonies", () => {
  it("unlocks with what the browser gives, for the challenge it was told", async () => {
    const { lock, get, asked } = rig();
    lock.told(UNLOCK);
    await lock.go();
    const options = (get.mock.calls as unknown as [{ rpId: string; challenge: Uint8Array; credentialId: Uint8Array }][])[0][0];
    expect([options.rpId, Array.from(options.challenge.slice(0, 2)), Array.from(options.credentialId)]).toEqual(["desk.example", [7, 7], [1, 2, 3]]);
    expect(asked).toEqual([{ type: "lock_unlock", auth_data: toB64(new Uint8Array([2])), client_data: toB64(new Uint8Array([3])), signature: toB64(new Uint8Array([4])) }]);
    expect(lock.asking).toBe(false);
  });

  it("only the computer's word opens it: a passed ceremony alone does not", async () => {
    const { lock } = rig();
    lock.told(UNLOCK);
    await lock.go();
    expect(lock.clear()).toBe(false);
    lock.told({ state: "open" });
    expect(lock.clear()).toBe(true);
  });

  it("sets up a passkey with its public key", async () => {
    const { lock, create, asked } = rig();
    lock.told(ENROL);
    await lock.go();
    expect(create).toHaveBeenCalledTimes(1);
    expect(asked[0]).toMatchObject({ type: "lock_enrol", cred_id: toB64(new Uint8Array([1])), spki: toB64(new Uint8Array(91)) });
  });

  it("a browser that gives no public key is told plainly, and nothing is sent", async () => {
    const { lock, asked, state } = rig();
    state.spki = null;
    lock.told(ENROL);
    await lock.go();
    expect(asked).toEqual([]);
    expect(lock.error).toBeTruthy();
  });

  it("deny or cancel leaves it locked with the button still there", async () => {
    const { lock, state, asked } = rig();
    lock.told(UNLOCK);
    state.denied = "NotAllowedError";
    await lock.go();
    expect([lock.face, lock.clear(), lock.asking, asked.length]).toEqual(["unlock", false, false, 0]);
    expect(lock.error).toBeTruthy();
    state.denied = null;
    await lock.go();
    expect(asked[0].type).toBe("lock_unlock");
  });

  it("a refusal says why, and the next try has a new challenge: a used one is no good", async () => {
    const { lock, state, asked, get } = rig();
    lock.told(UNLOCK);
    state.refusal = { t: "error", code: "lock_refused", message: "The computer did not accept that." };
    await lock.go();
    expect(lock.error).toBe("The computer did not accept that.");
    expect(asked.map((m) => m.type)).toEqual(["lock_unlock", "lock_challenge"]);
    await lock.go();
    const second = (get.mock.calls as unknown as [{ challenge: Uint8Array }][])[1][0];
    expect(second.challenge[0]).toBe(8);
    expect(lock.face).toBe("unlock");
  });

  it("does nothing on a state with nothing to verify", async () => {
    const { lock, get, create } = rig();
    for (const state of ["closed", "broken", "no_name", "open", "none"]) {
      lock.told({ state });
      await lock.go();
    }
    expect(get).not.toHaveBeenCalled();
    expect(create).not.toHaveBeenCalled();
  });

  it("keeps the challenge the button will use fresh, without asking on the tap", async () => {
    const { lock, asked, state } = rig();
    lock.told(UNLOCK);
    lock.tick();
    expect(asked).toEqual([]);
    state.now += 46_000;
    lock.tick();
    await Promise.resolve();
    expect(asked).toEqual([{ type: "lock_challenge" }]);
    state.connected = false;
    state.now += 46_000;
    lock.tick();
    expect(asked.length).toBe(1);
  });
});

describe("the idle time", () => {
  it("is told of the person at most every 30 s, and only while open", () => {
    const { lock, touch, state } = rig();
    lock.told(UNLOCK);
    lock.touched();
    expect(touch).not.toHaveBeenCalled();
    lock.told({ state: "open" });
    lock.touched();
    lock.touched();
    expect(touch).toHaveBeenCalledTimes(1);
    state.now += 31_000;
    lock.touched();
    expect(touch).toHaveBeenCalledTimes(2);
  });
});

describe("a dangerous approval", () => {
  it("the challenge is what the computer works out: over the request, the command and the time", () => {
    const enc = new TextEncoder();
    const parts = [...enc.encode("r1"), 0, ...sha256(enc.encode("rm -rf /")), ...enc.encode("1000")];
    expect(recheckChallenge("r1", "rm -rf /", 1000)).toEqual(sha256(new Uint8Array(parts)));
    expect(recheckChallenge("r2", "rm -rf /", 1000)).not.toEqual(recheckChallenge("r1", "rm -rf /", 1000));
    expect(recheckChallenge("r1", "rm -rf /tmp", 1000)).not.toEqual(recheckChallenge("r1", "rm -rf /", 1000));
  });

  it("goes by the computer's clock, and a refusal gives nothing", async () => {
    const { lock, state, get } = rig();
    lock.told({ state: "open", rp_id: "desk.example", cred_id: toB64(new Uint8Array([1])) });
    lock.sawServerClock(state.now + 5000);
    const proof = await lock.recheck("r1", "ls");
    expect(proof?.expiry).toBe(state.now + 5000 + 60_000);
    const options = (get.mock.calls as unknown as [{ challenge: Uint8Array }][])[0][0];
    expect(options.challenge).toEqual(recheckChallenge("r1", "ls", proof!.expiry));
    state.denied = "NotAllowedError";
    expect(await lock.recheck("r1", "ls")).toBeNull();
    expect(lock.error).toBeTruthy();
    expect(lock.asking).toBe(false);
  });
});

describe("starting the screen through the relay", () => {
  it("the challenge is a dangerous approval's, over this screen and the relay, until the time", () => {
    expect(screenChallenge("w:1:2", 1000)).toEqual(recheckChallenge("screen:w:1:2", "relay", 1000));
    expect(screenChallenge("w:1:3", 1000), "another screen").not.toEqual(screenChallenge("w:1:2", 1000));
    expect(screenChallenge("w:1:2", 1001), "another time").not.toEqual(screenChallenge("w:1:2", 1000));
    expect(screenChallenge("w:1:2", 1000), "not good for a dangerous approval").not.toEqual(recheckChallenge("w:1:2", "relay", 1000));
  });

  it("asks Face ID over it, by the computer's clock, within the minute the computer takes, and gives nothing when the person refuses", async () => {
    const { lock, state, get } = rig();
    lock.told({ state: "open", rp_id: "relay.example", cred_id: toB64(new Uint8Array([1])) });
    lock.sawServerClock(state.now + 5000);
    const proof = await lock.screenProof("m:DISPLAY1");
    expect(proof?.expiry).toBe(state.now + 5000 + 45_000);
    expect(Object.keys(proof!).sort()).toEqual(["auth_data", "client_data", "expiry", "signature"]);
    const options = (get.mock.calls as unknown as [{ challenge: Uint8Array; rpId: string }][])[0][0];
    expect(options.rpId).toBe("relay.example");
    expect(options.challenge).toEqual(screenChallenge("m:DISPLAY1", proof!.expiry));
    state.denied = "NotAllowedError";
    expect(await lock.screenProof("m:DISPLAY1")).toBeNull();
    expect(lock.error).toBeTruthy();
    expect(lock.asking).toBe(false);
  });

  it("cannot ask before the computer has said which passkey", async () => {
    const { lock, get } = rig();
    expect(await lock.screenProof("m:DISPLAY1")).toBeNull();
    expect(get).not.toHaveBeenCalled();
    expect(lock.error).toBeTruthy();
  });
});

describe("base64url", () => {
  it("round-trips, unpadded", () => {
    const all = Uint8Array.from({ length: 256 }, (_, i) => i);
    expect(toB64(all)).not.toMatch(/[+/=]/);
    expect(fromB64(toB64(all))).toEqual(all);
  });
});

describe("what a card shows", () => {
  it("writes out what could hide something, and leaves the rest as it is", () => {
    expect(escapeCommand("ls -la")).toBe("ls -la");
    expect(escapeCommand("rm -rf /\nls\tx")).toBe("rm -rf /\nls\tx");
    for (const [hostile, shown] of [["‮", "\\u{202E}"], ["​", "\\u{200B}"], ["⁦", "\\u{2066}"], ["؜", "\\u{061C}"], ["﻿", "\\u{FEFF}"], ["\u001B", "\\u{001B}"], ["\u0007", "\\u{0007}"], ["\u{E0041}", "\\u{E0041}"], ["­", "\\u{00AD}"], ["ㅤ", "\\u{3164}"], ["\u007F", "\\u{007F}"], ["\r", "\\u{000D}"]]) {
      expect(escapeCommand(`a${hostile}b`)).toBe(`a${shown}b`);
    }
  });

  it("marks each piece, so that the page can set it apart, and leaves nothing out", () => {
    const parts = commandParts("sa‮ge​cmd");
    expect(parts).toEqual([{ text: "sa", escaped: false }, { text: "\\u{202E}", escaped: true }, { text: "ge", escaped: false }, { text: "\\u{200B}", escaped: true }, { text: "cmd", escaped: false }]);
    const long = "x".repeat(100_000);
    expect(commandParts(long)).toEqual([{ text: long, escaped: false }]);
    expect(commandParts("")).toEqual([]);
  });

  it("gives the same answer however often it is asked", () => {
    expect(escapeCommand("a‮b")).toBe(escapeCommand("a‮b"));
  });

  it("has a word for each level, and what it does not know is not low", () => {
    expect(new Set(["low", "medium", "dangerous", "unknown"].map((level) => levelText({ level: level as "low", tags: [] }))).size).toBe(4);
    expect(levelText(null)).toBe(levelText({ level: "unknown", tags: [] }));
    expect(levelClass({ level: "<img src=x>" as "low", tags: [] })).toBe("unknown");
    expect(levelClass(undefined)).toBe("unknown");
    expect(levelClass({ level: "dangerous", tags: [] })).toBe("dangerous");
    expect(tagText("delete")).not.toBe("delete");
    expect(tagText("some_new_tag")).toBe("some_new_tag");
  });
});
