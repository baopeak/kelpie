// How many rooms were made today, per network and in all, and how many TURN credentials the relay minted today: one Durable Object for
// the whole relay, asked only when a room is about to be made without a registration key (only after the rate binding let the request
// through), or a credential minted. It stores a count per UTC day and per
// network, the network being a hash (never an address), and forgets every day before yesterday the next time it counts. It also keeps the
// last good reading of the month's TURN egress (turn-usage.ts), which the router's hourly cron writes. Nothing runs on a timer here.

import { DurableObject } from "cloudflare:workers";
import { USAGE_RETRY_MS, usageVerdict, type UsageReading, type UsageVerdict } from "./turn-usage";

export type Admission = "ok" | "network" | "full";

/** The row that counts every room of a day, beside the networks' rows (which are 64 hex characters). */
export const TOTAL = "total";
const NETWORK_KEY = /^[0-9a-f]{64}$/;

/** The UTC day of `now`, `YYYY-MM-DD`. */
export function utcDay(now: number): string {
  return new Date(now).toISOString().slice(0, 10);
}

export class Registrar extends DurableObject<object> {
  constructor(ctx: DurableObjectState, env: object) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS rooms (day TEXT NOT NULL, key TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (day, key))");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS turn_mints (day TEXT NOT NULL, key TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY (day, key))");
    // One row: the last good reading (all three null before the first), and when a refused mint last read the usage itself.
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS turn_usage (id INTEGER PRIMARY KEY CHECK (id = 1), month TEXT, bytes INTEGER, read_at INTEGER, tried_at INTEGER)");
  }

  /** Counts one more room for `network` today, unless the network or the whole relay has made its day's rooms already.
   *  Nothing in here waits, so two admissions never read the same count. */
  admit(network: string, now: number, perNetwork: number, perDay: number): Admission {
    if (!NETWORK_KEY.test(network)) throw new Error("not a network key");
    const day = utcDay(now);
    if (this.count(day, network) >= perNetwork) return "network";
    if (this.count(day, TOTAL) >= perDay) return "full";
    const sql = this.ctx.storage.sql;
    sql.exec("DELETE FROM rooms WHERE day < ?", utcDay(now - 24 * 3600 * 1000));
    for (const key of [network, TOTAL]) {
      sql.exec("INSERT INTO rooms (day, key, count) VALUES (?, ?, 1) ON CONFLICT (day, key) DO UPDATE SET count = count + 1", day, key);
    }
    return "ok";
  }

  /** Counts one more TURN credential minted today for `network`, unless that network (`perNetwork`) or the whole relay (`perDay`) has minted
   *  its day's. */
  admitTurn(network: string, now: number, perNetwork: number, perDay: number): Admission {
    if (!NETWORK_KEY.test(network)) throw new Error("not a network key");
    const day = utcDay(now);
    const sql = this.ctx.storage.sql;
    const count = (key: string) => sql.exec<{ count: number }>("SELECT count FROM turn_mints WHERE day = ? AND key = ?", day, key).toArray()[0]?.count ?? 0;
    if (count(network) >= perNetwork) return "network";
    if (count(TOTAL) >= perDay) return "full";
    sql.exec("DELETE FROM turn_mints WHERE day < ?", utcDay(now - 24 * 3600 * 1000));
    for (const key of [network, TOTAL]) {
      sql.exec("INSERT INTO turn_mints (day, key, count) VALUES (?, ?, 1) ON CONFLICT (day, key) DO UPDATE SET count = count + 1", day, key);
    }
    return "ok";
  }

  /** The last good reading of the month's TURN egress, or null before the first. */
  turnUsage(): UsageReading | null {
    const row = this.ctx.storage.sql
      .exec<{ month: string | null; bytes: number | null; read_at: number | null }>("SELECT month, bytes, read_at FROM turn_usage WHERE id = 1")
      .toArray()[0];
    return row && row.month !== null && row.bytes !== null && row.read_at !== null ? { month: row.month, bytes: row.bytes, readAt: row.read_at } : null;
  }

  /** Whether the monthly stop lets a credential be minted at `now` (the stop at `stopBytes`). Counts nothing. */
  turnStop(now: number, stopBytes: number): UsageVerdict {
    return usageVerdict(this.turnUsage(), now, stopBytes);
  }

  /** Keeps a good reading: `bytes` of TURN egress in `month`, read at `now`; never over a reading taken later (a slow one finishing last). */
  recordTurnUsage(month: string, bytes: number, now: number): void {
    if (!/^\d{4}-\d{2}$/.test(month) || !Number.isSafeInteger(bytes) || bytes < 0) throw new Error("not a reading");
    this.ctx.storage.sql.exec(
      "INSERT INTO turn_usage (id, month, bytes, read_at) VALUES (1, ?, ?, ?) ON CONFLICT (id) DO UPDATE SET month = excluded.month, bytes = excluded.bytes, read_at = excluded.read_at " +
        "WHERE turn_usage.read_at IS NULL OR excluded.read_at >= turn_usage.read_at",
      month,
      bytes,
      now,
    );
  }

  /** Whether a refused mint may read the usage itself now: once every `USAGE_RETRY_MS` for the whole relay. */
  claimUsageRetry(now: number): boolean {
    const sql = this.ctx.storage.sql;
    const tried = sql.exec<{ tried_at: number | null }>("SELECT tried_at FROM turn_usage WHERE id = 1").toArray()[0]?.tried_at ?? null;
    if (tried !== null && now - tried < USAGE_RETRY_MS && now >= tried) return false;
    sql.exec("INSERT INTO turn_usage (id, tried_at) VALUES (1, ?) ON CONFLICT (id) DO UPDATE SET tried_at = excluded.tried_at", now);
    return true;
  }

  private count(day: string, key: string): number {
    const row = this.ctx.storage.sql.exec<{ count: number }>("SELECT count FROM rooms WHERE day = ? AND key = ?", day, key).toArray()[0];
    return row?.count ?? 0;
  }
}
