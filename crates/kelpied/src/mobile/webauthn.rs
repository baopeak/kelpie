//! Checks a WebAuthn ceremony from the phone: ES256 only, on `p256`, with no CBOR parser and no attestation. What is checked here is
//! everything the daemon's lock rests on: the page's word about any of it counts for nothing, as a page can be swapped and a
//! Noise key copied. A passkey's private half never leaves the phone's authenticator, so only a signature made there passes.
//!
//! - **Enrolling** takes the public key as `getPublicKey()` gives it (SPKI), with attestation `none`: nobody vouches for the authenticator, and
//!   what makes this safe is who may enrol (see `Mobile::lock_enrol`). The credential id is read out of the authenticator data, which has fixed offsets up to it.
//! - **Asserting** checks the signature over `authenticatorData ‖ SHA-256(clientDataJSON)` with the key enrolled.
//! - **Both** check the RP ID hash, that the person was there and was verified (flags UP and UV), and in the client data the
//!   ceremony, the challenge and the exact origin. A client data that says `crossOrigin: true` came from a frame of another site.

use base64::Engine;
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;

const UP: u8 = 0x01;
const UV: u8 = 0x04;
const AT: u8 = 0x40;
/// `rpIdHash ‖ flags ‖ signCount`.
const FIXED: usize = 37;
/// What comes before the point in the SPKI of a P-256 key, whoever encodes it: no other key type or curve starts this way.
const SPKI_PREFIX: [u8; 26] = [0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00];
/// Limits on what a phone sends: well above anything real, so a page cannot make the daemon parse much.
pub const MAX_CLIENT_DATA: usize = 4096;
pub const MAX_AUTH_DATA: usize = 4096;
pub const MAX_SIGNATURE: usize = 128;
pub const MAX_CREDENTIAL_ID: usize = 1023;

/// Why a ceremony did not pass. For the audit log and the phone's message: nothing of what was sent is echoed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Malformed,
    Ceremony,
    Challenge,
    Origin,
    RpId,
    NotVerified,
    Key,
    Credential,
    Signature,
    Counter,
}

impl Refused {
    pub fn word(self) -> &'static str {
        match self {
            Refused::Malformed => "malformed",
            Refused::Ceremony => "ceremony",
            Refused::Challenge => "challenge",
            Refused::Origin => "origin",
            Refused::RpId => "rp_id",
            Refused::NotVerified => "not_verified",
            Refused::Key => "key",
            Refused::Credential => "credential",
            Refused::Signature => "signature",
            Refused::Counter => "counter",
        }
    }
}

/// What the ceremony must have been about.
pub struct Expected<'a> {
    pub origin: &'a str,
    pub challenge: &'a [u8],
}

/// The RP ID of an origin: its host. A number has none (`SecurityError` in every browser), so a page at an IP address cannot have a passkey.
pub fn rp_id(origin: &str) -> Option<String> {
    let rest = origin.strip_prefix("https://")?;
    let host = rest.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(rest, |(host, _)| host);
    let is_name = !host.is_empty() && !host.starts_with('[') && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.')) && host.chars().any(|c| c.is_ascii_alphabetic());
    let last_label_numeric = host.rsplit('.').next().is_some_and(|l| l.chars().all(|c| c.is_ascii_digit()));
    (is_name && !last_label_numeric).then(|| host.to_ascii_lowercase())
}

/// The key in a P-256 SPKI: exactly its one encoding, and a point on the curve.
pub fn public_key(spki: &[u8]) -> Result<VerifyingKey, Refused> {
    let point = spki.strip_prefix(&SPKI_PREFIX[..]).filter(|p| p.len() == 65 && p[0] == 4).ok_or(Refused::Key)?;
    VerifyingKey::from_sec1_bytes(point).map_err(|_| Refused::Key)
}

