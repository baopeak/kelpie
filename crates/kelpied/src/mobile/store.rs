//! Phone access settings and paired devices: `mobile.v2.json`.
//!
//! - A device is a Noise static public key with a name, a permission and a kind (the native app or a browser page). Each
//!   record carries an HMAC-SHA256 over what it grants, keyed by a secret in the OS credential store: a record edited in the
//!   file (a raised permission, a swapped key) no longer verifies and is refused until it is paired again. `last_seen` is
//!   not covered (it changes on every connection and grants nothing).
//! - The file of the token-based phone access (`mobile.json`, API ≤ 21) is read once, for its settings and the names of its
//!   devices, which are listed as "pair again" (their tokens mean nothing to kelpie-link). It is never written: an older
//!   Kelpie started after this one finds it as it left it.
//! - A file that cannot be read, or one from a newer Kelpie, is copied aside and left alone: phone access stays off and
//!   nothing is saved over it until the person resets phone access.

use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::verbs::Permission;

pub const FILE: &str = "mobile.v2.json";
pub const LEGACY_FILE: &str = "mobile.json";
pub const VERSION: u32 = 2;

/// What kind of phone client paired: the native app, or a page in a browser (whose code the daemon serves and which could
/// be swapped on a network that is not trusted, so it is held to less).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    App,
    Pwa,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::App => "app",
            Kind::Pwa => "pwa",
        }
    }

    /// The most a device of this kind may ever be granted (a browser page never gets control).
    pub fn ceiling(self) -> Permission {
        match self {
            Kind::App => Permission::Control,
            Kind::Pwa => Permission::Write,
        }
    }
}

/// Where this phone gets its alerts: what its browser gave when it subscribed. Its secret (`auth`) is in the credential store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PushSub {
    pub endpoint: String,
    pub p256dh: String,
    /// The language of its alerts.
    pub lang: String,
    #[serde(default)]
    pub created_at: u64,
    /// The origin of the page that subscribed, which is where its pairing is and so where its alerts must lead. None: from before this was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// HMAC-SHA256 (hex) over this record's `mac` and the fields above: an address edited in the file sends nothing.
    #[serde(default)]
    pub mac: String,
}

/// A phone's passkey as the daemon keeps it (`webauthn`): public, and only ever used to check a signature.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LockCredential {
    /// base64url, as the browser names it.
    pub cred_id: String,
    /// The public key as an SPKI, base64url.
    pub spki: String,
    /// The page's origin when it was enrolled: every assertion must come from there, and the RP ID is its name.
    pub origin: String,
    #[serde(default)]
    pub sign_count: u32,
    #[serde(default)]
    pub created_at: u64,
    /// HMAC-SHA256 (hex) over the phone's identity and the fields above: a key swapped in the file unlocks nothing, and no more does a counter wound back.
    #[serde(default)]
    pub mac: String,
}

/// How the app lock is set. A program of this user can edit the file, and must not weaken the lock by doing it: what does not verify, or has gone once a passkey is on record, is
/// `LockOptions::tightest`, tighter than anything the person could have set. Only a store from before the lock has `LockOptions::default`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LockOptions {
    pub on: bool,
    /// Seconds without the person doing anything before the phone locks again.
    pub idle_s: u32,
    /// A dangerous approval asks for the passkey again.
    pub recheck: bool,
    /// A page over plain HTTP, which can have no passkey, may read.
    pub plain_read: bool,
    #[serde(default)]
    pub mac: String,
}

pub const DEFAULT_IDLE_S: u32 = 300;
pub const IDLE_CHOICES: [u32; 5] = [60, 300, 900, 1800, 3600];

impl Default for LockOptions {
    fn default() -> Self {
        LockOptions { on: true, idle_s: DEFAULT_IDLE_S, recheck: false, plain_read: true, mac: String::new() }
    }
}

impl LockOptions {
    /// What applies when the settings cannot be believed: no choice of the person's is stricter, so going to this from anything weakens nothing.
    pub fn tightest() -> LockOptions {
        LockOptions { on: true, idle_s: IDLE_CHOICES[0], recheck: true, plain_read: false, mac: String::new() }
    }

