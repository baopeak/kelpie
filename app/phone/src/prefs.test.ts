import { describe, expect, it } from "vitest";
import { choice, confirmCtrlC, defaultKeyBar, flag, INBOX_ORDERS, INBOX_VIEWS, inboxView, KEY_IDS, keyBar, lockGrace, moveKey, order, pref, readKeyBar, rememberedVia, rememberVia, showStopped, shownKeys, textSize, toggleKey, vibrate, wrapLines } from "./prefs";
import { store } from "./storage";
import { viaKey } from "./screen-paths";

describe("a preference", () => {
  it("starts from what is kept, and keeps what is set", () => {
    store.set("test-pref-kept", "large");
    const p = choice("test-pref-kept", ["small", "large"] as const, "small");
    expect(p.get()).toBe("large");
    p.set("small");
    expect(p.get()).toBe("small");
    expect(store.get("test-pref-kept")).toBe("small");
  });

  it("reads a value this build does not know, or nothing kept, as the default", () => {
    store.set("test-pref-unknown", "huge");
    expect(choice("test-pref-unknown", ["small", "large"] as const, "small").get()).toBe("small");
    expect(choice("test-pref-nothing", [0, 60] as const, 60).get()).toBe(60);
    expect(pref("test-pref-unread", 5, () => undefined).get()).toBe(5);
  });

  it("keeps a flag as 1 or 0, and anything else reads as the default", () => {
    store.set("test-flag-off", "0");
    store.set("test-flag-odd", "yes");
    expect(flag("test-flag-off", true).get()).toBe(false);
    expect(flag("test-flag-odd", true).get()).toBe(true);
    const f = flag("test-flag-set", false);
    f.set(true);
    expect(store.get("test-flag-set")).toBe("1");
  });
});

describe("the defaults", () => {
  it("lock after 5 minutes, medium text, long lines wrapped, Ctrl+C asked first, vibration on, every key shown", () => {
    expect(lockGrace.get()).toBe(300);
    expect(textSize.get()).toBe("m");
    expect(wrapLines.get()).toBe(true);
    expect(confirmCtrlC.get()).toBe(true);
    expect(vibrate.get()).toBe(true);
    expect(keyBar.get()).toEqual(defaultKeyBar());
    expect(shownKeys(keyBar.get())).toEqual([...KEY_IDS]);
  });

  it("the inbox by status, stopped sessions shown, the most recent first", () => {
    expect(inboxView.get()).toBe("status");
    expect(showStopped.get()).toBe(true);
    expect(order.get()).toBe("recent");
  });
});

describe("the inbox's choices", () => {
  it("keep what is set, and read what they do not know as the default", () => {
    const view = choice("test-inbox-view", INBOX_VIEWS, "status");
    view.set("workspace");
    expect(choice("test-inbox-view", INBOX_VIEWS, "status").get()).toBe("workspace");
    store.set("test-inbox-order", "size");
    expect(choice("test-inbox-order", INBOX_ORDERS, "recent").get()).toBe("recent");
    store.set("test-inbox-order", "name");
    expect(choice("test-inbox-order", INBOX_ORDERS, "recent").get()).toBe("name");
  });
});

describe("the key bar", () => {
  it("reads what was kept, leaving out keys it does not know and adding the ones not kept", () => {
    const got = readKeyBar(JSON.stringify({ order: ["enter", "esc", "f13", "esc"], hidden: ["tab", "f13"] }))!;
    expect(got.order.slice(0, 2)).toEqual(["enter", "esc"]);
    expect([...got.order].sort()).toEqual([...KEY_IDS].sort());
    expect(got.hidden).toEqual(["tab"]);
  });

  it("reads something that is not a key bar as nothing", () => {
    for (const raw of ["not json", "null", "[]", JSON.stringify({ order: "esc", hidden: [] }), JSON.stringify({ order: [] })]) expect(readKeyBar(raw)).toBeUndefined();
  });

  it("hides and shows a key", () => {
    const hidden = toggleKey(defaultKeyBar(), "esc");
    expect(shownKeys(hidden)).not.toContain("esc");
    expect(shownKeys(toggleKey(hidden, "esc"))).toEqual([...KEY_IDS]);
  });

  it("moves a key, and at either end leaves it", () => {
    const s = defaultKeyBar();
    expect(moveKey(s, "tab", -1).order.slice(0, 2)).toEqual(["tab", "esc"]);
    expect(moveKey(s, "tab", 1).order.slice(1, 3)).toEqual(["shift-tab", "tab"]);
    expect(moveKey(s, "esc", -1)).toBe(s);
    expect(moveKey(s, "enter", 1)).toBe(s);
  });
});

describe("the way the screen connected last", () => {
  it("is kept for each pairing on its own, and read as nothing when it is not kept or not known", () => {
    expect(rememberedVia("pairing-a")).toBeNull();
    rememberVia("pairing-a", "relay");
    rememberVia("pairing-b", "tailscale");
    expect(rememberedVia("pairing-a")).toBe("relay");
    expect(rememberedVia("pairing-b")).toBe("tailscale");
    expect(store.get("kelpie.screen.via.pairing-a")).toBe("relay");
    store.set(viaKey("pairing-c"), "webrtc");
    expect(rememberedVia("pairing-c")).toBeNull();
  });
});
