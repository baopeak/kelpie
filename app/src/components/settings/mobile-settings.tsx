// Settings → Mobile: the daemon's web app for phones (crates/kelpied/src/mobile.rs). Off by default. A phone pairs with a
// one-time QR code and end-to-end encryption (kelpie-link); the person approves it here by typing the six digits the phone
// shows, and the daemon then asks the OS to verify them (Windows Hello or the password). Raising what a phone may do, resetting
// phone access, keeping unused phones longer and approvals from Telegram ask for that verification too (`mobileSecure`); what the prompt says is the daemon's.
// The page: a summary, one `OptionRow` per option, and the address, certificate, the shared screen's stop hotkey and paired phones under Advanced.
// Revoke all stays outside Advanced: a lost phone is when it is needed, and it must not take a search to find.
// A new setup (the daemon's `new_store`) turns phone access on with the relay and the phone app it hosts, and this network's listeners off; an existing
// one keeps what it had. One two-step guide pairs a phone on the hosted app or over Tailscale: install from the page's address, then scan the code.
import { For, Show, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../../lib/store";
import { daemon, mobileSecure, onEvent } from "../../lib/ipc";
import { invoke } from "@tauri-apps/api/core";
import { HOSTED_API, LOCK_API, RELAY_API, SCREEN_API, TAILSCALE_API, TAKEOVER_API } from "../../lib/attention";
import { formatRelative, t, errorText } from "../../lib/i18n";
import { approvalChoices, hostedPairing, hostedReason, hostedTrust, mobileSummary, newSetup, offerNewRoom, optionState, pageKindLabel, permissionChoices, plainPairing, statusError, telegramChoice, turnOnAccess, type OptionFacts, type OptionId } from "../../lib/mobile-options";
import { pushChanged, pushRevision, telegramProblems, type PushState, type TelegramMode } from "../../lib/push-config";
import { catalogEntry, entryMatches } from "../../lib/settings-catalog";
import type { MobileDevicePage, MobileHostedFields, MobilePairRequest, MobilePairStart } from "../../lib/types";
import { LoadBar } from "../load-bar";
import { Icon } from "../icons";
import { OptionRow } from "./option-row";
import { createRelayForm, NewRoomOffer, RelayDetails, type RelayStatus } from "./mobile-relay-settings";
import { SettingRow, settingsQuery, Toggle } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

export type MobilePermission = "read" | "write" | "control";

export interface MobileDevice extends MobileDevicePage {
  id: string;
  name: string;
  permission: MobilePermission;
  kind: "app" | "pwa";
  can_write: boolean;
  created_at: number;
  last_seen: number;
  /** It paired on an HTTPS origin. */
  secure_origin?: boolean;
  /** It turned alerts on (absent below `PHONE_API`). */
  push?: boolean;
  /** Its passkey is on record, it may set up a new one now, it is unlocked now (absent below `LOCK_API`). */
  passkey?: boolean;
  lock_reset?: boolean;
  unlocked?: boolean;
  /** Its record may be given control: the native app, or a browser device paired on a secure origin with its app lock set up and on (absent below `TAKEOVER_API`). */
  may_control?: boolean;
  /** `unverified`: its record was changed outside Kelpie (or the key that checks it is gone): it must be paired again.
   *  `unchecked`: the key that checks it cannot be read right now; nothing is concluded, and it is checked again when it can be. */
  state: "ok" | "unverified" | "unchecked";
}

/** `mobile.status`; `lan` and `new_store` absent below `HOSTED_API`. */
export interface MobileStatus extends Partial<MobileHostedFields> {
  enabled: boolean;
  running: boolean;
  url: string | null;
  urls: string[];
  error: string | null;
  /** The settings file could not be read: phone access stays off until it is reset (a copy was kept). */
  store_error: string | null;
  ip: string | null;
  port: number;
  tls: boolean;
  cert: string | null;
  key: string | null;
  ips: { ip: string; name: string }[];
  devices: MobileDevice[];
  /** Phones paired with a token before end-to-end encryption: they must be paired again. */
  legacy: { id: string; name: string; created_at: number }[];
  pairing_expires_in: number | null;
  pending: MobilePairRequest[];
  /** Phones not seen for this many days are removed (0: never). */
  dormant_days: number;
  /** Tailscale mode, as the daemon holds it (absent from a daemon older than `TAILSCALE_API`). `origin`: `<name>:8443` while phones can use it. */
  tailscale?: { on: boolean; origin: string | null; to_remove?: boolean };
  /** The phones' app lock, as the daemon applies it (absent below `LOCK_API`). */
  lock?: { on: boolean; idle_s: number; recheck: boolean; plain_read: boolean };
  /** The relay, as the daemon holds it (absent below `RELAY_API`). */
  relay?: RelayStatus;
  /** This window is the verified Kelpie app: it may approve phones and raise what they may do. */
  app: boolean;
}

/** What Tailscale says now (`mobile.tailscale_status`, which runs its CLI: asked for only while this page is open). */
interface TailscaleStatus {
  on: boolean;
  state: "not_installed" | "stopped" | "needs_login" | "https_off" | "unreadable" | "no_answer" | "ready";
  fqdn: string | null;
  /** `off` nothing of ours; `ours` published; `missing` the mode is on and the entry was removed; `left_behind` the mode is off and the entry could not be removed; `port_busy` another program holds our local port and the entry could not be removed; `not_listening` the mode is on and Kelpie's listener is not up; `renamed` the entry is under this machine's old Tailscale name; `taken` port 8443 is used by something else; `unknown` it could not be read; `no_answer` it did not answer in time. */
  serve: "off" | "ours" | "missing" | "left_behind" | "port_busy" | "not_listening" | "renamed" | "taken" | "unknown" | "no_answer";
  origin: string | null;
  /** A QR code of the page's address, with no secret. */
  install_qr: string | null;
}

/** The one thing to do about the state Tailscale is in. */
function tailscaleInstruction(s: TailscaleStatus): string {
  switch (s.state) {
    case "not_installed":
      return t("Tailscale is not installed on this computer. Install it from tailscale.com and sign in, on this computer and on your phone.");
    case "stopped":
      return t("Tailscale is not running on this computer. Start it, then check again.");
    case "needs_login":
      return t("Tailscale is not signed in on this computer. Sign in, then check again.");
    case "https_off":
      return t("Your tailnet does not have HTTPS certificates turned on. Turn on MagicDNS and HTTPS in the DNS page of the Tailscale admin console, then check again.");
    case "unreadable":
      return t("Kelpie could not understand what Tailscale says. Update Tailscale, then check again.");
    case "no_answer":
      return t("Tailscale did not answer in time, and this computer may be busy. Check again in a moment.");
    case "ready":
      switch (s.serve) {
        case "ours":
          return t("Phones with Tailscale on reach this computer at {origin}, from anywhere.", { origin: s.origin ?? "" });
        case "missing":
          return t("The entry in Tailscale Serve was removed, so phones cannot reach this computer through it. Restore it to go on.");
        case "left_behind":
          return t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere, and Tailscale would not remove it. Press Check again to try once more. If it stays, it may be under a name this computer had before: remove it with Tailscale's own tools.");
        case "port_busy":
          return t("Another program took the local port Tailscale Serve forwards to, and Kelpie could not remove its entry, so a phone that opens this address may reach that program. Do not open Kelpie on your phones until this is gone: stop that program, or press Restore to use another port.");
        case "not_listening":
          return t("Phone access is not running, so phones get no answer through Tailscale. Fix what is shown above, then turn phone access off and on, and this on again.");
        case "renamed":
          return t("This computer's Tailscale name changed, and the entry in Tailscale Serve is under the old one, so phones get no answer. Restore it under the new name. Phones must then be paired again: a browser keeps its pairing per address.");
        case "taken":
          return t("Port 8443 of Tailscale Serve is used by something else. Kelpie will not change it: free that port, then check again.");
        case "unknown":
          return t("Kelpie could not read what Tailscale Serve is doing. Update Tailscale, then check again.");
        case "no_answer":
          return t("Tailscale did not answer in time, and this computer may be busy. Check again in a moment.");
        case "off":
          return t("Tailscale is ready. Turn this on to reach this computer from anywhere with Tailscale on.");
      }
  }
}

export const mobileStatus = () => daemon<MobileStatus>("mobile.status");

/** What phones did, newest first. `intact` is false when the log's hash chain does not hold: it was edited or damaged. */
interface MobileAudit {
  entries: { at: number; device: string; action: string; summary: string }[];
  intact: boolean;
}

const DORMANT_DAYS = [30, 90, 180, 0];
const IDLE_SECONDS = [60, 300, 900, 1800, 3600];
/** The rows under Advanced. */
const ADVANCED = ["mobile-lan", "mobile-network", "mobile-tls", "mobile-screen", "mobile-devices"];

/** The pairing guide's two steps, for the page phones install from: the app the relay hosts, or this computer's Tailscale origin. */
type Guide = { via: "hosted" | "tailscale"; step: 1 | 2 };

const errText = (e: unknown) => String(e).replace(/^Error: /, "");

/** Said by the choice and by the question before it: what approving from Telegram gives up. */
const telegramWarning = () =>
  t("Telegram can read these messages: a bot chat is not end-to-end encrypted. A tap in Telegram asks for no Face ID, and anyone who has this Telegram account can approve. Only requests Kelpie labels low risk get buttons, each for one time and five minutes.");

export function MobileSettings() {
  const [status, { refetch }] = createResource(() => store.connected(), () => mobileStatus().catch(() => null));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [hotkeyError, setHotkeyError] = createSignal<string | null>(null);
  const [qr, setQr] = createSignal<{ url: string; qr: string; until: number; tls: boolean; tailscale?: boolean; hosted?: boolean; passkey?: boolean } | null>(null);
  const [now, setNow] = createSignal(Date.now());
  // The two steps of pairing on the hosted app or over Tailscale: 1 shows the page's address to install from, 2 the code to pair with.
  const [guide, setGuide] = createSignal<Guide | null>(null);
  const guideStep = () => guide()?.step ?? null;
  // A new setup whose relay would not make a room: Tailscale is offered instead.
  const [hostedRefused, setHostedRefused] = createSignal(false);
  // Only while phone access is on and this page is open; looked at again when what the daemon holds changes, or when asked.
  const tailscaleKey = () => (store.connected() && store.daemonApi() >= TAILSCALE_API && (status()?.enabled || status()?.tailscale?.to_remove) ? `${status()!.tailscale?.on}|${status()!.tailscale?.origin}|${status()!.tailscale?.to_remove}` : null);
  const [tailscale, { refetch: checkTailscale }] = createResource(tailscaleKey, () => daemon<TailscaleStatus>("mobile.tailscale_status").catch(() => null));
  // The guide's page and its install QR code (no secret in either): the hosted app while the relay is on, else this computer's Tailscale origin.
  const hostedPage = () => {
    const r = status()?.relay;
    return r?.on && r.hosted?.origin && r.hosted.install_qr ? { origin: r.hosted.origin, qr: r.hosted.install_qr } : null;
  };
  const guidePage = () => {
    if (guide()?.via === "hosted") return hostedPage();
    const ts = tailscale();
    return ts?.origin ? { origin: `https://${ts.origin}`, qr: ts.install_qr ?? "" } : null;
  };
  // What the channels are set to, the same settings as Settings → Notifications. Undefined: not asked yet; null: the daemon could not say.
  const [push, setPush] = createSignal<PushState | null | undefined>(undefined);
  const loadPush = () => daemon<PushState>("notify.get").then((p) => setPush(p), () => setPush(null));
  createEffect(() => {
    pushRevision();
    if (store.connected()) void loadPush();
  });
  // The two steps are about the origin: with it gone they end, and do not come back unasked when it returns.
  createEffect(() => {
    const g = guide();
    if (g && !(g.via === "hosted" ? status.loading : tailscale.loading) && !guidePage()) {
      setGuide(null);
      setQr(null);
    }
  });
  onMount(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    const un = onEvent<{ event: string; params?: { change?: string } }>("daemon-event", (m) => {
      if (m.event === "mobile.updated" || m.event === "mobile.pair_request" || m.event === "mobile.device_changed") void refetch();
      // A phone turned its alerts on or off, or was removed.
      if (m.event === "mobile.updated") void loadPush();
      // Approvals from Telegram changed on their own: an account was bound, the code ran out, or they stopped.
      if (m.event === "notify.telegram") {
        const change = m.params?.change;
        const shown = bindingOpen();
        // An error (another poller, say) leaves a code be only while the daemon still has it waiting.
        if (change === "error") void daemon<PushState>("notify.get").then(
            (p) => {
              if (!p.telegram?.binding_expires_in) setBinding(null);
            },
            () => {},
          );
        else setBinding(null);
        // Every page that shows these settings reads them again: one sending what it read before would undo the change.
        pushChanged();
        if (change === "bound") store.notify(t("Telegram account bound: it can approve low-risk requests from its chat."), undefined, 8000);
        else if (change === "bind_expired") store.notify(t("The Telegram code was not used in time, so nothing was bound. Choose approvals again for a new code."), undefined, 8000);
        else if (change === "bind_failed") store.notify(t("Nothing was bound with the Telegram code: too many wrong codes, or the settings changed meanwhile. Choose approvals again for a new code."), undefined, 8000);
        // This page dropped its own code when the person chose otherwise here: only a code still shown is news.
        else if (change === "bind_cancelled" && shown) store.notify(t("The Telegram code was cancelled because Telegram's settings changed, so nothing was bound. Choose approvals again for a new code."), undefined, 8000);
      }
      if (m.event === "mobile.device_changed") {
        setQr(null);
        setGuide(null);
      }
    });
    onCleanup(() => {
      clearInterval(timer);
      void un.then((f) => f());
    });
  });
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
      await refetch();
    }
  };
  const enable = (patch: Record<string, unknown> = {}) => run(() => daemon("mobile.enable", patch));
  const facts = (): OptionFacts => ({ api: store.daemonApi(), status: s() ?? null, push: push() ?? null });
  // The switch. A new setup gets the relay with it and the guide opens once the relay is on; if the relay would not make a room, it says why and
  // Tailscale is offered.
  const turnOn = async () => {
    const f = facts();
    const fresh = newSetup(f);
    let hosted = false;
    setHostedRefused(false);
    await run(async () => {
      try {
        hosted = (await turnOnAccess(f, { daemon: (m, p) => daemon(m, p), secure: (m, p) => mobileSecure(m, p), status: () => mobileStatus() })) === "hosted";
      } catch (e) {
        if (fresh) setHostedRefused(true);
        throw e;
      }
    });
    // After `run` has read the status again, which has the relay's page now.
    if (hosted) openGuide("hosted");
  };
  // Instead of the relay: this network's listeners back on, which Tailscale mode needs running, then Tailscale mode (the person verifies).
  const useTailscaleInstead = () =>
    run(async () => {
      await daemon("mobile.enable", { lan: true });
      setHostedRefused(false);
      await mobileSecure("mobile.set_tailscale", { on: true });
      await checkTailscale();
    });
  const pairBrowser = async () => {
    const ok = await confirmDialog(
      t("Pair a browser on this network? Anyone who can see the code on your screen in the next 2 minutes can try to use it, and you still approve each phone here. Over plain HTTP the page itself could be swapped by someone on this network, so a browser connected that way can only read; it can type over HTTPS or Tailscale."),
    );
    if (!ok) return;
    await run(async () => {
      // The plain pairing, whatever Tailscale is doing: its own address, and its own pairing.
      const r = await daemon<MobilePairStart>("mobile.pair_start", { plain: true });
      setGuide(null);
      setQr({ ...r, until: Date.now() + r.expires_in * 1000 });
    });
  };
  // Step 2 of the guide: a code for the page the guide is about.
  const pairInGuide = () =>
    run(async () => {
      const via = guide()?.via ?? "tailscale";
      const r = await daemon<MobilePairStart>("mobile.pair_start", via === "hosted" ? { hosted: true } : {});
      // The origin went while this was asked for: a code for another page is one the installed app refuses.
      if (via === "tailscale" && !r.tailscale) {
        setQr(null);
        setGuide(null);
        await checkTailscale();
        throw new Error(t("Tailscale is not publishing this computer right now, so no code was shown. See what Settings says, then try again."));
      }
      if (via === "hosted" && !r.hosted) {
        setQr(null);
        setGuide(null);
        throw new Error(t("The relay's phone app cannot be used right now, so no code was shown. See what Settings says, then try again."));
      }
      setQr({ ...r, until: Date.now() + r.expires_in * 1000 });
      setGuide({ via, step: 2 });
    });
  const openGuide = (via: Guide["via"]) => {
    setError(null);
    setGuide({ via, step: 1 });
  };
  const closeGuide = () => {
    setGuide(null);
    setQr(null);
  };
  const setTailscaleMode = (on: boolean) =>
    run(async () => {
      await mobileSecure("mobile.set_tailscale", { on });
      await checkTailscale();
    });
  // What takes protection away asks the person, and is back to what holds now if they did not verify: the status did not change, so nothing else redraws it.
  const setLock = (patch: Partial<NonNullable<MobileStatus["lock"]>>) => run(() => mobileSecure("mobile.set_lock", patch));
  const relay = createRelayForm(run);
  // The phone just removed while the relay was on: a new room is offered, and nothing happens unless the person makes it.
  const [roomOffer, setRoomOffer] = createSignal<string | null>(null);
  const unpair = async (d: MobileDevice) => {
    if (!(await confirmDialog(t("Unpair “{name}”? It is disconnected at once.", { name: d.name })))) return;
    const offer = offerNewRoom(s(), store.daemonApi());
    await run(() => daemon("mobile.revoke", { id: d.id }));
    if (offer && !s()?.devices.some((x) => x.id === d.id)) setRoomOffer(d.name);
  };
  const makeRoom = async () => {
    setRoomOffer(null);
    await relay.recreate();
  };
  const resetLock = (d: MobileDevice) => run(() => mobileSecure("mobile.reset_lock", { id: d.id }));
  const setPermission = (d: MobileDevice, permission: MobilePermission) =>
    run(() => mobileSecure("mobile.set_permission", { id: d.id, permission }));
  // One write after another, each from what the daemon holds then: two quick switches must not undo each other, and everything else in these settings goes back as it was.
  let channelWrites: Promise<unknown> = Promise.resolve();
  const setChannel = (name: "ntfy" | "telegram", on: boolean) => {
    channelWrites = channelWrites.then(() =>
      run(async () => {
        const now = await daemon<PushState>("notify.get");
        now.config[name].enabled = on;
        await daemon("notify.set", { config: now.config });
        pushChanged();
      }),
    );
    return channelWrites;
  };
  // What Telegram carries. Approvals are turned on by binding an account (the verified app, then the person); the code waits here until it is used.
  const [binding, setBinding] = createSignal<{ command: string; until: number } | null>(null);
  const bindingOpen = () => !!binding() && binding()!.until > now();
  const telegram = () => telegramChoice({ api: store.daemonApi(), status: s() ?? null, push: push() ?? null });
  const telegramMode = (): TelegramMode => (bindingOpen() ? "approve_low" : telegram()?.mode ?? "alert");
  // The bound account as Telegram names it, next to its id: a bare number is hard to tell from someone else's.
  const boundAccount = () => {
    const tg = push()?.config.telegram;
    const id = tg?.user_id ?? "";
    return tg?.user_name ? `${tg.user_name} (${id})` : id;
  };
  const setTelegramMode = async (mode: TelegramMode) => {
    if (mode === "approve_low") {
      if (!(await confirmDialog(`${telegramWarning()} ${t("Turn on approvals from Telegram? Kelpie asks you to verify first, then shows a code to send to your bot.")}`))) return;
      await run(async () => {
        const r = await mobileSecure<{ command: string; expires_in: number }>("notify.telegram_bind", {});
        setBinding({ command: r.command, until: Date.now() + r.expires_in * 1000 });
      });
      return;
    }
    setBinding(null);
    channelWrites = channelWrites.then(() =>
      run(async () => {
        const now = await daemon<PushState>("notify.get");
        now.config.telegram.mode = mode;
        // The person chose otherwise: a code shown for approvals binds nothing now. Any other save leaves it be.
        await daemon("notify.set", { config: now.config, end_binding: true });
        pushChanged();
      }),
    );
    await channelWrites;
  };
  const reset = async () => {
    if (!(await confirmDialog(t("Reset phone access? Every paired phone is removed and disconnected, the pairing keys are replaced and phone access turns off."))))
      return;
    await run(() => mobileSecure("mobile.reset", {}));
  };
  const revokeAll = async () => {
    if (!(await confirmDialog(t("Turn off phone access and remove every paired phone? They are disconnected at once and must be paired again."))))
      return;
    await run(() => daemon("mobile.revoke_all"));
  };
  const [audit, setAudit] = createSignal<MobileAudit | null>(null);
  const loadAudit = () => daemon<MobileAudit>("mobile.audit", { limit: 100 }).then(setAudit).catch((e) => setError(errText(e)));
  const s = () => status();
  // "Could not be saved" holds only until it could be: that is the daemon's to say, and it says so in its status.
  const daemonError = () => (s() ? statusError(s()!) : null);
  const shownError = () => (error()?.includes("could not be saved") ? daemonError() : error() ?? daemonError());
  const deviceName = (id: string) => s()?.devices.find((d) => d.id === id)?.name ?? id;
  const hasAccess = () => !!s() && (s()!.enabled || s()!.devices.length > 0 || (s()!.legacy ?? []).length > 0);
  const option = (id: OptionId) => optionState(id, facts());
  const trust = () => (store.daemonApi() >= HOSTED_API && s()?.relay ? hostedTrust(s()!.relay!) : null);
  const summary = () => mobileSummary(s()!, push() ?? null);
  const alertsOn = () => {
    const n = push()?.webpush?.devices ?? 0;
    return n === 0 ? t("Off") : n === 1 ? t("On for 1 phone") : t("On for {n} phones", { n });
  };
  const query = () => settingsQuery().trim();
  const advancedMatches = () => ADVANCED.some((id) => entryMatches(catalogEntry(id)!, query()));
  // Advanced opens by itself for what would otherwise be out of sight: a search or a link that leads into it, or a phone that must be paired again.
  const advancedForced = () => (!!query() && advancedMatches()) || ADVANCED.includes(store.settingsItem() ?? "") || !!s()?.legacy?.length || !!s()?.devices.some((d) => d.state !== "ok");
  const [advancedWanted, setAdvancedWanted] = createSignal(false);
  const advancedOpen = () => advancedWanted() || advancedForced();
  let advanced: HTMLDetailsElement | undefined;
  const showPhones = () => {
    setAdvancedWanted(true);
    queueMicrotask(() => advanced?.scrollIntoView({ block: "start" }));
  };
  return (
    <>
      <LoadBar when={busy()} />
      <Show when={s() && !query()}>
        <div class="mobile-summary" data-mobile-summary>
          <div data-summary-reach>{summary().reach}</div>
          <div data-summary-lock>{summary().lock}</div>
          <Show when={summary().alerts}><div data-summary-alerts>{summary().alerts}</div></Show>
          <div data-summary-phones>
            {summary().phones}
            {" "}
            <Show when={s()!.devices.length || s()!.legacy?.length}><button class="link-btn" data-summary-show-phones onClick={showPhones}>{t("Show them")}</button></Show>
          </div>
        </div>
      </Show>
      <OptionRow id="mobile-access" state={option("mobile-access")} onChange={(on) => void (on ? turnOn() : run(() => daemon("mobile.disable")))} />
      <Show when={shownError()}><div class="form-note error">{errorText(shownError())}</div></Show>
      <Show when={hostedRefused() && s()?.enabled && !s()!.relay?.on}>
        <div class="form-note warn-note" data-hosted-refused>
          {t("The relay did not make this computer's room, so phones cannot reach it yet. Try the relay's switch again later, or use Tailscale: it needs Tailscale on this computer and on the phone, and turns this network's listeners back on.")}
          {" "}
          <button class="btn" data-use-tailscale disabled={busy()} onClick={() => void useTailscaleInstead()}>{t("Use Tailscale instead")}</button>
        </div>
      </Show>
      <Show when={s()?.store_error}>
        <div class="form-note error" data-store-error>
          {t("The phone access settings could not be read ({error}). A copy was kept next to them; phone access stays off until you reset it.", { error: s()!.store_error! })}
          {" "}
          <button class="btn danger" disabled={busy()} onClick={() => void reset()}>{t("Reset phone access")}</button>
        </div>
      </Show>
      <Show when={hasAccess() && !s()!.store_error && (!query() || advancedMatches())}>
        <div class="inline-fields" style={{ "margin-top": "8px" }}>
          <button class="btn danger" data-revoke-all onClick={() => void revokeAll()}>{t("Turn off phone access and revoke all")}</button>
        </div>
      </Show>
      <Show when={s()}>
        <OptionRow id="mobile-hosted" state={option("mobile-hosted")} control={
          <Show when={hostedPairing(facts())}>
            <button class="btn primary" data-hosted-pair disabled={busy()} onClick={() => openGuide("hosted")}><Icon name="plus" size={13} /> {t("Pair a phone")}</button>
          </Show>
        }>
          <Show when={store.daemonApi() >= HOSTED_API && s()!.relay?.on && hostedReason(s()!.relay!)}>
            <div class="muted-inline" data-hosted-reason>{errorText(hostedReason(s()!.relay!))}</div>
          </Show>
          <Show when={hostedPairing(facts()) && !s()!.app}>
            <div class="form-note warn-note">{t("This window is not the installed Kelpie app, so it cannot approve phones or raise what they may do.")}</div>
          </Show>
          <Show when={trust()}>
            <details class="relay-sees" data-hosted-trust>
              <summary>{t("What this trusts")}</summary>
              <div class="form-note">{trust()}</div>
            </details>
          </Show>
        </OptionRow>
        <OptionRow id="mobile-tailscale" state={option("mobile-tailscale")} onChange={(on) => void setTailscaleMode(on)}>
          <Show when={store.daemonApi() >= TAILSCALE_API}>
            <Show when={s()!.enabled} fallback={
              <Show when={s()!.tailscale?.to_remove}>
                <Show when={tailscale()} fallback={<div class="form-note warn-note" data-tailscale-to-remove>{t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere. Kelpie removes it once Tailscale answers: start Tailscale, then check again.")}</div>}>
                  <div class="form-note warn-note" data-tailscale-to-remove data-tailscale-serve={tailscale()!.serve}>{tailscale()!.state === "ready" ? tailscaleInstruction(tailscale()!) : t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere. Kelpie removes it once Tailscale answers: start Tailscale, then check again.")}</div>
                </Show>
                <button class="btn" data-tailscale-check disabled={tailscale.loading} onClick={() => void checkTailscale()}>{t("Check again")}</button>
              </Show>
            }>
              <div class="inline-fields" style={{ "align-items": "center" }}>
                <button class="btn" data-tailscale-check disabled={tailscale.loading} onClick={() => void checkTailscale()}>{t("Check again")}</button>
                <Show when={s()!.running && (tailscale()?.serve === "missing" || tailscale()?.serve === "port_busy" || tailscale()?.serve === "renamed")}>
                  <button class="btn primary" data-tailscale-restore disabled={busy()} onClick={() => void setTailscaleMode(true)}>{t("Restore")}</button>
                </Show>
                <Show when={tailscale()?.serve === "left_behind"}>
                  <button class="btn primary" data-tailscale-remove disabled={busy()} onClick={() => void setTailscaleMode(false)}>{t("Remove")}</button>
                </Show>
              </div>
              <Show when={tailscale()}>
                <div class="muted-inline" data-tailscale-state={tailscale()!.state} data-tailscale-serve={tailscale()!.serve}>{tailscaleInstruction(tailscale()!)}</div>
              </Show>
              <Show when={s()!.tailscale?.to_remove && !["left_behind", "port_busy"].includes(tailscale()?.serve ?? "")}>
                <div class="form-note warn-note" data-tailscale-to-remove>{t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere. Kelpie removes it once Tailscale answers: start Tailscale, then check again.")}</div>
              </Show>
              <Show when={tailscale()?.origin}>
                <button class="btn primary" data-tailscale-pair disabled={busy() || !s()!.running} onClick={() => openGuide("tailscale")}><Icon name="plus" size={13} /> {t("Pair a phone over Tailscale")}</button>
              </Show>
            </Show>
          </Show>
        </OptionRow>
        <OptionRow id="mobile-relay" state={option("mobile-relay")} onChange={(on) => void relay.turn(on, s()!.relay)}>
          <Show when={store.daemonApi() >= RELAY_API && s()!.relay}>
            <RelayDetails status={s()!.relay!} form={relay} busy={busy()} api={store.daemonApi()} />
          </Show>
        </OptionRow>
      </Show>
      <Show when={s()?.enabled}>
        <Show when={guideStep() && guidePage()}>
          <div class="overlay" data-pair-guide={guideStep()} data-guide-via={guide()!.via} data-tailscale-guide={guide()!.via === "tailscale" ? guideStep() : undefined} data-hosted-guide={guide()!.via === "hosted" ? guideStep() : undefined}>
            <div class="modal" style={{ width: "560px" }}>
              <Show when={guideStep() === 1} fallback={
                <>
                  <h2>{t("Step 2: Pair")}</h2>
                  <Show when={qr() && qr()!.until > now()} fallback={
                    <>
                      <div style={{ color: "var(--text-dim)" }}>{t("The code expired. Show a new one.")}</div>
                      <button class="btn primary" disabled={busy()} onClick={() => void pairInGuide()}>{t("Show a new code")}</button>
                    </>
                  }>
                    <div class="mobile-qr">
                      <div class="mobile-qr-code" innerHTML={qr()!.qr} data-link={qr()!.url} />
                      <div>
                        <div>{t("In the installed Kelpie app press Scan the code, and point it at this. The app then shows six digits: type them here when Kelpie asks.")}</div>
                        <div class="muted-inline">{t("The code works once and expires in {n} s.", { n: Math.max(0, Math.round((qr()!.until - now()) / 1000)) })}</div>
                        <button class="btn" data-copy-pair-link onClick={() => void navigator.clipboard?.writeText(qr()!.url)}>{t("Copy the link, to paste it on the phone")}</button>
                      </div>
                    </div>
                  </Show>
                </>
              }>
                <h2>{t("Step 1: Install")}</h2>
                <div class="mobile-qr">
                  <div class="mobile-qr-code" innerHTML={guidePage()!.qr} data-install-link={`${guidePage()!.origin}/`} />
                  <div>
                    <Show when={guide()!.via === "hosted"} fallback={
                      <>
                        <div>{t("Turn on Tailscale on your phone and scan this with its camera. The page shows how to install Kelpie on the phone; then open it from the Home Screen.")}</div>
                        <div class="muted-inline">{t("This code holds no secret. The very first visit can take up to a minute while Tailscale gets its certificate.")}</div>
                      </>
                    }>
                      <div>{t("Scan this with the phone's camera. The page shows how to install Kelpie on the phone; then open it from the Home Screen.")}</div>
                      <div class="muted-inline">{t("This code holds no secret. The app comes from the relay: see What this trusts, under Hosted phone app.")}</div>
                    </Show>
                    <div class="mono mobile-qr-url">{`${guidePage()!.origin}/`}</div>
                    <button class="btn primary" data-guide-next disabled={busy()} onClick={() => void pairInGuide()}>{guide()!.via === "hosted" ? t("Already installed: Step 2") : t("Installed: Step 2")}</button>
                  </div>
                </div>
              </Show>
              <Show when={error()}><div class="form-note error" data-guide-error>{errorText(error())}</div></Show>
              <div class="footer"><span style={{ flex: 1 }} /><button class="btn" data-guide-close onClick={closeGuide}>{t("Close")}</button></div>
            </div>
          </div>
        </Show>
        <Show when={plainPairing(facts())}>
          <SettingRow id="mobile-pair" wide>
            <Show when={!s()!.app}>
              <div class="form-note warn-note">{t("This window is not the installed Kelpie app, so it cannot approve phones or raise what they may do.")}</div>
            </Show>
            <Show when={qr() && !guideStep() && qr()!.until > now()} fallback={
              <button class="btn primary" data-plain-pair disabled={!s()!.running || busy()} onClick={() => void pairBrowser()}><Icon name="plus" size={13} /> {t("Pair a browser on this network")}</button>
            }>
              <div class="mobile-qr">
                <div class="mobile-qr-code" innerHTML={qr()!.qr} data-link={qr()!.url} />
                <div>
                  <div>{t("Scan with the phone's camera. The phone then shows six digits: type them here when Kelpie asks.")}</div>
                  <div class="muted-inline">{t("The code works once and expires in {n} s.", { n: Math.max(0, Math.round((qr()!.until - now()) / 1000)) })}</div>
                  <Show when={!qr()!.tls}><div class="form-note warn-note">{t("Plain HTTP: a browser paired this way can only read. With the app lock off, it can type over a Tailscale address.")}</div></Show>
                  <Show when={qr()!.tls && qr()!.passkey === false}><div class="form-note warn-note" data-pair-no-passkey>{t("This address is a number, and a passkey needs a name, so a phone paired here can only read while the app lock is on.")}</div></Show>
                  <div class="mono mobile-qr-url">{qr()!.url.split("#")[0]}</div>
                  <button class="btn" onClick={() => setQr(null)}>{t("Cancel")}</button>
                </div>
              </div>
            </Show>
          </SettingRow>
        </Show>
      </Show>
      <Show when={s()}>
        <Show when={push() !== undefined}>
          <OptionRow id="mobile-webpush" state={option("mobile-webpush")} control={<span class="muted-inline" data-webpush-state>{alertsOn()}</span>}>
            <button class="link-btn" data-open-notifications onClick={() => store.openSettings("notifications", "push-webpush")}>{t("Send a test alert: Settings → Notifications")}</button>
          </OptionRow>
        </Show>
        <OptionRow id="mobile-lock" state={option("mobile-lock")} onChange={(on) => void setLock({ on })}>
          <Show when={store.daemonApi() >= HOSTED_API}><div class="muted-inline" data-lock-hosted>{t("Phones paired on the hosted app always use the lock, even with this off.")}</div></Show>
        </OptionRow>
        <OptionRow id="mobile-lock-idle" state={option("mobile-lock-idle")} control={
          <select class="text-input" data-lock-idle value={s()!.lock?.idle_s} disabled={busy() || !!option("mobile-lock-idle").disabled} onChange={(e) => {
            const select = e.currentTarget;
            void setLock({ idle_s: +select.value }).then(() => { select.value = String(s()?.lock?.idle_s ?? select.value); });
          }}>
            <For each={IDLE_SECONDS}>{(n) => <option value={n}>{t("{n} min idle", { n: n / 60 })}</option>}</For>
          </select>
        } />
        <OptionRow id="mobile-lock-recheck" state={option("mobile-lock-recheck")} onChange={(recheck) => void setLock({ recheck })} />
        <OptionRow id="mobile-lock-plain" state={option("mobile-lock-plain")} onChange={(plain_read) => void setLock({ plain_read })} />
        <Show when={push() !== undefined}>
          <OptionRow id="mobile-ntfy" state={option("mobile-ntfy")} onChange={(on) => void setChannel("ntfy", on)}>
            <button class="link-btn" data-open-notifications onClick={() => store.openSettings("notifications", "push-ntfy")}>{t("Server, topic and token: Settings → Notifications")}</button>
          </OptionRow>
          <OptionRow id="mobile-telegram" state={option("mobile-telegram")} onChange={(on) => void setChannel("telegram", on)}>
            <For each={push() ? telegramProblems(push()!.config, push()!.secrets) : []}>{(p) => <div class="form-note warn-note" data-telegram-problem>{p}</div>}</For>
            <button class="link-btn" data-open-notifications onClick={() => store.openSettings("notifications", "push-telegram")}>{t("Bot token and chat id: Settings → Notifications")}</button>
            <Show when={telegram()}>
              <div class="inline-fields" style={{ "align-items": "center" }}>
                <span>{t("Telegram gets")}</span>
                <select class="text-input" data-telegram-mode value={telegramMode()} disabled={busy()} onChange={(e) => {
                  const select = e.currentTarget;
                  // Back to what holds now when the person did not go on (nothing else redraws it then).
                  void setTelegramMode(select.value as TelegramMode).then(() => { select.value = telegramMode(); });
                }}>
                  <option value="alert">{t("Alerts only")}</option>
                  <option value="alert_link">{t("Alerts with a link")}</option>
                  <option value="approve_low" disabled={!!telegram()!.approveBlocked && telegramMode() !== "approve_low"}>{t("Alerts with a link, and approvals of low-risk requests")}</option>
                </select>
              </div>
              <Show when={telegram()!.approveBlocked && telegramMode() !== "approve_low"}><div class="muted-inline" data-telegram-approve-blocked>{telegram()!.approveBlocked}</div></Show>
              <Show when={telegramMode() !== "alert"}>
                <div class="form-note" data-telegram-link-note>{t("The link holds this computer's name or address on your network, which Telegram sees. Telegram does not open it, and it holds no code or token.")}</div>
              </Show>
              <Show when={telegramMode() === "approve_low"}><div class="form-note warn-note" data-telegram-warning>{telegramWarning()}</div></Show>
              <Show when={bindingOpen()}>
                <div class="form-note" data-telegram-bind>
                  {t("From the Telegram account that will approve, send this within {n} s in the chat set in Settings → Notifications (your chat with the bot, or the group):", { n: Math.max(0, Math.round((binding()!.until - now()) / 1000)) })}
                  {" "}
                  <span class="mono" data-telegram-bind-command>{binding()!.command}</span>
                  {" "}
                  <button class="btn mini" data-telegram-bind-copy onClick={() => void navigator.clipboard?.writeText(binding()!.command)}>{t("Copy")}</button>
                </div>
              </Show>
              <Show when={push()?.telegram?.approving && !bindingOpen()}>
                <div class="muted-inline" data-telegram-bound>{t("Approvals are on for Telegram account {user} in chat {chat}. Presses from anyone else do nothing.", { user: boundAccount(), chat: push()!.config.telegram.chat_id })}</div>
              </Show>
              <Show when={push()?.telegram?.error}><div class="form-note error" data-telegram-error>{errorText(push()!.telegram!.error)}</div></Show>
            </Show>
          </OptionRow>
        </Show>
      </Show>
      <Show when={s() && (!query() || advancedMatches())}>
        <details class="option-advanced" ref={advanced} data-mobile-advanced open={advancedOpen()} onToggle={(e) => {
          // Held open while something under it must be seen: the page's state and the element stay the same.
          if (advancedForced()) e.currentTarget.open = true;
          else setAdvancedWanted(e.currentTarget.open);
        }}>
          <summary>{t("Advanced: address, certificate, screen and paired phones")}</summary>
          <OptionRow id="mobile-lan" state={option("mobile-lan")} onChange={(lan) => void enable({ lan })} />
          <Show when={s()!.enabled}>
            <SettingRow id="mobile-network" wide>
              <div class="inline-fields">
                <select class="text-input" value={s()!.ip ?? ""} disabled={busy()} onChange={(e) => void enable({ ip: e.currentTarget.value })}>
                  <option value="">{t("Every private and Tailscale address")}</option>
                  <For each={s()!.ips}>{(a) => <option value={a.ip}>{a.ip} ({a.name})</option>}</For>
                  <Show when={s()!.ip && !s()!.ips.some((a) => a.ip === s()!.ip)}><option value={s()!.ip!}>{s()!.ip}</option></Show>
                </select>
                <input class="text-input" type="number" min="1024" max="65535" style={{ width: "100px" }} value={s()!.port} disabled={busy()}
                  onChange={(e) => void enable({ port: +e.currentTarget.value })} />
              </div>
              <For each={s()!.urls ?? []}>{(u) => <div class="mono mobile-url">{u}</div>}</For>
              <Show when={!s()!.tls}>
                <div class="form-note warn-note">{t("Plain HTTP: what phones see and send is still end-to-end encrypted, but a browser page served this way could be swapped by someone on the network, so browsers connected over plain HTTP can only read. With the app lock off, one on a Tailscale address can type.")}</div>
              </Show>
            </SettingRow>
            <SettingRow id="mobile-tls" wide>
              <div class="inline-fields">
                <input class="text-input mono" placeholder={t("certificate (.crt / .pem)")} value={s()!.cert ?? ""} onChange={(e) => void enable({ cert: e.currentTarget.value })} />
                <input class="text-input mono" placeholder={t("private key (.key / .pem)")} value={s()!.key ?? ""} onChange={(e) => void enable({ key: e.currentTarget.value })} />
              </div>
            </SettingRow>
          </Show>
          <Show when={store.daemonApi() >= SCREEN_API}>
            <SettingRow id="mobile-screen" wide>
              <div class="inline-fields" style={{ "align-items": "center" }}>
                <span>{t("Stop hotkey")}</span>
                <input class="text-input mono" data-screen-hotkey style={{ width: "180px" }} value={store.state.settings.screenStopHotkey} onChange={(e) => {
                  const input = e.currentTarget;
                  const hotkey = input.value.trim();
                  // The app checks it as the helper will: two modifiers, one of them Ctrl or Alt.
                  void invoke("screen_prefs", { hotkey, tiles: store.state.settings.screenCrispText })
                    .then(() => store.setSettings({ screenStopHotkey: hotkey }))
                    .catch((err) => { setHotkeyError(errorText(err)); input.value = store.state.settings.screenStopHotkey; });
                }} onInput={() => setHotkeyError(null)} />
                <Toggle on={store.state.settings.screenCrispText} label={t("Crisp text")} onChange={(on) => store.setSettings({ screenCrispText: on })} />
              </div>
              <Show when={hotkeyError()}><div class="form-note error">{hotkeyError()}</div></Show>
            </SettingRow>
          </Show>
          <SettingRow id="mobile-devices" wide>
            <Show when={roomOffer()}>
              <NewRoomOffer name={roomOffer()!} busy={busy()} onMake={() => void makeRoom()} onSkip={() => setRoomOffer(null)} />
            </Show>
            <div class="saved-layouts">
              <For each={s()!.devices} fallback={<div class="muted-inline">{t("No paired devices.")}</div>}>
                {(d) => (
                  <div class="saved-layout" data-device={d.id} data-state={d.state}>
                    <Icon name="server" size={16} />
                    <div class="saved-layout-main">
                      <div>{d.name} <Show when={pageKindLabel(d.page_kind)}><span class="chip" data-page-kind={d.page_kind}>{pageKindLabel(d.page_kind)}</span> </Show><span class="muted-inline">· {d.kind === "app" ? t("Kelpie app") : t("Browser")}<Show when={d.push}> · {t("alerts on")}</Show><Show when={d.passkey}> · {t("app lock set up")}</Show><Show when={d.lock_reset}> · {t("may set up a passkey")}</Show><Show when={d.kind === "pwa" && d.secure_origin && !d.may_control && d.page_kind !== "hosted" && store.daemonApi() >= TAKEOVER_API}> · {t("may take control once its app lock is set up and on")}</Show></span></div>
                      <div class="muted-inline" style={{ "font-size": "11.5px" }}>
                        {d.state === "unverified"
                          ? t("Its record was changed outside Kelpie: pair it again.")
                          : d.state === "unchecked"
                            ? t("Kelpie cannot read the key that checks this record right now. It will check again; the phone stays paired.")
                            : t("paired {when} · last seen {seen}", { when: formatRelative(d.created_at), seen: formatRelative(d.last_seen) })}
                      </div>
                    </div>
                    <Show when={d.state === "ok"}>
                      <select class="text-input" data-permission value={d.permission} disabled={busy()} onChange={(e) => void setPermission(d, e.currentTarget.value as MobilePermission)}>
                        <For each={permissionChoices(d, store.daemonApi())}>{(p) => <option value={p}>{p === "read" ? t("May read") : p === "write" ? t("May type") : t("May take control")}</option>}</For>
                      </select>
                    </Show>
                    <Show when={d.state === "ok" && d.secure_origin && store.daemonApi() >= LOCK_API}>
                      <button class="btn" data-reset-lock disabled={busy()} onClick={() => void resetLock(d)}>{t("Reset this phone's lock")}</button>
                    </Show>
                    <button class="btn" onClick={() => void unpair(d)}>{t("Unpair")}</button>
                  </div>
                )}
              </For>
              <For each={s()!.legacy ?? []}>
                {(d) => (
                  <div class="saved-layout" data-legacy={d.id}>
                    <Icon name="server" size={16} />
                    <div class="saved-layout-main">
                      <div>{d.name}</div>
                      <div class="muted-inline" style={{ "font-size": "11.5px" }}>{t("Paired before end-to-end encryption: pair it again.")}</div>
                    </div>
                    <button class="btn" onClick={() => void run(() => daemon("mobile.revoke", { id: d.id }))}>{t("Remove")}</button>
                  </div>
                )}
              </For>
            </div>
            <Show when={hasAccess() && !s()!.store_error}>
              <div class="inline-fields" style={{ "margin-top": "8px" }}>
                <button class="btn danger" disabled={busy()} onClick={() => void reset()}>{t("Reset phone access")}</button>
              </div>
            </Show>
            <div class="inline-fields" style={{ "margin-top": "8px", "align-items": "center" }}>
              <span>{t("Remove phones not seen for")}</span>
              <select class="text-input" data-dormant value={s()!.dormant_days} disabled={busy()} onChange={(e) => {
                const select = e.currentTarget;
                const dormant_days = +select.value;
                // Back to what holds now when the person did not verify (the status did not change, so nothing else redraws it).
                void run(() => mobileSecure("mobile.set_options", { dormant_days })).then(() => { select.value = String(s()?.dormant_days ?? dormant_days); });
              }}>
                <For each={DORMANT_DAYS}>{(n) => <option value={n}>{n === 0 ? t("Never remove") : t("{n} days", { n })}</option>}</For>
                <Show when={!DORMANT_DAYS.includes(s()!.dormant_days)}><option value={s()!.dormant_days}>{t("{n} days", { n: s()!.dormant_days })}</option></Show>
              </select>
            </div>
            <details style={{ "margin-top": "8px" }} data-audit onToggle={(e) => e.currentTarget.open && void loadAudit()}>
              <summary>{t("What phones did")}</summary>
              <Show when={audit() && !audit()!.intact}>
                <div class="form-note error" data-audit-broken>{t("This log was edited or damaged, so it cannot be fully trusted.")}</div>
              </Show>
              <For each={audit()?.entries ?? []} fallback={<div class="muted-inline">{t("Nothing yet.")}</div>}>
                {(a) => (
                  <div class="muted-inline" style={{ "font-size": "11.5px" }} data-audit-entry>
                    {formatRelative(a.at)} · {a.device === "-" ? "Kelpie" : deviceName(a.device)} · <span class="mono">{a.action}</span>{a.summary ? ` · ${a.summary}` : ""}
                  </div>
                )}
              </For>
            </details>
          </SettingRow>
        </details>
      </Show>
    </>
  );
}

/** Asks whether a phone that scanned the pairing code may connect (shown over everything): the person types the six digits
 *  the phone shows. Also says when a phone was added, changed or removed. A phone on the hosted app is offered reading first, and hears what typing
 *  from it means; control is never offered at approval. */
export function MobileApprovals() {
  const [requests, setRequests] = createSignal<MobilePairRequest[]>([]);
  const [codes, setCodes] = createSignal<Record<string, string>>({});
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  /** The request being allowed: a second click would burn another of its three tries and raise a second prompt. */
  const [allowing, setAllowing] = createSignal<string | null>(null);
  const add = (p: MobilePairRequest) => setRequests((r) => (r.some((x) => x.id === p.id) ? r : [...r, p]));
  onMount(() => {
    // Requests waiting already: this window was reloaded, or came up after the phone scanned.
    void mobileStatus().then((s) => s.pending.forEach(add)).catch(() => {});
    const un = onEvent<{ event: string; params: any }>("daemon-event", (m) => {
      if (m.event === "mobile.pair_request") add({ id: m.params.id, name: m.params.name, kind: m.params.kind ?? "pwa", fingerprint: m.params.fingerprint ?? "", want: m.params.want ?? "read", address: m.params.address ?? "", origin_kind: m.params.origin_kind ?? undefined });
      if (m.event === "mobile.lockout") {
        store.notify(m.params.reason === "sas" ? t("A pairing request was declined after three wrong codes.") : t("Too many failed phone connections from {address}: that address is blocked for a while.", { address: m.params.address ?? "" }), undefined, 10000);
      }
      if (m.event === "mobile.updated") void mobileStatus().then((s) => setRequests((r) => r.filter((x) => s.pending.some((p) => p.id === x.id)))).catch(() => {});
      if (m.event === "mobile.device_changed") {
        const p = m.params as { name: string; change: string; permission: MobilePermission | null };
        const what = p.permission === "control" ? t("may take control") : p.permission === "write" ? t("may type") : t("may read");
        if (p.change === "lock_enrolled") return store.notify(t("Phone “{name}” set up its app lock. If that was not you, unpair it.", { name: p.name }), undefined, 10000);
        store.notify(p.change === "added" ? t("Phone “{name}” was paired ({what}).", { name: p.name, what }) : p.change === "removed" ? t("Phone “{name}” was removed.", { name: p.name }) : t("Phone “{name}” now {what}.", { name: p.name, what }), undefined, 6000);
      }
    });
    onCleanup(() => void un.then((f) => f()));
  });
  const close = (id: string) => setRequests((r) => r.filter((x) => x.id !== id));
  const deny = (id: string) => {
    close(id);
    void mobileSecure("mobile.approve", { id, allow: false }).catch((e) => store.notify(errorText(e)));
  };
  const allow = async (r: { id: string; name: string }, permission: MobilePermission) => {
    const code = (codes()[r.id] ?? "").replace(/\D/g, "");
    if (code.length !== 6) return setErrors((e) => ({ ...e, [r.id]: t("Type the six digits the phone shows.") }));
    if (allowing()) return;
    setAllowing(r.id);
    try {
      await mobileSecure("mobile.approve", { id: r.id, allow: true, code, permission });
      close(r.id);
    } catch (e) {
      setErrors((x) => ({ ...x, [r.id]: errorText(e) }));
      // A request declined after three wrong codes is gone.
      void mobileStatus().then((s) => !s.pending.some((p) => p.id === r.id) && close(r.id)).catch(() => {});
    } finally {
      setAllowing(null);
    }
  };
  return (
    <For each={requests()}>
      {(r) => {
        const choice = approvalChoices(r);
        return (
          <div class="overlay mobile-approve" data-request={r.id} data-origin-kind={r.origin_kind}>
            <div class="modal" style={{ width: "460px" }}>
              <h2>{t("Pair “{name}”?", { name: r.name })}</h2>
              <div style={{ "font-weight": 600, margin: "4px 0" }}>{t("Did you just scan a QR code on your phone?")}</div>
              <div style={{ color: "var(--text-dim)" }}>{t("A device that scanned your pairing code wants to see your sessions. Only approve a device you are holding: type the six digits it shows.")}</div>
              <div class="muted-inline" style={{ margin: "8px 0 0" }} data-request-details>
                <div>{t("Device key (the phone shows the same): {fingerprint}", { fingerprint: r.fingerprint })}</div>
                <div>{t("It asks to: {what}", { what: choice.asks === "read" ? t("read") : choice.asks === "control" ? t("read, type and take control") : t("read and type") })}</div>
                <Show when={pageKindLabel(r.origin_kind)}><div data-request-page>{t("It pairs on: {page}", { page: pageKindLabel(r.origin_kind)! })}</div></Show>
                <Show when={r.address}><div>{t("From address: {address}", { address: r.address })}</div></Show>
              </div>
              <Show when={choice.typing}><div class="form-note warn-note" data-request-typing>{choice.typing}</div></Show>
              <input class="text-input mono mobile-approve-code" inputmode="numeric" maxlength="7" autocomplete="off" placeholder="000000" style={{ "font-size": "20px", "letter-spacing": "4px", width: "160px", margin: "12px 0" }}
                value={codes()[r.id] ?? ""} onInput={(e) => setCodes((c) => ({ ...c, [r.id]: e.currentTarget.value }))} />
              <Show when={errors()[r.id]}><div class="form-note error">{errors()[r.id]}</div></Show>
              <div class="footer">
                <button class="btn" onClick={() => deny(r.id)}>{t("Deny")}</button>
                <span style={{ flex: 1 }} />
                <For each={choice.choices}>
                  {(p) => <button class={choice.primary === p ? "btn primary" : "btn"} data-allow={p} disabled={allowing() === r.id} onClick={() => void allow(r, p)}>{p === "read" ? t("Allow reading") : t("Allow reading and typing")}</button>}
                </For>
              </div>
            </div>
          </div>
        );
      }}
    </For>
  );
}
