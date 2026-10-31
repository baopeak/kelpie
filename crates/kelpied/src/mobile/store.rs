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
    /// It paired on an HTTPS origin, so a page swapped on the network cannot have done it. What later grants rest on (the relay, control).
    #[serde(default)]
    pub secure_origin: bool,
    /// HMAC-SHA256 (hex) saying so, for a record with `secure_origin` only. A second tag, not a field of the first: a record paired here
    /// still verifies for an older Kelpie, which ignores both, and setting the flag in the file does not verify anywhere.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secure_mac: String,
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
        let mut d = Device { id: "d1".into(), name: "Phone".into(), pk: "AAAA".into(), permission: Permission::Read, kind: Kind::Pwa, created_at: 7, last_seen: 0, mac: String::new(), secure_origin: false, secure_mac: String::new(), verified: false };
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
