// The phone app's frame: header with the connection indicator, the banners that say what is wrong,
// and the inbox, the open session or Settings below.
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { clock, describeConn, ago } from "./conn-text";
import { Inbox } from "./inbox";
import { t } from "./i18n";
import { closeSession, conn, connected, device, deviceNews, forgetDevice, link, lastLiveAt, livePathNow, lockShown, movedHost, notPairedMessage, now, openId, paintedFrom, paired, pairDevice, pairing, pairShown, plainHttpOrigin, remoteScreen, remoteScreens, sessions, setPairing, setTrustOpen, takeover, takeovers, trustOpen } from "./phone-state";
import { APP_VERSION, HOSTED, servedVersion, trustText, type ServedVersion } from "./hosted";
import { PaneTakeover } from "./pane-takeover";
import { ScreenView } from "./screen-view";
import { LockCover, LockScreen, RecheckSheet } from "./lock-screen";
import { InstallGuide, ScanCode, screenNow } from "./install-guide";
import { labelOf, needsCount } from "./inbox-order";
import { SessionView } from "./session-view";
import { SettingsView } from "./settings-view";

function guessName(): string {
  const ua = navigator.userAgent;
  if (/iPhone/.test(ua)) return "iPhone";
  if (/iPad/.test(ua)) return "iPad";
  if (/Android/.test(ua)) return t("Android phone");
  return t("Browser");
}

/** What a page that is not paired shows: installing first on a secure origin, then the code, then naming this device. */
function NotPaired() {
  const screen = () => screenNow(paired(), !!pairing());
  return (
    <Show when={screen() !== "install"} fallback={<InstallGuide />}>
      <Show when={screen() !== "scan"} fallback={<ScanCode done={setPairing} message={notPairedMessage()} />}>
        <Pairing />
      </Show>
    </Show>
  );
}

function Pairing() {
  let nameBox!: HTMLInputElement;
  const [status, setStatus] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const go = async () => {
    setBusy(true);
    setStatus(t("Connecting to your computer…"));
    const error = await pairDevice(pairing()!, nameBox.value);
    if (error) {
      setStatus(error);
      setBusy(false);
    }
  };
  return (
    <div class="card">
      <h2>{pairing() ? t("Pair this device") : t("Not paired")}</h2>
      <Show when={pairing()} fallback={<p class="muted">{notPairedMessage() ?? t("In Kelpie on your computer open Settings → Mobile → Pair a phone, and scan the QR code with this device.")}</p>}>
        <Show when={pairShown()} fallback={
          <>
            <p class="muted">{t("Name this device. Your computer then asks you to type a code this phone shows.")}</p>
            <input class="field" ref={nameBox} value={guessName()} maxlength="40" />
            <p />
            <button class="btn primary" disabled={busy()} onClick={go}>{t("Pair this device")}</button>
            <p class="muted" role="status">{status()}</p>
          </>
        }>
          <p>{t("Type this code into Kelpie on your computer:")}</p>
          <p class="pair-code" data-pair-code>{pairShown()!.digits}</p>
          <p class="muted">{t("This device's key, which your computer shows too:")} <span class="pair-key" data-pair-key>{pairShown()!.key}</span></p>
        </Show>
      </Show>
    </div>
  );
}

/** The always-visible connection indicator: a dot and a short word, the whole thing a button. */
function ConnIndicator(props: { onClick: () => void }) {
  const info = () => describeConn(conn(), livePathNow());
  return (
    <button class={`conn ${info().tone}`} onClick={props.onClick} aria-label={t("Connection")} data-state={conn().kind}>
      <span class="dot" />
      <span class="conn-label">{info().label}</span>
    </button>
  );
}

