import { For, Match, Show, Switch, createEffect, createMemo, createSignal, lazy } from "solid-js";
import { HostChip, RunOnPicker, initialRunOn, resolveRunOn, runOnAgents, runOnChoices, type RunOn } from "./host-target";
import { hostOf, projectHosts, vault } from "../lib/ssh-hosts";
import { projectTarget, sessionTarget } from "../lib/remote";
import { openNewSession } from "./new-session-menu";
import { MicButton } from "./voice";
import { store } from "../lib/store";
import { allPanes, cyclePane, findPane } from "../lib/layout";
import { Icon, KindGlyph } from "./icons";
import { ContextRing } from "./context-ring";
import { StatusDot } from "./sidebar";
import { TerminalPane } from "./terminal-pane";
import { FileViewer } from "./file-viewer";
import { ChatPane } from "./chat-pane";
import { paneTitle } from "./workspace-view";
import { switchToChat, switchToTerminal, supportsChat } from "../chat/mode-switch";
import { startPaneDrag } from "./split-view";
import { BrowserPane } from "./browser-pane";
import { Kip, kipShown } from "./brand";
import { kindInfo } from "../lib/agents";
import type { Pane, Project, Tab } from "../lib/types";
import { t } from "../lib/i18n";
import { confirmDialog } from "../lib/confirm-dialog";
import { isLimited } from "../lib/limits";
import { commandHint } from "../lib/commands";
import { composerRequest, toggleComposer } from "../lib/composer-state";

// Loaded when a diff or a turn's file is first opened.
const DiffView = lazy(() => import("./diff-view").then((m) => ({ default: m.DiffView })));
const TurnFileView = lazy(() => import("./turn-file-view"));
const LimitBanner = lazy(() => import("./limit-banner"));
// Loaded the first time a composer opens.
const Composer = lazy(() => import("./composer").then((m) => ({ default: m.Composer })));

