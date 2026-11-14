//! TURN for a screen through the relay: short-lived credentials the relay mints for this computer's room (`relay/src/turn.ts`), handed to the
//! phone inside its encrypted link only, and the bytes that go through TURN counted against a screen's, a day's and a month's caps, since the
//! relay's operator pays for that traffic.
//!
//! - **Mint:** `POST <relay>/v1/turn?t=<ticket>` `{ts, sig}`, signed with the room key over `kelpie-relay/1 turn <room> <ts>`. A relay that has
//!   no TURN (or knows no such path) answers 404: the way is "not configured" and the screen goes on without it.
//! - **Revoke:** `POST <relay>/v1/turn/revoke?t=<ticket>` `{ts, sig, username}` over `kelpie-relay/1 turn_revoke <room> <ts> <username>`, when a
//!   screen ends or reaches a cap. Best effort: a credential ends on its own after 20 minutes anyway.
//! - **Nothing about a credential is logged**: mints, revokes and caps say only that they happened, and sizes.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::relay::{e2e_guard, http_url, https_agent, sign};
use super::store::MonthUse;
use super::Mobile;
use crate::session::now_millis;

/// A refused mint keeps the way off for this long (the phone's row says why meanwhile): the relay's own answer (no TURN, its day used up)
/// for ten minutes, a failure (the network, the provider) for one.
const REFUSED_FOR: Duration = Duration::from_secs(600);
const FAILED_FOR: Duration = Duration::from_secs(60);
const MINT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SERVERS: usize = 8;
const MAX_FIELD: usize = 512;

pub fn turn_statement(room: &str, ts: u64) -> String {
    format!("kelpie-relay/1 turn {room} {ts}")
}

pub fn revoke_statement(room: &str, ts: u64, username: &str) -> String {
    format!("kelpie-relay/1 turn_revoke {room} {ts} {username}")
}

/// Credentials as the relay minted them: the servers to give the phone, the name to revoke them by, and when they end (ms).
#[derive(Clone, Debug, PartialEq)]
pub struct Minted {
    pub ice_servers: Value,
    pub username: String,
    pub expires: u64,
}

/// Why there is no TURN now, as the phone's row says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The relay has none (or is from before TURN).
    NotConfigured,
    /// The relay's day, or this room's hour or day, is used up.
    Full,
    /// The relay is off, or this computer has no room there.
    RelayOff,
    Failed,
}

impl Refusal {
    pub fn code(self) -> &'static str {
        match self {
            Refusal::NotConfigured => "turn_not_configured",
            Refusal::Full => "turn_full",
            Refusal::RelayOff => "relay_off",
            Refusal::Failed => "turn_failed",
        }
    }
}

/// The servers of the relay's answer that a phone may be given: `stun:`, `turn:` and `turns:` URLs, with their name and credential, no more
/// than a few.
fn checked_servers(v: &Value) -> Option<Value> {
    let field = |x: &Value| x.as_str().filter(|s| !s.is_empty() && s.len() <= MAX_FIELD && !s.chars().any(char::is_control)).map(str::to_string);
    let out: Vec<Value> = v
        .as_array()?
        .iter()
        .take(MAX_SERVERS)
        .filter_map(|s| {
            let urls: Vec<String> = s["urls"].as_array()?.iter().filter_map(field).filter(|u| u.starts_with("stun:") || u.starts_with("turn:") || u.starts_with("turns:")).collect();
            if urls.is_empty() {
                return None;
            }
            let mut server = json!({"urls": urls});
            if let (Some(u), Some(c)) = (field(&s["username"]), field(&s["credential"])) {
                server["username"] = json!(u);
                server["credential"] = json!(c);
            }
            Some(server)
        })
        .collect();
    out.iter().any(|s| s["urls"].as_array().is_some_and(|u| u.iter().any(|u| u.as_str().is_some_and(|u| u.starts_with("turn"))))).then(|| json!(out))
}

