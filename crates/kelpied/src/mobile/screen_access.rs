//! The screen's ways as the person set them (`store::ScreenOptions`, sealed), what it sent through the relay this month, and the policy's facts for a
//! device on a connection (`screen_policy`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::screen_policy::{self, Facts, Reason, Uv, Via};
use super::store::{self, Kind, MonthUse, PageKind, ScreenOptions};
use super::verbs::Permission;
use super::{DeviceEvent, Mobile};

/// What the relay WebSocket's screen use is saved at most this often while it goes (and when a screen ends).
const SAVE_USE_EVERY: Duration = Duration::from_secs(60);
/// What a screen may be held to through the relay WebSocket, whatever the options say (kbps), and its monthly use (MB): the room's monthly
/// allowance is 2 GB, and the link itself needs the rest.
const MAX_RELAY_WS_KBPS: u32 = 500;
const MAX_RELAY_WS_MONTH_MB: u32 = 1024;
/// TURN's caps, each at most this (MB): 100 GB, a tenth of what Cloudflare carries free in a month.
const MAX_TURN_MB: u32 = 100 * 1024;

#[derive(Default)]
pub struct ScreenMemory {
    /// The options as last checked.
    options: Mutex<ScreenOptions>,
    /// The options on file could not be believed (a tag that fails, or a key that cannot be read): the screen is off through the relay until
    /// the person sets it again, and Settings says why.
    doubted: AtomicBool,
    /// When the month's use was last saved.
    saved_at: Mutex<Option<Instant>>,
    /// The last mint of TURN credentials that failed, and why (`turn::Mobile::turn_refused`).
    pub(super) turn_failed: Mutex<Option<(Instant, super::turn::Refusal)>>,
    /// Held across each TURN request to the relay: its `ts` is taken under it, so requests reach the relay in the order of their `ts` (the
    /// room takes only a later one).
    pub(super) turn_requests: Mutex<()>,
}

/// This computer's month, as the use is counted (local time).
fn this_month() -> String {
    chrono::Local::now().format("%Y-%m").to_string()
}

impl Mobile {
    /// Reads the screen's options from the store, if the tag says they are as they were saved; never set, the defaults (off through the relay).
    /// Options that cannot be believed are off through the relay and are not written over: Settings says so.
    pub(super) fn check_screen_options(&self) {
        let saved = self.config.lock().screen.clone();
        let (options, doubted) = match saved {
            None => (ScreenOptions::default(), false),
            Some(o) => match self.mac_key(false) {
                Ok(Some(key)) if store::screen_holds(&o, &key) => (o.settled(), false),
                _ => {
                    tracing::warn!("phone access: the screen's settings cannot be believed now: the defaults apply (off through the relay)");
                    (ScreenOptions::default(), true)
                }
            },
        };
        *self.screen_memory.options.lock() = options;
        self.screen_memory.doubted.store(doubted, Ordering::SeqCst);
    }

    pub fn screen_options(&self) -> ScreenOptions {
        self.screen_memory.options.lock().clone()
    }