fn check_client_data(client_data: &[u8], ceremony: &str, expected: &Expected) -> Result<(), Refused> {
    if client_data.len() > MAX_CLIENT_DATA {
        return Err(Refused::Malformed);
    }
    let v: Value = serde_json::from_slice(client_data).map_err(|_| Refused::Malformed)?;
    if v["type"].as_str() != Some(ceremony) {
        return Err(Refused::Ceremony);
    }
    let said = v["challenge"].as_str().and_then(|c| B64.decode(c).ok()).ok_or(Refused::Challenge)?;
    if said != expected.challenge {
        return Err(Refused::Challenge);
    }
    if v["origin"].as_str() != Some(expected.origin) || v["crossOrigin"].as_bool() == Some(true) {
        return Err(Refused::Origin);
    }
    Ok(())
}

/// The counter, once what the authenticator data says of the site and the person is what it must be.
fn check_auth_data(auth_data: &[u8], origin: &str) -> Result<u32, Refused> {
    if auth_data.len() < FIXED || auth_data.len() > MAX_AUTH_DATA {
        return Err(Refused::Malformed);
    }
    let rp = rp_id(origin).ok_or(Refused::RpId)?;
    if auth_data[..32] != Sha256::digest(rp.as_bytes())[..] {
        return Err(Refused::RpId);
    }
    if auth_data[32] & (UP | UV) != UP | UV {
        return Err(Refused::NotVerified);
    }
    Ok(u32::from_be_bytes(auth_data[33..37].try_into().expect("four bytes")))
}

/// Checks a new credential's ceremony; its sign count when it passes.
pub fn check_enrol(client_data: &[u8], auth_data: &[u8], credential_id: &[u8], spki: &[u8], expected: &Expected) -> Result<u32, Refused> {
    check_client_data(client_data, "webauthn.create", expected)?;
    let count = check_auth_data(auth_data, expected.origin)?;
    public_key(spki)?;
    if credential_id.is_empty() || credential_id.len() > MAX_CREDENTIAL_ID {
        return Err(Refused::Credential);
    }
    // `… ‖ aaguid (16) ‖ length (2) ‖ credential id ‖ key`.
    if auth_data[32] & AT == 0 || auth_data.len() < FIXED + 18 {
        return Err(Refused::Credential);
    }
    let len = u16::from_be_bytes(auth_data[FIXED + 16..FIXED + 18].try_into().expect("two bytes")) as usize;
    if auth_data.get(FIXED + 18..FIXED + 18 + len) != Some(credential_id) {
        return Err(Refused::Credential);
    }
    Ok(count)
}

/// Checks an assertion by the credential with public key `spki`; its sign count when it passes. A count that stayed 0 is what
/// passkeys report; any other that does not grow says the key was copied, or this is an old message.
pub fn check_assertion(client_data: &[u8], auth_data: &[u8], signature: &[u8], spki: &[u8], stored_count: u32, expected: &Expected) -> Result<u32, Refused> {
    check_client_data(client_data, "webauthn.get", expected)?;
    let count = check_auth_data(auth_data, expected.origin)?;
    if signature.len() > MAX_SIGNATURE {
        return Err(Refused::Malformed);
    }
    let key = public_key(spki)?;
    let signature = Signature::from_der(signature).map_err(|_| Refused::Signature)?;
    let mut signed = Vec::with_capacity(auth_data.len() + 32);
    signed.extend_from_slice(auth_data);
    signed.extend_from_slice(&Sha256::digest(client_data));
    key.verify(&signed, &signature).map_err(|_| Refused::Signature)?;
    if !(count == 0 && stored_count == 0) && count <= stored_count {
        return Err(Refused::Counter);
    }
    Ok(count)
}

/// What a dangerous approval's assertion must be over: what is approved, and until when. Nobody else can use it, for this request or any other.
pub fn recheck_challenge(request: &str, command: &str, expiry_ms: u64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(request.as_bytes());
    h.update([0u8]);
    h.update(Sha256::digest(command.as_bytes()));
    h.update(expiry_ms.to_string().as_bytes());
    h.finalize().into()
}

