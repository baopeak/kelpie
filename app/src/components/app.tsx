import { Match, Show, Switch, createEffect, createMemo, lazy, on, onMount } from "solid-js";
import { errorText } from "../lib/i18n";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { store } from "../lib/store";
import { peekView } from "../lib/terminal-host";
import { daemon } from "../lib/ipc";
import { Sidebar } from "./sidebar";
import { WorkspaceView } from "./workspace-view";
import { RightPanel } from "./right-panel";
import { StatusBar } from "./status-bar";
import { Palette } from "./palette";
import { Dashboard } from "./dashboard";
import { SettingsPage } from "./settings/settings-page";
import { Tips } from "./tips";
import { HoverHints } from "./hover-hints";
import { setEditorTheme } from "../lib/codemirror";
import { WorktreeDialog } from "./worktree-dialog";
import { NewSessionMenu } from "./new-session-menu";
import { Welcome } from "./welcome";
import { ContextMenuHost } from "./context-menu";
import { ImageLightbox } from "./image-lightbox";
import { bookmarks } from "../lib/chat-bookmarks";
import { FloatingTerminal } from "./floating-terminal";
import { LayoutMenuHost } from "./layout-menu";
import { RunsPage } from "./runs-page";
import { RemoteDialog } from "./remote-dialog";
import { RootUserDialogHost } from "./root-user-dialog";
import { HostsPage } from "./hosts-page";
import { initSshHosts, vault } from "../lib/ssh-hosts";
import { MobileApprovals } from "./settings/mobile-settings";
import { ChatWaitNotice } from "./chat-wait-notice";
import { daemon as daemonCall } from "../lib/ipc";
import { loadSshConfigPath, remoteConfigure } from "../lib/remote";
import { initRuns } from "../lib/runs";
import { initSessionUsage } from "../lib/session-usage";
import { initReviewWatch } from "../lib/review-queue";
import { setCovering } from "../lib/overlays";
import { allViews, setLinkOpener } from "../lib/terminal-host";
import { isLocalUrl, openBrowser } from "../lib/browser";
import "../lib/agent-web-pages";
import { openUrl } from "@tauri-apps/plugin-opener";
import { daemonPalette, findTerminalTheme } from "../lib/terminal-themes";
import { loadPlugins, syncPluginAgents } from "../lib/plugins";
import { initVoice } from "../lib/voice";
import { applyLocale } from "../lib/i18n";
import { VoicePill } from "./voice";
import { startUpdater } from "../lib/updater";
import { CHECKPOINT_API } from "../lib/checkpoint-api";
import { initNotifySync } from "../lib/notify-sync";
import { initLimits } from "../lib/limits";

const Board = lazy(() => import("./board").then((m) => ({ default: m.Board })));
const SavedReplyDialog = lazy(() => import("./bookmarks-panel").then((m) => ({ default: m.SavedReplyDialog })));
const FinishDialog = lazy(() => import("./finish-dialog"));
const ReviewWithDialog = lazy(() => import("./review-with-dialog"));
const TryDialog = lazy(() => import("./try-dialog"));
const UpdateDialog = lazy(() => import("./update-dialog"));
const QuickAnswer = lazy(() => import("./quick-answer"));
const CostPage = lazy(() => import("./cost-page"));

/** Tells the daemon which palette to answer terminal colour queries with. */
function syncDaemonTheme() {
  if (!store.connected()) return;
  const custom = findTerminalTheme(store.state.settings.terminalTheme);
  const theme = custom ? (custom.dark ? "dark" : "light") : store.state.settings.theme ?? "dark";
  void daemon("daemon.set_theme", { theme, palette: custom ? daemonPalette(custom) : undefined }).catch(() => {});
}

