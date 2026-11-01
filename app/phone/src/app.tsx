// The phone app's frame: header with the connection indicator, the banners that say what is wrong,
// and the inbox or the open session below.
import { createSignal, For, Show } from "solid-js";
import { clock, describeConn, ago } from "./conn-text";
import { Inbox } from "./inbox";
import { localeSetting, setLocale, t, type LocaleSetting } from "./i18n";
import { closeSession, conn, connected, device, deviceNews, forgetDevice, link, lastLiveAt, movedHost, notPairedMessage, now, openId, paintedFrom, paired, pairDevice, pairing, pairShown, plainHttpOrigin, sessions, setPairing } from "./phone-state";
import { InstallGuide, ScanCode, screenNow } from "./install-guide";
import { labelOf, needsCount } from "./inbox-order";
import { SessionView } from "./session-view";
import { setTheme, theme, type ThemeSetting } from "./theme";

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
  const info = () => describeConn(conn());
  return (
    <button class={`conn ${info().tone}`} onClick={props.onClick} aria-label={t("Connection")} data-state={conn().kind}>
      <span class="dot" />
      <span class="conn-label">{info().label}</span>
    </button>
  );
}

/** Under the header whenever what is on screen is not the computer as it is now. */
function StateBanner() {
  const info = () => describeConn(conn());
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
        <div class="banner warn" role="status" data-banner="plain-http">{t("Plain HTTP: someone on this network could swap this page, so it can only read here. Open it over HTTPS or Tailscale to type.")}</div>
      </Show>
      <Show when={movedHost()}>
        <div class="banner" role="status" data-banner="moved">
          {t("This computer can be reached at {host}. A browser keeps its pairing per address, so this device must be paired again there.", { host: movedHost()! })}
          <div class="actions"><a class="btn" href={`https://${movedHost()}/`}>{t("Open it")}</a></div>
        </div>
      </Show>
      <Show when={deviceNews()}><div class="banner" role="status" data-banner="device-news">{deviceNews()}</div></Show>
    </>
  );
}

function Sheet(props: { onClose: () => void }) {
  const info = () => describeConn(conn());
  return (
    <div class="sheet-back" onClick={props.onClose}>
      <div class="sheet" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={t("Connection")}>
        <h2>{info().label}</h2>
        <p class="muted">{info().detail}</p>
        <Show when={device()}><p class="muted">{t("This device: {name}", { name: device()!.name })} · {device()!.can_write ? t("can type") : t("read only")}</p></Show>
        <label class="row-setting">{t("Language")}
          <select class="field" value={localeSetting()} onChange={(e) => setLocale(e.currentTarget.value as LocaleSetting)}>
            <option value="auto">{t("Automatic")}</option>
            <option value="en">English</option>
            <option value="vi">Tiếng Việt</option>
          </select>
        </label>
        <label class="row-setting">{t("Theme")}
          <select class="field" value={theme()} onChange={(e) => setTheme(e.currentTarget.value as ThemeSetting)}>
            <For each={["auto", "light", "dark"] as const}>{(v) => <option value={v}>{v === "auto" ? t("Automatic") : v === "light" ? t("Light") : t("Dark")}</option>}</For>
          </select>
        </label>
        <div class="actions">
          <button class="btn" onClick={() => { link.kick(); props.onClose(); }}>{t("Reconnect now")}</button>
          <button class="btn danger" onClick={() => { if (confirm(t("Forget this pairing on this device?"))) { forgetDevice(); props.onClose(); } }}>{t("Forget this device")}</button>
        </div>
        <button class="btn" onClick={props.onClose}>{t("Close")}</button>
      </div>
    </div>
  );
}

export function App() {
  const [sheet, setSheet] = createSignal(false);
  const title = () => {
    const id = openId();
    const s = id ? sessions().find((x) => x.id === id) : undefined;
    return s ? labelOf(s) : "Kelpie";
  };
  const badge = () => needsCount(sessions());
  const back = () => (history.state?.s ? history.back() : closeSession());
  return (
    <>
      <header id="top">
        <Show when={openId()}>
          <button class="icon" aria-label={t("Back")} onClick={back}>‹</button>
        </Show>
        <h1>{title()}</h1>
        <Show when={!openId() && badge() > 0}><span class="badge" aria-label={t("Sessions that need you")}>{badge()}</span></Show>
        <Show when={paired()}><ConnIndicator onClick={() => setSheet(true)} /></Show>
      </header>
      <StateBanner />
      <SecurityBanner />
      <main id="view">
        <Show when={paired()} fallback={<NotPaired />}>
          <Show when={openId()} keyed fallback={<Inbox />}>{(id) => <SessionView id={id} />}</Show>
        </Show>
      </main>
      <Show when={sheet()}><Sheet onClose={() => setSheet(false)} /></Show>
    </>
  );
}