/// A software authenticator: real keys and real signatures, what a phone's would produce, for the tests here and in `mobile`.
#[cfg(test)]
pub mod soft {
    use super::*;
    use p256::ecdsa::signature::Signer;
    use p256::ecdsa::SigningKey;
    use serde_json::json;

    pub struct Authenticator {
        pub key: SigningKey,
        pub id: Vec<u8>,
        pub origin: String,
        pub count: u32,
        /// Flags it reports (UP and UV unless a test says otherwise).
        pub flags: u8,
    }

    impl Authenticator {
        pub fn new(origin: &str) -> Authenticator {
            let seed = Sha256::digest(uuid::Uuid::new_v4().as_bytes());
            Authenticator { key: SigningKey::from_slice(&seed).unwrap(), id: uuid::Uuid::new_v4().as_bytes().to_vec(), origin: origin.into(), count: 0, flags: UP | UV }
        }

        pub fn spki(&self) -> Vec<u8> {
            let mut out = SPKI_PREFIX.to_vec();
            out.extend_from_slice(self.key.verifying_key().to_encoded_point(false).as_bytes());
            out
        }

        pub fn client_data(&self, ceremony: &str, challenge: &[u8]) -> Vec<u8> {
            json!({"type": ceremony, "challenge": B64.encode(challenge), "origin": self.origin, "crossOrigin": false}).to_string().into_bytes()
        }

        pub fn auth_data(&self, flags: u8, count: u32) -> Vec<u8> {
            let mut out = Sha256::digest(rp_id(&self.origin).unwrap_or_default().as_bytes()).to_vec();
            out.push(flags);
            out.extend_from_slice(&count.to_be_bytes());
            out
        }

        /// (client data, authenticator data with the credential in it, as `credentials.create` gives them).
        pub fn create(&self, challenge: &[u8]) -> (Vec<u8>, Vec<u8>) {
            let mut auth = self.auth_data(self.flags | AT, self.count);
            auth.extend_from_slice(&[0u8; 16]);
            auth.extend_from_slice(&(self.id.len() as u16).to_be_bytes());
            auth.extend_from_slice(&self.id);
            // The key, which nothing here reads (a COSE map in a real one).
            auth.extend_from_slice(&[0xa0]);
            (self.client_data("webauthn.create", challenge), auth)
        }

        pub fn sign(&self, auth: &[u8], client_data: &[u8]) -> Vec<u8> {
            let mut signed = auth.to_vec();
            signed.extend_from_slice(&Sha256::digest(client_data));
            let signature: Signature = self.key.sign(&signed);
            signature.to_der().as_bytes().to_vec()
        }

        /// (client data, authenticator data, signature), as `credentials.get` gives them. The counter moves when it is not a passkey's 0.
        pub fn get(&mut self, challenge: &[u8]) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
            if self.count > 0 {
                self.count += 1;
            }
            let client = self.client_data("webauthn.get", challenge);
            let auth = self.auth_data(self.flags, self.count);
            let signature = self.sign(&auth, &client);
            (client, auth, signature)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::soft::Authenticator;
    use super::*;

    const ORIGIN: &str = "https://desk.tail1234.ts.net:8443";
    const CHALLENGE: [u8; 32] = [7; 32];

    fn expected() -> Expected<'static> {
        Expected { origin: ORIGIN, challenge: &CHALLENGE }
    }

    #[test]
    fn the_rp_id_is_the_name_of_the_origin_and_a_number_has_none() {
        assert_eq!(rp_id(ORIGIN).as_deref(), Some("desk.tail1234.ts.net"));
        assert_eq!(rp_id("https://Localhost:48170").as_deref(), Some("localhost"));
        assert_eq!(rp_id("https://desk.example").as_deref(), Some("desk.example"));
        for none in ["http://desk.example", "https://127.0.0.1:8443", "https://[::1]:8443", "https://", "https://100.64.1.2", "desk.example", "https://exa mple.com", "https://a.b/c"] {
            assert_eq!(rp_id(none), None, "{none}");
        }
    }

