// A screen the computer refused for its load (`host_busy`): why, in the phone's words with the computer's numbers, what helps, and when the
// phone asks again on its own. The computer refuses only when it is overloaded or has too little memory free for a stream
// (crates/kelpied/src/screen.rs `admit`); a busy one starts the screen at a low quality instead.

type Translate = (text: string, vars?: Record<string, string | number>) => string;

/** What the computer measured when it refused (`busy` in the refusal; absent from a computer before it). */
export interface BusyInfo {
  level?: number;
  reasons?: string[];
  /** `memory`, `cpu`, `lag`, `thermal` or `load`. */
  reason?: string;
  /** Free memory: percent of all of it, and MB; and the least a screen needs (MB). */
  memory_free?: number | null;
  memory_free_mb?: number | null;
  min_free_mb?: number | null;
  /** The processor's use (percent) and how late the computer's own timer ran (ms). */
  cpu?: number | null;
  lag_ms?: number | null;
}

/** The waits before each new try after a busy refusal: about four minutes in all, one try at a time, then the person taps again. */
export const BUSY_RETRY_MS: readonly number[] = [10_000, 15_000, 30_000, 60_000, 60_000, 60_000];

const num = (v: number | null | undefined): number | null => (typeof v === "number" && Number.isFinite(v) ? Math.round(v) : null);

/** Why the computer cannot show its screen now, and what helps. `fallback` is the computer's own words, for a computer that sends no reason. */
export function busyText(reason: string | null | undefined, busy: BusyInfo | null | undefined, fallback: string, t: Translate): string {
  const why = reason ?? busy?.reason ?? null;
  const pct = num(busy?.memory_free);
  const mb = num(busy?.memory_free_mb);
  const cpu = num(busy?.cpu);
  const heavy = t("Wait for a build or another heavy task on it to finish.");
  switch (why) {
    case "memory":
      return pct !== null && mb !== null
        ? t("Only {pct} % of the computer's memory is free ({mb} MB): too little to show its screen. Close some programs on the computer.", { pct, mb })
        : t("The computer is short of memory: too little to show its screen. Close some programs on the computer.");
    case "cpu":
      return `${cpu !== null ? t("The computer's processor is fully busy ({cpu} %).", { cpu }) : t("The computer's processor is fully busy.")} ${heavy}`;
    case "lag":
      return `${t("The computer is responding slowly.")} ${heavy}`;
    case "thermal":
      return t("The computer is too hot and slowed itself down. Let it cool down.");
    case "load":
      return t("The computer is too busy to show its screen now.");
    default:
      return fallback;
  }
}
