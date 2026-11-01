import { describe, expect, it } from "vitest";
import { backoffMs, canAct, dataAge, deriveConn, LagMeter, NAME_FAILS_WITHIN_MS, reachFromAnswer, RESTARTED_SHOWN_MS, whyNoAnswer, type ConnFacts } from "./conn-state";

const NOW = 10_000_000;
const facts = (over: Partial<ConnFacts> = {}): ConnFacts => ({
  online: true, ready: true, connecting: false, attempt: 0, nextAt: null, reach: "reachable", lastLiveAt: NOW, replaced: false, unpaired: false, notPairedHere: null,
  outdated: false, restartedAt: 0, rttMs: null, rttAt: 0, probeSince: null, lagMs: 0, lagAt: 0, resyncing: false, load: { level: 0, reasons: [] }, route: "direct", ...over,
});

describe("deriveConn", () => {
  it("live when connected and nothing is wrong", () => {
    expect(deriveConn(facts(), NOW)).toEqual({ kind: "live" });
  });

  it("no network beats everything but the device being removed", () => {
    expect(deriveConn(facts({ ready: false, online: false, attempt: 3 }), NOW).kind).toBe("no_network");
    expect(deriveConn(facts({ ready: false, online: false, unpaired: true }), NOW).kind).toBe("unpaired");
    expect(deriveConn(facts({ ready: false, notPairedHere: "unknown" }), NOW)).toEqual({ kind: "not_paired_here", why: "unknown" });
  });

  it("separates 'your connection is slow' from 'the computer is busy'", () => {
    expect(deriveConn(facts({ rttMs: 1200, rttAt: NOW - 1000 }), NOW)).toEqual({ kind: "degraded", why: "slow" });
    expect(deriveConn(facts({ probeSince: NOW - 1500 }), NOW)).toEqual({ kind: "degraded", why: "late" });
    expect(deriveConn(facts({ load: { level: 2, reasons: ["cpu"] } }), NOW)).toEqual({ kind: "busy", level: 2, reasons: ["cpu"] });
    // A slow link wins over a busy computer: the person can do something about it.
    expect(deriveConn(facts({ rttMs: 1200, rttAt: NOW - 1000, load: { level: 2, reasons: ["cpu"] } }), NOW).kind).toBe("degraded");
  });

  it("an old slow measurement says nothing about now", () => {
    expect(deriveConn(facts({ rttMs: 3000, rttAt: NOW - 60_000 }), NOW).kind).toBe("live");
    expect(deriveConn(facts({ lagMs: 4000, lagAt: NOW - 60_000 }), NOW).kind).toBe("live");
  });

  it("a restart shows for a few seconds", () => {
    expect(deriveConn(facts({ restartedAt: NOW - 1000 }), NOW).kind).toBe("restarted");
    expect(deriveConn(facts({ restartedAt: NOW - RESTARTED_SHOWN_MS - 1 }), NOW).kind).toBe("live");
  });

  it("reconnecting counts down to the next attempt", () => {
    expect(deriveConn(facts({ ready: false, attempt: 2, nextAt: NOW + 2500 }), NOW)).toEqual({ kind: "reconnecting", attempt: 2, nextInS: 3 });
    expect(deriveConn(facts({ ready: false, attempt: 1, nextAt: null }), NOW)).toEqual({ kind: "reconnecting", attempt: 1, nextInS: null });
  });

  it("names a sign-in page, an unreachable computer and (later) an unreachable relay", () => {
    expect(deriveConn(facts({ ready: false, attempt: 1, reach: "portal" }), NOW).kind).toBe("sign_in");
    expect(deriveConn(facts({ ready: false, attempt: 1, reach: "unreachable" }), NOW).kind).toBe("reconnecting");
    expect(deriveConn(facts({ ready: false, attempt: 3, reach: "unreachable", lastLiveAt: NOW - 90_000 }), NOW)).toEqual({ kind: "computer_offline", since: NOW - 90_000 });
    expect(deriveConn(facts({ ready: false, attempt: 3, reach: "unreachable", route: "relay" }), NOW).kind).toBe("relay_unreachable");
  });

  it("an answer is the computer's own, a login page, or Tailscale saying nothing is behind it", () => {
    const manifest = "application/manifest+json";
    expect(reachFromAnswer(200, false, manifest)).toBe("reachable");
    expect(reachFromAnswer(200, false, "text/html")).toBe("portal");
    expect(reachFromAnswer(200, true, manifest)).toBe("portal");
    expect(reachFromAnswer(302, false, "")).toBe("portal");
    expect(reachFromAnswer(404, false, "text/html")).toBe("portal");
    for (const status of [500, 502, 503, 504]) expect(reachFromAnswer(status, false, "")).toBe("unreachable");
    expect(deriveConn(facts({ ready: false, attempt: 3, reach: reachFromAnswer(502, false, ""), lastLiveAt: NOW }), NOW).kind).toBe("computer_offline");
  });

  it("a Tailscale name that fails at once asks for Tailscale, not for the computer", () => {
    expect(deriveConn(facts({ ready: false, attempt: 3, reach: "no_tailnet" }), NOW)).toEqual({ kind: "no_tailnet" });
    expect(deriveConn(facts({ ready: false, attempt: 1, reach: "no_tailnet" }), NOW).kind).toBe("reconnecting");
    expect(deriveConn(facts({ ready: false, attempt: 3, online: false, reach: "no_tailnet" }), NOW).kind).toBe("no_network");
    expect(canAct({ kind: "no_tailnet" })).toBe(false);
    const name = "desk.tail1234.ts.net";
    expect(whyNoAnswer(name, false, 40)).toBe("no_tailnet");
    expect(whyNoAnswer(name, false, NAME_FAILS_WITHIN_MS + 1)).toBe("unreachable");
    expect(whyNoAnswer(name, true, 40)).toBe("unreachable");
    expect(whyNoAnswer("192.168.1.5", false, 40)).toBe("unreachable");
  });

  it("the first connection is just 'connecting'", () => {
    expect(deriveConn(facts({ ready: false, connecting: true, lastLiveAt: 0 }), NOW).kind).toBe("connecting");
  });

  // The decision: input, keys and answers are allowed where the socket is open and the picture is current (live,
  // degraded, busy: a busy computer must not lock the person out); after a restart only once the resync is
  // done; everywhere else they are refused (the reply box stays editable, only sending is disabled).
  it("input, keys and answers are allowed in live, degraded and busy", () => {
    expect(canAct(deriveConn(facts(), NOW))).toBe(true);
    expect(canAct(deriveConn(facts({ load: { level: 3, reasons: ["lag"] } }), NOW))).toBe(true);
    expect(canAct(deriveConn(facts({ rttMs: 1500, rttAt: NOW }), NOW))).toBe(true);
    expect(canAct(deriveConn(facts({ probeSince: NOW - 2000 }), NOW))).toBe(true);
  });

  it("after a restart they wait for the resync, then are allowed", () => {
    const restarted = (resyncing: boolean) => deriveConn(facts({ restartedAt: NOW - 500, resyncing }), NOW);
    expect(restarted(true)).toEqual({ kind: "restarted", resyncing: true });
    expect(canAct(restarted(true))).toBe(false);
    expect(canAct(restarted(false))).toBe(true);
    // A resync that outlasts the "restarted" notice still blocks.
    expect(canAct(deriveConn(facts({ restartedAt: NOW - 60_000, resyncing: true }), NOW))).toBe(false);
  });

  it("every other state refuses", () => {
    const refused = [
      facts({ ready: false, attempt: 1 }),
      facts({ ready: false, attempt: 3, reach: "unreachable" }),
      facts({ ready: false, online: false }),
      facts({ ready: false, attempt: 1, reach: "portal" }),
      facts({ ready: false, connecting: true, lastLiveAt: 0 }),
      facts({ ready: false, attempt: 3, reach: "unreachable", route: "relay" }),
      facts({ replaced: true }),
      facts({ outdated: true }),
      facts({ unpaired: true }),
    ];
    for (const f of refused) expect(canAct(deriveConn(f, NOW)), JSON.stringify(deriveConn(f, NOW))).toBe(false);
  });
});

