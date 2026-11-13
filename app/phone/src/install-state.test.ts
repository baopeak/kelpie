import { describe, expect, it } from "vitest";
import { afterFailedPairing, applyAddrs, firstScreen, movedTo, platformOf, readCode, relayMovedTo, type PageFacts } from "./install-state";
import { b64url, randomBytes } from "./noise";

const facts = (over: Partial<PageFacts> = {}): PageFacts => ({ https: true, tailnet: true, hosted: false, installed: false, paired: false, hasCode: false, stayInBrowser: false, ...over });
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

  it("a person's own certificate on this network pairs from the link as it always did", () => {
    expect(firstScreen(facts({ tailnet: false, hasCode: true }))).toBe("pair");
    expect(firstScreen(facts({ tailnet: false }))).toBe("install");
  });

  it("a pairing that failed leads back to the scanner where that is what comes next, and otherwise stays where it can be read", () => {
    const after = (over: Partial<PageFacts>) => afterFailedPairing(firstScreen(facts({ ...over, hasCode: false })));
    expect(after({ installed: true })).toBe("back_to_scanner");
    expect(after({ stayInBrowser: true })).toBe("back_to_scanner");
    expect(after({ tailnet: false, stayInBrowser: true })).toBe("back_to_scanner");
    // A browser tab on a person's own certificate: with the code cleared it would land on the install steps, with no word of what went wrong.
    expect(after({ tailnet: false })).toBe("stay");
    expect(after({ https: false })).toBe("stay");
  });

  it("the app served from the relay is installed first, as at a Tailscale name, with or without a code", () => {
    const hosted = (over: Partial<PageFacts> = {}) => firstScreen(facts({ tailnet: false, hosted: true, ...over }));
    expect(hosted()).toBe("install");
    expect(hosted({ hasCode: true })).toBe("install");
    expect(hosted({ installed: true })).toBe("scan");
    expect(hosted({ installed: true, hasCode: true })).toBe("pair");
    expect(hosted({ stayInBrowser: true })).toBe("scan");
    expect(afterFailedPairing(hosted({ installed: true }))).toBe("back_to_scanner");
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

describe("the computer's relay, on the app the relay serves", () => {
  const RELAY_HOST = "kelpie-link-relay.example.workers.dev";

  it("is news only when it is on another host than this page", () => {
    expect(relayMovedTo([{ t: "ts", a: HERE }, { t: "relay", a: `wss://${RELAY_HOST}`, r: "ticket" }], RELAY_HOST)).toBeNull();
    expect(relayMovedTo([{ t: "relay", a: "wss://relay.other.example", r: "ticket" }], RELAY_HOST)).toBe("relay.other.example");
    expect(relayMovedTo([{ t: "relay", a: `wss://${RELAY_HOST}:8443`, r: "ticket" }], RELAY_HOST)).toBe(`${RELAY_HOST}:8443`);
    expect(relayMovedTo([{ t: "ts", a: HERE }], RELAY_HOST)).toBeNull();
    expect(relayMovedTo([{ t: "relay", a: "not a url" }], RELAY_HOST)).toBeNull();
  });
});

describe("where the computer is, as an addrs message tells it", () => {
  const TS = { t: "ts", a: HERE };
  const LAN = { t: "lan", a: "192.168.1.5:4817" };
  const RELAY = { t: "relay", a: "wss://relay.example", r: "room.ticket" };
  const NEW_RELAY = { t: "relay", a: "wss://relay.example", r: "new.ticket" };

  it("takes a whole newer list in place of all it held", () => {
    const held = { endpoints: [TS, LAN, RELAY], addrsSeq: 3 };
    expect(applyAddrs(held, { endpoints: [TS, NEW_RELAY], list: [], seq: 4 })).toEqual({ known: [TS, NEW_RELAY], keep: { endpoints: [TS, NEW_RELAY], addrsSeq: 4 } });
    expect(applyAddrs(held, { endpoints: [TS], list: [], seq: 3 }).keep).toBeNull();
    // A computer from before `endpoints` sends the direct addresses alone, and the relay stays.
    expect(applyAddrs(held, { list: ["10.0.0.2:4817"], seq: 4 }).keep).toEqual({ endpoints: [{ t: "lan", a: "10.0.0.2:4817" }, RELAY], addrsSeq: 4 });
  });

  it("takes only the relay from a partial one, keeps the rest and leaves the sequence for the whole list", () => {
    const held = { endpoints: [TS, LAN, RELAY], addrsSeq: 3 };
    const locked = applyAddrs(held, { endpoints: [NEW_RELAY], list: [], seq: 4, partial: true });
    expect(locked).toEqual({ known: [TS, LAN, NEW_RELAY], keep: { endpoints: [TS, LAN, NEW_RELAY], addrsSeq: 3 } });
    // Unlocked, the same number brings the whole list, and it is taken.
    expect(applyAddrs({ ...held, ...locked.keep! }, { endpoints: [TS, NEW_RELAY], list: [], seq: 4 }).keep).toEqual({ endpoints: [TS, NEW_RELAY], addrsSeq: 4 });
    // The relay turned off; nothing new; an older message.
    expect(applyAddrs(held, { endpoints: [], list: [], seq: 4, partial: true }).keep).toEqual({ endpoints: [TS, LAN], addrsSeq: 3 });
    expect(applyAddrs(held, { endpoints: [RELAY], list: [], seq: 3, partial: true }).keep).toBeNull();
    expect(applyAddrs(held, { endpoints: [NEW_RELAY], list: [], seq: 2, partial: true }).keep).toBeNull();
    expect(applyAddrs(null, { endpoints: [RELAY], list: [], seq: 1, partial: true })).toEqual({ known: [RELAY], keep: null });
    // In the order the computer sends them, the relay keeps its place: nothing new is nothing written.
    const asSent = { endpoints: [TS, RELAY, LAN], addrsSeq: 3 };
    expect(applyAddrs(asSent, { endpoints: [RELAY], list: [], seq: 3, partial: true })).toEqual({ known: [TS, RELAY, LAN], keep: null });
    expect(applyAddrs(asSent, { endpoints: [NEW_RELAY], list: [], seq: 3, partial: true }).keep).toEqual({ endpoints: [TS, NEW_RELAY, LAN], addrsSeq: 3 });
  });

  it("keeps nothing but the relay on the page the relay hosts, whatever a computer from before told it", () => {
    const held = { endpoints: [TS, LAN, RELAY], addrsSeq: 3 };
    expect(applyAddrs(held, { endpoints: [RELAY], list: [], seq: 3, partial: true }, true).keep).toEqual({ endpoints: [RELAY], addrsSeq: 3 });
    expect(applyAddrs(held, { endpoints: [TS, LAN, NEW_RELAY], list: ["192.168.1.5:4817"], seq: 4 }, true)).toEqual({ known: [NEW_RELAY], keep: { endpoints: [NEW_RELAY], addrsSeq: 3 } });
    // A computer that restarted counts from nothing again: its new room is still taken.
    expect(applyAddrs({ endpoints: [RELAY], addrsSeq: 9 }, { endpoints: [NEW_RELAY], list: [], seq: 0, partial: true }, true).keep).toEqual({ endpoints: [NEW_RELAY], addrsSeq: 9 });
  });
});
