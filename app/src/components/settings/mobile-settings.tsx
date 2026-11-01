// Settings → Mobile: the daemon's web app for phones (crates/kelpied/src/mobile.rs). Off by default. A phone pairs with a
// one-time QR code and end-to-end encryption (kelpie-link); the person approves it here by typing the six digits the phone
// shows, and the daemon then asks the OS to verify them (Windows Hello or the password). Raising what a phone may do, resetting
// phone access and keeping unused phones longer ask for that verification too (`mobileSecure`); what the prompt says is the daemon's.
import { For, Show, createEffect, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../../lib/store";
import { daemon, mobileSecure, onEvent } from "../../lib/ipc";
import { LOCK_API, TAILSCALE_API } from "../../lib/attention";
import { formatRelative, t, errorText } from "../../lib/i18n";
import { LoadBar } from "../load-bar";
import { Icon } from "../icons";
import { SettingRow, Toggle } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

export type MobilePermission = "read" | "write" | "control";

export interface MobileDevice {
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
  /** `unverified`: its record was changed outside Kelpie (or the key that checks it is gone): it must be paired again.
   *  `unchecked`: the key that checks it cannot be read right now; nothing is concluded, and it is checked again when it can be. */
  state: "ok" | "unverified" | "unchecked";
}

export interface MobileStatus {
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
  pending: { id: string; name: string; kind: "app" | "pwa"; fingerprint: string; want: MobilePermission; address: string }[];
  /** Phones not seen for this many days are removed (0: never). */
  dormant_days: number;
  /** Tailscale mode, as the daemon holds it (absent from a daemon older than `TAILSCALE_API`). `origin`: `<name>:8443` while phones can use it. */
  tailscale?: { on: boolean; origin: string | null; to_remove?: boolean };
  /** The phones' app lock, as the daemon applies it (absent below `LOCK_API`). */
  lock?: { on: boolean; idle_s: number; recheck: boolean; plain_read: boolean };
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

const errText = (e: unknown) => String(e).replace(/^Error: /, "");

export function MobileSettings() {
  const [status, { refetch }] = createResource(() => store.connected(), () => mobileStatus().catch(() => null));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [qr, setQr] = createSignal<{ url: string; qr: string; until: number; tls: boolean; tailscale?: boolean; passkey?: boolean } | null>(null);
  const [now, setNow] = createSignal(Date.now());
  // The two steps of pairing over Tailscale: 1 shows the page's address to install from, 2 the code to pair with.
  const [guideStep, setGuideStep] = createSignal<1 | 2 | null>(null);
  // Only while phone access is on and this page is open; looked at again when what the daemon holds changes, or when asked.
  const tailscaleKey = () => (store.connected() && store.daemonApi() >= TAILSCALE_API && (status()?.enabled || status()?.tailscale?.to_remove) ? `${status()!.tailscale?.on}|${status()!.tailscale?.origin}|${status()!.tailscale?.to_remove}` : null);
  const [tailscale, { refetch: checkTailscale }] = createResource(tailscaleKey, () => daemon<TailscaleStatus>("mobile.tailscale_status").catch(() => null));
  // The two steps are about the origin: with it gone they end, and do not come back unasked when it returns.
  createEffect(() => {
    if (guideStep() && !tailscale.loading && !tailscale()?.origin) {
      setGuideStep(null);
      setQr(null);
    }
  });
  onMount(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    const un = onEvent<{ event: string }>("daemon-event", (m) => {
      if (m.event === "mobile.updated" || m.event === "mobile.pair_request" || m.event === "mobile.device_changed") void refetch();
      if (m.event === "mobile.device_changed") {
        setQr(null);
        setGuideStep(null);
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
  const pairBrowser = async () => {
    const ok = await confirmDialog(
      t("Pair a browser on this network? Anyone who can see the code on your screen in the next 2 minutes can try to use it, and you still approve each phone here. Over plain HTTP the page itself could be swapped by someone on this network, so a browser connected that way can only read; it can type over HTTPS or Tailscale."),
    );
    if (!ok) return;
    await run(async () => {
      // The plain pairing, whatever Tailscale is doing: its own address, and its own pairing.
      const r = await daemon<{ url: string; qr: string; expires_in: number; tls: boolean; passkey?: boolean }>("mobile.pair_start", { plain: true });
      setGuideStep(null);
      setQr({ ...r, until: Date.now() + r.expires_in * 1000 });
    });
  };
  const pairOverTailscale = () =>
    run(async () => {
      const r = await daemon<{ url: string; qr: string; expires_in: number; tls: boolean; tailscale: boolean }>("mobile.pair_start", {});
      // The origin went while this was asked for: a code for this network's address is one the installed app refuses.
      if (!r.tailscale) {
        setQr(null);
        setGuideStep(null);
        await checkTailscale();
        throw new Error(t("Tailscale is not publishing this computer right now, so no code was shown. See what Settings says, then try again."));
      }
      setQr({ ...r, until: Date.now() + r.expires_in * 1000 });
      setGuideStep(2);
    });
  const closeGuide = () => {
    setGuideStep(null);
    setQr(null);
  };
  const setTailscaleMode = (on: boolean) =>
    run(async () => {
      await mobileSecure("mobile.set_tailscale", { on });
      await checkTailscale();
    });
  // What takes protection away asks the person, and is back to what holds now if they did not verify: the status did not change, so nothing else redraws it.
  const setLock = (patch: Partial<NonNullable<MobileStatus["lock"]>>) => run(() => mobileSecure("mobile.set_lock", patch));
  const resetLock = (d: MobileDevice) => run(() => mobileSecure("mobile.reset_lock", { id: d.id }));
  const setPermission = (d: MobileDevice, permission: MobilePermission) =>
    run(() => mobileSecure("mobile.set_permission", { id: d.id, permission }));
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
  const shownError = () => (error()?.includes("could not be saved") ? s()?.error : error() ?? s()?.error);
  const deviceName = (id: string) => s()?.devices.find((d) => d.id === id)?.name ?? id;
  const hasAccess = () => !!s() && (s()!.enabled || s()!.devices.length > 0 || (s()!.legacy ?? []).length > 0);
  return (
    <>
      <LoadBar when={busy()} />
      <SettingRow id="mobile-access">
        <Toggle on={!!s()?.enabled} onChange={(on) => void (on ? enable() : run(() => daemon("mobile.disable")))} />
      </SettingRow>
      <Show when={shownError()}><div class="form-note error">{errorText(shownError())}</div></Show>
      <Show when={s()?.store_error}>
        <div class="form-note error" data-store-error>
          {t("The phone access settings could not be read ({error}). A copy was kept next to them; phone access stays off until you reset it.", { error: s()!.store_error! })}
          {" "}
          <button class="btn danger" disabled={busy()} onClick={() => void reset()}>{t("Reset phone access")}</button>
        </div>
      </Show>
      <Show when={s()?.enabled}>
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
            <div class="form-note warn-note">{t("Plain HTTP: what phones see and send is still end-to-end encrypted, but a browser page served this way could be swapped by someone on the network, so browsers connected over plain HTTP outside Tailscale can only read.")}</div>
          </Show>
        </SettingRow>
        <SettingRow id="mobile-tls" wide>
          <div class="inline-fields">
            <input class="text-input mono" placeholder={t("certificate (.crt / .pem)")} value={s()!.cert ?? ""} onChange={(e) => void enable({ cert: e.currentTarget.value })} />
            <input class="text-input mono" placeholder={t("private key (.key / .pem)")} value={s()!.key ?? ""} onChange={(e) => void enable({ key: e.currentTarget.value })} />
          </div>
        </SettingRow>
        <Show when={store.daemonApi() >= TAILSCALE_API}>
          <SettingRow id="mobile-tailscale" wide>
            <div class="inline-fields" style={{ "align-items": "center" }}>
              <Toggle on={!!s()!.tailscale?.on} onChange={(on) => void setTailscaleMode(on)} />
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
            <Show when={!s()!.app}>
              <div class="form-note warn-note">{t("This window is not the installed Kelpie app, so it cannot change this.")}</div>
            </Show>
            <div class="muted-inline" data-tailscale-privacy>{t("This computer's name goes into public certificate logs when Tailscale gets its certificate, and that cannot be undone. Tailscale sees which devices connect and when, not what they say. Tailnet Lock protects you from a key handed out without your approval. The free Tailscale plan is for non-commercial use.")}</div>
            <Show when={tailscale()?.origin}>
              <div class="muted-inline">{t("A phone paired over plain HTTP keeps working there, read-only. To type over Tailscale it must be paired again: a browser keeps its pairing per address.")}</div>
              <button class="btn primary" data-tailscale-pair disabled={busy() || !s()!.running} onClick={() => setGuideStep(1)}><Icon name="plus" size={13} /> {t("Pair a phone over Tailscale")}</button>
            </Show>
          </SettingRow>
        </Show>
        <Show when={guideStep() && tailscale()?.origin}>
          <div class="overlay" data-tailscale-guide={guideStep()}>
            <div class="modal" style={{ width: "560px" }}>
              <Show when={guideStep() === 1} fallback={
                <>
                  <h2>{t("Step 2: Pair")}</h2>
                  <Show when={qr() && qr()!.until > now()} fallback={
                    <>
                      <div style={{ color: "var(--text-dim)" }}>{t("The code expired. Show a new one.")}</div>
                      <button class="btn primary" disabled={busy()} onClick={() => void pairOverTailscale()}>{t("Show a new code")}</button>
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
                  <div class="mobile-qr-code" innerHTML={tailscale()!.install_qr ?? ""} data-install-link={`https://${tailscale()!.origin}/`} />
                  <div>
                    <div>{t("Turn on Tailscale on your phone and scan this with its camera. The page shows how to install Kelpie on the phone; then open it from the Home Screen.")}</div>
                    <div class="muted-inline">{t("This code holds no secret. The very first visit can take up to a minute while Tailscale gets its certificate.")}</div>
                    <div class="mono mobile-qr-url">{`https://${tailscale()!.origin}/`}</div>
                    <button class="btn primary" data-guide-next disabled={busy()} onClick={() => void pairOverTailscale()}>{t("Installed: Step 2")}</button>
                  </div>
                </div>
              </Show>
              <div class="footer"><span style={{ flex: 1 }} /><button class="btn" data-guide-close onClick={closeGuide}>{t("Close")}</button></div>
            </div>
          </div>
        </Show>
        <Show when={store.daemonApi() >= LOCK_API && s()!.lock}>
          <SettingRow id="mobile-lock" wide>
            <div class="inline-fields" style={{ "align-items": "center" }}>
              <Toggle on={!!s()!.lock!.on} onChange={(on) => void setLock({ on })} />
              <span>{t("Lock after")}</span>
              <select class="text-input" data-lock-idle value={s()!.lock!.idle_s} disabled={busy() || !s()!.lock!.on} onChange={(e) => {
                const select = e.currentTarget;
                void setLock({ idle_s: +select.value }).then(() => { select.value = String(s()?.lock?.idle_s ?? select.value); });
              }}>
                <For each={IDLE_SECONDS}>{(n) => <option value={n}>{t("{n} min idle", { n: n / 60 })}</option>}</For>
              </select>
            </div>
            <Show when={!s()!.app}>
              <div class="form-note warn-note">{t("This window is not the installed Kelpie app, so it cannot change this.")}</div>
            </Show>
          </SettingRow>
          <Show when={s()!.lock!.on}>
            <SettingRow id="mobile-lock-recheck">
              <Toggle on={!!s()!.lock!.recheck} onChange={(recheck) => void setLock({ recheck })} />
            </SettingRow>
            <SettingRow id="mobile-lock-plain">
              <Toggle on={!!s()!.lock!.plain_read} onChange={(plain_read) => void setLock({ plain_read })} />
            </SettingRow>
          </Show>
        </Show>
        <SettingRow id="mobile-pair" wide>
          <Show when={!s()!.app}>
            <div class="form-note warn-note">{t("This window is not the installed Kelpie app, so it cannot approve phones or raise what they may do.")}</div>
          </Show>
          <Show when={qr() && !guideStep() && qr()!.until > now()} fallback={
            <button class="btn primary" disabled={!s()!.running || busy()} onClick={() => void pairBrowser()}><Icon name="plus" size={13} /> {t("Pair a browser on this network")}</button>
          }>
            <div class="mobile-qr">
              <div class="mobile-qr-code" innerHTML={qr()!.qr} data-link={qr()!.url} />
              <div>
                <div>{t("Scan with the phone's camera. The phone then shows six digits: type them here when Kelpie asks.")}</div>
                <div class="muted-inline">{t("The code works once and expires in {n} s.", { n: Math.max(0, Math.round((qr()!.until - now()) / 1000)) })}</div>
                <Show when={!qr()!.tls}><div class="form-note warn-note">{t("Plain HTTP: a browser paired this way can only read, unless it connects over Tailscale.")}</div></Show>
                <Show when={qr()!.tls && qr()!.passkey === false}><div class="form-note warn-note" data-pair-no-passkey>{t("This address is a number, and a passkey needs a name, so a phone paired here can only read while the app lock is on.")}</div></Show>
                <div class="mono mobile-qr-url">{qr()!.url.split("#")[0]}</div>
                <button class="btn" onClick={() => setQr(null)}>{t("Cancel")}</button>
              </div>
            </div>
          </Show>
        </SettingRow>
      </Show>
      <Show when={!s()?.enabled && s()?.tailscale?.to_remove}>
        <SettingRow id="mobile-tailscale" wide>
          <Show when={tailscale()} fallback={<div class="form-note warn-note" data-tailscale-to-remove>{t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere. Kelpie removes it once Tailscale answers: start Tailscale, then check again.")}</div>}>
            <div class="form-note warn-note" data-tailscale-to-remove data-tailscale-serve={tailscale()!.serve}>{tailscale()!.state === "ready" ? tailscaleInstruction(tailscale()!) : t("An entry of Kelpie's is still in Tailscale Serve, leading nowhere. Kelpie removes it once Tailscale answers: start Tailscale, then check again.")}</div>
          </Show>
          <button class="btn" data-tailscale-check disabled={tailscale.loading} onClick={() => void checkTailscale()}>{t("Check again")}</button>
        </SettingRow>
      </Show>
      <Show when={s()}>
        <SettingRow id="mobile-devices" wide>
          <div class="saved-layouts">
            <For each={s()!.devices} fallback={<div class="muted-inline">{t("No paired devices.")}</div>}>
              {(d) => (
                <div class="saved-layout" data-device={d.id} data-state={d.state}>
                  <Icon name="server" size={16} />
                  <div class="saved-layout-main">
                    <div>{d.name} <span class="muted-inline">· {d.kind === "app" ? t("Kelpie app") : t("Browser")}<Show when={d.push}> · {t("alerts on")}</Show><Show when={d.passkey}> · {t("app lock set up")}</Show><Show when={d.lock_reset}> · {t("may set up a new passkey")}</Show></span></div>
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
                      <option value="read">{t("May read")}</option>
                      <option value="write">{t("May type")}</option>
                      <Show when={d.kind === "app"}><option value="control">{t("May take control")}</option></Show>
                    </select>
                  </Show>
                  <Show when={d.state === "ok" && d.secure_origin && store.daemonApi() >= LOCK_API}>
                    <button class="btn" data-reset-lock disabled={busy() || !!d.lock_reset} onClick={() => void resetLock(d)}>{t("Reset this phone's lock")}</button>
                  </Show>
                  <button class="btn" onClick={async () => (await confirmDialog(t("Unpair “{name}”? It is disconnected at once.", { name: d.name }))) && void run(() => daemon("mobile.revoke", { id: d.id }))}>{t("Unpair")}</button>
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
              <button class="btn danger" data-revoke-all onClick={() => void revokeAll()}>{t("Turn off phone access and revoke all")}</button>
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
      </Show>
    </>
  );
}

interface PairRequest {
  id: string;
  name: string;
  kind: string;
  fingerprint: string;
  want: MobilePermission;
  address: string;
}

/** Asks whether a phone that scanned the pairing code may connect (shown over everything): the person types the six digits
 *  the phone shows. Also says when a phone was added, changed or removed. */
export function MobileApprovals() {
  const [requests, setRequests] = createSignal<PairRequest[]>([]);
  const [codes, setCodes] = createSignal<Record<string, string>>({});
  const [errors, setErrors] = createSignal<Record<string, string>>({});
  /** The request being allowed: a second click would burn another of its three tries and raise a second prompt. */
  const [allowing, setAllowing] = createSignal<string | null>(null);
  const add = (p: PairRequest) => setRequests((r) => (r.some((x) => x.id === p.id) ? r : [...r, p]));
  onMount(() => {
    // Requests waiting already: this window was reloaded, or came up after the phone scanned.
    void mobileStatus().then((s) => s.pending.forEach(add)).catch(() => {});
    const un = onEvent<{ event: string; params: any }>("daemon-event", (m) => {
      if (m.event === "mobile.pair_request") add({ id: m.params.id, name: m.params.name, kind: m.params.kind ?? "pwa", fingerprint: m.params.fingerprint ?? "", want: m.params.want ?? "read", address: m.params.address ?? "" });
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
      {(r) => (
        <div class="overlay mobile-approve" data-request={r.id}>
          <div class="modal" style={{ width: "460px" }}>
            <h2>{t("Pair “{name}”?", { name: r.name })}</h2>
            <div style={{ "font-weight": 600, margin: "4px 0" }}>{t("Did you just scan a QR code on your phone?")}</div>
            <div style={{ color: "var(--text-dim)" }}>{t("A device that scanned your pairing code wants to see your sessions. Only approve a device you are holding: type the six digits it shows.")}</div>
            <div class="muted-inline" style={{ margin: "8px 0 0" }} data-request-details>
              <div>{t("Device key (the phone shows the same): {fingerprint}", { fingerprint: r.fingerprint })}</div>
              <div>{t("It asks to: {what}", { what: r.want === "read" ? t("read") : r.want === "control" ? t("read, type and take control") : t("read and type") })}</div>
              <Show when={r.address}><div>{t("From address: {address}", { address: r.address })}</div></Show>
            </div>
            <input class="text-input mono mobile-approve-code" inputmode="numeric" maxlength="7" autocomplete="off" placeholder="000000" style={{ "font-size": "20px", "letter-spacing": "4px", width: "160px", margin: "12px 0" }}
              value={codes()[r.id] ?? ""} onInput={(e) => setCodes((c) => ({ ...c, [r.id]: e.currentTarget.value }))} />
            <Show when={errors()[r.id]}><div class="form-note error">{errors()[r.id]}</div></Show>
            <div class="footer">
              <button class="btn" onClick={() => deny(r.id)}>{t("Deny")}</button>
              <span style={{ flex: 1 }} />
              <button class={r.want === "read" ? "btn primary" : "btn"} data-allow="read" disabled={allowing() === r.id} onClick={() => void allow(r, "read")}>{t("Allow reading")}</button>
              <button class={r.want === "read" ? "btn" : "btn primary"} data-allow="write" disabled={allowing() === r.id} onClick={() => void allow(r, "write")}>{t("Allow reading and typing")}</button>
            </div>
          </div>
        </div>
      )}
    </For>
  );
}