/// The relay's answer to a mint, read.
fn read_minted(status: u16, reason: Option<&str>, body: &str) -> Result<Minted, Refusal> {
    match (status, reason) {
        (200, _) => {}
        (404, _) => return Err(Refusal::NotConfigured),
        (429, Some("turn_room_limit" | "turn_network")) | (503, Some("turn_full")) => return Err(Refusal::Full),
        _ => return Err(Refusal::Failed),
    }
    let v: Value = serde_json::from_str(body).map_err(|_| Refusal::Failed)?;
    let ice_servers = checked_servers(&v["ice_servers"]).ok_or(Refusal::Failed)?;
    let username = v["username"].as_str().filter(|u| !u.is_empty() && u.len() <= MAX_FIELD).ok_or(Refusal::Failed)?.to_string();
    let expires = v["expires"].as_u64().filter(|e| *e > now_millis()).ok_or(Refusal::Failed)?;
    Ok(Minted { ice_servers, username, expires })
}

impl Mobile {
    /// The relay's address, this computer's room and ticket, and its key, when the relay is on: what signs a TURN request.
    fn turn_room(&self) -> Result<(String, String, String, p256::ecdsa::SigningKey), Refusal> {
        let r = self.relay_live().ok_or(Refusal::RelayOff)?;
        let (Some(room), Some(ticket)) = (r.room.clone(), r.ticket.clone()) else { return Err(Refusal::RelayOff) };
        let key = self.room_key().ok().flatten().ok_or(Refusal::RelayOff)?;
        Ok((r.url, room, ticket, key))
    }

    /// New credentials for a screen through the relay. Blocking: called off the async threads. A refusal keeps the way off for a while
    /// (`turn_refused`).
    pub(crate) fn mint_turn(&self) -> Result<Minted, Refusal> {
        if let Some(why) = self.turn_refused() {
            return Err(why);
        }
        let out = self.ask_mint();
        match &out {
            Ok(_) => tracing::info!("screen: TURN credentials minted"),
            // The relay's own answer is kept a while; a relay that is off says so by itself.
            Err(Refusal::RelayOff) => {}
            Err(why) => {
                tracing::info!(why = why.code(), "screen: no TURN credentials");
                *self.screen_memory.turn_failed.lock() = Some((Instant::now(), *why));
            }
        }
        out
    }

    fn ask_mint(&self) -> Result<Minted, Refusal> {
        let (url, room, ticket, key) = self.turn_room()?;
        e2e_guard(&url).map_err(|_| Refusal::Failed)?;
        let _in_order = self.screen_memory.turn_requests.lock();
        let ts = self.relay.next_ts();
        let body = json!({"ts": ts, "sig": sign(&key, &turn_statement(&room, ts))}).to_string();
        let agent = https_agent(MINT_TIMEOUT);
        let mut resp = agent.post(&format!("{}/v1/turn?t={ticket}", http_url(&url))).content_type("application/json").send(body.as_bytes()).map_err(|_| Refusal::Failed)?;
        let status = resp.status().as_u16();
        let reason = resp.headers().get("x-kelpie-reason").and_then(|v| v.to_str().ok()).map(str::to_string);
        let text = if status == 200 { resp.body_mut().read_to_string().unwrap_or_default() } else { String::new() };
        read_minted(status, reason.as_deref(), &text)
    }

    /// Ends `username`'s credential before its time. Blocking, best effort.
    pub(crate) fn revoke_turn(&self, username: &str) {
        let Ok((url, room, ticket, key)) = self.turn_room() else { return };
        if e2e_guard(&url).is_err() {
            return;
        }
        let _in_order = self.screen_memory.turn_requests.lock();
        let ts = self.relay.next_ts();
        let body = json!({"ts": ts, "sig": sign(&key, &revoke_statement(&room, ts, username)), "username": username}).to_string();
        let status = https_agent(MINT_TIMEOUT).post(&format!("{}/v1/turn/revoke?t={ticket}", http_url(&url))).content_type("application/json").send(body.as_bytes()).map(|r| r.status().as_u16());
        tracing::info!(revoked = matches!(status, Ok(204)), "screen: TURN credentials ended");
    }

    /// Why TURN is off now after a failed mint, for a while after it.
    pub(crate) fn turn_refused(&self) -> Option<Refusal> {
        let mut failed = self.screen_memory.turn_failed.lock();
        match *failed {
            Some((at, why)) if at.elapsed() < if why == Refusal::Failed { FAILED_FOR } else { REFUSED_FOR } => Some(why),
            Some(_) => {
                *failed = None;
                None
            }
            None => None,
        }
    }

