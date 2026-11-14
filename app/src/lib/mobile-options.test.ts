import { describe, expect, it } from "vitest";
import type { MobileDevice, MobileStatus } from "../components/settings/mobile-settings";
import type { RelayStatus } from "../components/settings/mobile-relay-settings";
import { HOSTED_API, LOCK_API, RELAY_API, SCREEN_P2P_API, SCREEN_REMOTE_API, SCREEN_TURN_API, TAILSCALE_API, TELEGRAM_API } from "./attention";
import { translated } from "./i18n";
import {
  alreadyRemoved, approvalChoices, approveWithReplace, asksForKey, connectionLine, deviceMethodLabel, groupDevices, hostedLine, hostedPairing, hostedReason, hostedTrust, joinGroup, mobileSummary, movingRelayWarning, offerNewRoom,
  OPTION_IDS, optionState, pageKindLabel, pairMethodOf, pairMethods, permissionChoices, permissionLabel, plainPairing, prunePhoneGroups, relayLabel, relayServesApp, routeLabel, samePhoneSuggestion, screenDoubtedNote, screenMethodRows, turnUsageLine, screenPolicyLine, screenRemoteShown, screenUsageLine,
  telegramChoice, turnOnAccess, withConnection, withNewerConnections, type AccessCalls, type ConnectionNews, type MobileRoute, type ApprovalCalls, type OptionFacts, type OptionId, type PairMethod, type PairMethodId,
} from "./mobile-options";
import { NOTIFY_API } from "./notify-events";
import { emptyPushConfig, type PushState } from "./push-config";
import { catalogEntry, entryMatches } from "./settings-catalog";

const API = SCREEN_REMOTE_API;
const RESTART = "Restart Kelpie's background service";

/** A new install with phone access just turned on, as the daemon reports it. */
function status(patch: Partial<MobileStatus> = {}): MobileStatus {
  return {
    enabled: true, running: true, url: "http://192.168.1.5:4817/", urls: ["http://192.168.1.5:4817/"], error: null, store_error: null, ip: null, port: 4817, tls: false, cert: null, key: null, ips: [],
    devices: [], legacy: [], pairing_expires_in: null, pending: [], dormant_days: 90,
    tailscale: { on: false, origin: null },
    lock: { on: true, idle_s: 300, recheck: false, plain_read: true },
    screen: { remote: false, methods: { tailscale: true, p2p: true, relay_turn: true, relay_ws: true }, relay_ws_kbps: 500, relay_ws_month_mb: 1024, relay_ws_used_mb: 0, doubted: false },
    relay: {
      on: false, url: "wss://relay.example", default_url: "wss://relay.example", room: false, origin: null, operator_key: false, connected: false, error: null, rtt_ms: null, slow: false, phones: 0,
      version: null, hosted: { available: false, origin: "https://relay.example", version: null, reason: "not checked yet", install_qr: "<svg/>" }, hosted_devices: 0,
    },
    lan: true, new_store: false,
    app: true,
    ...patch,
  };
}

/** The relay on and connected, at a relay of the second version found serving the phone app. */
const hostedRelay = (patch: Partial<RelayStatus> = {}): RelayStatus => ({
  ...status().relay!, on: true, room: true, connected: true, version: 2, hosted: { available: true, origin: "https://relay.example", version: "0.1.16", reason: null, install_qr: "<svg/>" }, ...patch,
});

const device = (patch: Partial<MobileDevice> = {}): MobileDevice => ({ id: "a", name: "A", permission: "read", kind: "pwa", can_write: false, created_at: 0, last_seen: 0, state: "ok", ...patch });

const push = (phones = 0): PushState => ({ config: emptyPushConfig(), secrets: { ntfy_topic: false, ntfy_token: false, telegram_token: false, webhook_url: false }, webpush: { devices: phones } });
const facts = (patch: Partial<OptionFacts> = {}): OptionFacts => ({ api: API, status: status(), push: push(), ...patch });
const state = (id: OptionId, patch: Partial<OptionFacts> = {}) => optionState(id, facts(patch));

/** The options the daemon keeps with phone access, and the level that brought each. */
const PHONE_ROWS: [OptionId, number][] = [["mobile-tailscale", TAILSCALE_API], ["mobile-relay", RELAY_API], ["mobile-lock", LOCK_API], ["mobile-lock-idle", LOCK_API], ["mobile-lock-recheck", LOCK_API], ["mobile-lock-plain", LOCK_API], ["mobile-screen-remote", SCREEN_REMOTE_API]];

describe("a new install", () => {
  it("has the safe defaults", () => {
    const on = Object.fromEntries(OPTION_IDS.map((id) => [id, state(id).on]));
    expect(on).toEqual({ "mobile-access": true, "mobile-hosted": false, "mobile-tailscale": false, "mobile-relay": false, "mobile-webpush": false, "mobile-lock": true, "mobile-lock-idle": true, "mobile-lock-recheck": false, "mobile-lock-plain": true, "mobile-screen-remote": false, "mobile-ntfy": false, "mobile-telegram": false, "mobile-lan": true });
    expect(state("mobile-access", { status: status({ enabled: false }) }).on).toBe(false);
  });

  it("can change every option, pairs on the hosted app once the relay is on, and keeps this network's listeners while they are the only way in", () => {
    for (const id of OPTION_IDS) if (id !== "mobile-hosted" && id !== "mobile-lan") expect(state(id).disabled, id).toBeNull();
    expect(state("mobile-hosted").disabled).toBe("Turn on the relay first: the phone app comes from it.");
    expect(state("mobile-lan").disabled).toContain("nothing else would reach this computer");
  });
});

