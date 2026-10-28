// Pure helpers for the context ring and the usage page: thresholds and number formatting.

/** The ring turns amber at this share of the context window… */
export const WARN_AT = 0.7;
/** …and red at this one (also when the one-time Compact / Hand off hint appears). */
export const DANGER_AT = 0.85;

export type RingLevel = "ok" | "warn" | "danger";

/** Share of the window in use (0–1), or null when the window or the count is unknown. */
export function ringFraction(used: number, window: number | null | undefined): number | null {
  if (!window || window <= 0 || !Number.isFinite(used) || used < 0) return null;
  return Math.min(1, used / window);
}

export function ringLevel(fraction: number): RingLevel {
  return fraction >= DANGER_AT ? "danger" : fraction >= WARN_AT ? "warn" : "ok";
}

/** 999 → "999", 1234 → "1.2k", 12345 → "12k", 1234567 → "1.2M", 2500000000 → "2.5B". */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0";
  const unit = (v: number, suffix: string) => `${v >= 10 ? Math.round(v) : Math.round(v * 10) / 10}${suffix}`;
  if (n < 1000) return String(Math.round(n));
  if (n < 1e6) return unit(n / 1e3, "k");
  if (n < 1e9) return unit(n / 1e6, "M");
  return unit(n / 1e9, "B");
}

/** "$0.43" for an agent's own number, "≈ $0.43" for an estimate at API prices. */
export function formatCost(cost: number, exact: boolean): string {
  const body = cost > 0 && cost < 0.01 ? "<$0.01" : `$${cost >= 100 ? Math.round(cost).toLocaleString("en-US") : cost.toFixed(2)}`;
  return exact ? body : `≈ ${body}`;
}

/** Whether `cost` has reached the cap (0 = no cap). */
export function overCap(cost: number | null | undefined, cap: number): boolean {
  return cap > 0 && cost != null && cost >= cap;
}
