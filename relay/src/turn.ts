// TURN credentials for a room's screen: short-lived ones, minted only for the room's own desktop and only inside its limits, so that the
// API token never leaves the Worker and a stranger's room cannot make the operator's TURN a free relay.
//
// - `POST /v1/turn?t=<ticket>` `{ts, sig}`: `sig` is the room key's ECDSA P-256 signature over `kelpie-relay/1 turn <room> <ts>`, `ts`
//   within 5 minutes and above the last one used for TURN. The room must have at least one phone allowed in. Answered
//   `{ice_servers, username, expires}`: Cloudflare's TURN (secrets `TURN_KEY_ID` and `TURN_API_TOKEN`), or a coturn server's time-limited
//   credentials (`TURN_SECRET`, and `TURN_URLS` comma-separated), else 404 `turn_not_configured`.
// - `POST /v1/turn/revoke?t=<ticket>` `{ts, sig, username}`: the signature over `kelpie-relay/1 turn_revoke <room> <ts> <username>`.
// - Who: a room made with the registration key, or any room where `TURN_OPEN_ROOMS = "1"` (the limits count credentials, not bytes: on a relay
//   with open registration a stranger could otherwise spend the operator's TURN). Refused rooms hear 404 `turn_not_offered`.
// - Limits: per room `TURN_MINTS_PER_HOUR` and `TURN_MINTS_PER_ROOM_DAY` (the room counts), per network `TURN_MINTS_PER_NETWORK_DAY` and per
//   relay `TURN_MINTS_PER_DAY` a UTC day (the Registrar; vars may change them). Nothing about a credential is logged. The relay cannot read
//   the bytes TURN carried: the operator sets a usage notification at the provider, and removes the secrets to stop at once.

export interface TurnEnv {
  TURN_KEY_ID?: string;
  TURN_API_TOKEN?: string;
  TURN_SECRET?: string;
  TURN_URLS?: string;
  TURN_MINTS_PER_DAY?: string;
  TURN_MINTS_PER_NETWORK_DAY?: string;
  /** "1": rooms made without the registration key get TURN too. Off by default: limits count credentials, not bytes, and anyone can make
   *  such a room and pair a phone of their own to it, so turning it on means the operator accepts what strangers' traffic may cost (set a
   *  usage notification at the TURN provider first). */
  TURN_OPEN_ROOMS?: string;
}

/** How long a credential lasts (s): the daemon asks for a new one before, while a screen lasts. */
export const TURN_TTL_S = 1200;
export const TURN_MINTS_PER_HOUR = 10;
export const TURN_MINTS_PER_ROOM_DAY = 40;
export const TURN_MINTS_PER_DAY = 500;
export const TURN_MINTS_PER_NETWORK_DAY = 20;
/** Why a mint was refused (`x-kelpie-reason`): the daemon says it on the phone's row. */
export const TURN_NOT_CONFIGURED = "turn_not_configured";
export const TURN_FULL = "turn_full";
export const TURN_ROOM_LIMIT = "turn_room_limit";
export const TURN_NO_PHONES = "no_phones";
export const TURN_UPSTREAM = "turn_upstream";
export const TURN_NOT_OFFERED = "turn_not_offered";
export const TURN_NETWORK = "turn_network";

const CLOUDFLARE = "https://rtc.live.cloudflare.com/v1/turn/keys";
const USERNAME = /^[A-Za-z0-9:_.+/=-]{1,256}$/;

export function turnStatement(room: string, ts: number): string {
  return `kelpie-relay/1 turn ${room} ${ts}`;
}

export function revokeStatement(room: string, ts: number, username: string): string {
  return `kelpie-relay/1 turn_revoke ${room} ${ts} ${username}`;
}

export function validUsername(u: unknown): u is string {
  return typeof u === "string" && USERNAME.test(u);
}

export type TurnMode = "cloudflare" | "coturn";

/** Which TURN this relay mints for, from its secrets; null when none is set. */
export function turnMode(env: TurnEnv): TurnMode | null {
  if (env.TURN_KEY_ID && env.TURN_API_TOKEN) return "cloudflare";
  if (env.TURN_SECRET && env.TURN_URLS) return "coturn";
  return null;
}

