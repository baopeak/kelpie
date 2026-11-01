import { describe, expect, it } from "vitest";
import type { MobileStatus } from "../components/settings/mobile-settings";
import { LOCK_API, TAILSCALE_API } from "./attention";
import { translated } from "./i18n";
import { mobileSummary, OPTION_IDS, optionState, type OptionFacts, type OptionId } from "./mobile-options";
import { NOTIFY_API } from "./notify-events";
import { emptyPushConfig, type PushState } from "./push-config";
import { catalogEntry, entryMatches } from "./settings-catalog";

const API = LOCK_API;
const RESTART = "Restart Kelpie's background service";

/** A new install with phone access just turned on, as the daemon reports it. */
function status(patch: Partial<MobileStatus> = {}): MobileStatus {
  return {
    enabled: true, running: true, url: "http://192.168.1.5:4817/", urls: ["http://192.168.1.5:4817/"], error: null, store_error: null, ip: null, port: 4817, tls: false, cert: null, key: null, ips: [],
    devices: [], legacy: [], pairing_expires_in: null, pending: [], dormant_days: 90,
    tailscale: { on: false, origin: null },
    lock: { on: true, idle_s: 300, recheck: false, plain_read: true },
    app: true,
    ...patch,
  };
}

const push = (phones = 0): PushState => ({ config: emptyPushConfig(), secrets: { ntfy_topic: false, ntfy_token: false, telegram_token: false, webhook_url: false }, webpush: { devices: phones } });
const facts = (patch: Partial<OptionFacts> = {}): OptionFacts => ({ api: API, status: status(), push: push(), ...patch });
const state = (id: OptionId, patch: Partial<OptionFacts> = {}) => optionState(id, facts(patch));

/** The options the daemon keeps with phone access, and the level that brought each. */
const PHONE_ROWS: [OptionId, number][] = [["mobile-tailscale", TAILSCALE_API], ["mobile-lock", LOCK_API], ["mobile-lock-idle", LOCK_API], ["mobile-lock-recheck", LOCK_API], ["mobile-lock-plain", LOCK_API]];

describe("a new install", () => {
  it("has the safe defaults", () => {
    const on = Object.fromEntries(OPTION_IDS.map((id) => [id, state(id).on]));
    expect(on).toEqual({ "mobile-access": true, "mobile-tailscale": false, "mobile-webpush": false, "mobile-lock": true, "mobile-lock-idle": true, "mobile-lock-recheck": false, "mobile-lock-plain": true, "mobile-ntfy": false, "mobile-telegram": false });
    expect(state("mobile-access", { status: status({ enabled: false }) }).on).toBe(false);
  });

  it("can change every option", () => {
    for (const id of OPTION_IDS) expect(state(id).disabled, id).toBeNull();
  });
});

describe("what a change asks of the person", () => {
  const lock = (patch: Partial<NonNullable<MobileStatus["lock"]>>) => ({ status: status({ lock: { on: true, idle_s: 300, recheck: false, plain_read: true, ...patch } }) });

  it("asks when it takes protection away", () => {
    expect(state("mobile-tailscale").presence).toContain("on asks");
    expect(state("mobile-lock").presence).toContain("off asks");
    expect(state("mobile-lock-idle").presence).toContain("longer time asks");
    expect(state("mobile-lock-recheck", lock({ recheck: true })).presence).toContain("off asks");
    expect(state("mobile-lock-plain", lock({ plain_read: false })).presence).toContain("on asks");
  });

  it("asks nothing when it adds protection", () => {
    expect(state("mobile-tailscale", { status: status({ tailscale: { on: true, origin: null } }) }).presence).toBeNull();
    expect(state("mobile-lock", lock({ on: false })).presence).toBeNull();
    expect(state("mobile-lock-recheck").presence).toBeNull();
    expect(state("mobile-lock-plain").presence).toBeNull();
  });

  it("asks nothing for the channels or for phone access", () => {
    for (const id of ["mobile-access", "mobile-webpush", "mobile-ntfy", "mobile-telegram"] as const) expect(state(id).presence, id).toBeNull();
  });
});

describe("when an option cannot be changed", () => {
  it("says to restart the service when the daemon is older than the option", () => {
    for (const [id, level] of PHONE_ROWS) {
      expect(state(id, { api: level - 1 }).disabled, id).toContain(RESTART);
      expect(state(id, { api: level }).disabled, id).toBeNull();
    }
    for (const id of ["mobile-ntfy", "mobile-telegram"] as const) {
      expect(state(id, { api: NOTIFY_API - 1 }).disabled, id).toContain(RESTART);
      expect(state(id, { api: NOTIFY_API }).disabled, id).toBeNull();
    }
  });

  it("says so too when the daemon's status has no such field, whatever its level says", () => {
    expect(state("mobile-tailscale", { status: status({ tailscale: undefined }) }).disabled).toContain(RESTART);
    expect(state("mobile-lock", { status: status({ lock: undefined }) }).disabled).toContain(RESTART);
    const { webpush: _, ...older } = push();
    expect(state("mobile-webpush", { push: older }).disabled).toContain(RESTART);
  });

  it("asks for phone access first, and leaves the channels alone, which do not need it", () => {
    const off = { status: status({ enabled: false, running: false }) };
    for (const [id] of PHONE_ROWS) expect(state(id, off).disabled, id).toBe("Turn on phone access first.");
    for (const id of ["mobile-access", "mobile-webpush", "mobile-ntfy", "mobile-telegram"] as const) expect(state(id, off).disabled, id).toBeNull();
  });

  it("sends a store it could not read to the reset first", () => {
    const broken = { status: status({ enabled: false, running: false, store_error: "bad file" }) };
    for (const [id] of PHONE_ROWS) expect(state(id, broken).disabled, id).toContain("reset phone access");
    expect(state("mobile-access", broken).disabled).toBeNull();
  });

  it("does not offer what only the installed app may change", () => {
    const window = { status: status({ app: false }) };
    for (const [id] of PHONE_ROWS) expect(state(id, window).disabled, id).toContain("not the installed Kelpie app");
    for (const id of ["mobile-ntfy", "mobile-telegram"] as const) expect(state(id, window).disabled, id).toBeNull();
  });

  it("does not offer what the lock's being off makes meaningless, and says why", () => {
    const off = { status: status({ lock: { on: false, idle_s: 300, recheck: false, plain_read: true } }) };
    expect(state("mobile-lock", off).disabled).toBeNull();
    for (const id of ["mobile-lock-idle", "mobile-lock-recheck"] as const) expect(state(id, off).disabled, id).toBe("Turn on the app lock first.");
    expect(state("mobile-lock-plain", off).disabled).toContain("reads anyway");
  });

  it("does not offer a channel whose settings could not be read, and never shows it as on", () => {
    for (const id of ["mobile-ntfy", "mobile-telegram"] as const) {
      expect(state(id, { push: null }).disabled, id).toContain("could not read the notification settings");
      expect(state(id, { push: null }).on, id).toBe(false);
    }
  });
});

