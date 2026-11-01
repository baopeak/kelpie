import { describe, expect, it, vi } from "vitest";
import { Alerts, alertsUnavailable, readAlertLink, type AlertsEnv, type AlertsFacts, type HeldSubscription } from "./push";
import type { Outcome } from "./link";

const KEY = "the-computers-key";

function sub(key: string | null = KEY, endpoint = "https://fcm.googleapis.com/fcm/send/a") {
  const s: HeldSubscription & { ended: boolean } = {
    endpoint, p256dh: "p", auth: "s", key, ended: false,
    unsubscribe: async () => {
      s.ended = true;
    },
  };
  return s;
}

/** A browser and a computer, as much as the state machine sees of them. */
function rig(over: { facts?: Partial<AlertsFacts>; held?: ReturnType<typeof sub> | null; wanted?: boolean; marked?: boolean; answers?: Record<string, Outcome>; subscribeThrows?: boolean; permissionAnswer?: AlertsFacts["permission"] } = {}) {
  const asked: Record<string, unknown>[] = [];
  const state = { held: over.held ?? null, wanted: over.wanted ?? false, marked: over.marked ?? false, permission: over.facts?.permission ?? ("default" as AlertsFacts["permission"]), subscribed: 0, askedPermission: 0, changes: 0 };
  const env: AlertsEnv = {
    facts: () => ({ secure: true, supported: true, ios: false, installed: false, ...over.facts, permission: state.permission }),
    requestPermission: async () => {
      state.askedPermission++;
      state.permission = over.permissionAnswer ?? "granted";
      return state.permission;
    },
    current: async () => state.held,
    subscribe: async () => {
      if (over.subscribeThrows) throw new Error("no push service");
      state.subscribed++;
      state.held = sub(KEY, `https://fcm.googleapis.com/fcm/send/new${state.subscribed}`);
      return state.held;
    },
    marked: async () => state.marked,
    clearMark: async () => {
      state.marked = false;
    },
    wanted: () => state.wanted,
    setWanted: (on) => {
      state.wanted = on;
    },
    lang: () => "vi",
    ask: async (m) => {
      asked.push(m);
      const scripted = over.answers?.[String(m.type)];
      if (scripted) return scripted;
      return m.type === "push_info" ? ({ t: "ack", vapid: KEY } as Outcome) : ({ t: "ack" } as Outcome);
    },
  };
  const alerts = new Alerts(env, () => state.changes++);
  return { alerts, asked, state, sent: (type: string) => asked.filter((m) => m.type === type) };
}

describe("whether this page can have alerts", () => {
  const base: AlertsFacts = { secure: true, supported: true, ios: false, installed: false, permission: "default" };

  it("says why not, the first reason first", () => {
    expect(alertsUnavailable({ ...base, secure: false })).toBe("plain_http");
    expect(alertsUnavailable({ ...base, secure: false, ios: true, supported: false })).toBe("plain_http");
    expect(alertsUnavailable({ ...base, ios: true, installed: false, supported: false })).toBe("install_first");
    expect(alertsUnavailable({ ...base, ios: true, installed: true })).toBeNull();
    expect(alertsUnavailable({ ...base, supported: false })).toBe("no_support");
    expect(alertsUnavailable(base)).toBeNull();
  });

  it("turning on from a page that cannot says so and asks nothing", async () => {
    const r = rig({ facts: { secure: false } });
    await r.alerts.turnOn();
    expect(r.alerts.state).toBe("plain_http");
    expect(r.state.askedPermission).toBe(0);
    expect(r.asked).toEqual([]);
  });
});

describe("turning alerts on", () => {
  it("asks the permission, subscribes under the computer's key and tells the computer", async () => {
    const r = rig();
    await r.alerts.turnOn();
    expect(r.alerts.state).toBe("on");
    expect(r.state.wanted).toBe(true);
    expect(r.sent("push_sub")).toEqual([{ type: "push_sub", endpoint: "https://fcm.googleapis.com/fcm/send/new1", p256dh: "p", auth: "s", lang: "vi" }]);
  });

  it("stops at a no, and tells a no from no answer", async () => {
    for (const [answer, state] of [["denied", "blocked"], ["default", "off"]] as const) {
      const r = rig({ permissionAnswer: answer });
      await r.alerts.turnOn();
      expect(r.alerts.state).toBe(state);
      expect(r.state.subscribed).toBe(0);
      expect(r.state.wanted).toBe(false);
      expect(r.asked).toEqual([]);
    }
  });

  it("says what the computer refused and leaves no subscription behind", async () => {
    const r = rig({ answers: { push_sub: { t: "error", code: "bad_endpoint", message: "not a push service" } as Outcome } });
    await r.alerts.turnOn();
    expect(r.alerts.state).toBe("failed");
    expect(r.alerts.error).toBe("not a push service");
    expect(r.state.wanted).toBe(false);
    expect(r.state.held?.ended).toBe(true);
  });

  it("says when the browser could not subscribe", async () => {
    const r = rig({ subscribeThrows: true });
    await r.alerts.turnOn();
    expect(r.alerts.state).toBe("failed");
    expect(r.alerts.error).toContain("no push service");
    expect(r.state.wanted).toBe(false);
  });

  it("ends a subscription made under another key first: a browser refuses a second one", async () => {
    const old = sub("a-key-from-before-a-reset");
    const r = rig({ held: old });
    await r.alerts.turnOn();
    expect(old.ended).toBe(true);
    expect(r.state.subscribed).toBe(1);
    expect(r.alerts.state).toBe("on");
  });

  it("uses the subscription the browser already holds under this key", async () => {
    const r = rig({ held: sub() });
    await r.alerts.turnOn();
    expect(r.state.subscribed).toBe(0);
    expect(r.sent("push_sub")).toHaveLength(1);
    expect(r.alerts.state).toBe("on");
  });
});

