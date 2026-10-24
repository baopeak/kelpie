import { Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { redact } from "../lib/ipc";
import { getView, parkView } from "../lib/terminal-host";
import { Icon } from "./icons";
import { openContextMenu, type MenuItem } from "./context-menu";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { isMac } from "../lib/platform";
import { isAgentKind, kindInfo, launchableAgents } from "../lib/agents";
import { quickCommandsFor } from "../lib/quick-commands";
import { t } from "../lib/i18n";
import { commandHint as hint } from "../lib/commands";
import { handOffSession, sessionTranscript } from "../lib/agent-actions";
import { hostOf } from "../lib/ssh-hosts";
import { findTerminalTheme } from "../lib/terminal-themes";
import { selectionSource } from "../lib/composer-state";
import { sendSelectionToAgent } from "../lib/send-to-agent";

export function TerminalPane(props: { sessionId: string; focused: boolean }) {
  let host!: HTMLDivElement;
  let findInput!: HTMLInputElement;
  const session = () => store.sessions[props.sessionId];
  const view = getView(props.sessionId, store.state.settings);
  const [finding, setFinding] = createSignal(false);
  const [query, setQuery] = createSignal("");
  /** Background of the host's own terminal theme, around the terminal. */
  const hostBg = () => findTerminalTheme(hostOf(session()?.remote)?.terminalTheme)?.colors[0];

  onMount(() => {
    view.mount(host);
    // (Re)attach whenever the daemon (re)connects or the session's handle changes.
    createEffect(() => {
      const s = session();
      if (s && store.connected() && (!view.attached || view.handle !== s.handle)) void view.attach(s.handle);
    });
    createEffect(() => {
      if (props.focused && store.overlay() === null) queueMicrotask(() => view.focus());
    });
    const onFind = () => {
      if (!props.focused) return;
      setFinding(true);
      findInput?.focus();
      findInput?.select();
    };
    window.addEventListener("kelpie:find", onFind);
    // What is selected here is what `@selection` in a composer stands for.
    const selection = selectionSource(() => (view.term.hasSelection() ? { text: view.term.getSelection(), source: "terminal" as const } : null));
    const selecting = view.term.onSelectionChange(selection.changed);
    onCleanup(() => {
      selecting.dispose();
      selection.dispose();
      window.removeEventListener("kelpie:find", onFind);
      parkView(props.sessionId);
    });
  });

  const find = (forward: boolean) => {
    const q = query();
    if (!q) return;
    // Match highlights must read on both terminal backgrounds.
    const light = store.state.settings.theme === "light";
    const opts = {
      decorations: light
        ? { matchOverviewRuler: "#b7791f", activeMatchColorOverviewRuler: "#b4501f", matchBackground: "#fbe7a6", activeMatchBackground: "#b9d2fb" }
        : { matchOverviewRuler: "#f2c96d", activeMatchColorOverviewRuler: "#e8743b", matchBackground: "#5a4a1a", activeMatchBackground: "#1f4b7a" },
    };
    if (forward) view.search.findNext(q, opts);
    else view.search.findPrevious(q, opts);
  };

  const closeFind = () => {
    setFinding(false);
    view.search.clearDecorations();
    view.focus();
  };

  const restart = () => {
    const s = session();
    if (s) void store.resumeSession(s.id);
  };

  const transcript = async () => {
    const s = session();
    return s ? sessionTranscript(s) : "";
  };

  const menu = (e: MouseEvent) => {
    const s = session();
    const picked = view.term.hasSelection() ? view.term.getSelection() : "";
    const items: MenuItem[] = [
      { label: t("Copy"), hint: isMac ? "⌘C" : "Ctrl+C", run: () => view.term.hasSelection() && void writeText(view.term.getSelection()) },
      { label: t("Paste"), hint: isMac ? "⌘V" : "Ctrl+V", run: () => void view.pasteFromClipboard() },
      { label: t("Select All"), run: () => view.term.selectAll() },
      ...(picked && s?.project_id
        ? [{ label: t("Send Selection to Agent…"), run: () => sendSelectionToAgent(new MouseEvent("contextmenu", { clientX: e.clientX, clientY: e.clientY }), s.project_id!, { text: picked, source: "terminal" }) }]
        : []),
      { label: t("Copy Context"), run: () => void transcript().then(redact).then((text) => writeText(text)).then(() => store.notify(t("Copied the last 400 lines"))) },
      { label: t("Find…"), run: () => { setFinding(true); queueMicrotask(() => findInput?.focus()); } },
      { label: t("Clear"), run: () => view.term.clear() },
      null,
      { label: t("Split Right…"), run: () => s?.project_id && store.splitPicker(s.project_id, "right") },
      { label: t("Split Down…"), run: () => s?.project_id && store.splitPicker(s.project_id, "down") },
    ];
    const quick = quickCommandsFor(s?.project_id ?? null);
    if (quick.length) {
      items.push(null);
      for (const c of quick.slice(0, 8)) items.push({ label: t("Insert: {label}", { label: c.label }), run: () => { view.paste(c.text); view.focus(); } });
    }
    if (s && isAgentKind(s.kind) && s.project_id) {
      items.push(null);
      for (const kind of launchableAgents(store.state.settings.hiddenAgents).filter((k) => k !== s.kind).slice(0, 6)) {
        items.push({ label: t("Hand Off to {agent}", { agent: kindInfo(kind).label }), run: () => void handOffSession(s, kind) });
      }
    }
    openContextMenu(e, items);
  };

  return (
    <div class="term-wrap" data-session={props.sessionId} onContextMenu={menu} style={hostBg() ? { background: hostBg() } : undefined}>
      <div ref={host} style={{ width: "100%", height: "100%" }} />
      <Show when={view.scrolledAway().away}>
        <div class="scroll-pill" data-scroll-pill>
          <button class="icon-btn" title={t("Previous prompt") + hint("terminal.prevPrompt")} onClick={() => view.jumpPrompt(-1)}><Icon name="up" size={13} /></button>
          <button class="icon-btn" title={t("Next prompt") + hint("terminal.nextPrompt")} onClick={() => view.jumpPrompt(1)}><Icon name="down" size={13} /></button>
          <button class="scroll-pill-bottom" data-jump-bottom title={t("Scroll to bottom") + hint("terminal.bottom")} onClick={() => { view.scrollToBottom(); view.focus(); }}>
            {view.scrolledAway().newLines ? t("{n} new lines", { n: view.scrolledAway().newLines }) : t("Back to bottom")} <Icon name="down" size={12} />
          </button>
        </div>
      </Show>
      <Show when={finding()}>
        <div class="findbar">
          <input
            ref={findInput}
            placeholder={t("Find in terminal")}
            value={query()}
            onInput={(e) => { setQuery(e.currentTarget.value); find(true); }}
            onKeyDown={(e) => {
              if (e.key === "Enter") find(!e.shiftKey);
              if (e.key === "Escape") closeFind();
            }}
          />
          <button class="icon-btn" onClick={() => find(false)} title={t("Previous (Shift+Enter)")}><Icon name="up" size={14} /></button>
          <button class="icon-btn" onClick={() => find(true)} title={t("Next (Enter)")}><Icon name="down" size={14} /></button>
          <button class="icon-btn" onClick={closeFind} title={t("Close (Esc)")}><Icon name="close" size={12} /></button>
        </div>
      </Show>
      <Show when={!session()}>
        <div class="pane-overlay"><div class="exit-card"><span class="msg">{store.connected() ? t("Session not available (daemon has no such session)") : t("Session not available (daemon disconnected)")}</span></div></div>
      </Show>
      <Show when={session() && !session()!.alive}>
        <div style={{ position: "absolute", left: "12px", right: "12px", bottom: "10px", "z-index": 6, display: "flex", "justify-content": "center" }}>
          <div class="exit-card" style={{ "flex-direction": "row", padding: "8px 12px" }}>
            <span class="msg">
              {session()!.interrupted
                ? t("Stopped when the session daemon stopped (restart, update or crash)")
                : session()!.exit_code != null
                  ? t("Process exited (code {code})", { code: session()!.exit_code! })
                  : t("Process exited")}
            </span>
            <button class="btn primary" onClick={restart}><Icon name="restart" size={14} /> {store.resumable(session()!) ? t("Resume") : t("Restart")}</button>
          </div>
        </div>
      </Show>
    </div>
  );
}