describe("what the switches show", () => {
  it("follows the settings the daemon holds", () => {
    const p = push(2);
    p.config.ntfy.enabled = true;
    expect(state("mobile-webpush", { push: p }).on).toBe(true);
    expect(state("mobile-ntfy", { push: p }).on).toBe(true);
    expect(state("mobile-telegram", { push: p }).on).toBe(false);
  });
});

describe("the wording", () => {
  const lines = (id: OptionId) => {
    const e = catalogEntry(id)!;
    return [e.label, e.description, e.tradeoff, e.safety, e.phones].filter((l): l is string => !!l);
  };

  it.each(OPTION_IDS)("%s says what it does, what it costs and how safe it is, in English and Vietnamese", (id) => {
    const e = catalogEntry(id)!;
    expect(e.section).toBe("mobile");
    expect(e.description && e.tradeoff && e.safety).toBeTruthy();
    for (const line of lines(id)) expect(translated(line), line).toBeTruthy();
  });

  it.each(OPTION_IDS)("%s uses no word a person should not need to know", (id) => {
    for (const line of lines(id)) {
      for (const text of [line, translated(line)!]) expect(text).not.toMatch(/vapid|webauthn|rp id|noise|ikpsk/i);
    }
  });

  it("is found by the page's search, every line of it and in both languages", () => {
    const e = catalogEntry("mobile-tailscale")!;
    expect(entryMatches(e, "certificate logs")).toBe(true);
    expect(entryMatches(e, "one user account")).toBe(true);
    expect(entryMatches(e, "paired again")).toBe(true);
    const vi = translated(e.tradeoff!)!.split(" ").slice(0, 3).join(" ");
    expect(entryMatches(e, vi)).toBe(true);
    expect(entryMatches(catalogEntry("mobile-ntfy")!, "128 random")).toBe(true);
  });
});

describe("the summary", () => {
  const summary = (s: Partial<MobileStatus> = {}, p: PushState | null = push()) => mobileSummary(status(s), p);

  it("says phones cannot reach a computer with phone access off", () => {
    expect(summary({ enabled: false, running: false }).reach).toContain("phone access is off");
    expect(summary({ running: false }).reach).toContain("not running");
  });

  it("says how a phone gets here", () => {
    expect(summary().reach).toBe("Phones on this network connect over plain HTTP, and can only read.");
    expect(summary({ tls: true }).reach).toContain("over HTTPS");
    expect(summary({ lock: { on: true, idle_s: 300, recheck: false, plain_read: false } }).reach).toContain("see nothing");
    expect(summary({ lock: { on: false, idle_s: 300, recheck: false, plain_read: true } }).reach).toContain("unless the address is a Tailscale one");
  });

  it("names the Tailscale address when phones can use it, and says when they cannot", () => {
    expect(summary({ tailscale: { on: true, origin: "pc.tail1.ts.net:8443" } }).reach).toContain("pc.tail1.ts.net:8443");
    expect(summary({ tailscale: { on: true, origin: "pc.tail1.ts.net:8443" }, urls: [] }).reach).not.toContain("this network");
    expect(summary({ tailscale: { on: true, origin: null } }).reach).toContain("cannot use it right now");
  });

  it("says whether the lock is on, and after how long", () => {
    expect(summary().lock).toContain("on");
    expect(summary().lock).toContain("5 min");
    expect(summary({ lock: { on: false, idle_s: 300, recheck: false, plain_read: true } }).lock).toContain("off");
    expect(summary({ lock: undefined }).lock).toContain("Restart");
  });

  it("says where alerts go, with what Settings → Notifications says", () => {
    expect(summary().alerts).toBe("Nothing is sent off this computer.");
    expect(summary({}, push(1)).alerts).toBe("Sending to 1 phone.");
    expect(summary({}, null).alerts).toBeNull();
  });

  it("counts the phones, those to pair again as well", () => {
    const device = { id: "a", name: "A", permission: "read", kind: "pwa", can_write: false, created_at: 0, last_seen: 0, state: "ok" } as const;
    expect(summary().phones).toBe("No phone is paired.");
    expect(summary({ devices: [device] }).phones).toBe("1 phone is paired.");
    expect(summary({ devices: [device], legacy: [{ id: "b", name: "B", created_at: 0 }] }).phones).toBe("2 phones are paired.");
  });
});