export function PaneView(props: { pane: Pane; project: Project; tab: Tab }) {
  const focused = () => props.tab.focus === props.pane.id;
  const multi = createMemo(() => allPanes(props.tab.root).length > 1);
  const session = () => (props.pane.sessionId ? store.sessions[props.pane.sessionId] : undefined);
  const focus = () => store.focusPane(props.project.id, props.tab.id, props.pane.id);
  /** This pane's session has its composer open. */
  const composing = () => !!props.pane.sessionId && composerRequest()?.sessionId === props.pane.sessionId;
  const close = async () => {
    const s = session();
    if (s?.alive && s.kind !== "shell" && !(await confirmDialog(t("Stop {kind} and close this pane?", { kind: s.kind })))) return;
    void store.closePane(props.project.id, props.tab.id, props.pane.id);
  };
  const split = (where: "right" | "down") => {
    focus();
    store.splitPicker(props.project.id, where);
  };
  // A maximize left over after the other panes closed means nothing for a lone pane.
  const zoomedHere = () => multi() && store.zoomed()[props.tab.id] === props.pane.id;
  const tools = () => (
    <>
      <Show when={chatable()}>
        <Show
          when={props.pane.kind === "chat"}
          fallback={<button class="icon-btn" title={t("Open in chat view")} onClick={() => void switchToChat(props.project, props.tab, props.pane)}><Icon name="chat" size={16} /></button>}
        >
          <button class="icon-btn" title={t("Back to terminal (TUI)")} onClick={() => void switchToTerminal(props.project, props.tab, props.pane)}><Icon name="terminal" size={16} /></button>
        </Show>
      </Show>
      <Show when={(props.pane.kind === "terminal" || props.pane.kind === "chat") && session()?.alive}>
        <button class={`icon-btn${composing() ? " active" : ""}`} title={t("Compose") + commandHint("composer.open")} data-compose onClick={() => toggleComposer(session()!.id)}><Icon name="compose" size={16} /></button>
      </Show>
      <Show when={props.pane.kind === "terminal" && session()?.alive}>
        <MicButton sessionId={props.pane.sessionId} size={16} />
      </Show>
      <button class="icon-btn" title={t("Split right") + commandHint("split.right")} onClick={() => split("right")}><Icon name="splitRight" size={16} /></button>
      <button class="icon-btn" title={t("Split down") + commandHint("split.down")} onClick={() => split("down")}><Icon name="splitDown" size={16} /></button>
      <Show when={multi() || zoomedHere()}>
        <button class={`icon-btn${zoomedHere() ? " active" : ""}`} title={(zoomedHere() ? t("Restore layout") : t("Maximize pane")) + commandHint("pane.zoom")} onClick={() => store.toggleZoom(props.tab.id, props.pane.id)}>
          <Icon name={zoomedHere() ? "minimize" : "maximize"} size={16} />
        </button>
      </Show>
      <button class="icon-btn" title={t("Close pane") + commandHint("pane.close")} onClick={close}><Icon name="close" size={14} /></button>
    </>
  );
  /** The host this pane's session runs on, when it is not where the project is (a remote pane in
   *  a local project, or a pane on another host): named in the header of a split, and by the tab
   *  chip when the pane is alone. */
  const target = () => (session() ? sessionTarget(session()!) : undefined);
  const elsewhere = () => !!target() && target()!.dest !== projectTarget(props.project)?.dest;
  const hostColor = () => (target() ? hostOf(target())?.color || "var(--accent)" : undefined);
  // A header only where it tells something the tab does not: which pane is which in a split
  // (or a maximized one), and a bar for file/diff viewers so their own top bar never sits under
  // the floating toolbar. A lone pane is named by its tab (glyph, title, host chip) and keeps its
  // controls in the hover toolbar.
  const hasHeader = () => multi() || zoomedHere() || props.pane.kind === "file" || props.pane.kind === "diff";
  // Chat view runs the agent as a local process: not for sessions on another machine.
  const chatable = () => !!session() && supportsChat(session()!.kind) && !session()!.remote;
  const broadcasting = () => !!props.tab.broadcast && props.pane.kind === "terminal";

  return (
    <div class={`pane${focused() && multi() ? " focused" : ""}${broadcasting() ? " broadcast" : ""}${target() ? " on-host" : ""}`} data-pane={props.pane.id} data-tab={props.tab.id} data-pane-host={target()?.dest}
      style={{ "--host-color": hostColor(), "--pane-color": session() ? kindInfo(session()!.kind).color : undefined }} onPointerDown={focus}>
      {/* Above the pane's opaque children (::before / ::after mark broadcast and focus). */}
      <Show when={hostColor()}><div class="pane-host-stripe" /></Show>
      <Show when={!hasHeader()}>
        <div class="pane-tools" data-hints>{tools()}</div>
      </Show>
      <Show when={hasHeader()}>
        <div class="pane-header" data-hints onPointerDown={(e) => multi() && startPaneDrag(e, props.project, props.tab, props.pane.id)} title={multi() ? t("Drag to move or swap this pane, or onto a tab") : undefined}>
          <Show when={session()} fallback={<Icon name={props.pane.kind === "empty" ? "plus" : props.pane.kind === "browser" ? "globe" : "files"} size={13} />}>
            {(s) => <><StatusDot s={s()} /><KindGlyph kind={s().kind} size={14} /></>}
          </Show>
          <Show when={elsewhere() && target()}>{(tg) => <HostChip target={tg()} path />}</Show>
          <span class="title">{paneTitle(props.pane)}</span>
          <Show when={session()}>{(s) => <ContextRing s={s()} />}</Show>
          <Show when={broadcasting()}>
            <span class="badge broadcast-badge" title={t("Typing here goes to every terminal in this tab")}>{t("Broadcast")}</span>
          </Show>
          {tools()}
        </div>
      </Show>
      {/* The usage limit stopped this agent (the daemon says so for Claude Code). */}
      <Show when={props.pane.kind === "terminal" ? session() : undefined} keyed>
        {(s) => <Show when={isLimited(s)}><LimitBanner s={s} /></Show>}
      </Show>
      <Show when={composing() && session() ? composerRequest() : null} keyed>
        {(request) => <Composer session={session()!} project={props.project} request={request} />}
      </Show>
      <div class="pane-body">
        <Switch fallback={<EmptyPane project={props.project} tab={props.tab} pane={props.pane} focused={focused()} />}>
          <Match when={props.pane.kind === "terminal" && props.pane.sessionId} keyed>
            {(id) => <TerminalPane sessionId={id} focused={focused()} />}
          </Match>
          <Match when={props.pane.kind === "chat" && props.pane.sessionId} keyed>
            {(id) => <ChatPane sessionId={id} project={props.project} focused={focused()} />}
          </Match>
          <Match when={props.pane.kind === "file" && props.pane.path && props.pane.rev ? props.pane.rev : null} keyed>
            {(rev) => <TurnFileView project={props.project} path={props.pane.path!} rev={rev} />}
          </Match>
          <Match when={props.pane.kind === "file" && props.pane.path} keyed>
            {(path) => <FileViewer project={props.project} path={path} line={props.pane.line} focused={focused()} />}
          </Match>
          <Match when={props.pane.kind === "browser" && props.pane.id} keyed>
            {(_id) => <BrowserPane pane={props.pane} project={props.project} tab={props.tab} focused={focused()} />}
          </Match>
          <Match when={props.pane.kind === "diff" && props.pane.path} keyed>
            {(path) => <DiffView project={props.project} path={path} staged={!!props.pane.staged} rev={props.pane.rev} base={props.pane.base} />}
          </Match>
        </Switch>
      </div>
    </div>
  );
}

