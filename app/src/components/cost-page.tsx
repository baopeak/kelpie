// Usage & cost: tokens and cost by day, week, project, agent or model, from every agent
// transcript on this machine (counted once per file by the app core; a warm page only stats
// the files). Claude costs are estimates at API prices — subscriptions are not billed per
// token — OMP's are its own numbers, Codex has tokens only.
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { onEvent, usageReport } from "../lib/ipc";
import { accountHomes } from "../lib/accounts";
import { formatDate, formatNumber, t } from "../lib/i18n";
import { formatCost, formatTokens } from "../lib/usage-format";
import { folderName, groupRows, type Group, type GroupBy } from "../lib/usage-report";
import { Icon } from "./icons";
import type { UsageTotals } from "../lib/types";
import "./cost-page.css";

const RANGES = [
  { id: "7", days: 7 },
  { id: "30", days: 30 },
  { id: "90", days: 90 },
  { id: "all", days: null },
] as const;

const DAY_MS = 86_400_000;
/** The page asks again when a pass ends, but not more often than this (a live session keeps files changing). */
const MIN_GAP_MS = 3000;

const [by, setBy] = createSignal<GroupBy>("day");
const [range, setRange] = createSignal<(typeof RANGES)[number]["id"]>("30");

/** Local calendar day number of now, as the report counts days. */
const today = () => Math.floor((Date.now() - new Date().getTimezoneOffset() * 60_000) / DAY_MS);

/** A group's heading: a date for the time dimensions, a name otherwise. */
function labelOf(g: Group, dim: GroupBy): string {
  const dateOf = (day: number) => formatDate(new Date(day * DAY_MS), { timeZone: "UTC", weekday: "short", month: "short", day: "numeric" });
  switch (dim) {
    case "day":
      return dateOf(Number(g.key));
    case "week":
      return t("Week of {date}", { date: dateOf(Number(g.key)) });
    case "project":
      return g.key ? folderName(g.key) : t("(unknown folder)");
    case "agent":
      return g.key === "omp" ? "OMP" : g.key === "claude" ? "Claude" : g.key === "codex" ? "Codex" : g.key;
    default:
      return g.key || t("(unknown model)");
  }
}

const dimName = (d: GroupBy) => (d === "day" ? t("Day") : d === "week" ? t("Week") : d === "project" ? t("Project") : d === "agent" ? t("Agent") : t("Model"));

function costText(g: Group): string {
  if (g.cost === 0 && g.unpriced > 0) return "—";
  // Part of it unpriced (an unknown model): the sum is a floor.
  return `${formatCost(g.cost, !g.estimate)}${g.unpriced > 0 ? "+" : ""}`;
}

export default function CostPage() {
  let el!: HTMLDivElement;
  const [report, setReport] = createSignal<UsageTotals | null>(null);
  const [failed, setFailed] = createSignal<string | null>(null);
  let last = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const load = async () => {
    last = Date.now();
    try {
      setReport(await usageReport(-new Date().getTimezoneOffset(), accountHomes()));
      setFailed(null);
    } catch (e) {
      setFailed(String(e));
    }
  };
  onMount(() => {
    el.focus();
    void load();
    // A counting pass ends with `usage-scanned`: show what it found.
    const unlisten = onEvent("usage-scanned", () => {
      clearTimeout(timer);
      timer = setTimeout(() => void load(), Math.max(0, MIN_GAP_MS - (Date.now() - last)));
    });
    onCleanup(() => {
      clearTimeout(timer);
      void unlisten.then((f) => f());
    });
  });

  const minDay = () => {
    const days = RANGES.find((r) => r.id === range())?.days;
    return days ? today() - days + 1 : null;
  };
  const groups = createMemo(() => groupRows(report()?.rows ?? [], by(), minDay()));
  const total = createMemo(() => groupRows(report()?.rows ?? [], "agent", minDay()).reduce(
    (a, g) => ({ tokensIn: a.tokensIn + g.tokensIn, tokensOut: a.tokensOut + g.tokensOut, cost: a.cost + g.cost, unpriced: a.unpriced + g.unpriced, estimate: a.estimate || g.estimate, key: "" }),
    { key: "", tokensIn: 0, tokensOut: 0, cost: 0, unpriced: 0, estimate: false } as Group,
  ));
  const maxCost = createMemo(() => Math.max(0.0001, ...groups().map((g) => g.cost)));
  const close = () => store.setOverlay(null);

  return (
    <div
      class="cost-page"
      ref={el}
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "Escape" && !e.defaultPrevented) close();
      }}
    >
      <div class="board-head">
        <button class="settings-back" onClick={close} title={t("Back (Esc)")}><Icon name="back" size={14} /></button>
        <h2>{t("Usage & cost")}</h2>
        <span class="muted-inline">{t("Claude costs are estimates at API prices; OMP's are its own numbers; Codex shows tokens only.")}</span>
        <span style={{ flex: 1 }} />
        <button class="icon-btn" title={t("Refresh")} onClick={() => void load()}><Icon name="refresh" size={14} /></button>
      </div>
      <div class="cost-controls">
        <div class="toggles" role="group" aria-label={t("Group by")}>
          <For each={["day", "week", "project", "agent", "model"] as GroupBy[]}>
            {(d) => (
              <button class={`toggle${by() === d ? " on" : ""}`} data-by={d} onClick={() => setBy(d)}>
                {dimName(d)}
              </button>
            )}
          </For>
        </div>
        <div class="toggles" role="group" aria-label={t("Period")}>
          <For each={RANGES}>
            {(r) => (
              <button class={`toggle${range() === r.id ? " on" : ""}`} data-range={r.id} onClick={() => setRange(r.id)}>
                {r.days ? t("{n} days", { n: r.days }) : t("All time")}
              </button>
            )}
          </For>
        </div>
        <span style={{ flex: 1 }} />
        <Show when={report() && report()!.pending > 0}>
          <span class="muted-inline" data-pending>{t("Counting {n} transcripts…", { n: formatNumber(report()!.pending) })}</span>
        </Show>
      </div>
      <Show when={failed()}><p class="update-error cost-error">{failed()}</p></Show>
      <div class="cost-summary" data-total>
        <div><b>{costText(total())}</b><span>{t("cost")}</span></div>
        <div><b>{formatTokens(total().tokensIn)}</b><span>{t("tokens in")}</span></div>
        <div><b>{formatTokens(total().tokensOut)}</b><span>{t("tokens out")}</span></div>
      </div>
      <div class="cost-table">
        <div class="cost-row head">
          <span>{dimName(by())}</span>
          <span class="num">{t("Tokens in")}</span>
          <span class="num">{t("Tokens out")}</span>
          <span class="num">{t("Cost")}</span>
          <span />
        </div>
        <For each={groups()} fallback={<div class="muted cost-empty">{report() ? t("Nothing in this period.") : t("Reading transcripts…")}</div>}>
          {(g) => (
            <div class="cost-row" data-row={g.key} title={by() === "project" ? g.key : undefined}>
              <span class="label">{labelOf(g, by())}</span>
              <span class="num">{formatTokens(g.tokensIn)}</span>
              <span class="num">{formatTokens(g.tokensOut)}</span>
              <span class="num" title={g.unpriced > 0 ? t("{n} tokens have no known price", { n: formatNumber(g.unpriced) }) : undefined}>{costText(g)}</span>
              <span class="bar"><span style={{ width: `${(g.cost / maxCost()) * 100}%` }} /></span>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}
