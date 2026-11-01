//! Web Push as the daemon sends it, with no server of ours in between: RFC 8291 message encryption (`aes128gcm`, RFC 8188),
//! a VAPID signature (RFC 8292) and the guard on the address a phone hands in.
//!
//! The `web-push` crate is not used: it depends on `ece`, whose only real backend is OpenSSL, and Windows builds here stay free of
//! OpenSSL. The push service sees the endpoint, the size and the time, never the content: what goes out is an alert with a fixed
//! sentence and a deep link, and no project, session or command text is ever an input to it.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes128Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::Kind;

/// How long a push service may hold an alert for a phone that is away: later than this it says something old.
pub const TTL_SECONDS: &str = "600";
/// How long a signature holds. Push services refuse more than a day.
const JWT_LIFETIME_S: u64 = 12 * 3600;
/// Who runs this sender, as the push services ask to be told: the project, no person's address.
const JWT_SUBJECT: &str = "https://github.com/baopeak/kelpie";
/// What a push service may be asked to carry.
pub const MAX_ALERT_BYTES: usize = 2048;
const ENDPOINT_BYTES: usize = 1024;
const RECORD_SIZE: u32 = 4096;
/// The hosts an endpoint may be on. The address comes from a phone and the daemon will POST to it, so it is a closed list and not a
/// check for "looks public": an address nobody listed is refused. `*.push.services.mozilla.com` and `*.notify.windows.com` are from
/// the browsers' documentation and have not been seen from a real phone.
const PUSH_HOST_SUFFIXES: [&str; 3] = [".push.apple.com", ".push.services.mozilla.com", ".notify.windows.com"];
const PUSH_HOSTS: [&str; 1] = ["fcm.googleapis.com"];

// ---------- the address a phone hands in ----------

/// The part of `url` between the scheme and the path, when it is a plain `scheme://host[/path]` with nothing odd in it.
fn authority<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let rest = url.strip_prefix(scheme)?.strip_prefix("://")?;
    if url.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '#' | '\\' | '"' | '<' | '>')) {
        return None;
    }
    Some(rest.split(['/', '?']).next().unwrap_or(""))
}

/// Whether a push may be sent to `url`: https, a host on the closed list, no port, no login, no bytes that could mean something
/// else to another parser. `test_base` is what a test run may use besides (see `push_test_base`).
pub fn endpoint_allowed(url: &str, test_base: Option<&str>) -> bool {
    if url.is_empty() || url.len() > ENDPOINT_BYTES {
        return false;
    }
    if let Some(base) = test_base {
        if authority(url, "http").is_some() && url.strip_prefix(base).is_some_and(|rest| rest.starts_with('/')) {
            return true;
        }
    }
    let Some(host) = authority(url, "https") else { return false };
    if !host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-') {
        return false;
    }
    PUSH_HOSTS.contains(&host) || PUSH_HOST_SUFFIXES.iter().any(|s| host.strip_suffix(s).is_some_and(|label| !label.is_empty() && !label.starts_with('.') && !label.ends_with('.')))
}

/// `scheme://host` of an endpoint: what its signature is for.
fn origin(endpoint: &str) -> Option<&str> {
    let (scheme, rest) = endpoint.split_once("://")?;
    let host = rest.split(['/', '?']).next()?;
    (!host.is_empty()).then(|| &endpoint[..scheme.len() + 3 + host.len()])
}

/// The subscription's keys as the browser gives them: a P-256 point and 16 bytes, base64url.
pub fn keys_valid(p256dh: &str, auth: &str) -> bool {
    let point = B64.decode(p256dh).ok().filter(|b| b.len() == 65 && b[0] == 4).is_some_and(|b| PublicKey::from_sec1_bytes(&b).is_ok());
    point && B64.decode(auth).is_ok_and(|b| b.len() == 16)
}

// ---------- encryption ----------

