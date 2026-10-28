// Context fill of a session as a small ring (amber from 70 %, red from 85 %), with a cost chip
// when the cost is the only number there is (no window: OMP) or the session is over the cost cap.
// Click: Compact / Hand off / Usage & cost. The numbers come from lib/session-usage.ts.
import { Show, createMemo } from "solid-js";
import { isOverCap, openUsageMenu, sessionCost, usageView } from "../lib/session-usage";
import { formatCost, formatTokens, ringLevel } from "../lib/usage-format";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import type { SessionInfo } from "../lib/types";
import "./context-ring.css";

/** Circumference of the ring (r = 6 in a 16-unit box). */
const C = 2 * Math.PI * 6;

export function ContextRing(props: { s: SessionInfo; size?: number }) {
  const v = createMemo(() => usageView(props.s));
  const fraction = () => v()?.fraction ?? null;
  const over = () => isOverCap(props.s);
  // The chip shows what the whole session has cost (all its conversations).
  const chip = () => {
    const u = v();
    const cost = sessionCost(props.s);
    return u && cost !== null && (over() || fraction() === null) ? formatCost(cost, u.exact) : null;
  };
  const title = createMemo(() => {
    const u = v();
    if (!u) return "";
    const lines: string[] = [];
    if (u.fraction !== null) lines.push(t("Context {used} of {window} tokens ({pct}%)", { used: formatTokens(u.used), window: formatTokens(u.window!), pct: Math.round(u.fraction * 100) }));
    else if (u.used > 0) lines.push(t("Context {used} tokens", { used: formatTokens(u.used) }));
    if (u.tokensIn !== null && u.tokensOut !== null) lines.push(t("Tokens {input} in · {output} out", { input: formatTokens(u.tokensIn), output: formatTokens(u.tokensOut) }));
    if (u.cost !== null) lines.push(u.exact ? t("Cost {cost}", { cost: formatCost(u.cost, true) }) : t("Cost {cost} (estimate at API prices)", { cost: formatCost(u.cost, false) }));
    if (u.model) lines.push(u.model);
    if (u.counting) lines.push(t("Still counting the rest of the transcript…"));
    if (over()) lines.push(t("Over the {cap} cost cap", { cap: formatCost(store.state.settings.costCap, true) }));
    lines.push(t("Click for Compact, Hand off, Usage & cost"));
    return lines.join("\n");
  });
  const size = () => props.size ?? 14;
  return (
    <Show when={v() && (fraction() !== null || chip())}>
      <button
        class="ctx-ring-btn"
        data-ctx-ring={fraction() === null ? "" : String(Math.round(fraction()! * 100))}
        title={title()}
        aria-label={title()}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          openUsageMenu(e, props.s);
        }}
      >
        <Show when={fraction() !== null}>
          <svg class={`ctx-ring ${ringLevel(fraction()!)}`} viewBox="0 0 16 16" width={size()} height={size()} aria-hidden="true">
            <circle class="track" cx="8" cy="8" r="6" />
            <circle class="fill" cx="8" cy="8" r="6" stroke-dasharray={`${C * fraction()!} ${C}`} transform="rotate(-90 8 8)" />
          </svg>
        </Show>
        <Show when={chip()}>{(text) => <span class={`cost-chip${over() ? " over" : ""}`} data-cost-chip>{text()}</span>}</Show>
      </button>
    </Show>
  );
}
