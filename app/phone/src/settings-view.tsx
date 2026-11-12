// The phone's Settings: the app lock as the computer set it and how soon leaving locks it here, alerts, how text looks, how the inbox lists sessions, the key bar,
// vibration, how this phone reaches its computer, what it keeps, and what this app is. Everything here is kept on this phone, but for
// alerts (the computer sends them) and the lock's own facts (changed on the computer only).
import { createSignal, For, Show } from "solid-js";
import { methodText, pathText } from "./conn-path";
import { APP_VERSION, HOSTED } from "./hosted";
import { localeSetting, setLocale, t, type LocaleSetting } from "./i18n";
import { inboxViewName } from "./inbox";
import type { InboxOrder } from "./inbox-order";
import { KEYS } from "./key-bar";
import { currentPairing } from "./pairing-store";
import { alertsError, alertsState, clearKept, connected, daemonApi, device, forgetDevice, link, livePathNow, lockAppNow, lockInfo, lockShown, PAGE_METHOD, rtt, screenWhyNow, setTrustOpen, testAlert, turnAlertsOff, turnAlertsOn } from "./phone-state";
import { applyDisplay, confirmCtrlC, defaultKeyBar, INBOX_VIEWS, inboxView, keyBar, LOCK_GRACE_CHOICES, lockGrace, moveKey, order, showStopped, TEXT_SIZES, textSize, toggleKey, vibrate, wrapLines, type InboxView, type LockGrace, type TextSize } from "./prefs";
import { setTheme, theme, type ThemeSetting } from "./theme";

/** Alerts on this phone, with the computer's own push: what state they are in and the one thing to do next. */
function AlertsRow() {
  const [tested, setTested] = createSignal<string | null>(null);
  const test = async () => {
    setTested(t("Sending…"));
    const failed = await testAlert();
    setTested(failed ? t("Failed: {error}", { error: failed }) : t("Sent: it should show on this phone in a moment."));
  };
  const why = () => {
    switch (alertsState()) {
      case "plain_http":
        return t("Alerts need HTTPS, which a plain http:// page does not have. Open Kelpie through Tailscale, or use ntfy from Kelpie → Settings → Notifications.");
      case "install_first":
        return t("On an iPhone, alerts only work in the installed app: add Kelpie to the Home Screen and open it from there.");
      case "no_support":
        return t("This browser cannot receive alerts. ntfy or Telegram work instead: Kelpie → Settings → Notifications.");
      case "blocked":
        return t("Alerts are blocked for this app. Allow notifications for it in this phone's settings, then come back.");
      case "failed":
        return t("Alerts could not be turned on: {error}", { error: alertsError() ?? "" });
      case "on":
        return t("Alerts are on. They say only that a session needs you, and tapping one opens it.");
      default:
        return t("Get an alert on this phone when a session needs you, with the app closed and the phone locked. This computer has to be awake to send it.");
    }
  };
  return (
    <div class="alerts-row" data-alerts={alertsState()}>
      <h2>{t("Alerts")}</h2>
      <p class="muted">{why()}</p>
      <div class="actions">
        <Show when={["off", "failed"].includes(alertsState())}><button class="btn primary" data-alerts-on onClick={() => void turnAlertsOn()}>{t("Turn on alerts")}</button></Show>
        <Show when={alertsState() === "asking"}><button class="btn" disabled>{t("Asking…")}</button></Show>
        <Show when={alertsState() === "on"}>
          <button class="btn" data-alerts-test onClick={() => void test()}>{t("Send a test alert")}</button>
          <button class="btn" data-alerts-off onClick={() => void turnAlertsOff()}>{t("Turn off alerts")}</button>
        </Show>
      </div>
      <Show when={tested()}><p class="muted" role="status" data-alerts-tested>{tested()}</p></Show>
    </div>
  );
}