    #[test]
    fn a_real_enrolment_and_assertion_pass_and_the_counter_comes_back() {
        let mut a = Authenticator::new(ORIGIN);
        let (client, auth) = a.create(&CHALLENGE);
        assert_eq!(check_enrol(&client, &auth, &a.id, &a.spki(), &expected()), Ok(0));
        let (client, auth, signature) = a.get(&CHALLENGE);
        assert_eq!(check_assertion(&client, &auth, &signature, &a.spki(), 0, &expected()), Ok(0));
        a.count = 41;
        let (client, auth, signature) = a.get(&CHALLENGE);
        assert_eq!(check_assertion(&client, &auth, &signature, &a.spki(), 41, &expected()), Ok(42));
    }

    #[test]
    fn enrolling_is_refused_on_each_thing_it_checks() {
        let a = Authenticator::new(ORIGIN);
        let (client, auth) = a.create(&CHALLENGE);
        let good = |client: &[u8], auth: &[u8], id: &[u8], spki: &[u8]| check_enrol(client, auth, id, spki, &expected());
        assert_eq!(good(&a.client_data("webauthn.get", &CHALLENGE), &auth, &a.id, &a.spki()), Err(Refused::Ceremony));
        assert_eq!(good(&a.client_data("webauthn.create", &[8; 32]), &auth, &a.id, &a.spki()), Err(Refused::Challenge));
        assert_eq!(good(&Authenticator::new("https://evil.example").client_data("webauthn.create", &CHALLENGE), &auth, &a.id, &a.spki()), Err(Refused::Origin));
        assert_eq!(good(&client, &Authenticator::new("https://evil.example").create(&CHALLENGE).1, &a.id, &a.spki()), Err(Refused::RpId));
        let mut no_uv = auth.clone();
        no_uv[32] &= !UV;
        assert_eq!(good(&client, &no_uv, &a.id, &a.spki()), Err(Refused::NotVerified));
        let mut no_up = auth.clone();
        no_up[32] &= !UP;
        assert_eq!(good(&client, &no_up, &a.id, &a.spki()), Err(Refused::NotVerified));
        let mut no_at = auth.clone();
        no_at[32] &= !AT;
        assert_eq!(good(&client, &no_at, &a.id, &a.spki()), Err(Refused::Credential));
        assert_eq!(good(&client, &auth, &[9; 16], &a.spki()), Err(Refused::Credential), "not the id in the authenticator data");
        assert_eq!(good(&client, &auth, &[], &a.spki()), Err(Refused::Credential));
        assert_eq!(good(&client, &auth[..FIXED + 3], &a.id, &a.spki()), Err(Refused::Credential), "cut short");
        assert_eq!(good(&client, &auth[..FIXED - 1], &a.id, &a.spki()), Err(Refused::Malformed));
        for bad in [Vec::new(), vec![0u8; 91], a.spki()[..90].to_vec(), { let mut off_curve = a.spki(); off_curve[40] ^= 1; off_curve }] {
            assert_eq!(good(&client, &auth, &a.id, &bad), Err(Refused::Key));
        }
        assert_eq!(good(b"{not json", &auth, &a.id, &a.spki()), Err(Refused::Malformed));
        assert_eq!(good(&vec![b' '; MAX_CLIENT_DATA + 1], &auth, &a.id, &a.spki()), Err(Refused::Malformed));
        let framed = serde_json::json!({"type": "webauthn.create", "challenge": B64.encode(CHALLENGE), "origin": ORIGIN, "crossOrigin": true}).to_string();
        assert_eq!(good(framed.as_bytes(), &auth, &a.id, &a.spki()), Err(Refused::Origin));
    }