/// Encrypts `plaintext` to a subscription, under a key made for this one message.
pub fn encrypt(plaintext: &[u8], p256dh: &str, auth: &str) -> Result<Vec<u8>, &'static str> {
    let ua_public = B64.decode(p256dh).map_err(|_| "the subscription's key is not readable")?;
    let auth = B64.decode(auth).map_err(|_| "the subscription's secret is not readable")?;
    let salt: [u8; 16] = kelpie_link::random_key().map_err(|_| "no randomness")?[..16].try_into().expect("16 of 32 bytes");
    encrypt_with(plaintext, &ua_public, &auth, &random_secret()?, &salt)
}

/// A P-256 scalar from the OS generator. `from_slice` refuses the few values that are none: another draw is the way out.
fn random_secret() -> Result<SecretKey, &'static str> {
    for _ in 0..8 {
        if let Ok(key) = SecretKey::from_slice(&kelpie_link::random_key().map_err(|_| "no randomness")?) {
            return Ok(key);
        }
    }
    Err("no randomness")
}

/// RFC 8291 section 3.4, and the one record of RFC 8188 it allows, marked last with the delimiter `2`.
fn encrypt_with(plaintext: &[u8], ua_public: &[u8], auth: &[u8], as_secret: &SecretKey, salt: &[u8; 16]) -> Result<Vec<u8>, &'static str> {
    let ua = PublicKey::from_sec1_bytes(ua_public).map_err(|_| "the subscription's key is not a P-256 point")?;
    let shared = diffie_hellman(as_secret.to_nonzero_scalar(), ua.as_affine());
    let as_public = as_secret.public_key().to_encoded_point(false);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public.as_bytes());
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes()).expand(&key_info, &mut ikm).map_err(|_| "key derivation")?;
    let (cek, nonce) = content_keys(salt, &ikm)?;
    let mut record = plaintext.to_vec();
    record.push(2);
    let sealed = Aes128Gcm::new_from_slice(&cek).map_err(|_| "key length")?.encrypt(Nonce::from_slice(&nonce), record.as_slice()).map_err(|_| "encryption")?;
    let mut out = Vec::with_capacity(21 + as_public.len() + sealed.len());
    out.extend_from_slice(salt);
    out.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend_from_slice(as_public.as_bytes());
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn content_keys(salt: &[u8], ikm: &[u8; 32]) -> Result<([u8; 16], [u8; 12]), &'static str> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    hk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).map_err(|_| "key derivation")?;
    hk.expand(b"Content-Encoding: nonce\0", &mut nonce).map_err(|_| "key derivation")?;
    Ok((cek, nonce))
}

// ---------- the signature ----------

/// The key this daemon signs with, the "application server key" a phone subscribes under. Never printed.
pub struct Vapid {
    key: SigningKey,
}

impl Vapid {
    pub fn generate() -> Result<Vapid, &'static str> {
        Ok(Vapid { key: SigningKey::from(random_secret()?) })
    }

    /// A key as it was kept; None for a value that is none (the caller says so and never writes over it).
    pub fn from_hex(text: &str) -> Option<Vapid> {
        if text.len() != 64 {
            return None;
        }
        let bytes: Option<Vec<u8>> = (0..64).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect();
        Some(Vapid { key: SigningKey::from_slice(&bytes?).ok()? })
    }

    pub fn to_hex(&self) -> String {
        self.key.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// What `pushManager.subscribe` takes.
    pub fn public_key(&self) -> String {
        B64.encode(self.key.verifying_key().to_encoded_point(false).as_bytes())
    }

    fn jwt(&self, aud: &str, exp: u64) -> String {
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64.encode(json!({"aud": aud, "exp": exp, "sub": JWT_SUBJECT}).to_string());
        let signed = format!("{header}.{claims}");
        let signature: Signature = self.key.sign(signed.as_bytes());
        format!("{signed}.{}", B64.encode(signature.to_bytes()))
    }

    /// The `Authorization` header for one request to `endpoint`.
    pub fn authorization(&self, endpoint: &str, now_s: u64) -> Option<String> {
        Some(format!("vapid t={}, k={}", self.jwt(origin(endpoint)?, now_s + JWT_LIFETIME_S), self.public_key()))
    }
}