/** One fact, read only: a name and what it is. */
function Fact(props: { name: string; setting: string; children: string }) {
  return (
    <div class="fact" data-setting={props.setting}>
      <dt>{props.name}</dt>
      <dd>{props.children}</dd>
    </div>
  );
}

function LockSection() {
  const face = () => lockShown().face;
  /** No passkey lock applies to this phone here: nothing to lock after leaving. */
  const noLock = () => face() === "none" || face() === "read_only";
  const idleMin = () => {
    const s = lockInfo().idle_s;
    return typeof s === "number" ? Math.round(s / 60) : null;
  };
  const graceText = (s: LockGrace) => {
    const name = s === 0 ? t("Immediately") : t("{n} min", { n: s / 60 });
    const idle = lockInfo().idle_s;
    // The computer locks an idle phone sooner than this: the choice cannot take effect in full.
    return typeof idle === "number" && s > idle ? t("{choice} (limited to {n} min by your computer)", { choice: name, n: idleMin()! }) : name;
  };
  return (
    <section class="card" data-settings-section="lock">
      <h2>{t("App lock")}</h2>
      <label class="row-setting" data-setting="lock-grace">{t("Lock after leaving the app")}
        <select class="field" value={String(lockGrace.get())} disabled={noLock()} onChange={(e) => lockGrace.set(Number(e.currentTarget.value) as LockGrace)}>
          <For each={LOCK_GRACE_CHOICES}>{(s) => <option value={String(s)}>{graceText(s)}</option>}</For>
        </select>
      </label>
      <Show when={noLock()}>
        <p class="muted" data-lock-grace-why>
          {face() === "read_only" ? t("This address cannot have a passkey, so this phone only reads here and has no lock.") : t("The app lock is off on your computer, so this phone does not lock.")}
        </p>
      </Show>
      <Show when={face() === "open"}>
        <div class="actions"><button class="btn" data-setting="lock-now" onClick={lockAppNow}>{t("Lock now")}</button></div>
      </Show>
      <dl class="facts" data-setting="lock-facts">
        <Fact name={t("App lock")} setting="lock-on">{noLock() ? t("Off") : t("On")}</Fact>
        <Show when={!noLock() && idleMin() !== null}>
          <Fact name={t("Locks after no use for")} setting="lock-idle">{t("{n} min", { n: idleMin()! })}</Fact>
        </Show>
        <Show when={!noLock()}>
          <Fact name={t("Dangerous approvals ask for Face ID again")} setting="lock-recheck">{lockInfo().recheck ? t("On") : t("Off")}</Fact>
        </Show>
      </dl>
      <p class="muted">{t("Change these in Kelpie on your computer → Settings → Mobile.")}</p>
    </section>
  );
}

function DisplaySection() {
  const sizeName = (s: TextSize) => (s === "s" ? t("Small") : s === "m" ? t("Medium") : s === "l" ? t("Large") : t("Extra large"));
  return (
    <section class="card" data-settings-section="display">
      <h2>{t("Display")}</h2>
      <label class="row-setting" data-setting="theme">{t("Theme")}
        <select class="field" value={theme()} onChange={(e) => setTheme(e.currentTarget.value as ThemeSetting)}>
          <For each={["auto", "light", "dark"] as const}>{(v) => <option value={v}>{v === "auto" ? t("Automatic") : v === "light" ? t("Light") : t("Dark")}</option>}</For>
        </select>
      </label>
      <label class="row-setting" data-setting="language">{t("Language")}
        <select class="field" value={localeSetting()} onChange={(e) => setLocale(e.currentTarget.value as LocaleSetting)}>
          <option value="auto">{t("Automatic")}</option>
          <option value="en">English</option>
          <option value="vi">Tiếng Việt</option>
        </select>
      </label>
      <label class="row-setting" data-setting="text-size">{t("Text size in terminals and chats")}
        <select class="field" value={textSize.get()} onChange={(e) => { textSize.set(e.currentTarget.value as TextSize); applyDisplay(); }}>
          <For each={TEXT_SIZES}>{(s) => <option value={s}>{sizeName(s)}</option>}</For>
        </select>
      </label>
      <label class="row-check" data-setting="wrap">
        <input type="checkbox" checked={wrapLines.get()} onChange={(e) => { wrapLines.set(e.currentTarget.checked); applyDisplay(); }} />
        <span>{t("Wrap long lines in terminals")}<span class="muted block">{t("Off: a long line stays whole, and the screen scrolls sideways.")}</span></span>
      </label>
    </section>
  );
}