describe("the look at every open", () => {
  const facts = { permission: "granted" as const };
  const on = { vapid: KEY, subscribed: true };

  it("says the subscription again, and changes nothing else", async () => {
    const held = sub();
    const r = rig({ facts, held, wanted: true });
    await r.alerts.recheck(on);
    expect(r.alerts.state).toBe("on");
    expect(held.ended).toBe(false);
    expect(r.state.subscribed).toBe(0);
    expect(r.sent("push_sub")).toHaveLength(1);
  });

  it("makes a new one with no tap when the browser lost it", async () => {
    const r = rig({ facts, wanted: true });
    await r.alerts.recheck(on);
    expect(r.state.subscribed).toBe(1);
    expect(r.alerts.state).toBe("on");
    expect(r.state.askedPermission).toBe(0);
  });

  it("makes a new one when the computer lost it (the push service said it was over)", async () => {
    const held = sub();
    const r = rig({ facts, held, wanted: true });
    await r.alerts.recheck({ vapid: KEY, subscribed: false });
    expect(held.ended).toBe(true);
    expect(r.state.subscribed).toBe(1);
    expect(r.alerts.state).toBe("on");
  });

  it("makes a new one when the service worker marked it, once", async () => {
    const held = sub();
    const r = rig({ facts, held, wanted: true, marked: true });
    await r.alerts.recheck(on);
    expect(held.ended).toBe(true);
    expect(r.state.subscribed).toBe(1);
    expect(r.state.marked).toBe(false);
    await r.alerts.recheck(on);
    expect(r.state.subscribed).toBe(1);
  });

  it("makes a new one under a new key", async () => {
    const old = sub("old");
    const r = rig({ facts, held: old, wanted: true });
    await r.alerts.recheck({ vapid: "new", subscribed: true });
    expect(old.ended).toBe(true);
    expect(r.state.subscribed).toBe(1);
  });

  it("asks the computer for its key when the hello had none", async () => {
    const r = rig({ facts, wanted: true });
    await r.alerts.recheck({ vapid: null, subscribed: false });
    expect(r.sent("push_info")).toHaveLength(1);
    expect(r.state.subscribed).toBe(1);
  });

  it("never asks for the permission: that takes a tap", async () => {
    const r = rig({ wanted: true });
    await r.alerts.recheck(on);
    expect(r.state.askedPermission).toBe(0);
    expect(r.state.subscribed).toBe(0);
    expect(r.alerts.state).toBe("off");
  });

  it("shows a phone's no", async () => {
    const r = rig({ facts: { permission: "denied" }, wanted: true });
    await r.alerts.recheck(on);
    expect(r.alerts.state).toBe("blocked");
  });

  it("offers the button when a renewal needs a tap after all", async () => {
    const r = rig({ facts, wanted: true, subscribeThrows: true });
    await r.alerts.recheck(on);
    expect(r.alerts.state).toBe("off");
    expect(r.state.wanted).toBe(true);
  });

  it("clears what is left of alerts the person turned off", async () => {
    const held = sub();
    const r = rig({ facts, held, wanted: false });
    await r.alerts.recheck(on);
    expect(held.ended).toBe(true);
    expect(r.sent("push_unsub")).toHaveLength(1);
    expect(r.alerts.state).toBe("off");
  });

  it("does nothing with a computer that said nothing of it", async () => {
    const r = rig({ facts, wanted: true });
    await r.alerts.recheck(undefined);
    expect(r.asked).toEqual([]);
    expect(r.state.subscribed).toBe(0);
  });

  it("says plain HTTP without touching anything", async () => {
    const r = rig({ facts: { secure: false }, wanted: true });
    const current = vi.fn();
    (r.alerts as unknown as { env: AlertsEnv }).env.current = current;
    await r.alerts.recheck(on);
    expect(r.alerts.state).toBe("plain_http");
    expect(current).not.toHaveBeenCalled();
  });
});

describe("turning off, testing, forgetting", () => {
  it("off ends it on both sides", async () => {
    const held = sub();
    const r = rig({ facts: { permission: "granted" }, held, wanted: true });
    await r.alerts.turnOff();
    expect(held.ended).toBe(true);
    expect(r.state.wanted).toBe(false);
    expect(r.sent("push_unsub")).toHaveLength(1);
    expect(r.alerts.state).toBe("off");
  });

  it("a test says what came of it", async () => {
    expect(await rig().alerts.test()).toBeNull();
    expect(await rig({ answers: { push_test: { t: "error", code: "push_failed", message: "HTTP 500" } as Outcome } }).alerts.test()).toBe("HTTP 500");
    expect(await rig({ answers: { push_test: { t: "offline" } as Outcome } }).alerts.test()).toBe("offline");
  });

  it("forgetting the pairing ends it here without asking the computer, which has forgotten too", async () => {
    const held = sub();
    const r = rig({ held, wanted: true });
    await r.alerts.forget();
    expect(held.ended).toBe(true);
    expect(r.state.wanted).toBe(false);
    expect(r.asked).toEqual([]);
  });
});

describe("the link in an alert", () => {
  it("names the session, and whether someone was waited for", () => {
    expect(readAlertLink("#s=abc&a=1")).toEqual({ session: "abc", waited: true });
    expect(readAlertLink("#s=abc")).toEqual({ session: "abc", waited: false });
    expect(readAlertLink("")).toEqual({ session: null, waited: false });
  });
});
