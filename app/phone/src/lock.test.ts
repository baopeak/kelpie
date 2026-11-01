import { sha256 } from "@noble/hashes/sha2.js";
import { describe, expect, it, vi } from "vitest";
import { commandParts, escapeCommand, levelClass, levelText, tagText } from "./command-text";
import { fromB64, LockModel, recheckChallenge, toB64, type Asserted, type Enrolled, type LockEnv } from "./lock";
import type { Outcome } from "./link";

vi.mock("./i18n", () => ({ t: (s: string) => s }));

const UNLOCK = { state: "unlock", challenge: toB64(new Uint8Array(32).fill(7)), rp_id: "desk.example", cred_id: toB64(new Uint8Array([1, 2, 3])) };
const ENROL = { state: "enrol", challenge: toB64(new Uint8Array(32).fill(9)), rp_id: "desk.example", user: toB64(new Uint8Array([4])), name: "Phone" };

function rig(seen?: string) {
  const kept = new Map<string, string>(seen ? [["kelpie-lock-seen", seen]] : []);
  const asked: Record<string, unknown>[] = [];
  const state = { connected: true, now: 1000, refusal: null as Outcome | null, denied: null as string | null, spki: new Uint8Array(91) as Uint8Array | null };
  const lockNow = vi.fn();
  const touch = vi.fn();
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
      return state.refusal ?? { t: "ack" };
    },
    lockNow,
    touch,
    connected: () => state.connected,
    now: () => state.now,
    store: { get: (k) => kept.get(k) ?? null, set: (k, v) => void kept.set(k, v), del: (k) => void kept.delete(k) },
  };
  return { lock: new LockModel(env, changed), asked, state, lockNow, touch, create, get, kept };
}

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
    for (const [state, clear] of [["none", true], ["open", true], ["read_only", true], ["unlock", false], ["enrol", false], ["closed", false], ["no_name", false], ["broken", false]] as const) {
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
