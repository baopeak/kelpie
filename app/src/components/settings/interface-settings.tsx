// Terminal, Appearance and Notifications sections.
import { t } from "../../lib/i18n";
import { For, Show, Suspense, lazy } from "solid-js";
import { store } from "../../lib/store";
import { NOTIFY_API } from "../../lib/notify-events";
import { terminalThemes } from "../../lib/terminal-themes";
import { applySettings, Choice, SettingRow, Toggle } from "./setting-row";

const PushSettings = lazy(() => import("./push-settings"));

const s = () => store.state.settings;

export function TerminalSettings() {
  return (
    <>
      <SettingRow id="font-family">
        <input class="text-input" value={s().fontFamily} title={s().fontFamily} onChange={(e) => applySettings({ fontFamily: e.currentTarget.value })} />
      </SettingRow>
      <SettingRow id="font-size">
        <div class="inline-fields">
          <span class="muted-inline">{t("Font size")}</span>
          <input class="text-input num" type="number" min="8" max="28" value={s().fontSize} title={t("Font size")} onChange={(e) => applySettings({ fontSize: +e.currentTarget.value || 13 })} />
          <span class="muted-inline">{t("Line height")}</span>
          <input class="text-input num" type="number" step="0.05" min="1" max="2" value={s().lineHeight} title={t("Line height")} onChange={(e) => applySettings({ lineHeight: +e.currentTarget.value || 1.2 })} />
        </div>
      </SettingRow>
      <SettingRow id="scrollback">
        <input class="text-input" type="number" min="500" max="100000" step="500" value={s().scrollback} onChange={(e) => applySettings({ scrollback: +e.currentTarget.value || 5000 })} />
      </SettingRow>
      <SettingRow id="renderer">
        <Choice value={s().webgl ? "gpu" : "dom"} options={[["gpu", t("WebGL (GPU)")], ["dom", t("DOM")]]} onChange={(v) => applySettings({ webgl: v === "gpu" })} />
      </SettingRow>
      <SettingRow id="cursor-blink">
        <Toggle on={s().cursorBlink} onChange={(on) => applySettings({ cursorBlink: on })} />
      </SettingRow>
      <SettingRow id="shell">
        <input class="text-input" placeholder={t("(system default)")} value={s().shellArgs.join(" ")}
          onChange={(e) => applySettings({ shellArgs: e.currentTarget.value.trim() ? e.currentTarget.value.trim().split(/\s+/) : [] })} />
      </SettingRow>
      <SettingRow id="terminal-theme">
        <select class="text-input" value={s().terminalTheme ?? "default"} onChange={(e) => applySettings({ terminalTheme: e.currentTarget.value })}>
          <option value="default">{t("Default (follows the app theme)")}</option>
          <For each={terminalThemes()}>{(th) => <option value={th.id}>{th.label}</option>}</For>
        </select>
      </SettingRow>
      <SettingRow id="floating-terminal">
        <Toggle on={s().floatingTerminal} onChange={(on) => { store.setSettings({ floatingTerminal: on }); if (!on) store.toggleFloating(false); }} />
      </SettingRow>
      <SettingRow id="localhost-browser">
        <Toggle on={s().localhostInBrowser !== false} onChange={(on) => store.setSettings({ localhostInBrowser: on })} />
      </SettingRow>
    </>
  );
}

export function AppearanceSettings() {
  return (
    <>
      <SettingRow id="language">
        {/* Each language is named in itself. */}
        <Choice value={s().language ?? "system"} options={[["system", t("System")], ["en", "English"], ["vi", "Tiếng Việt"]]} onChange={(v) => store.setSettings({ language: v })} />
      </SettingRow>
      <SettingRow id="theme">
        <Choice value={s().theme} options={[["dark", t("Dark")], ["light", t("Light")]]} onChange={(v) => applySettings({ theme: v })} />
      </SettingRow>
      <SettingRow id="density">
        <Choice value={s().density} options={[["comfortable", t("Comfortable")], ["compact", t("Compact")]]} onChange={(v) => store.setSettings({ density: v })} />
      </SettingRow>
      <SettingRow id="ui-scale">
        <Choice value={s().uiScale} options={[[0.9, "90%"], [1, "100%"], [1.1, "110%"], [1.2, "120%"]]} onChange={(v) => store.setSettings({ uiScale: v })} />
      </SettingRow>
      <SettingRow id="word-wrap">
        <Toggle on={s().wordWrap} onChange={(on) => store.setSettings({ wordWrap: on })} />
      </SettingRow>
      <SettingRow id="mascot">
        <Toggle on={s().mascot !== false} onChange={(on) => store.setSettings({ mascot: on })} />
      </SettingRow>
      <Show when={s().mascot !== false}>
        <SettingRow id="mascot-motion">
          <Choice value={s().mascotMotion ?? "system"} options={[["on", t("Always")], ["system", t("Follow system")], ["off", t("Off")]]} onChange={(v) => store.setSettings({ mascotMotion: v })} />
        </SettingRow>
      </Show>
    </>
  );
}

export function NotificationSettings() {
  return (
    <>
      <SettingRow id="notifications">
        <Toggle on={s().notifications} onChange={(on) => store.setSettings({ notifications: on })} />
      </SettingRow>
      <SettingRow id="notification-sound">
        <Toggle on={s().notificationSound} onChange={(on) => store.setSettings({ notificationSound: on })} />
      </SettingRow>
      <SettingRow id="shortcut-hints">
        <Toggle on={s().shortcutHints} onChange={(on) => store.setSettings({ shortcutHints: on })} />
      </SettingRow>
      <SettingRow id="tips">
        <div class="inline-fields">
          <Toggle on={s().tips} onChange={(on) => store.setSettings({ tips: on })} />
          <button class="btn" disabled={!s().tipsSeen.length} onClick={() => store.setSettings({ tips: true, tipsSeen: [] })}>{t("Show tips again")}</button>
        </div>
      </SettingRow>
      {/* Phone channels live in the session daemon (API 12); the page loads on first use. */}
      <Show when={store.daemonApi() >= NOTIFY_API}>
        <Suspense><PushSettings /></Suspense>
      </Show>
    </>
  );
}

export function UsageSettings() {
  return (
    <>
      <SettingRow id="usage-meters">
        <Toggle on={s().usageMeters} onChange={(on) => store.setSettings({ usageMeters: on })} />
      </SettingRow>
      <SettingRow id="cost-cap">
        <div class="inline-fields">
          <input class="text-input" type="number" min="0" step="1" style={{ width: "90px" }} value={s().costCap ?? 0} title={t("US dollars (0 = off)")}
            onChange={(e) => store.setSettings({ costCap: Math.max(0, +e.currentTarget.value || 0) })} />
          <span class="muted-inline">{t("US dollars (0 = off)")}</span>
        </div>
      </SettingRow>
      <SettingRow id="cost-cap-esc">
        <Toggle on={s().costCapEsc ?? false} onChange={(on) => store.setSettings({ costCapEsc: on })} />
      </SettingRow>
      <SettingRow id="memory-meter">
        <Toggle on={s().memoryMeter} onChange={(on) => store.setSettings({ memoryMeter: on })} />
      </SettingRow>
    </>
  );
}