    /// Whether going from `self` to `next` takes any protection away: that needs the person.
    pub fn weakened_by(&self, next: &LockOptions) -> bool {
        (self.on && !next.on) || next.idle_s > self.idle_s || (self.recheck && !next.recheck) || (!self.plain_read && next.plain_read)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// The phone's Noise static public key, base64url.
    pub pk: String,
    pub permission: Permission,
    pub kind: Kind,
    pub created_at: u64,
    #[serde(default)]
    pub last_seen: u64,
    /// HMAC-SHA256 (hex) of the fields above but `last_seen`.
    pub mac: String,
    /// It paired on an HTTPS origin, so a page swapped on the network cannot have done it. What later grants rest on (the relay, control). A program of this
    /// user can get it too, with the pairing secret and the person's approval: it says what the person approved, not that no such program exists.
    #[serde(default)]
    pub secure_origin: bool,
    /// HMAC-SHA256 (hex) saying so, for a record with `secure_origin` only. A second tag, not a field of the first: a record paired here
    /// still verifies for an older Kelpie, which ignores both, and setting the flag in the file does not verify anywhere.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secure_mac: String,
    /// Alerts on this phone, with its own tag: a record paired here still verifies for an older Kelpie, which ignores this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<PushSub>,
    /// The passkey that unlocks this phone, with its own tag: a record paired here still verifies for an older Kelpie, which ignores this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<LockCredential>,
    /// Set when the record was loaded or made here and its MACs checked out.
    #[serde(skip)]
    pub verified: bool,
}

/// A device paired with a token (before kelpie-link): only its name is kept, to say it must be paired again.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LegacyDevice {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Store {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    /// PEM certificate chain and key: serve HTTPS (secure context, installable app).
    #[serde(default)]
    pub cert: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub legacy: Vec<LegacyDevice>,
    /// A device unused for this many days must be paired again (`None`: `DEFAULT_DORMANT_DAYS`; 0: never).
    #[serde(default)]
    pub dormant_days: Option<u32>,
    /// Tailscale mode: this computer is published with `tailscale serve`.
    #[serde(default)]
    pub tailscale: bool,
    /// The loopback port our serve entry forwards to, which is how the entry is known for ours.
    #[serde(default)]
    pub tailscale_port: Option<u16>,
    /// Our serve entry is still to be removed: Tailscale would not do it when the mode ended.
    #[serde(default)]
    pub tailscale_remove: bool,
    /// How the app lock is set. `None`: a store from before there was a lock, as `LockOptions::default`. It is written when the first passkey is, so with a passkey on record `None` means it was removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<LockOptions>,
}

pub const DEFAULT_DORMANT_DAYS: u32 = 90;

/// The token-based file: only what carries over is read.
#[derive(Deserialize)]
struct LegacyStore {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    cert: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    devices: Vec<LegacyDevice>,
}

/// How the store came out of the disk.
pub struct Loaded {
    pub store: Store,
    /// Why the file could not be used (it was copied aside, and nothing may be saved over it).
    pub broken: Option<String>,
    /// The store is new (from the old file or from nothing) and should be written once.
    pub fresh: bool,
}