describe("what a change asks of the person", () => {
  const lock = (patch: Partial<NonNullable<MobileStatus["lock"]>>) => ({ status: status({ lock: { on: true, idle_s: 300, recheck: false, plain_read: true, ...patch } }) });

  it("asks when it takes protection away", () => {
    expect(state("mobile-tailscale").presence).toContain("on asks");
    expect(state("mobile-relay").presence).toContain("on asks");
    expect(state("mobile-lock").presence).toContain("off asks");
    expect(state("mobile-lock-idle").presence).toContain("longer time asks");
    expect(state("mobile-lock-recheck", lock({ recheck: true })).presence).toContain("off asks");
    expect(state("mobile-lock-plain", lock({ plain_read: false })).presence).toContain("on asks");
  });

  it("asks nothing when it adds protection", () => {
    expect(state("mobile-tailscale", { status: status({ tailscale: { on: true, origin: null } }) }).presence).toBeNull();
    expect(state("mobile-relay", { status: status({ relay: { ...status().relay!, on: true } }) }).presence).toBeNull();
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
    expect(state("mobile-screen-remote", { status: status({ screen: undefined }) }).disabled).toContain(RESTART);
    expect(state("mobile-relay", { status: status({ relay: undefined }) }).disabled).toContain(RESTART);
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
    const relay = status().relay!;
    expect(summary({ relay: { ...relay, on: true, connected: true } }).reach).toContain("through the relay");
    expect(summary({ relay: { ...relay, on: true, connected: false } }).reach).toContain("not connected to it right now");
    expect(summary().reach).not.toContain("relay");
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

describe("what Telegram carries", () => {
  const telegram = (patch: Partial<PushState["config"]["telegram"]> = {}, token = true): PushState => {
    const p = push();
    p.config.telegram = { ...p.config.telegram, enabled: true, chat_id: "42", ...patch };
    p.secrets.telegram_token = token;
    p.telegram = { approving: false, polling: false, binding_expires_in: null, error: null, open: 0 };
    return p;
  };

  it("is offered only with Telegram on and a daemon that has the choice", () => {
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram() }))).toEqual({ mode: "alert_link", approveBlocked: null });
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram({ enabled: false }) }))).toBeNull();
    expect(telegramChoice(facts({ api: TELEGRAM_API - 1, push: telegram() }))).toBeNull();
    const old = telegram();
    delete old.telegram;
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: old }))).toBeNull();
  });

  it("reads a setting from before the choice as the alert it always was", () => {
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram({ mode: undefined }) }))?.mode).toBe("alert");
  });

  it("turns approvals on only from the installed app, with a bot token", () => {
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram(), status: status({ app: false }) }))?.approveBlocked).toContain("not the installed Kelpie app");
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram({}, false) }))?.approveBlocked).toBe("Telegram needs a bot token.");
    expect(telegramChoice(facts({ api: TELEGRAM_API, push: telegram(), status: null }))?.approveBlocked).toContain("not the installed Kelpie app");
  });
});

describe("a new room after a phone is removed", () => {
  const relay = (patch: Partial<NonNullable<MobileStatus["relay"]>>) => status({ relay: { ...status().relay!, ...patch } });

  it("is offered only while the relay is on, this computer has a room there and keeps the key that makes one", () => {
    expect(offerNewRoom(relay({ on: true, room: true, operator_key: true }), API)).toBe(true);
    expect(offerNewRoom(relay({ on: false, room: true, operator_key: true }), API)).toBe(false);
    expect(offerNewRoom(relay({ on: true, room: false, operator_key: true }), API)).toBe(false);
    expect(offerNewRoom(relay({ on: true, room: true, operator_key: false }), API), "no registration key kept").toBe(false);
    expect(offerNewRoom(status({ relay: undefined }), API)).toBe(false);
    expect(offerNewRoom(null, API)).toBe(false);
  });

  it("needs no key kept at a relay of the second version, from a daemon that makes rooms without one", () => {
    expect(offerNewRoom(relay({ on: true, room: true, operator_key: false, version: 2 }), HOSTED_API)).toBe(true);
    expect(offerNewRoom(relay({ on: true, room: true, operator_key: false, version: 2 }), HOSTED_API - 1), "an older daemon still sends a key").toBe(false);
    expect(offerNewRoom(relay({ on: true, room: true, operator_key: false, version: 1 }), HOSTED_API), "a relay of the first version wants its key").toBe(false);
  });

  it("says plainly what it costs, in English and Vietnamese", () => {
    const line = "Phones connected now move to the new room by themselves, over their encrypted link. A phone not connected now cannot use the relay until it connects once over Tailscale, and may need to be paired again. Making a room asks you to verify.";
    expect(translated(line)).toBeTruthy();
    expect(translated("Not now")).toBeTruthy();
  });
});

describe("turning phone access on", () => {
  const NOTHING_ELSE = "Turn on the relay or Tailscale: nothing else reaches this computer";
  /** Records every call; `enable` and `relay` say how the daemon answers, `after` what its status says after `mobile.enable`. */
  function daemon(answers: { enable?: Error; relay?: Error; after?: Partial<MobileStatus> | null } = {}) {
    const calls: [string, Record<string, unknown>][] = [];
    const call: AccessCalls = {
      daemon: async (method, params) => {
        calls.push([method, params]);
        if (answers.enable) throw answers.enable;
      },
      secure: async (method, params) => {
        calls.push([method, params]);
        if (answers.relay) throw answers.relay;
      },
      status: async () => (answers.after === null ? null : status({ enabled: true, running: false, ...answers.after })),
    };
    return { calls, call };
  }
  const fresh = (patch: Partial<OptionFacts> = {}) => facts({ status: status({ enabled: false, running: false, new_store: true, lan: true }), ...patch });

  it("sets up a new store with no listener on this network and the relay on, then opens the guide", async () => {
    const d = daemon({ enable: new Error(NOTHING_ELSE) });
    expect(await turnOnAccess(fresh(), d.call)).toBe("hosted");
    expect(d.calls).toEqual([["mobile.enable", { lan: false }], ["mobile.set_relay", { on: true }]]);
    const quiet = daemon();
    expect(await turnOnAccess(fresh(), quiet.call)).toBe("hosted");
    expect(quiet.calls).toEqual([["mobile.enable", { lan: false }], ["mobile.set_relay", { on: true }]]);
  });

  it("leaves an existing store as the switch always did: phone access on, nothing else asked", async () => {
    for (const f of [facts({ status: status({ enabled: false, new_store: false }) }), facts({ status: status({ enabled: false, new_store: undefined, lan: undefined }) }), fresh({ api: HOSTED_API - 1 }), fresh({ status: null })]) {
      const d = daemon();
      expect(await turnOnAccess(f, d.call)).toBe("plain");
      expect(d.calls).toEqual([["mobile.enable", {}]]);
    }
  });

  it("says why when the relay makes no room, after phone access is on", async () => {
    const d = daemon({ enable: new Error(NOTHING_ELSE), relay: new Error("This network made too many rooms today: try again tomorrow, or use Tailscale") });
    await expect(turnOnAccess(fresh(), d.call)).rejects.toThrow("too many rooms");
    expect(d.calls.map(([m]) => m)).toEqual(["mobile.enable", "mobile.set_relay"]);
  });

  it("stops at phone access when it did not turn on", async () => {
    for (const after of [{ enabled: false }, null]) {
      const d = daemon({ enable: new Error("the credential store did not keep the key"), after });
      await expect(turnOnAccess(fresh(), d.call)).rejects.toThrow("credential store");
      expect(d.calls).toEqual([["mobile.enable", { lan: false }]]);
    }
  });
});