// ---------- the alert ----------

/// What an alert says. None of these takes text from a session: that is what keeps the payload content-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wording {
    Kind(Kind),
    /// After an outage: this many sessions wait on the person.
    NeedYou(usize),
    /// Several things that wait for nobody, gathered into one message.
    Updates(usize),
    /// After an outage, both.
    NeedYouAndUpdates(usize, usize),
    Test,
}

/// The fixed sentence in the phone's language.
pub fn sentence(wording: Wording, lang: &str) -> String {
    let vi = lang == "vi";
    match (wording, vi) {
        (Wording::Kind(Kind::Waiting), false) => "A session needs you".into(),
        (Wording::Kind(Kind::Waiting), true) => "Một phiên đang cần bạn".into(),
        (Wording::Kind(Kind::Finished), false) => "A session finished".into(),
        (Wording::Kind(Kind::Finished), true) => "Một phiên đã xong".into(),
        (Wording::Kind(Kind::Error), false) => "A session stopped with an error".into(),
        (Wording::Kind(Kind::Error), true) => "Một phiên đã dừng vì lỗi".into(),
        (Wording::Kind(Kind::Automation), false) => "An automation ran".into(),
        (Wording::Kind(Kind::Automation), true) => "Một tự động hóa đã chạy".into(),
        (Wording::Kind(Kind::Approval), false) => "A run is waiting for your approval".into(),
        (Wording::Kind(Kind::Approval), true) => "Một lượt chạy đang chờ bạn duyệt".into(),
        (Wording::Kind(Kind::Cost), false) => "A session passed its cost cap".into(),
        (Wording::Kind(Kind::Cost), true) => "Một phiên đã vượt mức chi phí".into(),
        (Wording::NeedYou(1), false) => "1 session needs you".into(),
        (Wording::NeedYou(n), false) => format!("{n} sessions need you"),
        (Wording::NeedYou(n), true) => format!("{n} phiên đang cần bạn"),
        (Wording::Updates(1), false) => "1 update from your sessions".into(),
        (Wording::Updates(n), false) => format!("{n} updates from your sessions"),
        (Wording::Updates(n), true) => format!("{n} cập nhật từ các phiên của bạn"),
        (Wording::NeedYouAndUpdates(n, m), false) => format!("{}, and {}", sentence(Wording::NeedYou(n), "en"), if m == 1 { "1 more update".to_string() } else { format!("{m} more updates") }),
        (Wording::NeedYouAndUpdates(n, m), true) => format!("{n} phiên đang cần bạn và {m} cập nhật khác"),
        (Wording::Test, false) => "Alerts from this computer work".into(),
        (Wording::Test, true) => "Cảnh báo từ máy tính này hoạt động".into(),
    }
}

/// Declarative Web Push, which iOS 18.4 and later shows by itself and the service worker reads everywhere else.
pub fn alert_json(wording: Wording, lang: &str, navigate: &str, tag: &str) -> Vec<u8> {
    json!({"web_push": 8030, "notification": {"title": "Kelpie", "body": sentence(wording, lang), "navigate": navigate, "tag": tag}}).to_string().into_bytes()
}

/// A short name for what an alert is about, for `Topic` and the notification's `tag`: a later alert of the same name takes the place
/// of one that was not delivered or shown yet. A hash, so that what the push service sees of it says nothing.
pub fn topic(about: &str) -> String {
    B64.encode(&Sha256::digest(about.as_bytes())[..16])
}