    /// One reading of `mobile.set_screen`'s parameters: what is not named stays.
    pub fn requested_screen(&self, params: &Value) -> Result<ScreenOptions> {
        let mut next = self.screen_options();
        let flag = |v: &Value, name: &str| v.get(name).map(|v| v.as_bool().ok_or_else(|| anyhow!("{name} must be true or false"))).transpose();
        next.remote = flag(params, "remote")?.unwrap_or(next.remote);
        if let Some(m) = params.get("methods") {
            if !m.is_object() {
                return Err(anyhow!("methods must be an object"));
            }
            next.methods.tailscale = flag(m, "tailscale")?.unwrap_or(next.methods.tailscale);
            next.methods.p2p = flag(m, "p2p")?.unwrap_or(next.methods.p2p);
            next.methods.relay_turn = flag(m, "relay_turn")?.unwrap_or(next.methods.relay_turn);
            next.methods.relay_ws = flag(m, "relay_ws")?.unwrap_or(next.methods.relay_ws);
        }
        let number = |name: &str, low: u32, high: u32| -> Result<Option<u32>> {
            params.get(name).map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| (low..=high).contains(n)).ok_or_else(|| anyhow!("{name} must be {low} to {high}"))).transpose()
        };
        next.relay_ws_kbps = number("relay_ws_kbps", 150, MAX_RELAY_WS_KBPS)?.unwrap_or(next.relay_ws_kbps);
        next.relay_ws_month_mb = number("relay_ws_month_mb", 64, MAX_RELAY_WS_MONTH_MB)?.unwrap_or(next.relay_ws_month_mb);
        next.turn_session_mb = number("turn_session_mb", 16, MAX_TURN_MB)?.unwrap_or(next.turn_session_mb);
        next.turn_day_mb = number("turn_day_mb", 16, MAX_TURN_MB)?.unwrap_or(next.turn_day_mb);
        next.turn_month_mb = number("turn_month_mb", 16, MAX_TURN_MB)?.unwrap_or(next.turn_month_mb);
        Ok(next)
    }

    /// Applies `next` from the next message on. Turned off through the relay, the relay's screens end within a look of their task
    /// (`screen_held`), and a phone of the hosted app goes back to typing: only a new grant, verified on the desktop, gives it control again.
    pub fn set_screen(&self, mut next: ScreenOptions) -> Result<Value> {
        let key = self.mac_key(true)?.ok_or_else(|| anyhow!("the credential store did not keep the device key"))?;
        // Set by this build, which offers peer to peer and TURN: those ways and TURN's caps are the person's choice from now on.
        next.p2p_chosen = true;
        next.turn_chosen = true;
        store::seal_screen(&mut next, &key);
        let was = self.screen_options();
        let lock_on = self.lock_options().on;
        let lowered: Vec<DeviceEvent> = {
            let hosted: Vec<String> = self
                .config
                .lock()
                .devices
                .iter()
                .filter(|d| d.page_kind == Some(PageKind::Hosted) && d.permission == Permission::Control && screen_policy::hosted_ceiling(&next, lock_on, d.lock.is_some()) < Permission::Control)
                .map(|d| d.id.clone())
                .collect();
            for id in &hosted {
                self.stop_grants(id);
            }
            let mut c = self.config.lock();
            c.devices
                .iter_mut()
                .filter(|d| hosted.contains(&d.id) && d.permission == Permission::Control)
                .map(|d| {
                    d.permission = Permission::Write;
                    store::seal(d, &key);
                    DeviceEvent { id: d.id.clone(), name: d.name.clone(), change: "changed", permission: Some(Permission::Write) }
                })
                .collect()
        };
        self.config.lock().screen = Some(next.clone());
        *self.screen_memory.options.lock() = next.clone();
        self.screen_memory.doubted.store(false, Ordering::SeqCst);
        self.keep();
        let m = next.methods;
        self.audit.add(
            "-",
            "setting",
            &format!("screen without Tailscale: {} (was {}); ways: tailscale {}, p2p {}, relay turn {}, relay websocket {}; relay websocket {} kbps, {} MB a month; TURN {} MB a screen, {} MB a day, {} MB a month", next.remote, was.remote, m.tailscale, m.p2p, m.relay_turn, m.relay_ws, next.relay_ws_kbps, next.relay_ws_month_mb, next.turn_session_mb, next.turn_day_mb, next.turn_month_mb),
        );
        for event in lowered {
            self.audit.add(&event.id, "permission", &format!("{} now write: the screen without Tailscale was turned off", event.name));
            let _ = self.revoked.send(event);
        }
        Ok(self.screen_status())
    }

    /// The screen's part of `mobile.status`.
    pub fn screen_status(&self) -> Value {
        let o = self.screen_options();
        let m = o.methods;
        json!({
            "remote": o.remote,
            "methods": {"tailscale": m.tailscale, "p2p": m.p2p, "relay_turn": m.relay_turn, "relay_ws": m.relay_ws},
            "relay_ws_kbps": o.relay_ws_kbps,
            "relay_ws_month_mb": o.relay_ws_month_mb,
            "relay_ws_used_mb": self.relay_screen_used() / (1024 * 1024),
            "turn_session_mb": o.turn_session_mb,
            "turn_day_mb": o.turn_day_mb,
            "turn_month_mb": o.turn_month_mb,
            "turn_used_day_mb": self.turn_used().0 / (1024 * 1024),
            "turn_used_month_mb": self.turn_used().1 / (1024 * 1024),
            "turn": self.turn_refused().map(|r| r.code()),
            "doubted": self.screen_memory.doubted.load(Ordering::SeqCst),
        })
    }

    /// What the screen sent through the relay WebSocket this month (bytes).
    fn relay_screen_used(&self) -> u64 {
        let month = this_month();
        self.config.lock().screen_relay_use.as_ref().filter(|u| u.month == month).map_or(0, |u| u.bytes)
    }

    /// Whether this month's allowance for the screen through the relay WebSocket is spent.
    pub fn relay_screen_spent(&self) -> bool {
        self.relay_screen_used() >= u64::from(self.screen_options().relay_ws_month_mb) * 1024 * 1024
    }

    /// `bytes` more went through the relay for the screen. Saved at most once a minute while it goes, and at the end (`save_relay_screen_use`).
    pub fn count_relay_screen(&self, bytes: u64) {
        let month = this_month();
        {
            let mut c = self.config.lock();
            let used = c.screen_relay_use.get_or_insert_with(MonthUse::default);
            if used.month != month {
                *used = MonthUse { month, bytes: 0 };
            }
            used.bytes = used.bytes.saturating_add(bytes);
        }
        self.save_screen_use_soon();
    }

    /// Saves the screen's use (the relay WebSocket's and TURN's) if it was not saved in the last minute.
    pub(super) fn save_screen_use_soon(&self) {
        let due = self.screen_memory.saved_at.lock().is_none_or(|at| at.elapsed() >= SAVE_USE_EVERY);
        if due {
            self.save_relay_screen_use();
        }
    }

    /// Saves the month's use now.
    pub fn save_relay_screen_use(&self) {
        *self.screen_memory.saved_at.lock() = Some(Instant::now());
        self.keep();
    }

    /// The policy's facts for `device` on a connection that is a page's (`browser`) and comes through the relay (`relay`), as things are now.
    /// None: no such verified device.
    pub(crate) fn screen_facts(&self, device: &str, browser: bool, relay: bool) -> Option<Facts> {
        let lock_on = self.lock_options().on;
        let window = self.window_open(device);
        if self.broken.lock().is_some() {
            return None;
        }
        let c = self.config.lock();
        if !c.enabled {
            return None;
        }
        let d = c.devices.iter().find(|d| d.id == device && d.verified)?;
        Some(Facts {
            record: d.permission,
            native: d.kind == Kind::App && !browser,
            secure_origin: d.secure_origin,
            hosted: d.page_kind == Some(PageKind::Hosted),
            lock_on,
            passkey: d.lock.is_some(),
            window,
            relay,
        })
    }

    /// Whether `device` may use the screen on such a connection now (`screen_policy::allowed`).
    pub(crate) fn screen_allowed(&self, device: &str, browser: bool, relay: bool) -> Result<(), Reason> {
        let facts = self.screen_facts(device, browser, relay).ok_or(Reason::NotGranted)?;
        screen_policy::allowed(&facts, &self.screen_options())
    }

    /// What a hold going `way` rests on as it goes, read again by its task (`screen::watch`).
    pub(crate) fn screen_held(&self, device: &str, browser: bool, relay: bool, way: Via) -> bool {
        self.screen_facts(device, browser, relay).is_some_and(|f| screen_policy::held(&f, &self.screen_options(), way).is_ok())
    }

    /// Whether a screen of this connection may go `way` now (a switch mid-session, or the link to fall back on): its rule, that way turned on,
    /// and for the relay's WebSocket its month not spent. No Face ID: the screen already had one.
    pub(crate) fn screen_way(&self, device: &str, browser: bool, relay: bool, way: Via) -> Result<(), (&'static str, String)> {
        let refusal = |r: Reason| (r.code(), r.message().to_string());
        let facts = self.screen_facts(device, browser, relay).ok_or(refusal(Reason::NotGranted))?;
        screen_policy::held(&facts, &self.screen_options(), way).map_err(refusal)?;
        if way == Via::Relay && !facts.native && self.relay_screen_spent() {
            return Err(("relay_budget_spent", "This month's screen allowance through the relay is used up. It starts again next month, or use Tailscale.".to_string()));
        }
        Ok(())
    }

    /// The most a screen through the relay's WebSocket may send (kbps).
    pub(crate) fn relay_ws_kbps(&self) -> u32 {
        self.screen_options().relay_ws_kbps.min(MAX_RELAY_WS_KBPS)
    }

    /// Whether a new screen may start, over `via`, with the passkey check `proof` for `target`: the way it goes, and the most it may send (kbps)
    /// while on the relay WebSocket (a page through the relay). `can_p2p`: the phone said it can go peer to peer (Automatic goes that way only then). The passkey check is
    /// blocking: called off the async threads.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn screen_may_start(&self, device: &str, browser: bool, relay: bool, via: Via, can_p2p: bool, target: &str, proof: &Value) -> Result<(Via, Option<u32>), (&'static str, String)> {
        let refusal = |r: Reason| (r.code(), r.message().to_string());
        let facts = self.screen_facts(device, browser, relay).ok_or(refusal(Reason::NotGranted))?;
        let opts = self.screen_options();
        // Everything but the passkey first: no Face ID is asked for what would be refused anyway, nor once the month's allowance is spent.
        let way = screen_policy::may_start(&facts, &opts, via, Uv::Fresh).map_err(refusal)?;
        // Automatic through the relay with its WebSocket off goes peer to peer (or TURN) only: a phone that cannot (an older page, a browser without
        // WebRTC) would wait for a picture that never comes.
        if via == Via::Auto && matches!(way, Via::P2p | Via::Turn) && !can_p2p {
            return Err(("method_off", "Through the relay this computer sends the screen only peer to peer, which this phone cannot do. Turn on the relay WebSocket in Kelpie → Settings → Mobile → Advanced.".to_string()));
        }
        if facts.relay && !facts.native {
            if way == Via::Relay && self.relay_screen_spent() {
                return Err(("relay_budget_spent", "This month's screen allowance through the relay is used up. It starts again next month, or use Tailscale.".to_string()));
            }
            // The phone signs this screen's challenge over the target and the word "relay": a screen through the relay's connection, whichever way
            // its picture then takes.
            match self.screen_recheck(device, proof, target, Via::Relay.word()) {
                Uv::Fresh => {}
                Uv::Replayed => return Err(("needs_face_id", "That Face ID check was used already: confirm again.".to_string())),
                _ => return Err(refusal(Reason::NeedsFaceId)),
            }
        }
        // The relay WebSocket's rate, for any screen of a page through the relay: it applies whenever its picture is on that way.
        Ok((way, (facts.relay && !facts.native).then(|| opts.relay_ws_kbps.min(MAX_RELAY_WS_KBPS))))
    }

    /// The ways a phone on this connection may choose, each with whether it is available now and why not (`screens`' `paths`).
    pub(crate) fn screen_paths(&self, device: &str, browser: bool, relay: bool) -> Value {
        let opts = self.screen_options();
        let facts = self.screen_facts(device, browser, relay);
        let row = |id: &str, why: Option<&str>| match why {
            None => json!({"id": id, "ok": true}),
            Some(why) => json!({"id": id, "ok": false, "why": why}),
        };
        let check = |via: Via| -> Option<&'static str> {
            let f = facts.as_ref()?;
            match screen_policy::may_start(f, &opts, via, Uv::Fresh) {
                Ok(Via::Relay) if !f.native && self.relay_screen_spent() => Some("relay_budget_spent"),
                Ok(_) => None,
                Err(r) => Some(r.code()),
            }
        };
        let none = facts.is_none().then_some("not_granted");
        let relay_why = if self.relay_url().is_none() && !relay { Some("relay_off") } else { none.or_else(|| check(Via::Relay)) };
        // Peer to peer: only where this computer has it (the phone adds its own reason when its browser has no WebRTC).
        let p2p_why = if cfg!(windows) { none.or_else(|| check(Via::P2p)) } else { Some("no_p2p_here") };
        // TURN through the relay: as peer to peer, with the relay on, its last mint not refused, and its allowances left.
        let turn_why = if !cfg!(windows) {
            Some("no_p2p_here")
        } else if self.relay_url().is_none() && !relay {
            Some("relay_off")
        } else {
            none.or_else(|| check(Via::Turn)).or_else(|| self.turn_refused().map(|r| r.code())).or_else(|| self.turn_spent().then_some("turn_budget_spent"))
        };
        json!([row("auto", none.or_else(|| check(Via::Auto))), row("tailscale", none.or_else(|| check(Via::Tailscale))), row("p2p", p2p_why), row("relay_turn", turn_why), row("relay", relay_why)])
    }

    /// The relay's address, when it is on (`wss://host[:port]`).
    fn relay_url(&self) -> Option<String> {
        self.config.lock().relay.as_ref().filter(|r| r.on && !r.url.is_empty()).map(|r| r.url.clone())
    }

    /// How the relay row is named on the phone: Kelpie's own relay, or the person's (by its host).
    pub(crate) fn relay_label(&self) -> Option<Value> {
        let url = self.relay_url()?;
        let host = url.trim_start_matches("wss://").trim_start_matches("ws://").split(['/', ':']).next().unwrap_or_default().to_string();
        let ours = url.trim_end_matches('/') == super::relay::DEFAULT_URL;
        Some(json!({"ours": ours, "host": host}))
    }
}
