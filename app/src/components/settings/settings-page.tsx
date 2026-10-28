// Settings as a full page: grouped navigation on the left, one section at a time on the right,
// and a search box (Ctrl/Cmd+F) that shows every matching row across sections.
import { For, Suspense, createMemo, lazy, onCleanup, onMount, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";
import { store } from "../../lib/store";
import { t } from "../../lib/i18n";
import { isMac } from "../../lib/platform";
import { settingsCatalog, settingsSections, entryMatches, type SettingsSection } from "../../lib/settings-catalog";
import { QuickCommandsEditor } from "../quick-commands";
import { Icon } from "../icons";
import { SettingRow, setSettingsQuery, settingsQuery } from "./setting-row";
import { AgentSettings } from "./agent-settings";
import { AutomationSettings } from "./automation-settings";
import { AppearanceSettings, NotificationSettings, TerminalSettings, UsageSettings } from "./interface-settings";
import { ShortcutSettings, shortcutsMatch } from "./shortcut-settings";
import { AboutSettings, DaemonSettings } from "./system-settings";
import { LayoutSettings } from "./layout-settings";
import { AccountSettings } from "./account-settings";
import { IntegrationSettings } from "./integration-settings";
import { RemoteSettings } from "./remote-settings";
import { MobileSettings } from "./mobile-settings";
import { PluginSettings } from "./plugin-settings";
import { VoiceSettings } from "./voice-settings";

// The setup page (diffs, registry, skills) loads when it is opened, not with the app.
const AgentSetup = lazy(() => import("../agent-setup"));

const SECTIONS: Record<SettingsSection, () => JSX.Element> = {
  agents: AgentSettings,
  "agent-setup": () => (
    <SettingRow id="agent-setup" wide>
      <Suspense fallback={<div class="muted-inline">{t("Loading…")}</div>}>
        <AgentSetup />
      </Suspense>
    </SettingRow>
  ),
  accounts: AccountSettings,
  integrations: IntegrationSettings,
  remote: RemoteSettings,
  mobile: MobileSettings,
  automations: AutomationSettings,
  voice: VoiceSettings,
  commands: () => (
    <SettingRow id="quick-commands" wide>
      <QuickCommandsEditor />
    </SettingRow>
  ),
  terminal: TerminalSettings,
  layouts: LayoutSettings,
  appearance: AppearanceSettings,
  shortcuts: ShortcutSettings,
  notifications: NotificationSettings,
  plugins: PluginSettings,
  usage: UsageSettings,
  daemon: DaemonSettings,
  about: AboutSettings,
};

const sectionMatches = (id: SettingsSection, q: string) =>
  id === "shortcuts" ? shortcutsMatch(q) : settingsCatalog.some((e) => e.section === id && entryMatches(e, q));

export function SettingsPage() {
  let search!: HTMLInputElement;
  let page!: HTMLDivElement;
  const query = () => settingsQuery().trim();
  const groups = createMemo(() => {
    const out: { group: string; items: typeof settingsSections }[] = [];
    for (const s of settingsSections) {
      if (out[out.length - 1]?.group !== s.group) out.push({ group: s.group, items: [] });
      out[out.length - 1].items.push(s);
    }
    return out;
  });
  const shown = createMemo(() => (query() ? settingsSections.filter((s) => sectionMatches(s.id, query())) : settingsSections.filter((s) => s.id === store.settingsSection())));

  const close = () => store.setOverlay(null);
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !e.defaultPrevented) {
      if (query()) setSettingsQuery("");
      else close();
      return;
    }
    if ((isMac ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "f") {
      e.preventDefault();
      search.focus();
      search.select();
    }
  };
  onMount(() => {
    setSettingsQuery("");
    window.addEventListener("keydown", onKey);
    queueMicrotask(() => (store.settingsItem() ? page.focus() : search.focus()));
  });
  onCleanup(() => window.removeEventListener("keydown", onKey));

  return (
    <div class="settings-page" ref={page} tabIndex={-1}>
      <nav class="settings-nav">
        <button class="settings-back" onClick={close} title={t("Back (Esc)")}><Icon name="back" size={14} /> {t("Settings")}</button>
        <For each={groups()}>
          {(g) => (
            <>
              <div class="settings-group">{t(g.group)}</div>
              <For each={g.items}>
                {(s) => (
                  <button class={`settings-link${!query() && store.settingsSection() === s.id ? " on" : ""}${query() && !sectionMatches(s.id, query()) ? " dim" : ""}`}
                    data-section={s.id}
                    onClick={() => { setSettingsQuery(""); store.setSettingsSection(s.id); }}>
                    {t(s.label)}
                  </button>
                )}
              </For>
            </>
          )}
        </For>
      </nav>
      <main class="settings-main">
        <div class="settings-search">
          <Icon name="search" size={14} />
          <input ref={search} placeholder={t("Search settings ({key})", { key: isMac ? "⌘F" : "Ctrl+F" })} value={settingsQuery()} onInput={(e) => setSettingsQuery(e.currentTarget.value)} />
        </div>
        <div class="settings-scroll">
          <For each={shown()} fallback={<div class="muted">{t("No settings match “{query}”.", { query: query() })}</div>}>
            {(s) => (
              <section class="settings-section" data-section={s.id}>
                <h2>{t(s.label)}</h2>
                <Dynamic component={SECTIONS[s.id]} />
              </section>
            )}
          </For>
        </div>
      </main>
    </div>
  );
}