describe("the hosted app's rows", () => {
  const on = (patch: Partial<RelayStatus> = {}) => facts({ status: status({ relay: hostedRelay(patch) }) });

  it("say to restart the service below the level that has the hosted app, and offer no pairing there", () => {
    for (const id of ["mobile-hosted", "mobile-lan"] as const) {
      expect(state(id, { ...on(), api: HOSTED_API - 1 }).disabled, id).toContain(RESTART);
      expect(state(id, { status: status({ relay: { ...hostedRelay(), hosted: undefined }, lan: undefined }) }).disabled, id).toContain(RESTART);
    }
    expect(hostedPairing({ ...on(), api: HOSTED_API - 1 })).toBe(false);
    expect(hostedPairing(on())).toBe(true);
    expect(state("mobile-hosted", on())).toEqual({ on: true, disabled: null, presence: null });
  });

  it("offer pairing on the hosted app only while the relay serves it", () => {
    expect(hostedPairing(on({ on: false }))).toBe(false);
    expect(hostedPairing(on({ hosted: { ...hostedRelay().hosted!, available: false, reason: "not checked yet" } }))).toBe(false);
    expect(hostedPairing(facts({ status: status({ enabled: false, relay: hostedRelay() }) }))).toBe(false);
    expect(hostedPairing(facts({ status: status({ store_error: "bad", relay: hostedRelay() }) }))).toBe(false);
    expect(state("mobile-hosted", on({ hosted: { ...hostedRelay().hosted!, available: false, reason: "not checked yet" } })).disabled).toContain("checking");
    const refused = on({ hosted: { ...hostedRelay().hosted!, available: false, reason: "this relay is of an older version, which cannot let the hosted app in" } });
    expect(state("mobile-hosted", refused).disabled).toBe("This relay does not serve the phone app: pair phones over Tailscale.");
    expect(hostedReason(refused.status!.relay!)).toContain("older version");
    expect(hostedReason(hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false, reason: "not checked yet" } }))).toBeNull();
  });

  it("offer pairing a browser on this network only while its listeners are on", () => {
    expect(plainPairing(facts({ status: status({ lan: false }) }))).toBe(false);
    expect(plainPairing(facts({ status: status({ lan: true }) }))).toBe(true);
    expect(plainPairing(facts({ status: status({ lan: undefined }) })), "a daemon without the choice listens").toBe(true);
  });

  it("turn this network's listeners off only while something else reaches this computer", () => {
    expect(state("mobile-lan").disabled).toContain("Turn on the relay or Tailscale first");
    expect(state("mobile-lan", on()).disabled).toBeNull();
    expect(state("mobile-lan", facts({ status: status({ tailscale: { on: true, origin: null } }) })).disabled).toBeNull();
    expect(state("mobile-lan", facts({ status: status({ lan: false }) }))).toEqual({ on: false, disabled: null, presence: null });
    expect(state("mobile-lan", facts({ status: status({ enabled: false, running: false }) })).disabled).toBe("Turn on phone access first.");
  });

  it("show the app's origin and version on the relay's row, or why it is not served", () => {
    expect(hostedLine(hostedRelay())).toBe("Phone app at https://relay.example, version 0.1.16.");
    expect(hostedLine(status().relay!)).toContain("checking");
    expect(hostedLine(hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false, reason: "only a wss:// relay serves the phone app" } }))).toContain("pair phones over Tailscale");
    expect(hostedLine({ ...hostedRelay(), hosted: undefined })).toBeNull();
    expect(relayServesApp(hostedRelay())).toBe(true);
    expect(relayServesApp(hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false }, hosted_devices: 1 }))).toBe(true);
    expect(relayServesApp(status().relay!)).toBe(false);
  });

  it("show the registration key only once the relay asked for it", () => {
    expect(asksForKey(new Error("This relay needs a registration key"))).toBe(true);
    expect(asksForKey("the relay did not take that registration key")).toBe(true);
    expect(asksForKey("The relay is not taking new rooms today")).toBe(false);
  });
});