function InboxSection() {
  return (
    <section class="card" data-settings-section="inbox">
      <h2>{t("Inbox")}</h2>
      <label class="row-setting" data-setting="inbox-view">{t("Inbox view")}
        <select class="field" value={inboxView.get()} onChange={(e) => inboxView.set(e.currentTarget.value as InboxView)}>
          <For each={INBOX_VIEWS}>{(v) => <option value={v}>{inboxViewName(v)}</option>}</For>
        </select>
      </label>
      <label class="row-setting" data-setting="inbox-order">{t("Order inside a group")}
        <select class="field" value={order.get()} onChange={(e) => order.set(e.currentTarget.value as InboxOrder)}>
          <option value="recent">{t("Recent activity")}</option>
          <option value="name">{t("Name A–Z")}</option>
        </select>
      </label>
      <p class="muted">{t("Sessions that need you always come first, the longest wait first.")}</p>
      <label class="row-check" data-setting="show-stopped">
        <input type="checkbox" checked={showStopped.get()} onChange={(e) => showStopped.set(e.currentTarget.checked)} />
        <span>{t("Show stopped sessions")}<span class="muted block">{t("Off: the inbox leaves them out and says how many it hides.")}</span></span>
      </label>
    </section>
  );
}

function KeysSection() {
  const s = keyBar.get;
  return (
    <section class="card" data-settings-section="keys">
      <h2>{t("Keys")}</h2>
      <p class="muted">{t("The keys above the reply box, in this order. Ctrl always comes first.")}</p>
      <div class="key-list" data-setting="key-bar">
        <div class="key-row"><input type="checkbox" checked disabled aria-label={t("Show")} /><span class="key">Ctrl</span></div>
        <For each={s().order}>
          {(k, i) => (
            <div class="key-row" data-key-row={k}>
              <input type="checkbox" data-key-shown={k} checked={!s().hidden.includes(k)} aria-label={t("Show")} onChange={() => keyBar.set(toggleKey(s(), k))} />
              <span class="key" title={KEYS[k].title}>{KEYS[k].label}</span>
              <span class="grow" />
              <button class="btn small" aria-label={t("Move up")} disabled={i() === 0} onClick={() => keyBar.set(moveKey(s(), k, -1))}>↑</button>
              <button class="btn small" aria-label={t("Move down")} disabled={i() === s().order.length - 1} onClick={() => keyBar.set(moveKey(s(), k, 1))}>↓</button>
            </div>
          )}
        </For>
      </div>
      <div class="actions"><button class="btn" data-key-bar-reset onClick={() => keyBar.set(defaultKeyBar())}>{t("Reset")}</button></div>
      <label class="row-check" data-setting="confirm-ctrl-c">
        <input type="checkbox" checked={confirmCtrlC.get()} onChange={(e) => confirmCtrlC.set(e.currentTarget.checked)} />
        <span>{t("Ask before sending Ctrl+C")}<span class="muted block">{t("Ctrl+C stops what the agent is doing. This asks first, from the key bar and from Ctrl with C in the reply box.")}</span></span>
      </label>
    </section>
  );
}

/** Only where the phone can buzz at all (not on an iPhone). */
function VibrationSection() {
  return (
    <Show when={"vibrate" in navigator}>
      <section class="card" data-settings-section="vibration">
        <h2>{t("Vibration")}</h2>
        <label class="row-check" data-setting="vibrate">
          <input type="checkbox" checked={vibrate.get()} onChange={(e) => vibrate.set(e.currentTarget.checked)} />
          <span>{t("Vibrate when a session needs you")}<span class="muted block">{t("While the app is open.")}</span></span>
        </label>
      </section>
    </Show>
  );
}