/** Under the header whenever what is on screen is not the computer as it is now. */
function StateBanner() {
  const info = () => describeConn(conn(), livePathNow());
  const shown = () => !["live", "connecting"].includes(conn().kind) || (conn().kind === "connecting" && paintedFrom() !== null);
  const asOf = () => {
    const at = paintedFrom() ?? lastLiveAt();
    return at ? t("As of {time} ({ago})", { time: clock(at, true), ago: ago(at, now()) }) : "";
  };
  return (
    <Show when={shown() && paired()}>
      <div class={`banner ${info().tone}`} role="status" data-banner={conn().kind} onClick={() => conn().kind === "replaced" && link.kick()}>
        <b>{info().label}</b> {info().detail}
        <Show when={!connected() && asOf()}><div class="asof">{asOf()}</div></Show>
        <Show when={conn().kind === "not_paired_here"}>
          <div class="actions"><button class="btn danger" data-forget onClick={() => { if (confirm(t("Forget this pairing on this device?"))) forgetDevice(); }}>{t("Forget this device")}</button></div>
        </Show>
      </div>
    </Show>
  );
}

/** Why this page may only read, and news about the computer's other devices. */
function SecurityBanner() {
  return (
    <>
      <Show when={plainHttpOrigin() || device()?.capped}>
        <Show when={location.protocol === "https:"} fallback={<div class="banner warn" role="status" data-banner="plain-http">{t("Plain HTTP: someone on this network could swap this page, so it can only read here. To type, pair it again through Tailscale's HTTPS address.")}</div>}>
          <div class="banner warn" role="status" data-banner="no-passkey">{t("This device has no passkey, so it can only read here. To type, pair it again through Tailscale or at an HTTPS name.")}</div>
        </Show>
      </Show>
      <Show when={movedHost()}>
        <div class="banner" role="status" data-banner="moved">
          {HOSTED
            ? t("Your computer now uses the relay at {host}. This app works only through the relay it came from, so this phone must be paired again there.", { host: movedHost()! })
            : t("This computer can be reached at {host}. A browser keeps its pairing per address, so this device must be paired again there.", { host: movedHost()! })}
          <div class="actions"><a class="btn" href={`https://${movedHost()}/`}>{t("Open it")}</a></div>
        </div>
      </Show>
      <Show when={deviceNews() && lockShown().clear}><div class="banner" role="status" data-banner="device-news">{deviceNews()}</div></Show>
    </>
  );
}

/** What the hosted app trusts, its version, and the fingerprint the relay says it serves (asked of the network each time). */
function TrustSheet(props: { onClose: () => void }) {
  const [served, setServed] = createSignal<ServedVersion | null | undefined>(undefined);
  void servedVersion().then(setServed);
  const said = () => {
    const s = served();
    if (s === undefined) return t("Asking the relay which version it serves…");
    return s ? t("The relay says it serves {version}, with this fingerprint:", { version: s.version }) : t("The relay did not say which version it serves.");
  };
  return (
    <div class="sheet-back" onClick={props.onClose}>
      <div class="sheet" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={t("What this app trusts")} data-trust>
        <h2>{t("What this app trusts")}</h2>
        <For each={trustText(t)}>{(p) => <p class="muted">{p}</p>}</For>
        <p class="muted" data-trust-version>{t("This app's version: {version}", { version: APP_VERSION })}</p>
        <p class="muted">{said()}</p>
        <Show when={served()}><p class="code command" data-trust-bundle>{served()!.bundle}</p></Show>
        <button class="btn" onClick={props.onClose}>{t("Close")}</button>
      </div>
    </div>
  );
}

/** The connection in a sentence, Reconnect, and the way to Settings (behind the lock, like everything else). Locked, the sheet still
 *  forgets the pairing: a phone whose computer is gone for good could not unpair otherwise, and forgetting harms only its owner. */
