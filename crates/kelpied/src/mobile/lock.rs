//! The app lock: a phone that paired on a secure origin reaches nothing of this computer until a passkey's signature passes here (`webauthn`), and a copied
//! Noise key alone gets it no further than a lock screen. The page shows the lock; this file is what enforces it.
//!
//! - **Which connections it covers** (`LockPath`). A device that paired on a secure origin, or has a passkey, needs the passkey over every transport: the
//!   shape of a connection is the caller's to choose, so it decides nothing here. A device that never saw a secure origin can have no passkey: over the network it may
//!   read at most, while `plain_read` allows that. A plain connection from this machine has no network to be stolen on, and a program that can make it is out of
//!   scope anyway (`auth.rs`): it is as it would be with no lock.
//! - **The window** is in memory, per device, and gone at a restart: a Wi-Fi to 4G flap inside it asks nothing, and every expiry is checked against the clock at each message, so
//!   a sweep that runs late lets nothing through. Nothing runs for a device that is not unlocked.
//! - **A stolen key cannot enrol its own passkey** over one that exists: that takes the desktop (`reset_lock`, the verified app and the person, 5 minutes). A device with no passkey
//!   yet can enrol, which is the first open after pairing. That leaves a key stolen between pairing and the first open, stated in `docs/architecture.md`.
//! - **What cannot be read never unlocks.** A passkey that does not verify, or a key that cannot be read, leaves the phone locked, and settings that cannot be believed are tighter than anything the person could have set.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::store::{self, Device, LockCredential, LockOptions};
use super::{refuse, webauthn, DeviceEvent, Mobile, Peer, Refusal};
use crate::session::now_millis;

/// How long a challenge may wait for the person.
const CHALLENGE_TTL: Duration = Duration::from_secs(60);
/// How long "Reset this phone's lock" lets the phone set up a new passkey.
const RESET_WINDOW: Duration = Duration::from_secs(300);
/// Ceremonies a device may fail in a minute. A signature cannot be guessed: this keeps a stolen key from having the daemon check them all day, and from filling the audit log.
const FAILURES_PER_MINUTE: usize = 10;
/// How far ahead a dangerous approval's assertion may hold.
const RECHECK_MAX_MS: u64 = 120_000;

/// What a connection's lock goes by, from who it is and how it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockPath {
    /// It paired on a secure origin: locked until its passkey passes, whatever carries it.
    Passkey,
    /// It never had a secure origin, and comes over the network: reading at most.
    Plain,
    /// It never had a secure origin, and comes from this machine over plain HTTP.
    Local,
}

/// What may pass right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Open,
    ReadOnly,
    Locked,
}

/// The app lock's memory, none of which outlives this daemon.
#[derive(Default)]
pub struct LockMemory {
    /// How the lock is set, as `check_lock_options` last found it.
    options: Mutex<LockOptions>,
    /// Until when each unlocked device stays so. Absent: locked.
    unlocked: Mutex<HashMap<String, Instant>>,
    /// The one challenge each device may answer, and when it was given.
    challenges: Mutex<HashMap<String, ([u8; 32], Instant)>>,
    /// Until when a device may replace its passkey.
    resets: Mutex<HashMap<String, Instant>>,
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LockMemory {
    fn forget(&self, device: &str) {
        self.unlocked.lock().remove(device);
        self.challenges.lock().remove(device);
        self.resets.lock().remove(device);
        self.failures.lock().remove(device);
    }

    fn forget_all(&self) {
        self.unlocked.lock().clear();
        self.challenges.lock().clear();
        self.resets.lock().clear();
        self.failures.lock().clear();
    }
}

/// A field of a request as bytes: base64url, and no longer than the limit. Nothing of what was sent is said back.
fn field(req: &Value, name: &str, limit: usize) -> Result<Vec<u8>, Refusal> {
    let bad = || refuse("bad_request", "a passkey message is missing a part");
    let text = req[name].as_str().filter(|t| t.len() <= limit.div_ceil(3) * 4).ok_or_else(bad)?;
    B64.decode(text).map_err(|_| bad())
}

impl Mobile {
    /// Reads how the lock is set from the store, if the tag says it is as it was saved. A store from before the lock has the defaults. Anything else is tighter than whatever the person chose:
    /// settings that do not verify, a key that cannot be read, or settings gone while a passkey is on record, since they are written with the first one.
    pub(super) fn check_lock_options(&self) {
        let (saved, passkey) = {
            let c = self.config.lock();
            (c.lock.clone(), c.devices.iter().any(|d| d.lock.is_some()))
        };
        let believed = match saved {
            None if !passkey => Some(LockOptions::default()),
            None => None,
            Some(o) => match self.mac_key(false) {
                Ok(Some(key)) if store::options_hold(&o, &key) => Some(o),
                _ => None,
            },
        };
        let checked = believed.unwrap_or_else(|| {
            tracing::warn!("phone access: the app lock's settings cannot be believed now: the tightest apply");
            LockOptions::tightest()
        });
        *self.lock_memory.options.lock() = checked;
    }