#[cfg(test)]
pub(crate) fn decrypt(message: &[u8], ua_secret: &SecretKey, auth: &[u8]) -> Result<Vec<u8>, &'static str> {
    if message.len() < 21 {
        return Err("short");
    }
    let (salt, rest) = message.split_at(16);
    let id_len = rest[4] as usize;
    let as_public = rest.get(5..5 + id_len).ok_or("short")?;
    let sealed = &rest[5 + id_len..];
    let shared = diffie_hellman(ua_secret.to_nonzero_scalar(), PublicKey::from_sec1_bytes(as_public).map_err(|_| "point")?.as_affine());
    let ua_public = ua_secret.public_key().to_encoded_point(false);
    let mut key_info = b"WebPush: info\0".to_vec();
    key_info.extend_from_slice(ua_public.as_bytes());
    key_info.extend_from_slice(as_public);
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth), shared.raw_secret_bytes()).expand(&key_info, &mut ikm).map_err(|_| "key derivation")?;
    let (cek, nonce) = content_keys(salt, &ikm)?;
    let mut plain = Aes128Gcm::new_from_slice(&cek).map_err(|_| "key length")?.decrypt(Nonce::from_slice(&nonce), sealed).map_err(|_| "does not open")?;
    if plain.pop() != Some(2) {
        return Err("not the last record");
    }
    Ok(plain)
}