pub fn load(dir: &Path) -> Loaded {
    let path = dir.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Store>(&bytes) {
            Ok(store) if store.version == VERSION => Loaded { store, broken: None, fresh: false },
            Ok(store) => broken(&path, &bytes, format!("{FILE} has format {} (this Kelpie reads {VERSION}): it was written by another version", store.version)),
            Err(e) => broken(&path, &bytes, format!("{FILE} could not be read: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded { store: from_legacy(dir), broken: None, fresh: true },
        Err(e) => Loaded { store: Store { version: VERSION, ..Default::default() }, broken: Some(format!("{FILE} could not be read: {e}")), fresh: false },
    }
}

/// Copies the file it cannot use next to it and says why; phone access stays off.
fn broken(path: &Path, bytes: &[u8], why: String) -> Loaded {
    let backup = path.with_file_name(format!("mobile.v2.unreadable-{}.json", crate::session::now_millis()));
    match std::fs::write(&backup, bytes) {
        Ok(()) => tracing::warn!("{why}; a copy is at {}; phone access stays off until it is reset", backup.display()),
        Err(e) => tracing::warn!("{why}; copying it aside failed ({e}); phone access stays off until it is reset"),
    }
    // Nothing of it is used but this: a serve entry outlives the file, and a reset can only remove it knowing it for ours.
    let tailscale_port = serde_json::from_slice::<serde_json::Value>(bytes).ok().and_then(|v| v.get("tailscale_port")?.as_u64()).and_then(|p| u16::try_from(p).ok());
    Loaded { store: Store { version: VERSION, tailscale_port, ..Default::default() }, broken: Some(why), fresh: false }
}

/// The settings of the token-based file, and its devices as names to pair again. The file itself is left as it is.
fn from_legacy(dir: &Path) -> Store {
    let old = std::fs::read(dir.join(LEGACY_FILE)).ok().and_then(|b| serde_json::from_slice::<LegacyStore>(&b).ok());
    match old {
        Some(o) => Store { version: VERSION, enabled: o.enabled, ip: o.ip, port: o.port, cert: o.cert, key: o.key, legacy: o.devices, ..Default::default() },
        None => Store { version: VERSION, ..Default::default() },
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

// ---------- record MACs ----------

type HmacSha256 = Hmac<Sha256>;

fn mac_input(d: &Device) -> Vec<u8> {
    // Length-prefixed fields: no value can move bytes into another.
    let mut out = b"kelpie-device/1".to_vec();
    for field in [d.id.as_str(), d.name.as_str(), d.pk.as_str(), d.permission.name(), d.kind.name(), &d.created_at.to_string()] {
        out.extend_from_slice(&(field.len() as u32).to_le_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What `secure_mac` covers: this record and no other, and no other tag of it.
fn secure_input(d: &Device) -> Vec<u8> {
    let mut out = b"kelpie-device-secure/1".to_vec();
    out.extend_from_slice(d.mac.as_bytes());
    out
}

/// What a subscription's `mac` covers: this record and no other, and what it was told to do.
fn push_input(d: &Device, p: &PushSub) -> Vec<u8> {
    let mut out = b"kelpie-device-push/1".to_vec();
    // Only when there is one: a subscription from before the field still verifies, and adding one in the file does not.
    for field in [d.mac.as_str(), p.endpoint.as_str(), p.p256dh.as_str(), p.lang.as_str()].into_iter().chain(p.origin.as_deref()) {
        out.extend_from_slice(&(field.len() as u32).to_le_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    out
}

/// What a passkey's `mac` covers. The phone's identity, not its record's `mac`: a raised permission re-seals the record, and that must not bless a credential edited in the file.
fn lock_input(d: &Device, l: &LockCredential) -> Vec<u8> {
    let mut out = b"kelpie-device-lock/1".to_vec();
    for field in [d.id.as_str(), d.pk.as_str(), l.cred_id.as_str(), l.spki.as_str(), l.origin.as_str(), &l.sign_count.to_string()] {
        out.extend_from_slice(&(field.len() as u32).to_le_bytes());
        out.extend_from_slice(field.as_bytes());
    }
    out
}

/// Seals the record's passkey with its counter as it is now.
pub fn seal_lock(d: &mut Device, key: &[u8; 32]) {
    if let Some(mac) = d.lock.as_ref().map(|l| tag(key, &lock_input(d, l))) {
        d.lock.as_mut().expect("just read").mac = mac;
    }
}

/// Whether the record's passkey is as it was sealed with `key`.
pub fn lock_holds(d: &Device, key: &[u8; 32]) -> bool {
    d.lock.as_ref().is_some_and(|l| tag_holds(key, &lock_input(d, l), &l.mac))
}

fn options_input(o: &LockOptions) -> Vec<u8> {
    format!("kelpie-lock-options/1|{}|{}|{}|{}", o.on, o.idle_s, o.recheck, o.plain_read).into_bytes()
}

pub fn seal_options(o: &mut LockOptions, key: &[u8; 32]) {
    o.mac = tag(key, &options_input(o));
}

pub fn options_hold(o: &LockOptions, key: &[u8; 32]) -> bool {
    tag_holds(key, &options_input(o), &o.mac)
}

/// Whether the record's subscription is as it was sealed with `key`.
pub fn push_holds(d: &Device, key: &[u8; 32]) -> bool {
    d.push.as_ref().is_some_and(|p| tag_holds(key, &push_input(d, p), &p.mac))
}

fn tag(key: &[u8; 32], input: &[u8]) -> String {
    let mut m = HmacSha256::new_from_slice(key).expect("any key length");
    m.update(input);
    hex(&m.finalize().into_bytes())
}

fn tag_holds(key: &[u8; 32], input: &[u8], hex_tag: &str) -> bool {
    let Some(tag) = unhex(hex_tag) else { return false };
    let mut m = HmacSha256::new_from_slice(key).expect("any key length");
    m.update(input);
    m.verify_slice(&tag).is_ok()
}

pub fn seal(d: &mut Device, key: &[u8; 32]) {
    d.mac = tag(key, &mac_input(d));
    d.secure_mac = if d.secure_origin { tag(key, &secure_input(d)) } else { String::new() };
    if let Some(mac) = d.push.as_ref().map(|p| tag(key, &push_input(d, p))) {
        d.push.as_mut().expect("just read").mac = mac;
    }
    d.verified = true;
}

/// Whether the record is as it was sealed with `key` (constant time). A record that says it paired on a secure origin must prove that too.
pub fn verify(d: &Device, key: &[u8; 32]) -> bool {
    tag_holds(key, &mac_input(d), &d.mac) && (!d.secure_origin || tag_holds(key, &secure_input(d), &d.secure_mac))
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-store-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn device(key: &[u8; 32]) -> Device {
        let mut d = Device { id: "d1".into(), name: "Phone".into(), pk: "AAAA".into(), permission: Permission::Read, kind: Kind::Pwa, created_at: 7, last_seen: 0, mac: String::new(), secure_origin: false, secure_mac: String::new(), push: None, lock: None, verified: false };
        seal(&mut d, key);
        d
    }

    #[test]
    fn a_record_edited_in_the_file_no_longer_verifies() {
        let key = [3u8; 32];
        let d = device(&key);
        assert!(verify(&d, &key));
        assert!(!verify(&d, &[4u8; 32]), "another key");
        let mut raised = d.clone();
        raised.permission = Permission::Write;
        assert!(!verify(&raised, &key), "a raised permission");
        let mut swapped = d.clone();
        swapped.pk = "BBBB".into();
        assert!(!verify(&swapped, &key), "a swapped key");
        let mut kind = d.clone();
        kind.kind = Kind::App;
        assert!(!verify(&kind, &key), "a browser made into the app");
        let mut seen = d.clone();
        seen.last_seen = 99;
        assert!(verify(&seen, &key), "last seen grants nothing");
        let mut garbage = d.clone();
        garbage.mac = "zz".into();
        assert!(!verify(&garbage, &key));
        garbage.mac = String::new();
        assert!(!verify(&garbage, &key));
    }

    #[test]
    fn a_secure_origin_cannot_be_claimed_in_the_file_and_an_older_kelpie_still_reads_the_record() {
        let key = [3u8; 32];
        let plain = device(&key);
        assert!(plain.secure_mac.is_empty(), "as before");
        let json = serde_json::to_value(&plain).unwrap();
        assert!(json.get("secure_mac").is_none());

        let mut claimed = plain.clone();
        claimed.secure_origin = true;
        assert!(!verify(&claimed, &key), "the flag set in the file");
        claimed.secure_mac = plain.mac.clone();
        assert!(!verify(&claimed, &key), "the other tag copied over");

        let mut secure = plain.clone();
        secure.secure_origin = true;
        seal(&mut secure, &key);
        assert!(verify(&secure, &key));
        assert_eq!(secure.mac, plain.mac, "what an older Kelpie checks has not changed");
        let mut other = device(&key);
        other.id = "d2".into();
        seal(&mut other, &key);
        other.secure_origin = true;
        other.secure_mac = secure.secure_mac.clone();
        assert!(!verify(&other, &key), "another record's tag");

        // A record written before this field exists: no flag, and it verifies.
        let old: Device = serde_json::from_str(&format!(r#"{{"id":"d1","name":"Phone","pk":"AAAA","permission":"read","kind":"pwa","created_at":7,"last_seen":0,"mac":"{}"}}"#, plain.mac)).unwrap();
        assert!(!old.secure_origin);
        assert!(verify(&old, &key));
    }

    #[test]
    fn a_subscription_edited_in_the_file_is_not_used_and_the_record_still_holds() {
        let key = [3u8; 32];
        let plain = device(&key);
        let mut on = plain.clone();
        on.push = Some(PushSub { endpoint: "https://fcm.googleapis.com/fcm/send/a".into(), p256dh: "k".into(), lang: "en".into(), created_at: 1, origin: None, mac: String::new() });
        assert!(!push_holds(&on, &key), "not sealed yet");
        seal(&mut on, &key);
        assert!(push_holds(&on, &key));
        assert!(verify(&on, &key));
        assert_eq!(on.mac, plain.mac, "what an older Kelpie checks has not changed");
        assert!(!push_holds(&on, &[4u8; 32]), "another key");
        let mut edited = on.clone();
        edited.push.as_mut().unwrap().endpoint = "https://fcm.googleapis.com/fcm/send/mine".into();
        assert!(!push_holds(&edited, &key), "a changed address");
        assert!(verify(&edited, &key), "the pairing is not what was tampered with");
        let mut other = device(&key);
        other.id = "d2".into();
        seal(&mut other, &key);
        other.push = on.push.clone();
        assert!(!push_holds(&other, &key), "another record's subscription");
        let mut led_away = on.clone();
        led_away.push.as_mut().unwrap().origin = Some("https://elsewhere.example".into());
        assert!(!push_holds(&led_away, &key), "an origin added in the file");
        let mut with_origin = on.clone();
        with_origin.push.as_mut().unwrap().origin = Some("https://desk.tail.ts.net:8443".into());
        seal(&mut with_origin, &key);
        assert!(push_holds(&with_origin, &key));
        let mut changed = with_origin.clone();
        changed.push.as_mut().unwrap().origin = Some("https://elsewhere.example".into());
        assert!(!push_holds(&changed, &key), "an origin changed in the file");
        changed.push.as_mut().unwrap().origin = None;
        assert!(!push_holds(&changed, &key), "an origin removed from the file");
        let mut raised = on.clone();
        raised.permission = Permission::Write;
        seal(&mut raised, &key);
        assert!(push_holds(&raised, &key), "a permission change keeps it");
        // A record from before the field, and one written with it, read back.
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("push"));
        let back: Device = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert_eq!(back.push, on.push);
    }

    #[test]
    fn a_passkey_edited_in_the_file_unlocks_nothing_and_the_record_still_holds() {
        let key = [3u8; 32];
        let plain = device(&key);
        let mut on = plain.clone();
        on.lock = Some(LockCredential { cred_id: "c".into(), spki: "s".into(), origin: "https://desk.example".into(), sign_count: 4, created_at: 1, mac: String::new() });
        assert!(!lock_holds(&on, &key), "not sealed yet");
        seal_lock(&mut on, &key);
        assert!(lock_holds(&on, &key));
        assert!(verify(&on, &key));
        assert_eq!(on.mac, plain.mac, "what an older Kelpie checks has not changed");
        assert!(!lock_holds(&on, &[4u8; 32]), "another key");
        let edits: [fn(&mut LockCredential); 4] = [|l| l.spki = "mine".into(), |l| l.cred_id = "mine".into(), |l| l.origin = "https://evil.example".into(), |l| l.sign_count = 0];
        for edit in edits {
            let mut edited = on.clone();
            edit(edited.lock.as_mut().unwrap());
            assert!(!lock_holds(&edited, &key));
            assert!(verify(&edited, &key), "the pairing is not what was tampered with");
        }
        let mut other = device(&key);
        other.id = "d2".into();
        seal(&mut other, &key);
        other.lock = on.lock.clone();
        assert!(!lock_holds(&other, &key), "another record's passkey");
        // Raising a permission re-seals the record, and does not bless what was edited in the file.
        let mut tampered = on.clone();
        tampered.lock.as_mut().unwrap().spki = "mine".into();
        tampered.permission = Permission::Write;
        seal(&mut tampered, &key);
        assert!(!lock_holds(&tampered, &key));
        let mut raised = on.clone();
        raised.permission = Permission::Write;
        seal(&mut raised, &key);
        assert!(lock_holds(&raised, &key), "a permission change keeps a passkey that holds");
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("lock"));
        let back: Device = serde_json::from_str(&serde_json::to_string(&on).unwrap()).unwrap();
        assert_eq!(back.lock, on.lock);
    }

    #[test]
    fn lock_settings_edited_in_the_file_do_not_hold_and_a_store_from_before_has_the_defaults() {
        let key = [3u8; 32];
        let mut o = LockOptions { on: false, idle_s: 3600, recheck: false, plain_read: true, mac: String::new() };
        assert!(!options_hold(&o, &key), "not sealed");
        seal_options(&mut o, &key);
        assert!(options_hold(&o, &key));
        assert!(!options_hold(&o, &[4u8; 32]));
        let mut strict = LockOptions::default();
        seal_options(&mut strict, &key);
        let mut turned_off = strict.clone();
        turned_off.on = false;
        assert!(!options_hold(&turned_off, &key), "the switch flipped in the file");
        let store: Store = serde_json::from_str(r#"{"version":2,"enabled":true}"#).unwrap();
        assert!(store.lock.is_none());
        assert!(LockOptions::default().on && !LockOptions::default().recheck && LockOptions::default().plain_read);
    }

    #[test]
    fn nothing_the_person_can_set_is_stricter_than_what_applies_when_the_settings_cannot_be_believed() {
        for on in [true, false] {
            for recheck in [true, false] {
                for plain_read in [true, false] {
                    for idle_s in IDLE_CHOICES {
                        let chosen = LockOptions { on, idle_s, recheck, plain_read, mac: String::new() };
                        assert!(!chosen.weakened_by(&LockOptions::tightest()), "{chosen:?}");
                    }
                }
            }
        }
        assert!(LockOptions::tightest().weakened_by(&LockOptions::default()), "the defaults are not it");
    }

    #[test]
    fn what_takes_protection_away_is_told_from_what_adds_it() {
        let base = LockOptions { on: true, idle_s: 300, recheck: true, plain_read: false, mac: String::new() };
        for (weaker, why) in [(LockOptions { on: false, ..base.clone() }, "off"), (LockOptions { idle_s: 900, ..base.clone() }, "longer"), (LockOptions { recheck: false, ..base.clone() }, "no re-check"), (LockOptions { plain_read: true, ..base.clone() }, "reading over plain HTTP")] {
            assert!(base.weakened_by(&weaker), "{why}");
        }
        assert!(!base.weakened_by(&base));
        assert!(!base.weakened_by(&LockOptions { idle_s: 60, ..base.clone() }));
        let off = LockOptions { on: false, idle_s: 3600, recheck: false, plain_read: true, mac: String::new() };
        assert!(!off.weakened_by(&LockOptions { on: true, ..off.clone() }));
    }

    #[test]
    fn the_old_file_gives_its_settings_and_names_and_is_never_written() {
        let d = dir("legacy");
        let old = br#"{"enabled":true,"ip":"192.168.1.5","port":5000,"devices":[{"id":"x","name":"Old phone","token_sha256":"ab","can_write":true,"created_at":3}]}"#;
        std::fs::write(d.join(LEGACY_FILE), old).unwrap();
        let l = load(&d);
        assert!(l.broken.is_none() && l.fresh);
        assert!(l.store.enabled);
        assert_eq!(l.store.port, Some(5000));
        assert_eq!(l.store.ip.as_deref(), Some("192.168.1.5"));
        assert!(l.store.devices.is_empty(), "a token is not a key");
        assert_eq!(l.store.legacy.len(), 1);
        assert_eq!(l.store.legacy[0].name, "Old phone");
        assert_eq!(std::fs::read(d.join(LEGACY_FILE)).unwrap(), old);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_cannot_be_used_still_says_which_serve_entry_is_ours() {
        let d = dir("port");
        std::fs::write(d.join(FILE), br#"{"version":3,"enabled":true,"tailscale":true,"tailscale_port":5000}"#).unwrap();
        let l = load(&d);
        assert!(l.broken.is_some());
        assert_eq!(l.store.tailscale_port, Some(5000));
        assert!(!l.store.enabled && !l.store.tailscale, "nothing else of it counts");
        let _ = std::fs::remove_dir_all(&d);
        let d = dir("garbage-port");
        std::fs::write(d.join(FILE), b"{not json").unwrap();
        assert_eq!(load(&d).store.tailscale_port, None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unreadable_or_newer_file_is_copied_aside_and_reported() {
        for (tag, body) in [("garbage", &b"{not json"[..]), ("newer", &br#"{"version":3,"enabled":true}"#[..])] {
            let d = dir(tag);
            std::fs::write(d.join(FILE), body).unwrap();
            let l = load(&d);
            assert!(l.broken.is_some(), "{tag}");
            assert!(!l.store.enabled, "{tag}: phone access stays off");
            assert_eq!(std::fs::read(d.join(FILE)).unwrap(), body, "{tag}: left as it was");
            let copies: Vec<_> = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("mobile.v2.unreadable-")).collect();
            assert_eq!(copies.len(), 1, "{tag}");
            assert_eq!(std::fs::read(copies[0].path()).unwrap(), body);
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}