export interface IceServer {
  urls: string[];
  username?: string;
  credential?: string;
}

/** A URL on port 53: some networks treat it as DNS and break it; Cloudflare offers it, browsers time out on it. */
function port53(url: string): boolean {
  return /:53(\?|$)/.test(url);
}

/** The servers as the phone gets them: only `stun:`, `turn:` and `turns:` URLs, none on port 53, and no server left without one. */
export function cleanServers(servers: unknown): IceServer[] {
  if (!Array.isArray(servers)) return [];
  const out: IceServer[] = [];
  for (const s of servers) {
    const raw = (s as { urls?: unknown })?.urls;
    const urls = (Array.isArray(raw) ? raw : [raw]).filter((u): u is string => typeof u === "string" && /^(stun|turns?):/.test(u) && !port53(u));
    if (!urls.length) continue;
    const { username, credential } = s as { username?: unknown; credential?: unknown };
    out.push({ urls, ...(typeof username === "string" ? { username } : {}), ...(typeof credential === "string" ? { credential } : {}) });
  }
  return out;
}

export interface Minted {
  ice_servers: IceServer[];
  username: string;
  /** When it stops working (ms). */
  expires: number;
}

export type MintOutcome = { ok: true; minted: Minted } | { ok: false; status: number; reason: string };

/** Credentials from Cloudflare's TURN API (`fetchFn` for tests). */
export async function cloudflareMint(env: TurnEnv, now: number, fetchFn: typeof fetch = fetch): Promise<MintOutcome> {
  let answer: Response;
  try {
    answer = await fetchFn(`${CLOUDFLARE}/${encodeURIComponent(env.TURN_KEY_ID ?? "")}/credentials/generate-ice-servers`, {
      method: "POST",
      headers: { authorization: `Bearer ${env.TURN_API_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ ttl: TURN_TTL_S }),
    });
  } catch {
    return { ok: false, status: 502, reason: TURN_UPSTREAM };
  }
  if (answer.status !== 201 && answer.status !== 200) return { ok: false, status: 502, reason: TURN_UPSTREAM };
  const body = (await answer.json().catch(() => null)) as { iceServers?: unknown } | null;
  const ice = cleanServers(body?.iceServers);
  const username = ice.find((s) => s.username)?.username;
  if (!ice.some((s) => s.urls.some((u) => u.startsWith("turn"))) || !username) return { ok: false, status: 502, reason: TURN_UPSTREAM };
  return { ok: true, minted: { ice_servers: ice, username, expires: now + TURN_TTL_S * 1000 } };
}

/** coturn's time-limited credentials (its `use-auth-secret`): `<expiry s>:<room>`, and HMAC-SHA1 of it with the shared secret, base64. */
export async function coturnMint(env: TurnEnv, room: string, now: number): Promise<MintOutcome> {
  const expires = now + TURN_TTL_S * 1000;
  const username = `${Math.floor(expires / 1000)}:${room}`;
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(env.TURN_SECRET ?? ""), { name: "HMAC", hash: "SHA-1" }, false, ["sign"]);
  const credential = btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(username)))));
  const urls = (env.TURN_URLS ?? "").split(",").map((u) => u.trim()).filter(Boolean);
  const ice = cleanServers([{ urls, username, credential }]);
  if (!ice.length) return { ok: false, status: 404, reason: TURN_NOT_CONFIGURED };
  return { ok: true, minted: { ice_servers: ice, username, expires } };
}

/** Ends a credential before its time (Cloudflare; coturn's cannot be, and expire on their own). */
export async function cloudflareRevoke(env: TurnEnv, username: string, fetchFn: typeof fetch = fetch): Promise<number> {
  try {
    const answer = await fetchFn(`${CLOUDFLARE}/${encodeURIComponent(env.TURN_KEY_ID ?? "")}/credentials/${encodeURIComponent(username)}/revoke`, {
      method: "POST",
      headers: { authorization: `Bearer ${env.TURN_API_TOKEN}` },
    });
    return answer.status === 204 || answer.status === 200 ? 204 : 502;
  } catch {
    return 502;
  }
}

