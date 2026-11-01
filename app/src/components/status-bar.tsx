import { For, Show, createMemo, createSignal, onCleanup, onMount, createEffect } from "solid-js";
import { store } from "../lib/store";
import { Icon } from "./icons";
import { MemoryMeter, UsageMeters } from "./usage-meter";
import { InboxPanel } from "./inbox";
import { setCovering } from "../lib/overlays";
import { activeRuns } from "../lib/runs";
import { available, whenIdle } from "../lib/updater";
import { pluginStatus, pluginStatusItems, type PluginInfo, type PluginStatusItem } from "../lib/plugins";
import { t, msg } from "../lib/i18n";
import { automationEvents } from "../lib/automation-events";

const waitingApproval = () => activeRuns().reduce((n, r) => n + r.tasks.filter((t) => t.status === "waiting_approval").length, 0);

export function StatusBar() {
  const [inbox, setInbox] = createSignal(false);
  createEffect(() => setCovering("inbox", inbox()));
  onMount(() => {
    const close = () => setInbox(false);
    const open = () => setInbox(true);
    // Capture phase, like the context menu: Escape closes the inbox before a terminal sees it.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !inbox()) return;
      e.preventDefault();
      e.stopPropagation();
      close();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("kelpie:inbox", open);
    onCleanup(() => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("kelpie:inbox", open);
    });
  });
  const counts = createMemo(() => {
    let alive = 0, working = 0, waiting = 0;
    for (const s of Object.values(store.sessions)) {
      if (!s.alive) continue;
      alive++;
      if (s.status === "working") working++;
      if (s.status === "waiting") waiting++;
    }
    return { alive, working, waiting };
  });
  // The sidebar's branch summary (debounced `git status -uno`) — never a second full status:
  // on a large repository each full status takes seconds.
  const branch = () => {
    const p = store.activeProject();
    const info = p && store.projectInfo[p.id];
    return info?.is_repo ? info.branch ?? t("detached") : null;
  };

  return (
    <div class="statusbar" data-hints>
      <span class="item" title={store.connected() ? t("Session daemon connected") : t("Session daemon not connected")}>
        <span class={`conn${store.connected() ? " on" : ""}`} />
        {store.connected() ? "kelpied" : t("kelpied offline")}
      </span>
      <Show when={branch()}>
        <span class="item clickable branch" title={branch()!} onClick={() => store.setRightPanel("git")}><Icon name="git" size={13} /> <span class="label">{branch()}</span></span>
      </Show>
      <span class="spacer" />
      <Show when={store.daemonNeedsSwitch()}>
        <span class="item clickable daemon-switch" title={t("The session daemon still running lacks a security fix in this release. Running sessions stop for a moment; agents resume.")} onClick={() => void store.switchDaemon()}>
          <Icon name="alert" size={13} /> {t("Switch session daemon")}
        </span>
      </Show>
      <Show when={available()}>
        {(u) => (
          <span class="item clickable update-chip" title={t("A newer Kelpie is ready to install")} onClick={() => store.setOverlay("update")}>
            <Icon name="down" size={13} /> {whenIdle() ? t("Update {version} when idle", { version: u().version }) : t("Update {version}", { version: u().version })}
          </span>
        )}
      </Show>
      <For each={pluginStatusItems()}>{({ plugin, item }) => <PluginStatus plugin={plugin} item={item} />}</For>
      <Show when={store.state.settings.usageMeters}><UsageMeters /></Show>
      <Show when={store.state.settings.memoryMeter}><MemoryMeter /></Show>
      <Show when={store.lastSwitchMs() != null}>
        {/* Room for three digits: a faster or slower switch must not shift the items after it. */}
        <span class="item fixed" style={{ "min-width": `${t("switch {ms} ms", { ms: 888 }).length}ch` }} title={t("Time to render the last project switch")}>{t("switch {ms} ms", { ms: store.lastSwitchMs()! })}</span>
      </Show>
      <span class="item clickable" onClick={() => store.setOverlay("dashboard")}>
        {counts().alive === 1 ? t("{n} session", { n: 1 }) : t("{n} sessions", { n: counts().alive })}
        {/* Always shown, dimmed at 0: counts coming and going must not shift the bar. */}
        {" · "}<span style={{ color: counts().working ? "var(--working)" : "var(--text-faint)" }}>{t("{n} working", { n: counts().working })}</span>
        {" · "}<span style={{ color: counts().waiting ? "var(--waiting)" : "var(--text-faint)" }}>{t("{n} need you", { n: counts().waiting })}</span>
      </span>
      <Show when={activeRuns().length}>
        <span class="item clickable" title={t("Orchestration runs in progress")} onClick={() => store.setOverlay("runs")}>
          <Icon name="board" size={13} /> {activeRuns().length === 1 ? t("{n} run", { n: 1 }) : t("{n} runs", { n: activeRuns().length })}
          <Show when={waitingApproval()}><span class="badge waiting" title={t("Tasks waiting for approval")}>{waitingApproval()}</span></Show>
        </span>
      </Show>
      {/* The global switch: nothing starts by itself while it is off. Shown when there is something to switch. */}
      <Show when={automationEvents.active() || automationEvents.state().paused}>
        <span class={`item clickable${automationEvents.state().paused ? "" : " on"}`} title={automationEvents.state().paused ? t("Automations are paused: nothing starts by itself. Click to resume.") : t("Automations are on. Click to pause every automatic run.")} onClick={() => void automationEvents.setPaused(!automationEvents.state().paused)}>
          <Icon name="bolt" size={13} />
          <Show when={automationEvents.state().paused}>{t("paused")}</Show>
        </span>
      </Show>
      <span class={`item clickable${awake() !== "off" ? " on" : ""}`} title={t(AWAKE_TITLE[awake()])} onClick={cycleAwake}>
        <Icon name="coffee" size={13} />
        <Show when={awake() !== "off"}>{awake() === "agents" ? t("agents") : t("on")}</Show>
      </span>
      <Show when={store.state.settings.floatingTerminal}>
        <span class={`item clickable${store.state.floating?.open ? " on" : ""}`} title={t("Floating terminal")} onClick={() => store.toggleFloating()}>
          <Icon name="float" size={13} />
        </span>
      </Show>
      <span class="item clickable inbox-anchor" title={t("Activity and notifications")} onPointerDown={(e) => e.stopPropagation()} onClick={() => setInbox(!inbox())}>
        <Icon name="bell" size={13} />
        <Show when={store.unreadInbox()}><span class="badge waiting">{store.unreadInbox()}</span></Show>
        <Show when={counts().waiting}><span class="bell-needs-you" title={t("{n} need you", { n: counts().waiting })} /></Show>
        <Show when={inbox()}><InboxPanel onClose={() => setInbox(false)} /></Show>
      </span>
    </div>
  );
}