    /// `bytes` more went through TURN for the screen: this UTC day's and this month's counts. Saved with the relay WebSocket's use.
    pub fn count_turn(&self, bytes: u64) {
        let (day, month) = turn_periods();
        let add = |slot: &mut Option<MonthUse>, period: String| {
            let used = slot.get_or_insert_with(MonthUse::default);
            if used.month != period {
                *used = MonthUse { month: period, bytes: 0 };
            }
            used.bytes = used.bytes.saturating_add(bytes);
        };
        {
            let mut c = self.config.lock();
            add(&mut c.screen_turn_day, day);
            add(&mut c.screen_turn_month, month);
        }
        self.save_screen_use_soon();
    }

    /// What went through TURN this UTC day and this month (bytes).
    pub fn turn_used(&self) -> (u64, u64) {
        let (day, month) = turn_periods();
        let c = self.config.lock();
        let of = |u: &Option<MonthUse>, p: &str| u.as_ref().filter(|u| u.month == p).map_or(0, |u| u.bytes);
        (of(&c.screen_turn_day, &day), of(&c.screen_turn_month, &month))
    }

    /// Whether the day's or the month's TURN allowance is spent.
    pub fn turn_spent(&self) -> bool {
        let o = self.screen_options();
        let (day, month) = self.turn_used();
        day >= u64::from(o.turn_day_mb) * 1024 * 1024 || month >= u64::from(o.turn_month_mb) * 1024 * 1024
    }
}

/// The UTC day (`YYYY-MM-DD`) and month (`YYYY-MM`) TURN's use is counted in.
fn turn_periods() -> (String, String) {
    let now = chrono::Utc::now();
    (now.format("%Y-%m-%d").to_string(), now.format("%Y-%m").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../../relay/test/vectors.json")).unwrap()
    }

    #[test]
    fn the_statements_are_the_relays() {
        let v = vectors();
        let (room, ts) = (v["room"].as_str().unwrap(), v["ts"].as_u64().unwrap());
        assert_eq!(turn_statement(room, ts), v["turn_statement"]);
        assert_eq!(revoke_statement(room, ts, v["revoke_username"].as_str().unwrap()), v["revoke_statement"]);
    }

    #[test]
    fn the_relays_answer_is_read_as_far_as_a_phone_needs_it() {
        let later = now_millis() + 1_200_000;
        let body = json!({"ice_servers": [{"urls": ["stun:s:3478"]}, {"urls": ["turn:t:3478?transport=udp", "javascript:alert(1)", "turns:t:443?transport=tcp"], "username": "u", "credential": "c"}, {"urls": []}], "username": "u", "expires": later}).to_string();
        let m = read_minted(200, None, &body).unwrap();
        assert_eq!(m.ice_servers, json!([{"urls": ["stun:s:3478"]}, {"urls": ["turn:t:3478?transport=udp", "turns:t:443?transport=tcp"], "username": "u", "credential": "c"}]));
        assert_eq!((m.username.as_str(), m.expires), ("u", later));
        assert_eq!(read_minted(404, Some("turn_not_configured"), ""), Err(Refusal::NotConfigured));
        assert_eq!(read_minted(404, None, ""), Err(Refusal::NotConfigured), "a relay from before TURN");
        assert_eq!(read_minted(503, Some("turn_full"), ""), Err(Refusal::Full));
        assert_eq!(read_minted(429, Some("turn_room_limit"), ""), Err(Refusal::Full));
        assert_eq!(read_minted(429, Some("turn_network"), ""), Err(Refusal::Full));
        assert_eq!(read_minted(403, None, ""), Err(Refusal::Failed));
        assert_eq!(read_minted(200, None, &json!({"ice_servers": [{"urls": ["stun:s"]}], "username": "u", "expires": later}).to_string()), Err(Refusal::Failed), "no TURN server in it");
        assert_eq!(read_minted(200, None, &json!({"ice_servers": [{"urls": ["turn:t"]}], "username": "u", "expires": 1}).to_string()), Err(Refusal::Failed), "already over");
    }
}