function ConnectionSection(props: { onClose: () => void }) {
  const may = () => {
    const d = device();
    const p = d?.permission ?? (d?.can_write ? "write" : "read");
    return p === "control" ? t("may take control") : p === "write" ? t("may type") : t("may read");
  };
  const forget = () => {
    if (!confirm(t("Forget this pairing on this device?"))) return;
    forgetDevice();
    props.onClose();
  };
  return (
    <section class="card" data-settings-section="connection">
      <h2>{t("Connection")}</h2>
      <dl class="facts">
        <Fact name={t("Computer")} setting="computer">{currentPairing()?.computer || "—"}</Fact>
        <Fact name={t("Paired through")} setting="paired-through">{methodText(PAGE_METHOD, t)}</Fact>
        <Fact name={t("Connected now through")} setting="live-path">{connected() ? pathText(livePathNow(), t) : t("Not connected")}</Fact>
        <Fact name={t("Round trip")} setting="rtt">{connected() && rtt() !== null ? t("{ms} ms", { ms: rtt()! }) : "—"}</Fact>
        <Show when={device()}>
          <Fact name={t("This device")} setting="this-device">{`${device()!.name} · ${may()}`}</Fact>
        </Show>
        <Fact name={t("Computer screen")} setting="screen">{screenWhyNow() ?? t("Available from here")}</Fact>
      </dl>
      <div class="actions">
        <button class="btn" data-setting="reconnect" onClick={() => link.kick()}>{t("Reconnect now")}</button>
        <button class="btn danger" data-setting="forget" onClick={forget}>{t("Forget this computer")}</button>
      </div>
    </section>
  );
}

function DataSection() {
  const [drafts, setDrafts] = createSignal(false);
  const [done, setDone] = createSignal<string | null>(null);
  const clear = async () => {
    await clearKept(drafts());
    setDone(drafts() ? t("Cleared, drafts too. This phone stays paired.") : t("Cleared. This phone stays paired."));
  };
  return (
    <section class="card" data-settings-section="data">
      <h2>{t("Data on this phone")}</h2>
      <p class="muted">{t("This phone keeps the session list and the last screens it saw, to show them at once when it opens.")}</p>
      <label class="row-check">
        <input type="checkbox" data-clear-drafts checked={drafts()} onChange={(e) => setDrafts(e.currentTarget.checked)} />
        <span>{t("Drafts too")}</span>
      </label>
      <div class="actions"><button class="btn" data-setting="clear-kept" onClick={() => void clear()}>{t("Clear what this phone keeps")}</button></div>
      <Show when={done()}><p class="muted" role="status" data-cleared>{done()}</p></Show>
    </section>
  );
}

function AboutSection() {
  return (
    <section class="card" data-settings-section="about">
      <h2>{t("About")}</h2>
      <dl class="facts">
        <Fact name={t("App version")} setting="version">{APP_VERSION}</Fact>
        <Fact name={t("The computer's feature level")} setting="feature-level">{daemonApi() ? String(daemonApi()) : "—"}</Fact>
      </dl>
      <Show when={HOSTED}>
        <div class="actions"><button class="btn" data-trust-open onClick={() => setTrustOpen(true)}>{t("What this app trusts")}</button></div>
      </Show>
    </section>
  );
}

export function SettingsView(props: { onClose: () => void }) {
  return (
    <div class="settings" data-settings>
      <LockSection />
      <Show when={device()}>
        <section class="card" data-settings-section="alerts" data-setting="alerts"><AlertsRow /></section>
      </Show>
      <DisplaySection />
      <InboxSection />
      <KeysSection />
      <VibrationSection />
      <ConnectionSection onClose={props.onClose} />
      <DataSection />
      <AboutSection />
    </div>
  );
}