describe("backoff", () => {
  it("full jitter: first retry 0-200 ms, then 0-1 s doubling to 0-30 s", () => {
    const max = () => 0.999999;
    const min = () => 0;
    expect([1, 2, 3, 4, 5, 6, 7, 8].map((n) => backoffMs(n, max))).toEqual([199, 999, 1999, 3999, 7999, 15999, 29999, 29999]);
    expect([1, 2, 9].map((n) => backoffMs(n, min))).toEqual([0, 0, 0]);
    expect(backoffMs(40, max)).toBeLessThan(30_000);
  });
});

describe("data age", () => {
  it("measures lag against the best delivery seen, whatever the clock difference", () => {
    const m = new LagMeter();
    // The phone's clock is 5 minutes ahead of the computer's.
    const skew = 300_000;
    expect(m.add(1000, 1000 + skew + 40)).toBe(0);
    expect(m.add(2000, 2000 + skew + 90)).toBe(50);
    expect(m.add(3000, 3000 + skew + 2540)).toBe(2500);
    expect(m.add(4000, 4000 + skew + 40)).toBe(0);
  });

  it("is amber after 2 s and red after 5 s", () => {
    expect(dataAge(100, 500)).toBe("fresh");
    expect(dataAge(2500, 0)).toBe("slow");
    expect(dataAge(0, 5500)).toBe("stuck");
  });
});