    pub fn lock_options(&self) -> LockOptions {
        self.lock_memory.options.lock().clone()
    }

    /// Which lock a connection of `device` goes by.
    pub(super) fn lock_path(&self, device: &Device, peer: &Peer) -> LockPath {
        if device.secure_origin || device.lock.is_some() {
            LockPath::Passkey
        } else if !peer.through_serve && !peer.tls && self.this_machine(peer.ip()) {
            LockPath::Local
        } else {
            LockPath::Plain
        }
    }

    /// The check on every message: what `device` may do on a connection of this path, as things are at this moment.
    pub(super) fn lock_gate(&self, device: &str, path: LockPath) -> Gate {
        let (on, plain_read) = {
            let o = self.lock_memory.options.lock();
            (o.on, o.plain_read)
        };
        if !on {
            return Gate::Open;
        }
        match path {
            LockPath::Local => Gate::Open,
            LockPath::Plain if plain_read => Gate::ReadOnly,
            LockPath::Plain => Gate::Locked,
            LockPath::Passkey => {
                if self.lock_memory.unlocked.lock().get(device).is_some_and(|until| *until > Instant::now()) {
                    Gate::Open
                } else {
                    Gate::Locked
                }
            }
        }
    }

    /// How the phone is told its lock stands: `none` no lock applies to it, `open` unlocked, `read_only` it may read and no more, `closed` reading over plain HTTP is off,
    /// `unlock` its passkey is asked for, `enrol` it may set one up, `no_name` this address can have no passkey, `broken` its passkey does not verify (the desktop can reset it).
    pub(super) fn lock_state(&self, device: &str, path: LockPath, gate: Gate, page: Option<&str>) -> &'static str {
        match gate {
            Gate::ReadOnly => "read_only",
            Gate::Open if path == LockPath::Passkey && self.lock_memory.options.lock().on => "open",
            Gate::Open => "none",
            Gate::Locked if path != LockPath::Passkey => "closed",
            Gate::Locked => {
                // The key as it was read when the record was checked: this runs on the connection's task, which does not wait on a credential store. A verified record means it was read.
                let key = *self.mac_key.lock();
                let c = self.config.lock();
                let Some(d) = c.devices.iter().find(|d| d.id == device) else { return "closed" };
                let may_set_up = d.lock.is_none() || self.lock_memory.resets.lock().get(device).is_some_and(|until| *until > Instant::now());
                if may_set_up {
                    return if page.and_then(webauthn::rp_id).is_some() { "enrol" } else { "no_name" };
                }
                if key.is_some_and(|key| store::lock_holds(d, &key)) {
                    "unlock"
                } else {
                    "broken"
                }
            }
        }
    }