/** A plugin's status item: its command's output, re-run in the active project every interval
 *  (and on click). Local projects only — the command runs on this computer. */
function PluginStatus(props: { plugin: PluginInfo; item: PluginStatusItem }) {
  const [out, setOut] = createSignal<{ text: string; error?: string } | null>(null);
  const cwd = () => {
    const p = store.activeProject();
    return p && !p.remote ? p.path : null;
  };
  let busy = false;
  const refresh = async () => {
    const dir = cwd();
    if (busy || !dir) return;
    busy = true;
    try {
      const r = await pluginStatus(props.plugin.id, props.item.id, dir);
      setOut(r.ok ? { text: r.text.split(/\r?\n/)[0] } : { text: "error", error: r.error || r.text });
    } catch (e) {
      setOut({ text: "error", error: String(e) });
    } finally {
      busy = false;
    }
  };
  createEffect(() => {
    if (!cwd()) return setOut(null);
    void refresh();
    const timer = setInterval(() => void refresh(), Math.max(5, props.item.intervalSeconds ?? 30) * 1000);
    onCleanup(() => clearInterval(timer));
  });
  return (
    <Show when={out()}>
      {(o) => (
        <span class={`item clickable plugin-status${o().error ? " error" : ""}`} data-status-item={`${props.plugin.id}.${props.item.id}`}
          title={t("{name}: {label}{error} (click to refresh)", { name: props.plugin.manifest?.name ?? props.plugin.id, label: props.item.label ?? props.item.id, error: o().error ? `\n${o().error}` : "" })} onClick={() => void refresh()}>
          <Show when={props.item.label}><span class="muted-inline">{props.item.label}</span> </Show>{o().error ? t("error") : o().text.slice(0, 60)}
        </span>
      )}
    </Show>
  );
}

const AWAKE_TITLE = {
  off: msg("Keep computer awake: off (click: while agents work)"),
  agents: msg("Keep computer awake while an agent is working (click: always)"),
  always: msg("Keep computer awake: always (click: off)"),
} as const;

const awake = () => store.state.settings.keepAwake ?? "off";

function cycleAwake() {
  const order = ["off", "agents", "always"] as const;
  store.setSettings({ keepAwake: order[(order.indexOf(awake()) + 1) % order.length] });
}