const workspaceHasPane = (projectId: string, paneId: string) => store.workspace(projectId).tabs.some((t) => findPane(t.root, paneId)?.kind === "empty");

/** An empty pane (a new split or grid cell): asks what to run in it. One click or key 1–9
 *  starts an agent or a shell; "More…" opens the full dialog for this pane; Esc closes it. */
function EmptyPane(props: { project: Project; tab: Tab; pane: Pane; focused: boolean }) {
  let el!: HTMLDivElement;
  const [runOn, setRunOn] = createSignal<RunOn>(initialRunOn(props.project, props.pane.target));
  const kinds = () => [...runOnAgents(props.project, runOn(), store.state.settings.hiddenAgents), "shell"];
  const into = () => ({ tabId: props.tab.id, paneId: props.pane.id });
  // One start per pane: a double click or a held key must not start a second session.
  let starting = false;
  const start = async (kind: string) => {
    if (starting) return;
    starting = true;
    // Read before the host check (it can take seconds): a pane closed meanwhile must not start
    // anything, and its props stop resolving once it leaves the tree.
    const projectId = props.project.id;
    const place = into();
    const target = await resolveRunOn(props.project, runOn(), kind);
    const open = workspaceHasPane(projectId, place.paneId);
    const s = target === null || !open ? undefined : await store.newSession(projectId, kind, { into: place, target });
    if (!s) starting = false;
  };
  /** Tab moves to the next machine of the "Run on" row. */
  const nextMachine = (delta: number) => {
    const list = runOnChoices(props.project, runOn());
    const id = list[(list.indexOf(runOn().hostId) + delta + list.length) % list.length];
    if (id !== runOn().hostId) setRunOn({ hostId: id, path: projectHosts(props.project).find((l) => l.host.id === id)?.path ?? "" });
  };
  // Take the keyboard when focused (and again when a dialog closes), so a split can be
  // answered without the mouse; never from behind an open dialog.
  createEffect(() => {
    if (props.focused && store.overlay() === null) queueMicrotask(() => el?.focus({ preventScroll: true }));
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.repeat) return;
    const n = Number(e.key);
    if (Number.isInteger(n) && n >= 1 && n <= Math.min(9, kinds().length)) {
      e.preventDefault();
      void start(kinds()[n - 1]);
    } else if (e.key === "Tab" && runOnChoices(props.project, runOn()).length > 1) {
      e.preventDefault();
      nextMachine(e.shiftKey ? -1 : 1);
    } else if (e.key === "Escape") {
      e.preventDefault();
      // Cancelling a split goes back to the pane before it (the one that was split).
      // Ids are read now: once the pane leaves the tree its props no longer resolve.
      const { id: projectId } = props.project;
      const { id: tabId } = props.tab;
      const paneId = props.pane.id;
      const back = cyclePane(props.tab.root, paneId, -1);
      void store.closePane(projectId, tabId, paneId).then(() => {
        if (back !== paneId) store.focusPane(projectId, tabId, back);
      });
    }
  };
  return (
    <div class="empty-pane" ref={el} tabindex={-1} onKeyDown={onKey}>
      <Show when={kipShown()}><Kip mood="peek" size={56} /></Show>
      <div class="empty-pane-title">{t("What should run in this pane?")}</div>
      <Show when={vault().hosts.length || props.project.remote}>
        <RunOnPicker project={props.project} value={runOn()} onChange={setRunOn} />
      </Show>
      <div class="empty-pane-kinds">
        <For each={kinds()}>
          {(k, i) => (
            <button class="empty-pane-kind" title={kindInfo(k).label} onClick={() => void start(k)}>
              <KindGlyph kind={k} size={22} />
              <span>{kindInfo(k).label}</span>
              <Show when={i() < 9}><kbd class="empty-pane-key">{i() + 1}</kbd></Show>
            </button>
          )}
        </For>
        <button class="empty-pane-kind" title={t("Command, arguments, account, chat view…")} onClick={() => openNewSession(into(), runOn())}>
          <Icon name="plus" size={16} />
          <span>{t("More…")}</span>
        </button>
      </div>
      <div class="empty-pane-hint">{runOnChoices(props.project, runOn()).length > 1 ? t("1–9 to start · Tab: next machine · Esc to close") : t("1–9 to start · Esc to close")}</div>
    </div>
  );
}
