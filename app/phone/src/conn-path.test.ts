import { describe, expect, it } from "vitest";
import { computerBlock, isLocalHost, isTailscaleHost, knownScreenReason, livePath, methodLines, methodText, pageMethod, pairWay, pathText, screenBlock, screenOffered, screenReasonText, screenReady, screenWhy, tailscaleHint, type ScreenFacts } from "./conn-path";

const fill = (text: string, vars?: Record<string, string | number>) => (vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text);
const page = (url: string) => {
  const u = new URL(url);
  return { protocol: u.protocol, hostname: u.hostname };
};

describe("how the app was paired, from where its page comes", () => {
  it("knows the hosted app whatever its address", () => {
    expect(pageMethod(true, page("https://phone.kelpie.example/"))).toBe("hosted");
  });

  it("knows the app Tailscale publishes by its HTTPS .ts.net name, and a Tailscale address by name and by its IPv4 and IPv6 ranges", () => {
    expect(pageMethod(false, page("https://desk.tail1234.ts.net:8443/"))).toBe("tailscale");
    expect([isTailscaleHost("desk.tail1234.ts.net"), isTailscaleHost("100.101.102.103"), isTailscaleHost("[fd7a:115c:a1e0::1]")]).toEqual([true, true, true]);
    expect(isTailscaleHost("100.63.0.1")).toBe(false);
    expect(isTailscaleHost("100.128.0.1")).toBe(false);
  });

  it("calls the computer's own listener at a Tailscale address this network, as Kelpie on the computer lists it", () => {
    expect(pageMethod(false, page("http://100.101.102.103:48170/"))).toBe("http");
    expect(pageMethod(false, page("http://[fd7a:115c:a1e0::1]:48170/"))).toBe("http");
    expect(pageMethod(false, page("https://100.101.102.103:48170/"))).toBe("https");
    expect(pairWay(pageMethod(false, page("http://100.101.102.103:48170/")))).toBe("network");
  });

  it("tells this computer, another HTTPS name and plain HTTP on this network apart", () => {
    expect(pageMethod(false, page("https://localhost:48170/"))).toBe("local");
    expect(pageMethod(false, page("http://127.0.0.1:48170/"))).toBe("local");
    expect(isLocalHost("[::1]")).toBe(true);
    expect(pageMethod(false, page("https://kelpie.home.example/"))).toBe("https");
    expect(pageMethod(false, page("http://192.168.1.20:48170/"))).toBe("http");
  });

  it("says each in words", () => {
    expect(methodText("hosted", fill)).toMatch(/relay/);
    expect(methodText("tailscale", fill)).toMatch(/Tailscale/);
    expect(methodText("https", fill)).toMatch(/HTTPS/);
    expect(methodText("http", fill)).toMatch(/plain HTTP/);
    expect(methodText("local", fill)).toMatch(/localhost/);
  });
});

describe("which app this is, and the other ways to connect this phone", () => {
  it("knows the way from the page", () => {
    expect(["hosted", "tailscale", "https", "http", "local"].map((m) => pairWay(m as Parameters<typeof pairWay>[0]))).toEqual(["relay", "tailscale", "network", "network", "network"]);
  });

  it("names this app's way", () => {
    expect(methodLines("hosted", fill).self, "the phone cannot tell whose relay it is").toBe("This is the app from the relay.");
    expect(methodLines("tailscale", fill).self).toBe("This is the app from your computer over Tailscale.");
    for (const m of ["https", "http", "local"] as const) expect(methodLines(m, fill).self, m).toBe("This is the app on this network.");
  });

  it("lists the two other ways in one line each, and where to choose one", () => {
    const hosted = methodLines("hosted", fill);
    expect(hosted.others).toHaveLength(2);
    expect(hosted.others[0]).toMatch(/^Tailscale:/);
    expect(hosted.others[1]).toMatch(/^This network:/);
    expect(methodLines("tailscale", fill).others.map((l) => l.split(":")[0])).toEqual(["The relay", "This network"]);
    expect(methodLines("http", fill).others.map((l) => l.split(":")[0])).toEqual(["Tailscale", "The relay"]);
    expect(hosted.choose).toContain("Settings → Mobile → Pair a phone");
  });
});