describe("what the hosted app trusts", () => {
  it("says who could change the app, what the fingerprint shows and what it cannot, in English and Vietnamese", () => {
    const text = hostedTrust(hostedRelay())!;
    expect(text).toContain("The phone app comes from Kelpie's relay at relay.example, on Cloudflare.");
    expect(text).toContain("fingerprint");
    expect(text).toContain("nobody can check what one phone received");
    expect(text).toContain("use Tailscale mode");
    expect(text).not.toMatch(/verified|guarantee/i);
    const other = hostedTrust(hostedRelay({ url: "wss://mine.example" }))!;
    for (const t of [text, other]) expect(t, "typing starts agents too, bypass mode included").toMatch(/start agents in your projects with this computer's settings, bypass mode included/);
    expect(other).toContain("The phone app comes from the relay at relay.example, not from Kelpie's.");
    expect(other).toContain("not this one");
    expect(other).not.toMatch(/verified|guarantee/i);
    expect(hostedTrust({ ...hostedRelay(), hosted: { ...hostedRelay().hosted!, origin: null } })).toBeNull();
  });

  it("is in Vietnamese too, with no word of overclaiming in either", () => {
    for (const r of [hostedRelay(), hostedRelay({ url: "wss://mine.example" })]) {
      const en = hostedTrust(r)!.replace("relay.example", "{host}");
      expect(translated(en), en).toBeTruthy();
      expect(translated(en)).not.toMatch(/xác minh|đảm bảo/i);
    }
  });
});

describe("approving a phone", () => {
  it("offers a phone on the hosted app reading first, typing with what it means, and never control", () => {
    for (const want of ["read", "write", "control"] as const) {
      const c = approvalChoices({ want, origin_kind: "hosted" });
      expect(c.choices).toEqual(["read", "write"]);
      expect(c.choices).not.toContain("control");
      expect(c.primary).toBe("read");
      expect(c.typing).toBe("Typing from the hosted app lets code served by Kelpie's relay type into your sessions and start agents in your projects with this computer's settings, bypass mode included, while the phone is unlocked.");
      expect(translated(c.typing!)).toBeTruthy();
    }
    expect(approvalChoices({ want: "control", origin_kind: "hosted" }).asks).toBe("write");
  });

  it("leaves other pages as they were: what the phone asks for is the default", () => {
    expect(approvalChoices({ want: "write", origin_kind: "ts" })).toEqual({ choices: ["read", "write"], primary: "write", asks: "write", typing: null });
    expect(approvalChoices({ want: "read" })).toEqual({ choices: ["read", "write"], primary: "read", asks: "read", typing: null });
  });

  it("names the page it pairs on", () => {
    expect([pageKindLabel("hosted"), pageKindLabel("ts"), pageKindLabel("own"), pageKindLabel("plain"), pageKindLabel(null), pageKindLabel(undefined)]).toEqual(["hosted app", "Tailscale", "this computer's HTTPS", "this network", null, null]);
    for (const k of ["hosted", "ts", "own", "plain"] as const) expect(translated(pageKindLabel(k)!), k).toBeTruthy();
  });
});

describe("the screen without Tailscale", () => {
  const screen = (patch: Partial<NonNullable<MobileStatus["screen"]>>) => ({ status: status({ screen: { ...status().screen!, ...patch } }) });

  it("is off by default, asks for verification to turn on and nothing to turn off", () => {
    expect(state("mobile-screen-remote").on).toBe(false);
    expect(state("mobile-screen-remote").presence).toBe("Turning this on asks you to verify first.");
    expect(state("mobile-screen-remote", screen({ remote: true })).on).toBe(true);
    expect(state("mobile-screen-remote", screen({ remote: true })).presence).toBeNull();
  });

  it("is shown only from a daemon that has it", () => {
    expect(screenRemoteShown(SCREEN_REMOTE_API)).toBe(true);
    expect(screenRemoteShown(SCREEN_REMOTE_API - 1)).toBe(false);
  });

  it("says when the daemon could not trust what it stored, and says nothing from an older one", () => {
    expect(screenDoubtedNote(facts())).toBeNull();
    expect(screenDoubtedNote(facts(screen({ doubted: true })))).toContain("failed its check");
    expect(screenDoubtedNote(facts({ ...screen({ doubted: true }), api: SCREEN_REMOTE_API - 1 }))).toBeNull();
  });

  it("lists the ways in order, as they are set, and says which are not built yet", () => {
    const rows = screenMethodRows(facts(screen({ methods: { tailscale: true, p2p: false, relay_turn: true, relay_ws: false } })));
    expect(rows.map((r) => [r.id, r.label, r.on])).toEqual([["tailscale", "Tailscale", true], ["p2p", "Direct link (P2P)", false], ["relay_turn", "Relay (TURN)", true], ["relay_ws", "Relay WebSocket", false]]);
    expect(rows.find((r) => r.id === "p2p")!.note, "a computer before peer to peer").toContain("later update");
    expect(rows.find((r) => r.id === "relay_turn")!.note).toContain("later update");
    const p2p = screenMethodRows({ ...facts(screen({})), api: SCREEN_P2P_API }).find((r) => r.id === "p2p")!;
    expect(p2p.note).toContain("network addresses");
    const turn = (patch: Partial<NonNullable<MobileStatus["screen"]>>) => screenMethodRows({ ...facts(screen(patch)), api: SCREEN_TURN_API }).find((r) => r.id === "relay_turn")!.note;
    expect(turn({})).toContain("operator pays");
    expect(turn({ turn: "turn_not_configured" })).toContain("offers no TURN");
    expect(turnUsageLine(status({ screen: { ...status().screen!, turn_session_mb: 1024, turn_day_mb: 3072, turn_month_mb: 30720, turn_used_day_mb: 12.4, turn_used_month_mb: 900 } }).screen)).toBe("TURN: 12 MB of 3072 MB today, 900 MB of 30720 MB this month, at most 1024 MB a screen");
    expect(turnUsageLine(status().screen), "a daemon before TURN").toBeNull();
    expect(rows.find((r) => r.id === "tailscale")!.note).toBeNull();
    expect(rows.every((r) => r.disabled === null)).toBe(true);
  });

  it("says the relay WebSocket counts only while the switch is on, and blocks the ways like the switch", () => {
    expect(screenMethodRows(facts()).find((r) => r.id === "relay_ws")!.note).toContain("only while");
    expect(screenMethodRows(facts(screen({ remote: true }))).find((r) => r.id === "relay_ws")!.note).toBeNull();
    for (const r of screenMethodRows(facts({ api: SCREEN_REMOTE_API - 1 }))) expect(r.disabled).toContain(RESTART);
    for (const r of screenMethodRows(facts({ status: status({ app: false }) }))) expect(r.disabled).toContain("not the installed Kelpie app");
  });

  it("shows the relay WebSocket's use this month", () => {
    expect(screenUsageLine({ ...status().screen!, relay_ws_used_mb: 120.4, relay_ws_month_mb: 1024 })).toBe("120 MB of 1024 MB this month");
    expect(screenUsageLine(undefined)).toBeNull();
  });

  it("is found by search in English and Vietnamese", () => {
    const e = catalogEntry("mobile-screen-remote")!;
    for (const q of ["screen", "without tailscale", "face id", "relay"]) expect(entryMatches(e, q), q).toBe(true);
    expect(entryMatches(catalogEntry("mobile-screen-ways")!, "websocket")).toBe(true);
    expect(e.api).toBe(SCREEN_REMOTE_API);
    expect(catalogEntry("mobile-screen-ways")!.api).toBe(SCREEN_REMOTE_API);
  });

  it("says what the relay way of pairing allows once it is on", () => {
    expect(screenPolicyLine("relay", true)).toContain("Face ID");
    expect(screenPolicyLine("relay", false)).toContain("Never the screen");
    const way = (patch: Partial<OptionFacts>) => pairMethods(facts({ status: status({ relay: hostedRelay() }), ...patch })).find((m) => m.id === "relay")!.allows;
    expect(way({})).toContain("Never the screen");
    expect(way(screen({ remote: true }))).toContain("Face ID");
    expect(way({ ...screen({ remote: true }), api: SCREEN_REMOTE_API - 1 })).toContain("Never the screen");
  });
});

describe("the paired devices", () => {
  it("offer control to a phone on the hosted app only while the screen without Tailscale is on, and call it screen only", () => {
    const hosted = device({ page_kind: "hosted", page: "https://relay.example", secure_origin: true, may_control: true });
    expect(permissionChoices(hosted, API, true)).toEqual(["read", "write", "control"]);
    expect(permissionChoices(hosted, API, false)).toEqual(["read", "write"]);
    expect(permissionChoices(hosted, SCREEN_REMOTE_API - 1, true)).toEqual(["read", "write"]);
    expect(permissionChoices(device({ page_kind: "hosted", may_control: false }), API, true)).toEqual(["read", "write"]);
    expect(permissionChoices(device({ page_kind: "ts", secure_origin: true, may_control: true }), API, true)).toEqual(["read", "write", "control"]);
    expect(permissionLabel("control", hosted, true)).toBe("May take control (screen only)");
    expect(permissionLabel("control", hosted, false)).toBe("May take control");
    expect(permissionLabel("control", device({ page_kind: "ts" }), true)).toBe("May take control");
    expect([permissionLabel("read", hosted, true), permissionLabel("write", hosted, true)]).toEqual(["May read", "May type"]);
  });

  it("never offer control to a phone on the hosted app", () => {
    expect(permissionChoices(device({ page_kind: "hosted", page: "https://relay.example", secure_origin: true, may_control: true }), API)).toEqual(["read", "write"]);
    expect(permissionChoices(device({ page_kind: "ts", secure_origin: true, may_control: true }), API)).toEqual(["read", "write", "control"]);
    expect(permissionChoices(device({ kind: "app" }), API)).toEqual(["read", "write", "control"]);
    expect(permissionChoices(device({ may_control: false }), API)).toEqual(["read", "write"]);
  });

  it("say the lock holds for phones on the hosted app with the lock off", () => {
    const off = { on: false, idle_s: 300, recheck: false, plain_read: true };
    expect(mobileSummary(status({ lock: off, devices: [device({ page_kind: "hosted" })] }), null).lock).toContain("Phones on the hosted app still ask for Face ID");
    expect(mobileSummary(status({ lock: off, devices: [device({ page_kind: "ts" })] }), null).lock).toBe("The app lock is off: whoever holds a paired phone reaches what it may do.");
  });
});

describe("moving the relay", () => {
  const relay = (n: number) => hostedRelay({ on: false, hosted_devices: n });

  it("warns when phones use the app at the present address", () => {
    expect(movingRelayWarning(relay(2), "wss://other.example")).toBe("2 phones using the app at https://relay.example lose their way to this computer and must be paired again.");
    expect(movingRelayWarning(relay(1), "wss://other.example")).toBe("1 phone using the app at https://relay.example loses its way to this computer and must be paired again.");
    for (const line of ["{n} phones using the app at {origin} lose their way to this computer and must be paired again.", "1 phone using the app at {origin} loses its way to this computer and must be paired again."]) expect(translated(line), line).toBeTruthy();
  });

  it("says nothing when no phone uses it, or the address stays", () => {
    expect(movingRelayWarning(relay(0), "wss://other.example")).toBeNull();
    expect(movingRelayWarning(relay(2), "wss://relay.example/ ")).toBeNull();
    expect(movingRelayWarning({ ...relay(2), hosted_devices: undefined }, "wss://other.example"), "a daemon without the count").toBeNull();
    expect(movingRelayWarning(undefined, "wss://other.example")).toBeNull();
  });
});

describe("the summary of a computer with the hosted app", () => {
  it("names the hosted app as a way in", () => {
    const reach = mobileSummary(status({ urls: [], lan: false, relay: hostedRelay() }), null).reach;
    expect(reach).toBe("Phones with the hosted app from https://relay.example reach this computer through the relay, from anywhere.");
    expect(reach, "no app installed over Tailscale to speak of").not.toContain("installed app");
    expect(mobileSummary(status({ urls: [], relay: hostedRelay({ origin: "https://pc.tail1.ts.net:8443" }) }), null).reach).toContain("With Tailscale off, the installed app");
    expect(mobileSummary(status({ urls: [], relay: { ...hostedRelay(), hosted: undefined } }), null).reach, "an older daemon").toContain("With Tailscale off, the installed app");
    expect(mobileSummary(status({ urls: [], relay: hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false } }) }), null).reach).not.toContain("hosted app");
    expect(mobileSummary(status({ urls: [], relay: hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false }, hosted_devices: 1 }) }), null).reach, "phones paired on it still come").toContain("hosted app");
  });

  it("is found by the page's search: hosted, trust, fingerprint", () => {
    const e = catalogEntry("mobile-hosted")!;
    for (const q of ["hosted", "trust", "fingerprint", "cloudflare"]) expect(entryMatches(e, q), q).toBe(true);
    expect(entryMatches(catalogEntry("mobile-lan")!, "network listen")).toBe(true);
    const vi = translated(e.safety!)!.split(" ").slice(0, 4).join(" ");
    expect(entryMatches(e, vi)).toBe(true);
  });
});