    /// What a page needs for the ceremony its state calls for, with a fresh challenge: the old one stops counting.
    pub(super) fn lock_info(&self, device: &str, state: &str, page: Option<&str>) -> Value {
        let o = self.lock_options();
        let mut info = json!({"state": state, "idle_s": o.idle_s, "recheck": o.recheck});
        if !matches!(state, "enrol" | "unlock" | "open") {
            return info;
        }
        let c = self.config.lock();
        let Some(d) = c.devices.iter().find(|d| d.id == device) else { return info };
        if let Some(l) = &d.lock {
            info["cred_id"] = json!(l.cred_id);
            info["rp_id"] = json!(webauthn::rp_id(&l.origin));
        }
        if state == "enrol" {
            info["rp_id"] = json!(page.and_then(webauthn::rp_id));
            info["user"] = json!(B64.encode(d.id.as_bytes()));
            info["name"] = json!(d.name);
        }
        if state != "open" {
            info["challenge"] = json!(B64.encode(self.new_challenge(device)));
        }
        info
    }

    fn new_challenge(&self, device: &str) -> [u8; 32] {
        let mut challenge = [0u8; 32];
        challenge[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        challenge[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        self.lock_memory.challenges.lock().insert(device.to_string(), (challenge, Instant::now()));
        challenge
    }

    /// The device's challenge, used up by asking: a ceremony gets one go at it.
    fn take_challenge(&self, device: &str) -> Result<[u8; 32], Refusal> {
        match self.lock_memory.challenges.lock().remove(device) {
            Some((challenge, at)) if at.elapsed() < CHALLENGE_TTL => Ok(challenge),
            _ => Err(refuse("challenge", "That took too long, or was already used: try again.")),
        }
    }

    fn lock_attempt_allowed(&self, device: &str) -> bool {
        let mut all = self.lock_memory.failures.lock();
        let Some(failures) = all.get_mut(device) else { return true };
        while failures.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            failures.pop_front();
        }
        failures.len() < FAILURES_PER_MINUTE
    }

    fn lock_failed(&self, device: &str, what: &str, why: webauthn::Refused) -> Refusal {
        self.lock_memory.failures.lock().entry(device.to_string()).or_default().push_back(Instant::now());
        self.audit.add(device, "lock_refused", &format!("{what}: {}", why.word()));
        refuse("lock_refused", "The computer did not accept that. Try again.")
    }

    fn open_window(&self, device: &str) {
        let idle = Duration::from_secs(u64::from(self.lock_memory.options.lock().idle_s));
        self.lock_memory.unlocked.lock().insert(device.to_string(), Instant::now() + idle);
    }

    /// Takes a new passkey for `device`, whose page is at `page`. Blocking: it saves the store, and what cannot be saved is not taken: a restart would find the device with no passkey, free to enrol again.
    pub(super) fn lock_enrol(&self, device: &str, req: &Value, page: Option<&str>) -> Result<(), Refusal> {
        if !self.lock_memory.options.lock().on {
            return Err(refuse("lock_off", "The app lock is off on this computer."));
        }
        let origin = page.filter(|p| webauthn::rp_id(p).is_some()).ok_or_else(|| refuse("insecure", "A passkey needs an HTTPS address with a name: open Kelpie through Tailscale."))?;
        let allowed = {
            let c = self.config.lock();
            let d = c.devices.iter().find(|d| d.id == device).ok_or_else(|| refuse("not_paired", "This device is no longer paired."))?;
            if !d.secure_origin {
                return Err(refuse("insecure", "This device was paired over plain HTTP: pair it again over HTTPS to use a passkey."));
            }
            d.lock.is_none() || self.lock_memory.resets.lock().get(device).is_some_and(|until| *until > Instant::now())
        };
        if !allowed {
            return Err(refuse("needs_desktop", "This phone has a passkey already. To set up another, press \"Reset this phone's lock\" in Kelpie on the computer."));
        }
        if !self.lock_attempt_allowed(device) {
            return Err(refuse("too_many", "Too many tries: wait a minute."));
        }
        let challenge = self.take_challenge(device)?;
        let client_data = field(req, "client_data", webauthn::MAX_CLIENT_DATA)?;
        let auth_data = field(req, "auth_data", webauthn::MAX_AUTH_DATA)?;
        let cred_id = field(req, "cred_id", webauthn::MAX_CREDENTIAL_ID)?;
        let spki = field(req, "spki", 128)?;
        let sign_count = webauthn::check_enrol(&client_data, &auth_data, &cred_id, &spki, &webauthn::Expected { origin, challenge: &challenge }).map_err(|why| self.lock_failed(device, "enrol", why))?;
        let key = match self.mac_key(true) {
            Ok(Some(key)) => key,
            _ => return Err(refuse("not_kept", "This computer's credential store would not give the key that seals the passkey.")),
        };
        // The settings go in with the first passkey: from now on, none in the file is an edit.
        let mut settings = self.lock_options();
        store::seal_options(&mut settings, &key);
        let (name, previous, previous_settings) = {
            let mut c = self.config.lock();
            let d = c.devices.iter_mut().find(|d| d.id == device).ok_or_else(|| refuse("not_paired", "This device is no longer paired."))?;
            let previous = d.lock.replace(LockCredential { cred_id: B64.encode(&cred_id), spki: B64.encode(&spki), origin: origin.to_string(), sign_count, created_at: now_millis(), mac: String::new() });
            store::seal_lock(d, &key);
            let name = d.name.clone();
            let previous_settings = c.lock.clone();
            c.lock.get_or_insert(settings);
            (name, previous, previous_settings)
        };
        if let Err(e) = self.save() {
            tracing::warn!("phone access: {e:#}");
            let mut c = self.config.lock();
            c.lock = previous_settings;
            if let Some(d) = c.devices.iter_mut().find(|d| d.id == device) {
                d.lock = previous;
            }
            return Err(refuse("not_kept", "The passkey could not be saved on the computer: try again."));
        }
        self.lock_memory.resets.lock().remove(device);
        self.lock_memory.failures.lock().remove(device);
        self.open_window(device);
        self.audit.add(device, "lock_enrol", &name);
        Ok(())
    }

    /// Checks an assertion against the device's passkey, with its record and counter as they are; the new count, not yet kept.
    fn check_passkey(&self, device: &str, req: &Value, challenge: &[u8]) -> Result<u32, Refusal> {
        let client_data = field(req, "client_data", webauthn::MAX_CLIENT_DATA)?;
        let auth_data = field(req, "auth_data", webauthn::MAX_AUTH_DATA)?;
        let signature = field(req, "signature", webauthn::MAX_SIGNATURE)?;
        let key = self.mac_key(false);
        let credential = {
            let c = self.config.lock();
            let d = c.devices.iter().find(|d| d.id == device).ok_or_else(|| refuse("not_paired", "This device is no longer paired."))?;
            let holds = match key {
                Ok(Some(key)) => store::lock_holds(d, &key),
                _ => false,
            };
            if !holds {
                return Err(refuse("lock_broken", "This phone's passkey cannot be checked: press \"Reset this phone's lock\" in Kelpie on the computer."));
            }
            d.lock.clone().expect("it verified")
        };
        let spki = B64.decode(&credential.spki).map_err(|_| refuse("lock_broken", "This phone's passkey cannot be checked: press \"Reset this phone's lock\" in Kelpie on the computer."))?;
        webauthn::check_assertion(&client_data, &auth_data, &signature, &spki, credential.sign_count, &webauthn::Expected { origin: &credential.origin, challenge })
            .map_err(|why| self.lock_failed(device, "assertion", why))
    }

    /// Keeps a counter that moved. It is sealed with the record, so it costs a save, and a passkey that stays at 0 costs none.
    fn keep_count(&self, device: &str, count: u32) {
        let Ok(Some(key)) = self.mac_key(false) else { return };
        let changed = {
            let mut c = self.config.lock();
            let Some(d) = c.devices.iter_mut().find(|d| d.id == device) else { return };
            let Some(l) = d.lock.as_mut().filter(|l| l.sign_count != count) else { return };
            l.sign_count = count;
            store::seal_lock(d, &key);
            true
        };
        if changed {
            self.keep();
        }
    }

    /// An assertion that unlocks `device`. Blocking: a counter that moved is saved.
    pub(super) fn lock_unlock(&self, device: &str, req: &Value) -> Result<(), Refusal> {
        if !self.lock_attempt_allowed(device) {
            return Err(refuse("too_many", "Too many tries: wait a minute."));
        }
        let challenge = self.take_challenge(device)?;
        let count = self.check_passkey(device, req, &challenge)?;
        self.keep_count(device, count);
        self.lock_memory.failures.lock().remove(device);
        self.open_window(device);
        self.audit.add(device, "unlock", "");
        Ok(())
    }

    /// Whether dangerous approvals ask for the passkey again.
    pub(super) fn recheck_on(&self) -> bool {
        let o = self.lock_memory.options.lock();
        o.on && o.recheck
    }

    /// A dangerous approval's own assertion: over this request and this command, and fresh. Blocking, as `lock_unlock` is.
    pub(super) fn lock_recheck(&self, device: &str, proof: &Value, request: &str, command: &str) -> Result<(), Refusal> {
        let asked = || refuse("recheck", "This request is marked dangerous: confirm it with your passkey.");
        if !proof.is_object() {
            return Err(asked());
        }
        if !self.lock_attempt_allowed(device) {
            return Err(refuse("too_many", "Too many tries: wait a minute."));
        }
        let expiry = proof["expiry"].as_u64().ok_or_else(asked)?;
        let now = now_millis();
        if expiry <= now || expiry > now + RECHECK_MAX_MS {
            return Err(asked());
        }
        let challenge = webauthn::recheck_challenge(request, command, expiry);
        let count = self.check_passkey(device, proof, &challenge)?;
        self.keep_count(device, count);
        self.audit.add(device, "recheck", "");
        Ok(())
    }

    /// `device` is locked from now on: its control stops first, then its connection hears.
    pub(super) fn lock_now(&self, device: &str) {
        self.lock_memory.unlocked.lock().remove(device);
        self.stop_grants(device);
        self.tell_lock(device);
    }

    /// The person did something: the idle time starts over, if the phone is unlocked at all. A touch never unlocks.
    pub(super) fn lock_touch(&self, device: &str) {
        let idle = Duration::from_secs(u64::from(self.lock_memory.options.lock().idle_s));
        let now = Instant::now();
        if let Some(until) = self.lock_memory.unlocked.lock().get_mut(device).filter(|u| **u > now) {
            *until = now + idle;
        }
    }

    fn tell_lock(&self, device: &str) {
        let _ = self.revoked.send(DeviceEvent { id: device.to_string(), name: String::new(), change: "lock", permission: None });
    }

    /// The devices whose window has run out are locked: control stops whether or not a connection is there to notice. Called from the daemon's ticker; with none unlocked it costs one look.
    pub fn lock_sweep(&self) {
        let now = Instant::now();
        let expired: Vec<String> = {
            let mut unlocked = self.lock_memory.unlocked.lock();
            if unlocked.is_empty() {
                return;
            }
            let gone = unlocked.iter().filter(|(_, until)| **until <= now).map(|(id, _)| id.clone()).collect();
            unlocked.retain(|_, until| *until > now);
            gone
        };
        for device in expired {
            self.stop_grants(&device);
            self.tell_lock(&device);
        }
    }

    pub(super) fn forget_lock(&self, device: &str) {
        self.lock_memory.forget(device);
    }

    pub(super) fn forget_all_locks(&self) {
        self.lock_memory.forget_all();
    }

    /// One reading of `mobile.set_lock`'s parameters: what is not named stays, and what is named must be a value offered.
    pub fn requested_lock(&self, params: &Value) -> Result<LockOptions> {
        let mut next = self.lock_options();
        let flag = |name: &str| params.get(name).map(|v| v.as_bool().ok_or_else(|| anyhow!("{name} must be true or false"))).transpose();
        next.on = flag("on")?.unwrap_or(next.on);
        next.recheck = flag("recheck")?.unwrap_or(next.recheck);
        next.plain_read = flag("plain_read")?.unwrap_or(next.plain_read);
        if let Some(v) = params.get("idle_s") {
            let idle = v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| store::IDLE_CHOICES.contains(n)).ok_or_else(|| anyhow!("idle_s must be one of {:?}", store::IDLE_CHOICES))?;
            next.idle_s = idle;
        }
        Ok(next)
    }