/// A browser's side of a subscription, for tests: its secret key, and the two values it hands the page.
#[cfg(test)]
pub(crate) fn test_subscription() -> (SecretKey, String, String) {
    let secret = random_secret().unwrap();
    let p256dh = B64.encode(secret.public_key().to_encoded_point(false).as_bytes());
    let auth = B64.encode(&kelpie_link::random_key().unwrap()[..16]);
    (secret, p256dh, auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;

    fn b(s: &str) -> Vec<u8> {
        B64.decode(s).unwrap()
    }

    /// The worked example of RFC 8291, appendix A: from the two parties' keys to the very bytes on the wire.
    #[test]
    fn the_rfc_8291_example_comes_out_byte_for_byte_and_opens_again() {
        let plaintext = b("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        assert_eq!(plaintext, b"When I grow up, I want to be a watermelon");
        let as_secret = SecretKey::from_slice(&b("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw")).unwrap();
        let ua_public = b("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4");
        let auth = b("BTBZMqHH6r4Tts7J_aSIgg");
        let salt: [u8; 16] = b("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let wire = encrypt_with(&plaintext, &ua_public, &auth, &as_secret, &salt).unwrap();
        let expected = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";
        assert_eq!(B64.encode(&wire), expected);
        let ua_secret = SecretKey::from_slice(&b("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")).unwrap();
        assert_eq!(decrypt(&wire, &ua_secret, &auth).unwrap(), plaintext);
    }

    /// The example of RFC 8292, section 2.4: what a push service does with such a header, on the RFC's own values.
    #[test]
    fn the_rfc_8292_example_verifies_the_way_ours_is_checked() {
        let token = "eyJ0eXAiOiJKV1QiLCJhbGciOiJFUzI1NiJ9.eyJhdWQiOiJodHRwczovL3B1c2guZXhhbXBsZS5uZXQiLCJleHAiOjE0NTM1MjM3NjgsInN1YiI6Im1haWx0bzpwdXNoQGV4YW1wbGUuY29tIn0.i3CYb7t4xfxCDquptFOepC9GAu_HLGkMlMuCGSK2rpiUfnK9ojFwDXb1JrErtmysazNjjvW2L9OkSSHzvoD1oA";
        let public = "BA1Hxzyi1RUM1b5wjxsn7nGxAszw2u61m164i3MrAIxHF6YK5h4SDYic-dRuU_RCPCfA5aq9ojSwk5Y2EmClBPs";
        assert!(verifies(token, public), "the RFC's own");
        let mut tampered = token.to_string();
        tampered.replace_range(40..41, if &token[40..41] == "A" { "B" } else { "A" });
        assert!(!verifies(&tampered, public), "a changed claim");
    }

    fn verifies(token: &str, public: &str) -> bool {
        let Some((signed, signature)) = token.rsplit_once('.') else { return false };
        let key = VerifyingKey::from_sec1_bytes(&b(public)).unwrap();
        let signature = Signature::from_slice(&b(signature)).unwrap();
        key.verify(signed.as_bytes(), &signature).is_ok()
    }

    fn claims(token: &str) -> serde_json::Value {
        serde_json::from_slice(&b(token.split('.').nth(1).unwrap())).unwrap()
    }

    #[test]
    fn a_signature_is_for_the_endpoints_origin_for_less_than_a_day_and_names_no_person() {
        let vapid = Vapid::generate().unwrap();
        let header = vapid.authorization("https://web.push.apple.com/QGr-abc/def?x=1", 1_000).unwrap();
        let (token, key) = header.strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
        assert_eq!(key, vapid.public_key());
        assert!(verifies(token, key), "signed with the key it names");
        let c = claims(token);
        assert_eq!(c["aud"], "https://web.push.apple.com");
        assert!(c["exp"].as_u64().unwrap() > 1_000 && c["exp"].as_u64().unwrap() <= 1_000 + 24 * 3600);
        assert_eq!(c["sub"], JWT_SUBJECT);
        assert!(!c.to_string().contains('@'), "no address of a person");
        let header: serde_json::Value = serde_json::from_slice(&b(token.split('.').next().unwrap())).unwrap();
        assert_eq!(header["alg"], "ES256");
    }

    #[test]
    fn a_key_comes_back_as_it_was_kept_and_what_is_no_key_is_refused() {
        let vapid = Vapid::generate().unwrap();
        let again = Vapid::from_hex(&vapid.to_hex()).unwrap();
        assert_eq!(again.public_key(), vapid.public_key());
        assert_eq!(b(&vapid.public_key()).len(), 65);
        for bad in ["", "zz", &"0".repeat(64), &"g".repeat(64), &"a".repeat(63)] {
            assert!(Vapid::from_hex(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn only_the_known_push_services_are_reachable() {
        for good in [
            "https://web.push.apple.com/QGr-abc",
            "https://api.push.apple.com/3/device/x",
            "https://fcm.googleapis.com/fcm/send/dMQ:abc",
            "https://updates.push.services.mozilla.com/wpush/v2/abc",
            "https://wns2-par02p.notify.windows.com/w/?token=abc",
        ] {
            assert!(endpoint_allowed(good, None), "{good}");
        }
        let long = format!("https://fcm.googleapis.com/{}", "a".repeat(ENDPOINT_BYTES));
        for bad in [
            "",
            "http://fcm.googleapis.com/x",
            "ftp://fcm.googleapis.com/x",
            "https://127.0.0.1/x",
            "https://[::1]/x",
            "https://10.0.0.1/x",
            "https://192.168.1.5/x",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost/x",
            "https://push.apple.com/x",
            "https://.push.apple.com/x",
            "https://a..push.apple.com/x",
            "https://evil.com/x",
            "https://fcm.googleapis.com.evil.com/x",
            "https://push.apple.com.evil.com/x",
            "https://evilpush.apple.com/x",
            "https://fcm.googleapis.com:8443/x",
            "https://fcm.googleapis.com@evil.com/x",
            "https://evil.com@fcm.googleapis.com/x",
            "https://user:pass@fcm.googleapis.com/x",
            "https://FCM.googleapis.com/x",
            "https://fcm.googleapis.com/x y",
            "https://fcm.googleapis.com/x\r\nHost: evil.com",
            "https://fcm.googleapis.com/x#y",
            "https://fcm.googleapis.com\\@evil.com/x",
            "https://evil.com\\.push.apple.com/x",
            "https://evil.com/.push.apple.com",
            "https://evil.com?.push.apple.com",
            "HTTPS://fcm.googleapis.com/x",
            "fcm.googleapis.com/x",
            &long,
        ] {
            assert!(!endpoint_allowed(bad, None), "{bad:?}");
        }
    }

    #[test]
    fn a_test_run_reaches_only_the_address_it_was_given_and_never_replaces_the_list() {
        let base = Some("http://127.0.0.1:9000");
        assert!(endpoint_allowed("http://127.0.0.1:9000/p/1", base));
        assert!(endpoint_allowed("https://fcm.googleapis.com/x", base), "the list still holds");
        for bad in ["http://127.0.0.1:9000", "http://127.0.0.1:90001/p", "http://127.0.0.1:9001/p", "http://evil.com/p", "http://127.0.0.1:9000@evil.com/p", "http://127.0.0.1:9000/p q"] {
            assert!(!endpoint_allowed(bad, base), "{bad:?}");
        }
        assert!(!endpoint_allowed("http://127.0.0.1:9000/p/1", None), "and none without it");
    }

    #[test]
    fn what_a_browser_hands_in_is_checked() {
        let (_, p256dh, auth) = test_subscription();
        assert!(keys_valid(&p256dh, &auth));
        assert!(!keys_valid(&p256dh, "AAAA"), "short secret");
        assert!(!keys_valid("AAAA", &auth), "short key");
        assert!(!keys_valid(&B64.encode([4u8; 65]), &auth), "not on the curve");
        assert!(!keys_valid("not base64!", &auth));
    }

    const ALL: [Kind; 6] = [Kind::Waiting, Kind::Finished, Kind::Error, Kind::Automation, Kind::Approval, Kind::Cost];

    #[test]
    fn every_alert_says_a_fixed_sentence_and_fits() {
        for lang in ["en", "vi", "xx"] {
            for wording in ALL.iter().map(|k| Wording::Kind(*k)).chain([Wording::NeedYou(1), Wording::NeedYou(49), Wording::Updates(1), Wording::Updates(3), Wording::NeedYouAndUpdates(2, 1), Wording::Test]) {
                let body = alert_json(wording, lang, "https://desk.tail1234.ts.net:8443/#s=abc", &topic("abc"));
                assert!(body.len() <= MAX_ALERT_BYTES);
                let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v["web_push"], 8030);
                assert_eq!(v["notification"]["title"], "Kelpie");
                assert_eq!(v["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/#s=abc");
                assert_eq!(v["notification"]["body"], sentence(wording, lang));
            }
        }
        assert_eq!(sentence(Wording::Kind(Kind::Waiting), "xx"), "A session needs you", "an unknown language is English");
        assert_ne!(sentence(Wording::Kind(Kind::Waiting), "vi"), sentence(Wording::Kind(Kind::Waiting), "en"));
        assert_eq!(sentence(Wording::NeedYou(3), "en"), "3 sessions need you");
        assert_eq!(sentence(Wording::NeedYouAndUpdates(1, 2), "en"), "1 session needs you, and 2 more updates");
    }

    #[test]
    fn what_goes_out_is_opaque_and_opens_only_with_the_subscriptions_keys() {
        let (secret, p256dh, auth) = test_subscription();
        let body = alert_json(Wording::Kind(Kind::Waiting), "en", "https://h/#s=needle-session", "tag");
        let wire = encrypt(&body, &p256dh, &auth).unwrap();
        let hay = String::from_utf8_lossy(&wire);
        assert!(!hay.contains("needle-session") && !hay.contains("Kelpie") && !hay.contains("navigate"));
        assert_eq!(decrypt(&wire, &secret, &b(&auth)).unwrap(), body);
        assert_eq!(&wire[16..20], &RECORD_SIZE.to_be_bytes());
        assert!(wire.len() <= RECORD_SIZE as usize);
        let (other, ..) = test_subscription();
        assert!(decrypt(&wire, &other, &b(&auth)).is_err(), "another phone's key");
        assert!(decrypt(&wire, &secret, &[9u8; 16]).is_err(), "another secret");
        assert_ne!(encrypt(&body, &p256dh, &auth).unwrap(), wire, "a key and a salt of its own each time");
    }

    #[test]
    fn a_topic_is_what_a_push_service_accepts_and_says_nothing() {
        let t = topic("session-id|123");
        assert!(t.len() <= 32 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{t}");
        assert!(!t.contains("session"));
        assert_eq!(t, topic("session-id|123"));
        assert_ne!(t, topic("session-id|124"));
    }
}