    #[test]
    fn an_assertion_is_refused_on_each_thing_it_checks() {
        let mut a = Authenticator::new(ORIGIN);
        let (client, auth, signature) = a.get(&CHALLENGE);
        let spki = a.spki();
        let check = |client: &[u8], auth: &[u8], signature: &[u8]| check_assertion(client, auth, signature, &spki, 0, &expected());
        assert_eq!(check(&client, &auth, &signature), Ok(0));

        let mut tampered = auth.clone();
        tampered[36] = 9;
        assert_eq!(check(&client, &tampered, &signature), Err(Refused::Signature), "tampered authenticator data");
        let wrong_origin = Authenticator::new("https://desk.tail1234.ts.net:9999").client_data("webauthn.get", &CHALLENGE);
        assert_eq!(check(&wrong_origin, &auth, &a.sign(&auth, &wrong_origin)), Err(Refused::Origin));
        let other_site = Authenticator::new("https://evil.example").auth_data(UP | UV, 0);
        assert_eq!(check(&client, &other_site, &a.sign(&other_site, &client)), Err(Refused::RpId));
        let other_challenge = a.client_data("webauthn.get", &[8; 32]);
        assert_eq!(check(&other_challenge, &auth, &a.sign(&auth, &other_challenge)), Err(Refused::Challenge));
        let no_uv = a.auth_data(UP, 0);
        assert_eq!(check(&client, &no_uv, &a.sign(&no_uv, &client)), Err(Refused::NotVerified), "no user verification, signed all the same");
        let created = a.client_data("webauthn.create", &CHALLENGE);
        assert_eq!(check(&created, &auth, &a.sign(&auth, &created)), Err(Refused::Ceremony));
        let other_key = Authenticator::new(ORIGIN);
        assert_eq!(check(&client, &auth, &other_key.sign(&auth, &client)), Err(Refused::Signature), "another passkey");
        assert_eq!(check(&client, &auth, b"not der"), Err(Refused::Signature));
        assert_eq!(check(&client, &auth, &[0u8; MAX_SIGNATURE + 1]), Err(Refused::Malformed));

        // The counter: staying at 0 is what passkeys do, and any other that does not grow is refused.
        a.count = 10;
        let at = |n: u32, a: &Authenticator| {
            let auth = a.auth_data(UP | UV, n);
            let client = a.client_data("webauthn.get", &CHALLENGE);
            let signature = a.sign(&auth, &client);
            (client, auth, signature)
        };
        let (c, d, s) = at(9, &a);
        assert_eq!(check_assertion(&c, &d, &s, &spki, 10, &expected()), Err(Refused::Counter), "lower");
        let (c, d, s) = at(10, &a);
        assert_eq!(check_assertion(&c, &d, &s, &spki, 10, &expected()), Err(Refused::Counter), "the same, not 0");
        let (c, d, s) = at(0, &a);
        assert_eq!(check_assertion(&c, &d, &s, &spki, 10, &expected()), Err(Refused::Counter), "back to 0");
        let (c, d, s) = at(11, &a);
        assert_eq!(check_assertion(&c, &d, &s, &spki, 10, &expected()), Ok(11));
        let (c, d, s) = at(0, &a);
        assert_eq!(check_assertion(&c, &d, &s, &spki, 0, &expected()), Ok(0));
    }

    #[test]
    fn a_dangerous_approvals_challenge_is_over_the_request_the_command_and_the_time() {
        let base = recheck_challenge("r1", "rm -rf /", 1000);
        assert_eq!(base, recheck_challenge("r1", "rm -rf /", 1000));
        assert_ne!(base, recheck_challenge("r2", "rm -rf /", 1000));
        assert_ne!(base, recheck_challenge("r1", "rm -rf /tmp", 1000));
        assert_ne!(base, recheck_challenge("r1", "rm -rf /", 1001));
    }

    #[test]
    fn verifying_is_well_under_the_limit() {
        let mut a = Authenticator::new(ORIGIN);
        let (client, auth, signature) = a.get(&CHALLENGE);
        let spki = a.spki();
        // A debug build is many times slower than the 5 ms of a release one: this only catches a verification that became a different kind of work.
        let began = std::time::Instant::now();
        for _ in 0..3 {
            check_assertion(&client, &auth, &signature, &spki, 0, &expected()).unwrap();
        }
        assert!(began.elapsed() < std::time::Duration::from_secs(5), "{:?}", began.elapsed());
    }
}