    /// Applies `next` from the next message on. A shorter idle time holds for the windows open now as well, a lock turned off forgets them, and every connection looks again.
    pub fn set_lock(&self, mut next: LockOptions) -> Result<Value> {
        let key = self.mac_key(true)?.ok_or_else(|| anyhow!("the credential store did not keep the device key"))?;
        store::seal_options(&mut next, &key);
        let was = self.lock_options();
        self.config.lock().lock = Some(next.clone());
        *self.lock_memory.options.lock() = next.clone();
        {
            let now = Instant::now();
            let longest = now + Duration::from_secs(u64::from(next.idle_s));
            let mut unlocked = self.lock_memory.unlocked.lock();
            if !next.on {
                unlocked.clear();
            }
            for until in unlocked.values_mut() {
                *until = (*until).min(longest);
            }
        }
        self.keep();
        self.audit.add("-", "setting", &format!("app lock: {}, idle {} s, re-check {}, reading over plain HTTP {} (was {}, {} s, {}, {})", next.on, next.idle_s, next.recheck, next.plain_read, was.on, was.idle_s, was.recheck, was.plain_read));
        let devices: Vec<String> = self.config.lock().devices.iter().map(|d| d.id.clone()).collect();
        for device in devices {
            self.tell_lock(&device);
        }
        Ok(json!({"on": next.on, "idle_s": next.idle_s, "recheck": next.recheck, "plain_read": next.plain_read}))
    }

