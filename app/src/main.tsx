import { render } from "solid-js/web";
import "@xterm/xterm/css/xterm.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";
import "./styles.css";
import { App } from "./components/app";
import { installKeymap } from "./lib/commands";
import { startNavHistory } from "./lib/nav-history";
import { store } from "./lib/store";
import { bookmarks } from "./lib/chat-bookmarks";
import { platformInfo } from "./lib/ipc";

// Paint the last used theme before the state file is read (no dark flash in light mode).
try {
  document.documentElement.dataset.theme = localStorage.getItem("kelpie-theme") ?? "dark";
} catch {
  /* storage unavailable */
}
installKeymap();
startNavHistory();
// Dev builds, and release builds started by the test runner (KELPIE_E2E), expose internals to
// the WebDriver end-to-end tests (scripts/e2e).
void (import.meta.env.DEV ? Promise.resolve(true) : platformInfo().then((i) => i.e2e, () => false)).then((expose) => {
  if (!expose) return;
  void Promise.all([import("./lib/terminal-host"), import("./lib/commands"), import("./chat/mode-switch"), import("./lib/layout"), import("./lib/accounts"), import("./lib/runs"), import("./lib/integrations"), import("./lib/pull-requests"), import("./lib/browser"), import("./lib/plugins"), import("./lib/voice"), import("./lib/agents"), import("./lib/worktrees"), import("./lib/ipc"), import("./components/diff-view"), import("./lib/review-queue"), import("./lib/spec-index"), import("./lib/ssh-hosts"), import("./lib/spec-parse-client"), import("./lib/automation-events"), import("./components/settings/config-transfer"), import("./lib/session-export")]).then(([th, cmds, chat, layout, accounts, runs, integrations, prs, browser, plugins, voice, agents, worktrees, ipc, diffView, review, specIndex, sshHosts, specParse, automations, configTransfer, sessionExport]) => {
    (window as any).__kelpie = { store, views: th.allViews, peekView: th.peekView, commands: cmds.commands, chat, layout, accounts, runs, integrations, prs, browser, plugins, voice, agents, worktrees, ipc, diffView, review, specIndex, sshHosts, specParse, automations: automations.automationEvents, configTransfer, sessionExport, perf: perfProbes(th.peekView) };
  });
});

// Timings for scripts/perf/perf_gate.py, taken with performance.now() inside the page so that
// WebDriver round trips (about 20 ms each) never enter a measurement.
function perfProbes(peekView: (sessionId: string) => { term: any } | undefined) {
  // Tasks over 50 ms; buffered, so those from before this point count too.
  const longTasks: { at: number; ms: number }[] = [];
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) longTasks.push({ at: Math.round(e.startTime), ms: Math.round(e.duration) });
    }).observe({ type: "longtask", buffered: true });
  } catch {
    /* Long Tasks API unavailable */
  }
  // Wall-clock time of the first frame with the sidebar drawn and the state loaded.
  let interactiveAt: number | null = null;
  const watch = () => {
    if (store.loaded() && document.querySelector(".sidebar")) interactiveAt = performance.timeOrigin + performance.now();
    else requestAnimationFrame(watch);
  };
  watch();
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const frames = (): Promise<unknown> => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(undefined))));
  return {
    interactiveAt: () => interactiveAt,
    longTasks: () => longTasks.slice(),
    clearLongTasks: () => { longTasks.length = 0; },
    /** Switches through `projectIds` in turn; resolves with each switch's time to the second frame (ms). */
    async switchProjects(projectIds: string[], count: number, gapMs = 120): Promise<number[]> {
      const out: number[] = [];
      for (let i = 0; i < count; i++) {
        const id = projectIds[i % projectIds.length];
        if (store.state.activeProject === id) continue;
        const t0 = performance.now();
        store.switchProject(id);
        await frames();
        out.push(performance.now() - t0);
        await sleep(gapMs);
      }
      return out;
    },
    /** Types `text` into the session's visible terminal as keypresses; per key, ms until xterm has parsed
     *  the echo and the screen shows it (null when it never did within `timeoutMs`). */
    async echo(sessionId: string, text: string, gapMs = 40, timeoutMs = 3000): Promise<(number | null)[]> {
      const term = peekView(sessionId)?.term;
      if (!term) throw new Error("no terminal view for session");
      const screen = () => {
        const b = term.buffer.active;
        let s = "";
        for (let i = b.viewportY; i < b.viewportY + term.rows; i++) s += b.getLine(i)?.translateToString(true) ?? "";
        return s;
      };
      const out: (number | null)[] = [];
      for (let i = 0; i < text.length; i++) {
        // The last few keys are unique within `text`, so a stale echo cannot satisfy the check.
        const tail = text.slice(Math.max(0, i - 5), i + 1);
        out.push(await new Promise<number | null>((resolve) => {
          const t0 = performance.now();
          const timer = setTimeout(() => { sub.dispose(); resolve(null); }, timeoutMs);
          const sub = term.onWriteParsed(() => {
            if (!screen().includes(tail)) return;
            sub.dispose();
            clearTimeout(timer);
            resolve(performance.now() - t0);
          });
          term.input(text[i], true);
        }));
        await sleep(gapMs);
      }
      return out;
    },
  };
}
// Terminals measure glyphs once at open: wait for the bundled font before the first render.
void Promise.all([document.fonts.load('13px "JetBrains Mono"'), document.fonts.load('bold 13px "JetBrains Mono"')])
  .catch(() => {})
  .finally(() => {
    void store.init();
    void bookmarks.load();
    render(() => <App />, document.getElementById("root")!);
  });

// Native context menu / reload shortcuts make no sense in a desktop workbench.
window.addEventListener("contextmenu", (e) => {
  if (!(e.target as HTMLElement).closest("input, textarea, .cm-editor, .markdown, .chat-scroll")) e.preventDefault();
});