describe("the way the connection takes now", () => {
  it("is the relay on the relay, and on the hosted app always", () => {
    for (const m of ["tailscale", "https", "http", "local", "hosted"] as const) expect(livePath("relay", m)).toBe("relay");
    expect(livePath("direct", "hosted")).toBe("relay");
  });

  it("is the page's own origin otherwise", () => {
    expect(livePath("direct", "tailscale")).toBe("tailscale");
    expect(livePath("direct", "local")).toBe("computer");
    expect(livePath("direct", "http")).toBe("network");
    expect(livePath("direct", "https"), "another HTTPS name may be reached from outside this network").toBe("address");
  });

  it("is Tailscale to a page at a Tailscale address, whatever way the app paired", () => {
    expect(livePath("direct", "http", "100.101.102.103")).toBe("tailscale");
    expect(livePath("direct", "https", "[fd7a:115c:a1e0::1]")).toBe("tailscale");
    expect(livePath("direct", "http", "192.168.1.20")).toBe("network");
    expect(livePath("relay", "http", "100.101.102.103")).toBe("relay");
  });

  it("says each in words", () => {
    expect(pathText("relay", fill)).toBe("the relay");
    expect(pathText("tailscale", fill)).toBe("Tailscale");
    expect(pathText("network", fill)).toBe("this network");
    expect(pathText("address", fill)).toBe("this computer's address");
    expect(pathText("computer", fill)).toBe("this computer");
  });
});

describe("why the screen is not offered here", () => {
  const ok: ScreenFacts = { known: true, older: false, hosted: false, permission: "control", route: "direct", unlocked: true };

  it("offers it when nothing stands in the way", () => {
    expect(screenBlock(ok)).toBeNull();
    expect(screenWhy(ok, fill)).toBeNull();
  });

  it("names each reason with what would change it", () => {
    expect(screenWhy({ ...ok, known: false }, fill)).toMatch(/Connect to your computer/);
    expect(screenWhy({ ...ok, older: true }, fill)).toMatch(/Update Kelpie on your computer/);
    expect(screenWhy({ ...ok, hosted: true }, fill)).toMatch(/comes from the relay/);
    expect(screenWhy({ ...ok, permission: "write" }, fill)).toMatch(/May take control.*Settings → Mobile/);
    expect(screenWhy({ ...ok, permission: undefined }, fill)).toMatch(/May take control/);
    expect(screenWhy({ ...ok, route: "relay" }, fill)).toMatch(/needs Tailscale; this phone is on the relay/);
    expect(screenWhy({ ...ok, unlocked: false }, fill)).toMatch(/app lock on and unlocked/);
  });

  it("checks in order: nothing heard yet, an older computer, the hosted app, the relay, the permission, the lock", () => {
    let f: ScreenFacts = { known: false, older: true, hosted: true, permission: "read", route: "relay", unlocked: false };
    const seen: (string | null)[] = [];
    for (const clear of [{ known: true }, { older: false }, { hosted: false }, { route: "direct" as const }, { permission: "control" as const }, { unlocked: true }]) {
      seen.push(screenBlock(f));
      f = { ...f, ...clear };
    }
    seen.push(screenBlock(f));
    expect(seen).toEqual(["unknown", "older", "hosted", "relay", "not_control", "lock", null]);
  });

  it("on the relay, says Tailscale is needed even to a phone the computer capped below control there", () => {
    expect(screenBlock({ ...ok, permission: "write", route: "relay" })).toBe("relay");
  });

  it("puts what the computer says first", () => {
    expect(screenBlock({ ...ok, computer: "not_control" })).toBe("not_control");
    expect(screenBlock({ ...ok, older: true, computer: "relay" })).toBe("relay");
    expect(screenBlock({ ...ok, computer: null })).toBeNull();
  });

  it("reads each of the computer's reasons as the phone's own, and works it out itself for ok, an older computer or a reason it does not know", () => {
    const codes = ["hosted_app", "not_granted", "plain_page", "via_relay", "lock_off", "no_passkey", "locked"];
    expect(codes.map(computerBlock)).toEqual(["hosted", "not_control", "plain_page", "relay", "lock", "lock", "lock"]);
    for (const none of ["ok", undefined, "a_later_reason"]) expect(computerBlock(none), String(none)).toBeNull();
    expect(screenWhy({ ...ok, computer: computerBlock("plain_page") }, fill)).toMatch(/did not come from your computer over HTTPS/);
    expect(screenWhy({ ...ok, unlocked: false, computer: computerBlock("ok") }, fill), "ok, and locked here since").toMatch(/app lock on and unlocked/);
    expect(screenWhy({ ...ok, permission: "write", computer: computerBlock("not_granted") }, fill)).toBe(screenWhy({ ...ok, permission: "write" }, fill));
  });
});