    /// What the prompt asks before a phone may replace its passkey. What cannot be done at all is refused here, with no prompt.
    pub fn reset_lock_prompt(&self, id: &str) -> Result<crate::user_presence::Prompt> {
        let c = self.config.lock();
        let d = c.devices.iter().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
        anyhow::ensure!(d.verified, "this device must be paired again");
        anyhow::ensure!(d.secure_origin, "this device was paired over plain HTTP, so it has no passkey to reset");
        Ok(crate::user_presence::Prompt::ResetLock { name: d.name.clone() })
    }

    /// Opens the window in which this phone may set up a new passkey. It stays locked meanwhile, and the old passkey counts until a new one replaces it.
    pub fn reset_lock(&self, id: &str) {
        self.lock_memory.resets.lock().insert(id.to_string(), Instant::now() + RESET_WINDOW);
        self.lock_memory.failures.lock().remove(id);
        self.lock_memory.unlocked.lock().remove(id);
        self.stop_grants(id);
        self.audit.add(id, "lock_reset", "the phone may set up a new passkey for 5 minutes");
        self.tell_lock(id);
    }

    /// For `mobile.status`: whether a device's window to set up a passkey is open, and whether it is unlocked now.
    pub(super) fn lock_facts(&self, id: &str) -> (bool, bool) {
        let now = Instant::now();
        (self.lock_memory.resets.lock().get(id).is_some_and(|u| *u > now), self.lock_memory.unlocked.lock().get(id).is_some_and(|u| *u > now))
    }
}

/// What a test winds back, and looks at.
#[cfg(test)]
impl Mobile {
    pub(super) fn expire_window(&self, device: &str) {
        if let Some(until) = self.lock_memory.unlocked.lock().get_mut(device) {
            *until = Instant::now() - Duration::from_millis(1);
        }
    }

    pub(super) fn age_challenge(&self, device: &str) {
        if let Some((_, at)) = self.lock_memory.challenges.lock().get_mut(device) {
            *at -= CHALLENGE_TTL;
        }
    }

    pub(super) fn is_unlocked(&self, device: &str) -> bool {
        self.lock_facts(device).1
    }

    /// Whether anything of a window is still held for the device, past its end or not.
    pub(super) fn lock_memory_holds(&self, device: &str) -> bool {
        self.lock_memory.unlocked.lock().contains_key(device)
    }

    pub(super) fn open_window_for_test(&self, device: &str) {
        self.open_window(device);
    }
}