function Sheet(props: { onClose: () => void; onSettings: () => void }) {
  const info = () => describeConn(conn(), livePathNow());
  return (
    <div class="sheet-back" onClick={props.onClose}>
      <div class="sheet" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={t("Connection")}>
        <h2>{info().label}</h2>
        <p class="muted">{info().detail}</p>
        <div class="actions">
          <button class="btn" onClick={() => { link.kick(); props.onClose(); }}>{t("Reconnect now")}</button>
          <Show when={lockShown().clear} fallback={
            <button class="btn danger" data-sheet-forget onClick={() => { if (confirm(t("Forget this pairing on this device?"))) { forgetDevice(); props.onClose(); } }}>{t("Forget this device")}</button>
          }>
            <button class="btn" data-sheet-settings onClick={() => { props.onClose(); props.onSettings(); }}>{t("Settings")}</button>
          </Show>
        </div>
        <button class="btn" onClick={props.onClose}>{t("Close")}</button>
      </div>
    </div>
  );
}

export function App() {
  const [sheet, setSheet] = createSignal(false);
  const [settings, setSettings] = createSignal(false);
  // Settings has an entry of its own in the history, so the phone's own Back leaves it for where the person was.
  const onPop = () => setSettings(!!(history.state as { settings?: boolean } | null)?.settings);
  window.addEventListener("popstate", onPop);
  onCleanup(() => window.removeEventListener("popstate", onPop));
  const openSettings = () => {
    // A screen or a pane under control ends (the gear is not shown over them; the sheet's Settings button can still lead here).
    remoteScreens.close();
    takeovers.close();
    if (!settings()) history.pushState({ settings: true }, "");
    setSettings(true);
  };
  const closeSettings = () => {
    if ((history.state as { settings?: boolean } | null)?.settings) history.back();
    setSettings(false);
  };
  // A pairing that ends (forgotten, or removed on the computer) leaves no Settings behind for the next one.
  createEffect(() => {
    if (!paired()) setSettings(false);
  });
  // Behind the lock nothing says what is there: not a session's name, not how many need the person.
  const open = () => !paired() || lockShown().clear;
  const inSettings = () => settings() && paired() && open();
  const title = () => {
    if (inSettings()) return t("Settings");
    if (takeover() && open()) return t("Browser pane");
    if (remoteScreen() && open()) return t("Computer screen");
    const id = openId();
    const s = id && open() ? sessions().find((x) => x.id === id) : undefined;
    return s ? labelOf(s) : "Kelpie";
  };
  const badge = () => (open() ? needsCount(sessions()) : 0);
  const back = () => (inSettings() ? closeSettings() : remoteScreen() ? remoteScreens.close() : takeover() ? takeovers.close() : history.state?.s ? history.back() : closeSession());
  return (
    <>
      <header id="top">
        <Show when={(inSettings() || openId() || takeover() || remoteScreen()) && open()}>
          <button class="icon" aria-label={t("Back")} onClick={back}>‹</button>
        </Show>
        <h1>{title()}</h1>
        <Show when={!openId() && !inSettings() && badge() > 0}><span class="badge" aria-label={t("Sessions that need you")}>{badge()}</span></Show>
        <Show when={paired() && lockShown().clear && !inSettings() && !remoteScreen() && !takeover()}>
          <button class="gear" data-open-settings aria-label={t("Settings")} onClick={openSettings}><svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" /></svg></button>
        </Show>
        <Show when={paired()}><ConnIndicator onClick={() => setSheet(true)} /></Show>
      </header>
      <StateBanner />
      <SecurityBanner />
      <main id="view">
        <Show when={paired()} fallback={<NotPaired />}>
          <Show when={lockShown().clear} fallback={<LockScreen />}>
            <Show when={!settings()} fallback={<SettingsView onClose={closeSettings} />}>
              <Show when={!remoteScreen()} fallback={<ScreenView />}>
                <Show when={!takeover()} fallback={<PaneTakeover />}>
                  <Show when={openId()} keyed fallback={<Inbox />}>{(id) => <SessionView id={id} />}</Show>
                </Show>
              </Show>
            </Show>
          </Show>
        </Show>
      </main>
      <Show when={sheet()}><Sheet onClose={() => setSheet(false)} onSettings={openSettings} /></Show>
      <Show when={HOSTED && trustOpen()}><TrustSheet onClose={() => setTrustOpen(false)} /></Show>
      <RecheckSheet />
      <LockCover />
    </>
  );
}
