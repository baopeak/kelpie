// How many rooms were made today, per network and in all: one Durable Object for the whole relay, asked only when a room is about to be
// made without a registration key, and only after the rate binding let the request through. It stores a count per UTC day and per
// network, the network being a hash (never an address), and forgets every day before yesterday the next time it counts. Nothing runs
// on a timer: an idle Registrar costs nothing.

import { DurableObject } from "cloudflare:workers";

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

  private count(day: string, key: string): number {
    const row = this.ctx.storage.sql.exec<{ count: number }>("SELECT count FROM rooms WHERE day = ? AND key = ?", day, key).toArray()[0];
    return row?.count ?? 0;
  }
}
