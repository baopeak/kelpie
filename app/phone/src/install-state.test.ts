import { describe, expect, it } from "vitest";
import { firstScreen, movedTo, platformOf, readCode, type PageFacts } from "./install-state";
import { b64url, randomBytes } from "./noise";

const facts = (over: Partial<PageFacts> = {}): PageFacts => ({ https: true, installed: false, paired: false, hasCode: false, stayInBrowser: false, ...over });
const code = b64url(new TextEncoder().encode(JSON.stringify({ v: 1, dpk: b64url(randomBytes(32)), psk: b64url(randomBytes(32)), ep: [{ t: "ts", a: "desk.tail1234.ts.net:8443" }], name: "Desk" })));
const HERE = "desk.tail1234.ts.net:8443";

describe("the first screen", () => {
  it("a plain http page stays what it was: the link pairs that page", () => {
    expect(firstScreen(facts({ https: false }))).toBe("pair");
    expect(firstScreen(facts({ https: false, hasCode: true }))).toBe("pair");
  });

  it("in a browser on a secure origin, installing comes first, with or without a code", () => {
    expect(firstScreen(facts())).toBe("install");
    expect(firstScreen(facts({ hasCode: true }))).toBe("install");
  });

  it("in the installed app the code comes from the camera or the clipboard, and then the device is named", () => {
    expect(firstScreen(facts({ installed: true }))).toBe("scan");
    expect(firstScreen(facts({ installed: true, hasCode: true }))).toBe("pair");
  });

  it("the person may go on in the browser", () => {
    expect(firstScreen(facts({ stayInBrowser: true }))).toBe("scan");
    expect(firstScreen(facts({ stayInBrowser: true, hasCode: true }))).toBe("pair");
  });

  it("a paired device is past all of it", () => {
    for (const installed of [true, false]) expect(firstScreen(facts({ paired: true, installed }))).toBe("pair");
  });
});

describe("the kind of phone", () => {
  it("tells them apart, an iPad that says it is a Mac among them", () => {
    expect(platformOf("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)", 5)).toBe("ios");
    expect(platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15", 5)).toBe("ios");
    expect(platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15", 0)).toBe("other");
    expect(platformOf("Mozilla/5.0 (Linux; Android 15; Pixel 9) Chrome/130", 5)).toBe("android");
    expect(platformOf("Mozilla/5.0 (Windows NT 10.0) Chrome/130", 0)).toBe("other");
  });
});

describe("what a scan or a paste reads", () => {
  it("a whole link for this address, or only its code", () => {
    expect(readCode(`https://${HERE}/#p=${code}`, HERE)).toEqual({ ok: true, code });
    expect(readCode(`  https://${HERE}/#p=${code}\n`, HERE)).toEqual({ ok: true, code });
    expect(readCode(code, HERE)).toEqual({ ok: true, code });
    expect(readCode(`#p=${code}`, HERE)).toEqual({ ok: true, code });
  });

  it("the first step's code says so", () => {
    expect(readCode(`https://${HERE}/`, HERE)).toEqual({ ok: false, why: "install_code" });
  });

  it("a code for another address cannot pair this page", () => {
    expect(readCode(`http://192.168.1.5:4817/#p=${code}`, HERE)).toEqual({ ok: false, why: "other_address", host: "192.168.1.5:4817" });
  });

  it("anything else is no code", () => {
    for (const bad of ["", "hello", "https://example.com/", `https://${HERE}/#p=garbage`, "https://example.com/#s=abc"]) {
      expect(readCode(bad, HERE)).toEqual({ ok: false, why: "not_code" });
    }
  });
});

describe("the computer's Tailscale address", () => {
  it("is news only when this page is somewhere else", () => {
    expect(movedTo([{ t: "ts", a: HERE }, { t: "lan", a: "192.168.1.5:4817" }], HERE)).toBeNull();
    expect(movedTo([{ t: "ts", a: "new.tail1234.ts.net:8443" }], HERE)).toBe("new.tail1234.ts.net:8443");
    expect(movedTo([{ t: "ts", a: HERE }], "192.168.1.5:4817")).toBe(HERE);
    expect(movedTo([{ t: "lan", a: "192.168.1.5:4817" }], "192.168.1.5:4817")).toBeNull();
    expect(movedTo([], HERE)).toBeNull();
  });
});
