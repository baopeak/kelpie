// What a review card and a compared candidate say about a worktree besides its diff: what its
// sessions cost, how many agent turns ran there, and the state of the base's checks.
import { store } from "./store";
import { sessionCost } from "./session-usage";
import { listTurns, visibleTurns } from "./checkpoints";
import { CHECKPOINT_API } from "./checkpoint-api";
import { runnable, type ChecksPlan } from "./finish";
import { t } from "./i18n";

/** The sessions of a project (worktree). */
export const sessionsOf = (projectId: string) => Object.values(store.sessions).filter((s) => s.project_id === projectId);

/** What the worktree's sessions cost (0 when no reading exists yet). */
export const worktreeCost = (projectId: string) => sessionsOf(projectId).reduce((sum, s) => sum + (sessionCost(s) ?? 0), 0);

/** Agent turns the sessions ran, from the daemon's snapshots; null on a daemon without them. */
export async function countTurns(sessionIds: string[]): Promise<number | null> {
  if (store.daemonApi() < CHECKPOINT_API) return null;
  let n = 0;
  for (const id of sessionIds) {
    const list = await listTurns(id).catch(() => null);
    if (list) n += visibleTurns(list).filter((r) => r.kind === "turn").length;
  }
  return n;
}

export interface ChecksChip {
  text: string;
  cls: "ok" | "warn" | "";
  title: string;
  /** Machine-readable state, for comparing candidates. */
  state: "unreadable" | "off" | "approval" | "ready" | "not-runnable";
}

/** The base's checks as a chip (null: none, or not read yet). */
export function checksChip(pl: ChecksPlan | undefined): ChecksChip | null {
  if (!pl) return null;
  if (pl.error) return { text: t("checks unreadable"), cls: "warn", title: pl.error, state: "unreadable" };
  if (!pl.commands.length) return null;
  if (pl.blocked) return { text: t("checks off: edits .kelpie.yaml"), cls: "warn", title: t("This branch changes .kelpie.yaml, so its checks do not run unless you allow it in Finish."), state: "off" };
  if (!pl.approved) return { text: t("checks need approval"), cls: "warn", title: pl.commands.join("\n"), state: "approval" };
  const ok = runnable(pl, false);
  return { text: pl.commands.length === 1 ? t("1 check") : t("{n} checks", { n: pl.commands.length }), cls: ok ? "ok" : "", title: pl.commands.join("\n"), state: ok ? "ready" : "not-runnable" };
}