describe("choosing how a phone pairs", () => {
  const ways = (f: Partial<OptionFacts> = {}) => Object.fromEntries(pairMethods(facts(f)).map((m) => [m.id, m])) as Record<PairMethodId, PairMethod>;
  const ts = { on: true, origin: "pc.tail1.ts.net:8443" };
  const recommended = (f: Partial<OptionFacts> = {}) => pairMethods(facts(f)).filter((m) => m.recommended).map((m) => m.id);

  it("lists Tailscale, the relay and this network, each with what it needs, allows and trusts, in English and Vietnamese", () => {
    const all = pairMethods(facts({ status: status({ tailscale: ts, relay: hostedRelay() }) }));
    expect(all.map((m) => m.id)).toEqual(["tailscale", "relay", "network"]);
    for (const m of all) {
      for (const line of [m.needs, m.allows, m.trusts]) expect(line, m.id).toBeTruthy();
      expect(translated(m.needs), m.needs).toBeTruthy();
      expect(translated(m.trusts), m.trusts).toBeTruthy();
    }
    expect(all.map((m) => m.label)).toEqual(["Tailscale", "Kelpie's relay", "This network"]);
  });

  it("names another relay by its host", () => {
    expect(relayLabel(hostedRelay({ url: "wss://relay.mine.example/x" }))).toBe("Your relay at relay.mine.example");
    expect(relayLabel(hostedRelay())).toBe("Kelpie's relay");
    expect(ways({ status: status({ relay: hostedRelay({ url: "wss://mine.example" }) }) }).relay.trusts).toBe("Whoever runs that relay and deploys its app.");
  });

  it("says what each may do with the screen from one sentence each, and never offers it through the relay", () => {
    const w = ways({ status: status({ tailscale: ts, relay: hostedRelay() }) });
    for (const id of ["tailscale", "relay", "network"] as const) {
      expect(w[id].allows, id).toContain(screenPolicyLine(id));
      expect(translated(screenPolicyLine(id)), id).toBeTruthy();
    }
    expect(screenPolicyLine("relay")).toMatch(/^Never the screen/);
    expect(screenPolicyLine("network")).toContain("never over plain HTTP");
  });

  it("says that a phone which may type starts agents too, wherever it types from", () => {
    const w = ways({ status: status({ tailscale: ts, relay: hostedRelay(), tls: true }) });
    for (const id of ["tailscale", "relay", "network"] as const) expect(w[id].allows, id).toContain("starting agents");
    expect(ways().network.allows, "not over plain HTTP, which only reads").not.toContain("starting agents");
    for (const line of ["Reading, typing, approvals and starting agents, from anywhere.", "Reading, typing, approvals and starting agents, over HTTPS with your certificate."]) expect(translated(line), line).toBeTruthy();
  });

  it("says this network reads only over plain HTTP, and types with a certificate", () => {
    expect(ways().network.allows).toContain("Reading only, over plain HTTP");
    expect(ways().network.trusts).toContain("not to swap the page");
    expect(ways({ status: status({ tls: true }) }).network.allows).toContain("over HTTPS with your certificate");
    expect(ways({ status: status({ tls: true }) }).network.trusts).toContain("your certificate");
  });

  it("offers each way only while it can pair, says why not, and leads to the row that turns it on", () => {
    const w = ways();
    expect(w.tailscale).toMatchObject({ available: false, why: "Tailscale mode is off.", fix: { row: "mobile-tailscale" } });
    expect(w.relay).toMatchObject({ available: false, why: "The relay is off.", fix: { row: "mobile-relay" } });
    expect(w.network).toMatchObject({ available: true, why: null, fix: null });
    const on = ways({ status: status({ tailscale: ts, relay: hostedRelay() }) });
    expect([on.tailscale.available, on.relay.available]).toEqual([true, true]);
  });

  it("says why for each shape of the status", () => {
    expect(ways({ status: status({ tailscale: { on: true, origin: null } }) }).tailscale.why).toContain("cannot use it right now");
    expect(ways({ status: status({ tailscale: ts, running: false }) }).tailscale.why).toContain("not running");
    const checking = ways({ status: status({ relay: hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false, reason: "not checked yet" } }) }) }).relay;
    expect(checking).toMatchObject({ available: false, fix: null });
    expect(checking.why).toContain("checking");
    const refused = ways({ status: status({ relay: hostedRelay({ hosted: { ...hostedRelay().hosted!, available: false, reason: "only a wss:// relay serves the phone app" } }) }) }).relay;
    expect(refused).toMatchObject({ available: false, why: "This relay does not serve the phone app: pair phones over Tailscale.", fix: { row: "mobile-relay" } });
    expect(ways({ status: status({ lan: false, relay: hostedRelay() }) }).network).toMatchObject({ available: false, why: "This network's listeners are off.", fix: { row: "mobile-lan" } });
    expect(ways({ status: status({ running: false }) }).network.why).toContain("not running");
    for (const id of ["tailscale", "relay", "network"] as const) {
      expect(ways({ status: status({ enabled: false, running: false }) })[id], id).toMatchObject({ available: false, why: "Turn on phone access first.", fix: null });
      expect(ways({ status: status({ store_error: "bad" }) })[id].why, id).toContain("reset phone access");
      expect(ways({ status: null })[id].why, id).toContain(RESTART);
    }
    for (const line of ["Tailscale mode is off.", "The relay is off.", "This network's listeners are off.", "Phone access is on, but not running: see the message at the top of this page."]) expect(translated(line), line).toBeTruthy();
  });

  it("asks for a restart from a daemon older than the way", () => {
    expect(ways({ api: TAILSCALE_API - 1, status: status({ tailscale: ts }) }).tailscale.why).toContain(RESTART);
    expect(ways({ api: HOSTED_API - 1, status: status({ relay: hostedRelay() }) }).relay.why).toContain(RESTART);
    expect(ways({ status: status({ tailscale: undefined, relay: { ...hostedRelay(), hosted: undefined } }) }).relay.why, "no hosted app in the status").toContain(RESTART);
    expect(ways({ status: status({ tailscale: undefined }) }).tailscale.why, "no Tailscale in the status").toContain(RESTART);
    expect(ways({ api: HOSTED_API - 1, status: status({ lan: undefined }) }).network.available, "an older daemon always listens").toBe(true);
  });

  it("agrees with the hosted row on when the relay's app pairs", () => {
    for (const f of [facts({ status: status({ relay: hostedRelay() }) }), facts(), facts({ api: HOSTED_API - 1, status: status({ relay: hostedRelay() }) }), facts({ status: status({ enabled: false, relay: hostedRelay() }) })]) {
      expect(pairMethods(f).find((m) => m.id === "relay")!.available).toBe(hostedPairing(f));
    }
  });

  it("recommends Tailscale while phones can use it, else the relay's app while it is served, else this network", () => {
    expect(recommended({ status: status({ tailscale: ts, relay: hostedRelay() }) })).toEqual(["tailscale"]);
    expect(recommended({ status: status({ tailscale: { on: true, origin: null }, relay: hostedRelay() }) })).toEqual(["relay"]);
    expect(recommended({ status: status({ relay: hostedRelay({ url: "wss://mine.example" }) }) })).toEqual(["relay"]);
    expect(recommended()).toEqual(["network"]);
    expect(recommended({ status: status({ lan: false }) })).toEqual(["network"]);
  });
});

