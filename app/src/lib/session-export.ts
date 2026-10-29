// Sessions panel → Export as Markdown… (loaded on first use). The app core finds, reads, redacts
// and writes the conversation (session_export.rs); this asks where to save it and reports. The
// end-to-end tests replace the save dialog (`window.__kelpie.sessionExport.replacePicker`).
import { save } from "@tauri-apps/plugin-dialog";
import { agentSessionExport } from "./ipc";
import { store } from "./store";
import { errorText, t } from "./i18n";
import type { HistoryItem } from "./types";

type Picker = (defaultPath: string) => Promise<string | null>;

let pick: Picker = (defaultPath) => save({ title: t("Export conversation as Markdown"), defaultPath, filters: [{ name: "Markdown", extensions: ["md"] }] });

/** End-to-end tests only (this module is reachable from them through `window.__kelpie`, which dev
 *  and KELPIE_E2E builds alone expose): answers the save dialog instead of showing it. */
export function replacePicker(fn: Picker) {
  pick = fn;
}

/** `claude-0c1d2e3f-2026-10-24.md`: no title in the name (it is not redacted). */
export function fileName(it: HistoryItem): string {
  const d = new Date(it.updated_at);
  const day = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  return `${it.agent}-${it.id.slice(0, 8)}-${day}.md`;
}

/** Conversations being exported now (a second click waits for the first). */
const running = new Set<string>();

export async function exportConversation(it: HistoryItem) {
  const key = `${it.agent}:${it.id}`;
  if (running.has(key)) return;
  running.add(key);
  try {
    const path = await pick(fileName(it));
    if (!path) return;
    const r = await agentSessionExport({ agent: it.agent, id: it.id, cwd: it.cwd || undefined, home: it.home ?? null, title: it.title ?? it.first_prompt ?? null, path });
    const lines = [t("Exported to {path}", { path: r.path })];
    if (r.truncated) lines.push(t("The conversation is too long: the file stops at the size limit and says so."));
    if (r.skipped) lines.push(r.skipped === 1 ? t("{n} unreadable transcript line was left out.", { n: 1 }) : t("{n} unreadable transcript lines were left out.", { n: r.skipped }));
    store.notify(lines.join("\n"));
  } catch (e) {
    store.notify(t("Could not export the conversation: {error}", { error: errorText(e) }));
  } finally {
    running.delete(key);
  }
}
