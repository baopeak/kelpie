// Reactions to a session's usage that most sessions never need: the ring's menu, the one-time hint
// at 85 % and the cost cap's inbox entry and Esc. Loaded on first use, not at startup.
import { store, sessionLabel } from "./store";
import { daemon, sendInput } from "./ipc";
import { NOTIFY_API } from "./notify-events";
import { conversationCommands, handOffSession, runConversationCommand } from "./agent-actions";
import { isAgentKind, kindInfo, launchableAgents } from "./agents";
import { openContextMenu, type MenuItem } from "../components/context-menu";
import { hasExactReading, isOverCap, sessionCost, usageView } from "./session-usage";
import { DANGER_AT, formatCost, overCap } from "./usage-format";
import { t } from "./i18n";
import type { SessionInfo } from "./types";

/** Escape into a terminal session whose numbers come from its own exact transcript. */
async function interrupt(s: SessionInfo) {
  if (!s.alive || !hasExactReading(s)) return;
  await sendInput(s.handle, "\u001b").catch((e) => store.notify(String(e)));
}

/** The ring's menu, and whether it holds an action on the conversation (Compact, Hand off). */
function menuFor(s: SessionInfo): { items: MenuItem[]; actions: boolean } {
  const items: MenuItem[] = [];
  if (conversationCommands(s) && !s.remote) items.push({ label: t("Compact conversation"), run: () => void runConversationCommand(s, "compact") });
  if (!s.chat && s.project_id && isAgentKind(s.kind)) {
    for (const kind of launchableAgents(store.state.settings.hiddenAgents).filter((k) => k !== s.kind).slice(0, 4)) {
      items.push({ label: t("Hand Off to {agent}", { agent: kindInfo(kind).label }), run: () => void handOffSession(s, kind) });
    }
  }
  const actions = items.length > 0;
  if (isOverCap(s) && !s.chat && s.status === "working") items.push({ label: t("Interrupt (Esc)"), run: () => void interrupt(s) });
  if (items.length) items.push(null);
  items.push({ label: t("Usage & cost…"), run: () => store.setOverlay("cost") });
  return { items, actions };
}

export function openMenu(e: MouseEvent, s: SessionInfo) {
  openContextMenu(e, menuFor(s).items);
}

// ---------- one-time hint and cost cap ----------

const HINTED_KEY = "kelpie-context-hinted";
const hinted = (() => {
  try {
    return new Set<string>(JSON.parse(localStorage.getItem(HINTED_KEY) ?? "[]"));
  } catch {
    return new Set<string>();
  }
})();

function markHinted(id: string) {
  hinted.add(id);
  try {
    localStorage.setItem(HINTED_KEY, JSON.stringify([...hinted].slice(-300)));
  } catch {
    /* storage unavailable: the hint may show again after a restart */
  }
}

function hintFull(s: SessionInfo, fraction: number) {
  markHinted(s.id);
  // A button only where there is something to offer (an ACP chat has neither Compact nor Hand off).
  const action = menuFor(s).actions ? { label: t("Compact / Hand off…"), run: (e?: MouseEvent) => e && openMenu(e, s) } : undefined;
  store.notify(t("{name}: context {pct}% full", { name: sessionLabel(s), pct: Math.round(fraction * 100) }), action, 12_000);
}

/** Sessions already reported over the cap in this run; a report survives restarts as an inbox entry. */
const capReported = new Set<string>();

/** Sessions already interrupted for the cap: once each, at the first moment one is over and working. */
const escSent = new Set<string>();

function reportOverCap(s: SessionInfo, cost: number, exact: boolean) {
  if (capReported.has(s.id)) return;
  capReported.add(s.id);
  if ((store.state.inbox ?? []).some((i) => i.kind === "cost" && i.sessionId === s.id)) return;
  const cap = store.state.settings.costCap;
  const title = t("{name} passed the cost cap", { name: sessionLabel(s) });
  const body = t("{cost} of {cap}", { cost: formatCost(cost, exact), cap: formatCost(cap, true) });
  store.addInbox({ kind: "cost", title, body, sessionId: s.id, projectId: s.project_id ?? undefined });
  void store.desktopNotify(title, body);
  // Only this window reads usage, so it tells the daemon, which carries it to the phone channels.
  if (store.daemonApi() >= NOTIFY_API) {
    const detail = `Passed the cost cap: ${formatCost(cost, exact)} of ${formatCost(cap, true)}`; // i18n-ignore: goes to the phone channels
    void daemon("notify.report", { kind: "cost", session: s.id, detail }).catch(() => {});
  }
}

/** Hints and cap reactions for every session that is due one. Safe to call again at any change. */
export function run() {
  const cap = store.state.settings.costCap;
  for (const s of Object.values(store.sessions)) {
    const v = usageView(s);
    if (v?.fraction != null && v.fraction >= DANGER_AT && !hinted.has(s.id) && document.hasFocus() && store.isSessionVisible(s.id)) hintFull(s, v.fraction);
    const cost = sessionCost(s);
    if (!overCap(cost, cap) || cost === null) continue;
    reportOverCap(s, cost, v?.exact ?? false);
    // The first time it is both over the cap and working (also the turn after the one that crossed it).
    if (store.state.settings.costCapEsc && s.status === "working" && !escSent.has(s.id)) {
      escSent.add(s.id);
      void interrupt(s);
    }
  }
}