describe("one phone paired several ways", () => {
  const a = device({ id: "a", name: "Pixel", page_kind: "ts" });
  const b = device({ id: "b", name: "Pixel", page_kind: "hosted" });
  const c = device({ id: "c", name: "iPad", page_kind: "plain" });

  it("lists the records of one group as one phone, at the place of the first", () => {
    expect(groupDevices([c, a, b], { a: "g", b: "g" })).toEqual([{ group: null, devices: [c] }, { group: "g", devices: [a, b] }]);
    expect(groupDevices([a, c, b], { a: "g", b: "g" })).toEqual([{ group: "g", devices: [a, b] }, { group: null, devices: [c] }]);
  });

  it("leaves ungrouped records, and a group with one record left, as they were", () => {
    expect(groupDevices([a, b, c], {})).toEqual([a, b, c].map((d) => ({ group: null, devices: [d] })));
    expect(groupDevices([a, c], { a: "g", b: "g" })).toEqual([{ group: null, devices: [a] }, { group: null, devices: [c] }]);
  });

  it("suggests the record of the same name paired another way, and nothing else", () => {
    expect(samePhoneSuggestion({ name: " pixel ", origin_kind: "hosted" }, [c, a])).toBe("a");
    expect(samePhoneSuggestion({ name: "Pixel", origin_kind: "ts" }, [a]), "the same way is not another way").toBeNull();
    expect(samePhoneSuggestion({ name: "iPad", origin_kind: "own" }, [c]), "HTTPS and plain HTTP are both this network").toBeNull();
    expect(samePhoneSuggestion({ name: "Galaxy", origin_kind: "hosted" }, [a, c])).toBeNull();
    expect(samePhoneSuggestion({ name: "Pixel", origin_kind: "hosted" }, [device({ ...a, state: "unverified" })]), "a record to pair again").toBeNull();
    expect([pairMethodOf("hosted"), pairMethodOf("ts"), pairMethodOf("own"), pairMethodOf("plain"), pairMethodOf(null)]).toEqual(["relay", "tailscale", "network", "network", null]);
  });

  it("joins a new record to the chosen phone's group, or makes one named after it", () => {
    expect(joinGroup({}, "b", "a")).toEqual({ a: "a", b: "a" });
    expect(joinGroup({ a: "g", x: "g" }, "b", "a")).toEqual({ a: "g", x: "g", b: "g" });
  });

  it("forgets the records no longer paired, and says when nothing changes", () => {
    expect(prunePhoneGroups({ a: "g", b: "g", z: "g" }, ["a", "b", "c"])).toEqual({ a: "g", b: "g" });
    expect(prunePhoneGroups({ a: "g", b: "g" }, ["a", "b"])).toBeNull();
    expect(prunePhoneGroups({}, [])).toBeNull();
  });

  describe("approving a phone as the same phone as a record, and replacing that record", () => {
    /** Records each call in order; `approve` and `revoke` say how the daemon answers, `listed` which records its status lists afterwards. */
    function calls(answers: { approve?: Error; revoke?: Error; listed?: string[] | null; storeError?: boolean } = {}) {
      const seen: string[] = [];
      const call: ApprovalCalls = {
        approve: async () => {
          seen.push("approve");
          if (answers.approve) throw answers.approve;
          return { id: "new" };
        },
        join: (id, same) => void seen.push(`join ${id} ${same}`),
        revoke: async (id) => {
          seen.push(`revoke ${id}`);
          if (answers.revoke) throw answers.revoke;
        },
        status: async () => {
          seen.push("status");
          if (answers.listed === null) throw new Error("the daemon is gone");
          return status({ devices: (answers.listed ?? ["new"]).map((id) => device({ id })), store_error: answers.storeError ? "unreadable" : null });
        },
        keepOnly: (paired) => void seen.push(`keep ${paired.join(",")}`),
      };
      return { seen, call };
    }

    it("approves first, then groups, then removes the old record, then keeps the grouping to what is listed", async () => {
      const d = calls();
      expect(await approveWithReplace({ same: "old", replace: "old" }, d.call)).toEqual({ id: "new", kept: null });
      expect(d.seen).toEqual(["approve", "join new old", "revoke old", "status", "keep new"]);
    });

    it("does nothing more when the approval fails: no group, no removal", async () => {
      const d = calls({ approve: new Error("wrong code") });
      await expect(approveWithReplace({ same: "old", replace: "old" }, d.call)).rejects.toThrow("wrong code");
      expect(d.seen).toEqual(["approve"]);
    });

    it("keeps both records and says why when the removal is refused", async () => {
      const refused = new Error("Removed for now, but this could not be saved: it will come back after a restart unless this is fixed.");
      const d = calls({ revoke: refused, listed: ["new", "old"] });
      expect(await approveWithReplace({ same: "old", replace: "old" }, d.call)).toEqual({ id: "new", kept: { error: refused } });
      expect(d.seen).toEqual(["approve", "join new old", "revoke old", "status", "keep new,old"]);
    });

    it("takes a record gone already as removed: the daemon replaces a record with the same key itself", async () => {
      const d = calls({ revoke: new Error("no such device") });
      expect(await approveWithReplace({ same: "old", replace: "old" }, d.call)).toEqual({ id: "new", kept: null });
      expect(d.seen).toEqual(["approve", "join new old", "revoke old", "status", "keep new"]);
      expect([alreadyRemoved("Error: no such device"), alreadyRemoved(new Error("could not be saved"))]).toEqual([true, false]);
    });

    it("groups without removing anything unless asked, and leaves the grouping be when the list cannot be read", async () => {
      const d = calls();
      await approveWithReplace({ same: "old", replace: null }, d.call);
      expect(d.seen).toEqual(["approve", "join new old", "status", "keep new"]);
      for (const unreadable of [calls({ listed: null }), calls({ storeError: true })]) {
        await approveWithReplace({ same: "old", replace: null }, unreadable.call);
        expect(unreadable.seen).toEqual(["approve", "join new old", "status"]);
      }
      const alone = calls();
      expect(await approveWithReplace({ same: null, replace: null }, alone.call)).toEqual({ id: "new", kept: null });
      expect(alone.seen, "a new device: only the approval").toEqual(["approve"]);
    });
  });

  it("names each record's way and how it is connected now", () => {
    expect([a, b, c, device({ page_kind: "own" }), device({ kind: "app" })].map((d) => deviceMethodLabel(d, hostedRelay()))).toEqual(["Tailscale", "Kelpie's relay", "This network", "This network, over HTTPS", "Kelpie app"]);
    expect(deviceMethodLabel(b, hostedRelay({ url: "wss://mine.example" }))).toBe("Your relay at mine.example");
    const routes = ["relay", "tailscale", "lan", "local"] as const;
    expect(routes.map(routeLabel)).toEqual(["the relay", "Tailscale", "this network", "this computer"]);
    expect(routes.map((route) => connectionLine({ route, since: 0 }))).toEqual(["connected now · through the relay", "connected now · through Tailscale", "connected now · through this network", "connected now · through this computer"]);
    for (const text of [...routes.map((route) => routeLabel(route)!), "connected now · through {route}", "paired {when} · {connection}"]) expect(translated(text), text).toBeTruthy();
  });

  it("changes one row in place when a phone connects, changes its way or goes, and leaves the list be otherwise", () => {
    const list = [device({ id: "a" }), device({ id: "b", connection: { route: "relay", since: 5 } })];
    const on = withConnection(list, "a", "tailscale", 100);
    expect(on.map((d) => d.connection ?? null)).toEqual([{ route: "tailscale", since: 100 }, { route: "relay", since: 5 }]);
    expect(on[1], "the other row is the same object").toBe(list[1]);
    expect(withConnection(on, "b", "lan", 200)[1].connection).toEqual({ route: "lan", since: 200 });
    expect(withConnection(on, "b", null, 200)[1].connection).toBeNull();
    expect(withConnection(list, "b", "relay", 300), "the same way: nothing changes, not even since when").toBe(list);
    expect(withConnection(list, "a", null, 300), "not connected already").toBe(list);
    expect(withConnection(list, "gone", "relay", 300), "a device not listed").toBe(list);
  });

  it("says plainly that a phone is connected when the daemon names a way this app does not know", () => {
    const route = "cloud" as unknown as MobileRoute;
    expect(routeLabel(route)).toBeNull();
    expect(connectionLine({ route, since: 0 })).toBe("connected now");
    expect(translated("connected now")).toBeTruthy();
  });

  it("keeps what a phone's connection said after a full read started, over that read's older reply", () => {
    // The reply was read before "a" went and "b" came; "c" changed before the read started, so the reply already holds it.
    const reply = [device({ id: "a", connection: { route: "tailscale", since: 5 } }), device({ id: "b" }), device({ id: "c", connection: { route: "lan", since: 7 } })];
    const news = new Map<string, ConnectionNews>([
      ["a", { route: null, since: 100, seq: 3 }],
      ["b", { route: "relay", since: 110, seq: 4 }],
      ["c", { route: null, since: 50, seq: 1 }],
      ["gone", { route: "relay", since: 120, seq: 5 }],
    ]);
    const out = withNewerConnections(reply, news, 2);
    expect(out.map((d) => d.connection ?? null)).toEqual([null, { route: "relay", since: 110 }, { route: "lan", since: 7 }]);
    expect(out[2], "a row nothing newer is heard about is the reply's own").toBe(reply[2]);
    expect(withNewerConnections(reply, news, 5), "nothing heard since the read started").toBe(reply);
    const same = new Map<string, ConnectionNews>([["a", { route: "tailscale", since: 200, seq: 9 }]]);
    expect(withNewerConnections(reply, same, 0)[0].connection, "the same way keeps the daemon's since").toEqual({ route: "tailscale", since: 5 });
  });
});