describe("where the screen is, from the hosted app", () => {
  it("says to pair the Tailscale app when the computer has Tailscale, on the hosted app only", () => {
    expect(tailscaleHint("hosted", true, fill)).toMatch(/also has Tailscale.*Pair a phone → Tailscale/);
    expect(tailscaleHint("hosted", false, fill)).toBeNull();
    for (const m of ["tailscale", "https", "http", "local"] as const) expect(tailscaleHint(m, true, fill), m).toBeNull();
  });
});

describe("what the computer says about the screen on this connection", () => {
  const ok: ScreenFacts = { known: true, older: false, hosted: true, permission: "write", route: "relay", unlocked: true };
  const REASONS = ["not_granted", "plain_page", "method_off", "no_direct_link", "not_on_relay", "screen_remote_off", "lock_off", "no_passkey", "locked", "needs_face_id", "relay_off", "relay_budget_spent", "hosted_app_no_tailscale"];

  it("says each reason in words, and nothing for one it does not know", () => {
    for (const code of REASONS) expect(screenReasonText(code, fill), code).toBeTruthy();
    expect(screenReasonText("a_later_reason", fill)).toBeNull();
    expect(screenReasonText("screen_remote_off", fill)).toBe("Turn on \"See and control the screen without Tailscale\" in Kelpie → Settings → Mobile.");
    expect(screenReasonText("hosted_app_no_tailscale", fill)).toBe("This app came from the relay, so it can reach the computer only through the relay.");
    expect(screenReasonText("no_direct_link", fill)).toMatch(/Tailscale is off on this phone or not reachable now/);
  });

  it("knows ok and the reasons, and is ready for ok and for Face ID alone", () => {
    for (const code of ["ok", ...REASONS]) expect(knownScreenReason(code), code).toBe(true);
    expect([knownScreenReason("later"), knownScreenReason(undefined), knownScreenReason(null)]).toEqual([false, false, false]);
    expect(REASONS.filter(screenReady)).toEqual(["needs_face_id"]);
    expect(screenReady("ok")).toBe(true);
  });

  it("a hosted phone is offered the screen when the computer says ok, else it gets the reason", () => {
    expect(screenBlock({ ...ok, screen: "ok" })).toBeNull();
    expect(screenWhy({ ...ok, screen: "ok" }, fill)).toBeNull();
    expect(screenBlock({ ...ok, screen: "ok", unlocked: false }), "ok, and locked here since").toBe("lock");
    expect(screenBlock({ ...ok, screen: "needs_face_id" })).toBeNull();
    expect(screenWhy({ ...ok, screen: "screen_remote_off" }, fill)).toMatch(/See and control the screen without Tailscale.*Settings → Mobile/);
    expect(screenWhy({ ...ok, screen: "no_passkey" }, fill)).toMatch(/app lock/);
  });

  it("goes by the old rules when the computer does not say, or says what this app does not know", () => {
    expect(screenWhy(ok, fill)).toMatch(/comes from the relay/);
    expect(screenWhy({ ...ok, screen: undefined }, fill)).toMatch(/comes from the relay/);
    expect(screenWhy({ ...ok, screen: "a_later_reason" }, fill)).toMatch(/comes from the relay/);
    expect(screenWhy({ ...ok, hosted: false, route: "direct", permission: "control", screen: null }, fill)).toBeNull();
  });

  it("offers the entry by the computer's word, and by the permission from one that does not say", () => {
    expect(screenOffered({ older: false, screen: "ok", permission: "write" })).toBe(true);
    expect(screenOffered({ older: false, screen: "needs_face_id", permission: "write" })).toBe(true);
    expect(screenOffered({ older: false, screen: "screen_remote_off", permission: "control" })).toBe(false);
    expect(screenOffered({ older: false, screen: undefined, permission: "control" })).toBe(true);
    expect(screenOffered({ older: false, screen: undefined, permission: "write" })).toBe(false);
    expect(screenOffered({ older: false, screen: "a_later_reason", permission: "control" })).toBe(true);
    expect(screenOffered({ older: true, screen: "ok", permission: "control" })).toBe(false);
  });
});
