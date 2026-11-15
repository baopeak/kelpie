// The monthly stop on Cloudflare's TURN: the bytes TURN sent to clients this UTC month, for the whole Cloudflare account (what Cloudflare
// bills, after its free allowance), read every hour from Cloudflare's GraphQL Analytics API by the router's cron and kept in the Registrar.
// At `TURN_MONTHLY_STOP_GB` (900 unless set; GB = 10^9 bytes) the relay mints no more credentials until the month rolls over.
//
// - On only with Cloudflare's TURN (`TURN_KEY_ID` and `TURN_API_TOKEN`) and both `TURN_ANALYTICS_TOKEN` (an API token with Account →
//   Account Analytics → Read) and `TURN_ACCOUNT_ID`. Without them minting is as it was, held only by the credential limits (turn.ts).
// - A failed reading keeps the last good one: minting goes on with it for 24 hours after it was taken (so one failed hour, or a few, stops
//   nothing), then stops (`turn_usage_unknown`) until a reading succeeds. With no good reading at all, minting is stopped the same way; a
//   refused mint then reads the usage itself, at most once every 5 minutes for the whole relay.
// - A reading of an earlier month no longer stops anything: the new month starts from nothing.
// - The reading lags: Cloudflare's analytics come in minutes late and the cron reads hourly, and credentials minted before the stop keep
//   working until they expire (20 minutes). The stop is a ceiling under the provider's own allowance, not an exact meter.

export interface TurnUsageEnv {
  TURN_ANALYTICS_TOKEN?: string;
  TURN_ACCOUNT_ID?: string;
  /** Where minting stops in a UTC month, in GB (10^9 bytes) of TURN egress. */
  TURN_MONTHLY_STOP_GB?: string;
}

export const TURN_MONTHLY_STOP_GB = 900;
/** How long a reading may stand in for newer ones that failed. */
export const USAGE_STALE_MS = 24 * 3600 * 1000;
/** How often a refused mint may read the usage itself (the cron reads it every hour regardless). */
export const USAGE_RETRY_MS = 5 * 60 * 1000;
/** Why a mint was refused (`x-kelpie-reason`): the month's TURN is spent, or the relay does not know how much is. */
export const TURN_MONTH_FULL = "turn_month_full";
export const TURN_USAGE_UNKNOWN = "turn_usage_unknown";

const GRAPHQL = "https://api.cloudflare.com/client/v4/graphql";
const QUERY_TIMEOUT_MS = 10_000;
// No dimensions: one group, the sum over the range (an empty list when nothing went through TURN). `date_*` are UTC days.
const QUERY =
  "query TurnEgress($account: String!, $from: Date!, $to: Date!) { viewer { accounts(filter: { accountTag: $account }) { " +
  "callsTurnUsageAdaptiveGroups(limit: 1, filter: { date_geq: $from, date_leq: $to }) { sum { egressBytes } } } } }";

/** The UTC month of `now`, `YYYY-MM`. */
export function utcMonth(now: number): string {
  return new Date(now).toISOString().slice(0, 7);
}

export interface UsageReading {
  month: string;
  bytes: number;
  /** When it was read (ms). */
  readAt: number;
}

export type UsageVerdict = "ok" | "month_full" | "unknown";

/** Whether a credential may be minted at `now` by the last good reading. */
export function usageVerdict(reading: UsageReading | null, now: number, stopBytes: number): UsageVerdict {
  if (!reading || now - reading.readAt > USAGE_STALE_MS) return "unknown";
  if (reading.month !== utcMonth(now)) return "ok";
  return reading.bytes >= stopBytes ? "month_full" : "ok";
}

/** The byte count minting stops at, or null when the stop is off (it is never on without Cloudflare's TURN). */
export function usageStopBytes(env: TurnUsageEnv & { TURN_KEY_ID?: string; TURN_API_TOKEN?: string }): number | null {
  if (!env.TURN_ANALYTICS_TOKEN || !env.TURN_ACCOUNT_ID || !env.TURN_KEY_ID || !env.TURN_API_TOKEN) return null;
  const gb = Number(env.TURN_MONTHLY_STOP_GB);
  return (env.TURN_MONTHLY_STOP_GB && Number.isFinite(gb) && gb > 0 ? gb : TURN_MONTHLY_STOP_GB) * 1e9;
}

/** The account's TURN egress so far this UTC month (bytes), or null when the API did not answer it (`fetchFn` for tests). */
export async function readTurnEgress(env: TurnUsageEnv, now: number, fetchFn: typeof fetch = fetch): Promise<number | null> {
  const month = utcMonth(now);
  let body: unknown;
  try {
    const answer = await fetchFn(GRAPHQL, {
      method: "POST",
      headers: { authorization: `Bearer ${env.TURN_ANALYTICS_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ query: QUERY, variables: { account: env.TURN_ACCOUNT_ID, from: `${month}-01`, to: new Date(now).toISOString().slice(0, 10) } }),
      signal: AbortSignal.timeout(QUERY_TIMEOUT_MS),
    });
    if (!answer.ok) return null;
    body = await answer.json();
  } catch {
    return null;
  }
  const { data, errors } = (body ?? {}) as { data?: { viewer?: { accounts?: unknown } }; errors?: unknown };
  if (Array.isArray(errors) && errors.length) return null;
  const accounts = data?.viewer?.accounts;
  // Exactly the one account asked for: none means the token cannot see it.
  if (!Array.isArray(accounts) || accounts.length !== 1) return null;
  const groups = (accounts[0] as { callsTurnUsageAdaptiveGroups?: unknown })?.callsTurnUsageAdaptiveGroups;
  if (!Array.isArray(groups)) return null;
  let total = 0;
  for (const g of groups) {
    const bytes = (g as { sum?: { egressBytes?: unknown } })?.sum?.egressBytes;
    if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) return null;
    total += bytes;
  }
  return total;
}