export function App() {
  createEffect(syncDaemonTheme);
  startUpdater();
  initNotifySync();
  initLimits();
  createEffect(() => applyLocale(store.state.settings.language));
  // Plugin agents go to the daemon on every connection (it may have restarted without them).
  createEffect(() => store.connected() && void syncPluginAgents(true));
  // Phones show project names (the daemon only knows ids).
  createEffect(() => {
    const names = Object.fromEntries(store.state.projects.map((p) => [p.id, p.name]));
    if (store.connected()) void daemonCall("mobile.set_projects", { projects: names }).catch(() => {});
  });
  // Remote projects' ssh calls use the configured SSH config file.
  createEffect(() => void remoteConfigure(store.state.settings.sshConfig?.trim() || null).catch(() => {}));
  createEffect(() => setCovering("floating-terminal", !!(store.state.settings.floatingTerminal && store.state.floating?.open)));
  createEffect(() => {
    const mode = store.state.settings.keepAwake ?? "off";
    if (store.connected()) void daemon("daemon.set_awake", { mode }).catch(() => {});
  });
  createEffect(() => {
    const enabled = store.state.settings.prioritizeVisible ?? true;
    if (store.connected()) void daemon("daemon.set_priority", { enabled }).catch(() => {});
  });
  createEffect(() => {
    const on = store.state.settings.checkpoints ?? false;
    if (store.connected() && store.daemonApi() >= CHECKPOINT_API) void daemon("checkpoint.set_enabled", { enabled: on }).catch(() => {});
  });
  createEffect(() => {
    const on = store.state.settings.agentsScripting ?? true;
    if (store.connected()) void daemon("daemon.set_policy", { agents_control_others: on }).catch(() => {});
  });
  // A host's terminal theme changed: repaint the terminals (those on that host pick it up).
  const hostThemes = createMemo(() => vault().hosts.map((h) => `${h.alias}=${h.terminalTheme}`).join("|"));
  createEffect(on(hostThemes, () => { for (const v of allViews()) v.applySettings(store.state.settings); }, { defer: true }));
  // Hibernated agents resume as soon as their pane is on screen.
  createEffect(() => store.wakeVisible());
  // Theme, density and interface scale are plain CSS switches on the root element.
  createEffect(() => {
    const s = store.state.settings;
    const root = document.documentElement;
    root.dataset.theme = s.theme ?? "dark";
    setEditorTheme(s.theme !== "light");
    // Remembered outside the state file so the next launch paints the right theme at once.
    try {
      localStorage.setItem("kelpie-theme", s.theme ?? "dark");
    } catch {
      /* storage unavailable: the theme still applies once the state is loaded */
    }
    root.dataset.density = s.density ?? "comfortable";
    root.style.setProperty("--ui-scale", String(s.uiScale ?? 1));
    // The terminal area takes the terminal colour theme's background.
    const custom = findTerminalTheme(s.terminalTheme);
    if (custom) root.style.setProperty("--terminal-bg", custom.colors[0]);
    else root.style.removeProperty("--terminal-bg");
  });
  onMount(() => {
    initRuns();
    initSessionUsage();
    initReviewWatch();
    void loadSshConfigPath().catch(() => {});
    void initSshHosts();
    void loadPlugins();
    initVoice();
    // Dev-server links from terminals open next to the terminal.
    setLinkOpener((uri) => (store.state.settings.localhostInBrowser !== false && isLocalUrl(uri) && store.activeProject() ? openBrowser(uri, "right") : void openUrl(uri)));
    // Files dropped from Explorer/Finder onto a terminal paste their (quoted) paths.
    void getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const { x, y } = event.payload.position;
      const ratio = window.devicePixelRatio || 1;
      const el = document.elementFromPoint(x / ratio, y / ratio)?.closest<HTMLElement>("[data-session]");
      const view = el && peekView(el.dataset.session!);
      if (!view) return;
      const quote = (p: string) => (/[\s'"&()]/.test(p) ? `"${p}"` : p);
      view.paste(event.payload.paths.map(quote).join(" ") + " ");
      view.focus();
    });
  });

  return (
    <div class="app" style={{ "--sidebar-w": `${store.state.settings.sidebarWidth}px`, "--right-w": `${store.state.settings.rightPanelWidth}px` }}>
      <Sidebar />
      <div class="main">
        <div class="workarea">
          <div class="center">
            <Show when={store.activeProject()} fallback={<Welcome />} keyed>
              {(project) => <WorkspaceView project={project} />}
            </Show>
          </div>
          <Show when={store.activeProject() && store.workspace(store.activeProject()!.id).rightPanel ? store.activeProject() : null} keyed>
            {(project) => <RightPanel project={project} />}
          </Show>
        </div>
        <VoicePill />
        <StatusBar />
      </div>
      <ChatWaitNotice />
      <Switch>
        <Match when={store.overlay() === "dashboard"}><Dashboard /></Match>
        <Match when={store.overlay() === "settings"}><SettingsPage /></Match>
        <Match when={store.overlay() === "newsession"}><NewSessionMenu /></Match>
        <Match when={store.overlay() === "board"}><Board /></Match>
        <Match when={store.overlay() === "runs"}><RunsPage /></Match>
        <Match when={store.overlay() === "remote"}><RemoteDialog /></Match>
        <Match when={store.overlay() === "hosts"}><HostsPage /></Match>
        <Match when={store.overlay() === "worktree"}><WorktreeDialog /></Match>
        <Match when={store.overlay() === "update"}><UpdateDialog /></Match>
        <Match when={store.overlay() === "finish"}><FinishDialog /></Match>
        <Match when={store.overlay() === "review"}><ReviewWithDialog /></Match>
        <Match when={store.overlay() === "try"}><TryDialog /></Match>
        <Match when={store.overlay() === "answer"}><QuickAnswer /></Match>
        <Match when={store.overlay() === "cost"}><CostPage /></Match>
        <Match when={["palette", "quickopen", "jump"].includes(store.overlay() ?? "") && store.overlay()} keyed>
          {(o) => <Palette scope={o === "quickopen" ? "files" : o === "jump" ? "sessions" : "all"} />}
        </Match>
      </Switch>
      <Show when={store.state.settings.floatingTerminal && store.state.floating?.open}>
        <FloatingTerminal />
      </Show>
      <RootUserDialogHost />
      <ContextMenuHost />
      <Show when={bookmarks.shown()}><SavedReplyDialog /></Show>
      <ImageLightbox />
      <LayoutMenuHost />
      <MobileApprovals />
      <Tips />
      <HoverHints />
      <Show when={store.toast()}>
        {(t) => (
          <div class="toast">
            <span>{errorText(t().text)}</span>
            <Show when={t().action}>
              {(a) => <button class="btn" style={{ height: "24px", "margin-left": "10px" }} onClick={(e) => a().run(e)}>{a().label}</button>}
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
