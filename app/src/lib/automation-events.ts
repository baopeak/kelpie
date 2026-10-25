// Event-triggered automations, the window's side. The daemon matches events to automations and runs
// them; the window reports what only it can see (changed files come from the watcher in Rust, pull
// request checks and finished worktrees from here) and holds the global switch.
import { createEffect, createRoot, createSignal } from "solid-js";
import { daemon } from "./ipc";
import { store } from "./store";
import type { Automation, AutomationEvent, AutomationState } from "./types";

/** Daemon API level that has event triggers, commands and `automation.event`. */
export const AUTOMATION_API = 15;

/** Pull requests with an automation on failing checks are looked at this often. */
const PR_POLL_MS = 5 * 60_000;

function createAutomationEvents() {
  const [list, setList] = createSignal<Automation[]>([]);
  const [state, setState] = createSignal<AutomationState>({ paused: false, error: null });
  const supported = () => store.connected() && store.daemonApi() >= AUTOMATION_API;

  async function load() {
    if (!store.connected()) return;
    try {
      setList(await daemon<Automation[]>("automation.list"));
      if (store.daemonApi() >= AUTOMATION_API) setState(await daemon<AutomationState>("automation.state"));
    } catch {
      // The settings page shows its own error when it loads the list.
    }
  }

  /** Tells the daemon something happened in the window. Never blocks or fails the caller. */
  function report(event: AutomationEvent) {
    try {
      if (supported()) void daemon("automation.event", event).catch(() => {});
    } catch {
      // Telling automations is never a reason for the caller to fail.
    }
  }

  /** The user's answer to a refusal to save (`dismiss`, `restore` or `keep`). */
  async function resolve(action: "dismiss" | "restore" | "keep") {
    try {
      await daemon("automation.resolve", { action });
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    }
    void load();
  }

  async function setPaused(paused: boolean) {
    setState({ ...state(), paused });
    try {
      await daemon("automation.set_paused", { paused });
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    }
    void load();
  }

  /** True while at least one automation is enabled: the switch in the status bar is worth showing. */
  const active = () => supported() && list().some((a) => a.enabled);
  const wantsPr = () => list().filter((a) => a.enabled && a.trigger?.event === "pr_checks_failed");

  // Reloaded when the daemon says the list changed and whenever the connection (re)appears. (Not in a
  // plain Node test run, where importing this module must stay harmless.)
  if (typeof window !== "undefined") {
    window.addEventListener("kelpie:automations", () => void load());
    createEffect(() => {
      if (store.connected() && store.daemonApi() > 0) void load();
    });
    setInterval(async () => {
      if (!supported() || state().paused) return;
      const targets = wantsPr();
      if (!targets.length) return;
      const { pullRequests } = await import("./pull-requests");
      // A project with two such automations is looked at once.
      const projects = new Map<string, { id: string; path: string }>();
      for (const a of targets) {
        const p = store.state.projects.find((x) => x.id === a.project_id) ?? store.state.projects.find((x) => x.path === a.cwd);
        if (p && !p.remote) projects.set(p.id, p);
      }
      for (const p of projects.values()) void pullRequests.refresh(p.id, p.path);
    }, PR_POLL_MS);
  }

  return { list, state, supported, report, setPaused, resolve, active, reload: load };
}

export const automationEvents = createRoot(createAutomationEvents);
