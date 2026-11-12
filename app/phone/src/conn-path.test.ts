import { describe, expect, it } from "vitest";
import { isLocalHost, isTailscaleHost, livePath, methodLines, methodText, pageMethod, pairWay, pathText, screenBlock, screenWhy, type ScreenFacts } from "./conn-path";

const fill = (text: string, vars?: Record<string, string | number>) => (vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text);
const page = (url: string) => {
  const u = new URL(url);
  return { protocol: u.protocol, hostname: u.hostname };
};

describe("how the app was paired, from where its page comes", () => {
  it("knows the hosted app whatever its address", () => {
    expect(pageMethod(true, page("https://phone.kelpie.example/"))).toBe("hosted");
  });

  it("knows a Tailscale address by name, by its IPv4 range and by its IPv6 range", () => {
    expect(pageMethod(false, page("https://desk.tail1234.ts.net:8443/"))).toBe("tailscale");
    expect(pageMethod(false, page("http://100.101.102.103:48170/"))).toBe("tailscale");
    expect(pageMethod(false, page("http://[fd7a:115c:a1e0::1]:48170/"))).toBe("tailscale");
    expect(isTailscaleHost("100.63.0.1")).toBe(false);
    expect(isTailscaleHost("100.128.0.1")).toBe(false);
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
    expect(methodLines("hosted", fill).self).toBe("This is the app from Kelpie's relay.");
    expect(methodLines("tailscale", fill).self).toBe("This is the app from your computer over Tailscale.");
    for (const m of ["https", "http", "local"] as const) expect(methodLines(m, fill).self, m).toBe("This is the app on this network.");
  });

  it("lists the two other ways in one line each, and where to choose one", () => {
    const hosted = methodLines("hosted", fill);
    expect(hosted.others).toHaveLength(2);
    expect(hosted.others[0]).toMatch(/^Tailscale:/);
    expect(hosted.others[1]).toMatch(/^This network:/);
    expect(methodLines("tailscale", fill).others.map((l) => l.split(":")[0])).toEqual(["Kelpie's relay", "This network"]);
    expect(methodLines("http", fill).others.map((l) => l.split(":")[0])).toEqual(["Tailscale", "Kelpie's relay"]);
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
});
