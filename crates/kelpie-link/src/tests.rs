use super::*;

fn keys() -> (Keypair, Keypair) {
    (Keypair::generate().unwrap(), Keypair::generate().unwrap())
}

/// What the desktop learnt from a first message.
struct Seen {
    mode: u8,
    psk_index: Option<usize>,
    remote: [u8; 32],
}

/// A paired phone and desktop: (phone session, desktop session, what the desktop learnt).
fn pair(desktop: &Keypair, phone: &Keypair, psks: &[[u8; 32]], use_psk: &[u8; 32]) -> Result<(Session, Session, Seen), LinkError> {
    let (init, first) = Initiator::pair(phone, &desktop.public, use_psk)?;
    let opened = read_first(desktop, &first, psks)?;
    let shown = Seen { mode: opened.mode, psk_index: opened.psk_index, remote: opened.remote };
    let (desk, reply) = opened.accept()?;
    let ph = init.finish(&reply)?;
    Ok((ph, desk, shown))
}

#[test]
fn a_pairing_finds_its_code_among_the_live_ones_and_both_ends_show_the_same_six_digits() {
    let (desktop, phone) = keys();
    let codes = [random_key().unwrap(), random_key().unwrap(), random_key().unwrap()];
    let (ph, desk, opened) = pair(&desktop, &phone, &codes, &codes[1]).unwrap();
    assert_eq!(opened.psk_index, Some(1));
    assert_eq!(opened.mode, MODE_PAIR);
    assert_eq!(opened.remote, phone.public, "the desktop learns the phone's static key");
    assert_eq!(ph.handshake_hash(), desk.handshake_hash());
    let code = ph.pair_code();
    assert_eq!(code.len(), 6);
    assert!(code.chars().all(|c| c.is_ascii_digit()));
    assert_eq!(code, desk.pair_code());
}

#[test]
fn a_wrong_or_spent_code_decrypts_nothing_and_leaves_the_live_ones_usable() {
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let wrong = random_key().unwrap();
    let (_, first) = Initiator::pair(&phone, &desktop.public, &wrong).unwrap();
    assert_eq!(read_first(&desktop, &first, &live).unwrap_err(), LinkError::BadMac, "its mac1 says so before any Diffie-Hellman");
    assert_eq!(read_first(&desktop, &first, &[]).unwrap_err(), LinkError::BadMac, "no live code: nothing to try");
    // The live code still works afterwards: reading never spends anything.
    assert!(pair(&desktop, &phone, &live, &live[0]).is_ok());
    // A first message made for another desktop key does not decrypt here either.
    let other = Keypair::generate().unwrap();
    let (_, first) = Initiator::pair(&phone, &other.public, &live[0]).unwrap();
    assert_eq!(read_first(&desktop, &first, &live).unwrap_err(), LinkError::Decrypt);
}

#[test]
fn a_reconnect_names_the_phones_key_and_the_reply_is_written_only_on_accept() {
    let (desktop, phone) = keys();
    let (init, first) = Initiator::reconnect(&phone, &desktop.public).unwrap();
    assert_eq!((first[0], first[1]), (VERSION, MODE_RECONNECT));
    let opened = read_first(&desktop, &first, &[]).unwrap();
    assert_eq!(opened.psk_index, None);
    assert_eq!(opened.remote, phone.public);
    // An unknown key is simply not answered: the phone's handshake then times out.
    let (mut desk, reply) = opened.accept().unwrap();
    let mut ph = init.finish(&reply).unwrap();
    let frames = ph.seal(br#"{"caps":["ack","hb"]}"#).unwrap();
    assert_eq!(desk.open(&frames[0], 0).unwrap().unwrap(), br#"{"caps":["ack","hb"]}"#);
    // A reconnect message cannot pass as a pairing one or the other way round (the mode is in the prologue).
    let mut swapped = first.clone();
    swapped[1] = MODE_PAIR;
    assert!(read_first(&desktop, &swapped, &[random_key().unwrap()]).is_err());
    let live = [random_key().unwrap()];
    let (_, pairing) = Initiator::pair(&phone, &desktop.public, &live[0]).unwrap();
    let mut swapped = pairing.clone();
    swapped[1] = MODE_RECONNECT;
    assert!(read_first(&desktop, &swapped, &live).is_err());
    assert_eq!(read_first(&desktop, &[9, 1, 2, 3], &live).unwrap_err(), LinkError::Malformed);
    assert_eq!(read_first(&desktop, &[], &live).unwrap_err(), LinkError::Malformed);
    let mut unknown_mode = pairing.clone();
    unknown_mode[1] = 9;
    assert_eq!(read_first(&desktop, &unknown_mode, &live).unwrap_err(), LinkError::Malformed);
}

#[test]
fn a_reply_from_anyone_but_the_desktop_the_phone_named_is_refused() {
    let (desktop, phone) = keys();
    let impostor = Keypair::generate().unwrap();
    let (init, first) = Initiator::reconnect(&phone, &desktop.public).unwrap();
    // The impostor cannot even read the first message: it was made for the desktop's key.
    assert!(read_first(&impostor, &first, &[]).is_err());
    // Nor can it answer one of its own making in the phone's name.
    let (_, other_first) = Initiator::reconnect(&Keypair::generate().unwrap(), &impostor.public).unwrap();
    let (_, reply) = read_first(&impostor, &other_first, &[]).unwrap().accept().unwrap();
    assert!(init.finish(&reply).is_err());
}

#[test]
fn altered_repeated_or_reordered_messages_end_the_session() {
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let a = ph.seal(b"one").unwrap().remove(0);
    let b = ph.seal(b"two").unwrap().remove(0);
    let c = ph.seal(b"three").unwrap().remove(0);
    let mut tampered = a.clone();
    tampered[5] ^= 1;
    assert_eq!(desk.open(&tampered, 0).unwrap_err(), LinkError::Decrypt);
    // Fresh pair for each case: a failed read leaves the session unusable on purpose.
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let a = ph.seal(b"one").unwrap().remove(0);
    let b2 = ph.seal(b"two").unwrap().remove(0);
    assert_eq!(desk.open(&a, 0).unwrap().unwrap(), b"one");
    assert_eq!(desk.open(&a, 0).unwrap_err(), LinkError::Decrypt, "replayed");
    let _ = (b, c, b2);
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let _a = ph.seal(b"one").unwrap().remove(0);
    let b = ph.seal(b"two").unwrap().remove(0);
    assert_eq!(desk.open(&b, 0).unwrap_err(), LinkError::Decrypt, "reordered");
    // Frames of a session from another handshake never open in this one.
    let (mut ph1, _, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let (_, mut desk2, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let old = ph1.seal(b"from an old session").unwrap().remove(0);
    assert_eq!(desk2.open(&old, 0).unwrap_err(), LinkError::Decrypt);
    assert_eq!(desk2.open(&[0u8; 5], 0).unwrap_err(), LinkError::Malformed);
}

#[test]
fn messages_up_to_a_mebibyte_go_in_chunks_that_each_fit_one_noise_message() {
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    let big: Vec<u8> = (0..MAX_APP_MESSAGE).map(|i| (i % 251) as u8).collect();
    let frames = desk.seal(&big).unwrap();
    assert_eq!(frames.len(), MAX_APP_MESSAGE.div_ceil(MAX_CHUNK));
    assert!(frames.iter().all(|f| f.len() <= MAX_NOISE_MESSAGE));
    let mut got = None;
    for (i, f) in frames.iter().enumerate() {
        let r = ph.open(f, 0).unwrap();
        assert_eq!(r.is_some(), i == frames.len() - 1);
        got = r.or(got);
    }
    assert_eq!(got.unwrap(), big);
    assert_eq!(desk.seal(&vec![0u8; MAX_APP_MESSAGE + 1]).unwrap_err(), LinkError::TooLarge);
    // An empty message is one frame.
    let empty = desk.seal(b"").unwrap();
    assert_eq!(empty.len(), 1);
    assert_eq!(ph.open(&empty[0], 0).unwrap().unwrap(), b"");
    // A message whose chunks stop coming is given up on after 10 s.
    let two = desk.seal(&vec![7u8; MAX_CHUNK + 10]).unwrap();
    assert!(ph.open(&two[0], 1000).unwrap().is_none());
    assert!(!ph.reassembly_expired(1000 + REASSEMBLY_TIMEOUT_MS));
    assert!(ph.reassembly_expired(1001 + REASSEMBLY_TIMEOUT_MS));
    assert_eq!(ph.open(&two[1], 1001 + REASSEMBLY_TIMEOUT_MS).unwrap_err(), LinkError::Reassembly);
}

#[test]
fn both_ends_move_to_new_keys_at_the_same_message_by_counting_them() {
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    desk.set_rekey_every(3);
    ph.set_rekey_every(3);
    for i in 0..20u32 {
        let m = format!("message {i}");
        for f in desk.seal(m.as_bytes()).unwrap() {
            assert_eq!(ph.open(&f, 0).unwrap().unwrap(), m.as_bytes());
        }
        for f in ph.seal(m.as_bytes()).unwrap() {
            assert_eq!(desk.open(&f, 0).unwrap().unwrap(), m.as_bytes());
        }
    }
    // The keys really changed: an end that did not count along cannot read past the switch.
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    desk.set_rekey_every(1);
    let first = desk.seal(b"a").unwrap().remove(0);
    let second = desk.seal(b"b").unwrap().remove(0);
    assert_eq!(ph.open(&first, 0).unwrap().unwrap(), b"a");
    assert_eq!(ph.open(&second, 0).unwrap_err(), LinkError::Decrypt);
    // Chunks count as messages too.
    let (mut ph, mut desk, _) = pair(&desktop, &phone, &live, &live[0]).unwrap();
    desk.set_rekey_every(2);
    ph.set_rekey_every(2);
    let long = vec![3u8; MAX_CHUNK * 3 + 1];
    let mut got = None;
    for f in desk.seal(&long).unwrap() {
        got = ph.open(&f, 0).unwrap();
    }
    assert_eq!(got.unwrap(), long);
    assert_eq!(REKEY_EVERY_MESSAGES, 1 << 16);
}

#[test]
fn garbage_and_other_versions_stop_before_any_diffie_hellman() {
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let (_, reconnect) = Initiator::reconnect(&phone, &desktop.public).unwrap();
    let (_, pairing) = Initiator::pair(&phone, &desktop.public, &live[0]).unwrap();
    assert_eq!(prefilter(&desktop.public, &reconnect, &[]), Ok(Prefiltered { mode: MODE_RECONNECT, psk_index: None }));
    assert_eq!(prefilter(&desktop.public, &pairing, &[[7u8; 32], live[0]]), Ok(Prefiltered { mode: MODE_PAIR, psk_index: Some(1) }));
    // Any changed bit of the message or the tag.
    for at in [2usize, FIRST_HEADER, reconnect.len() - 1] {
        let mut bad = reconnect.clone();
        bad[at] ^= 1;
        assert_eq!(prefilter(&desktop.public, &bad, &[]), Err(LinkError::BadMac), "byte {at}");
    }
    // Keyed from this desktop's public key: a first message for another computer fails it.
    assert_eq!(prefilter(&phone.public, &reconnect, &[]), Err(LinkError::BadMac));
    // Random bytes of the right size.
    let mut noise = vec![VERSION, MODE_RECONNECT];
    noise.extend((0..96).map(|i| (i * 37) as u8));
    assert_eq!(prefilter(&desktop.public, &noise, &[]), Err(LinkError::BadMac));
    // A lower or an unknown version is refused outright, whatever the rest says; a message cannot be relabelled either,
    // because the version is under the MAC and in the prologue.
    for version in [0u8, VERSION + 1, 255] {
        let mut other = reconnect.clone();
        other[0] = version;
        assert_eq!(prefilter(&desktop.public, &other, &[]), Err(LinkError::Version), "{version}");
        assert_eq!(read_first(&desktop, &other, &[]).unwrap_err(), LinkError::Version);
    }
    assert_eq!(MIN_VERSION, VERSION, "nothing older exists yet");
    assert_eq!(prologue(1, MODE_PAIR), b"kelpie-link/1\x01");
    assert_ne!(prologue(2, MODE_PAIR), prologue(1, MODE_PAIR));
}

#[test]
fn the_qr_payload_round_trips_and_refuses_anything_else() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..200 {
        let mut dpk = [0u8; 32];
        let mut psk = [0u8; 32];
        dpk.iter_mut().chain(psk.iter_mut()).for_each(|b| *b = next() as u8);
        let mut endpoints: Vec<Endpoint> = (0..(next() % 5)).map(|i| Endpoint::lan(&format!("192.168.{}.{}:4817", next() % 256, i))).collect();
        if next() % 2 == 0 {
            endpoints.push(Endpoint { kind: "relay".into(), addr: "wss://relay.example/v1".into(), room: Some("r".repeat((next() % 20) as usize)) });
        }
        let name: String = (0..(next() % 30)).map(|_| char::from_u32(0x41 + (next() % 26) as u32).unwrap()).chain(["ệ", "😀"].into_iter().flat_map(str::chars)).collect();
        let p = QrPayload { dpk, psk, endpoints, name };
        let text = p.encode();
        assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "url-safe");
        assert_eq!(QrPayload::decode(&text).unwrap(), p);
    }
    for bad in ["", "!!!", &b64(b"{}"), &b64(br#"{"v":2,"dpk":"","psk":""}"#), &b64(br#"{"v":1,"dpk":"AAAA","psk":"AAAA"}"#)] {
        assert_eq!(QrPayload::decode(bad).unwrap_err(), LinkError::Malformed, "{bad}");
    }
    let p = QrPayload { dpk: [1; 32], psk: [2; 32], endpoints: vec![], name: "n".into() };
    assert!(!format!("{p:?}").contains(&b64(&[2; 32])), "Debug never shows the secret");
    let k = Keypair::generate().unwrap();
    assert!(!format!("{k:?}").contains(&b64(&k.private)));
}

#[test]
fn a_first_message_or_reply_that_carries_anything_is_not_ours() {
    // Noise IK's first message is replayable: the protocol never puts anything in it, and refuses one that does.
    let (desktop, phone) = keys();
    let live = [random_key().unwrap()];
    let params: snow::params::NoiseParams = PAIR_PATTERN.parse().unwrap();
    let mut hs = snow::Builder::new(params).local_private_key(&phone.private).unwrap().remote_public_key(&desktop.public).unwrap().prologue(b"kelpie-link/1\x01").unwrap().psk(1, &live[0]).unwrap().build_initiator().unwrap();
    let mut msg = vec![0u8; MAX_NOISE_MESSAGE];
    let n = hs.write_message(b"a command", &mut msg).unwrap();
    let tag = mac1_state(&mac1_key(MAC1_LABEL_PSK, &live[0]), VERSION, MODE_PAIR, &msg[..n]).finalize().into_bytes();
    let mut first = vec![VERSION, MODE_PAIR];
    first.extend_from_slice(&tag);
    first.extend_from_slice(&msg[..n]);
    assert_eq!(read_first(&desktop, &first, &live).unwrap_err(), LinkError::Malformed);
    // A reply with a payload is refused by the phone too.
    let (init, first) = Initiator::reconnect(&phone, &desktop.public).unwrap();
    let params: snow::params::NoiseParams = RECONNECT_PATTERN.parse().unwrap();
    let mut resp = snow::Builder::new(params).local_private_key(&desktop.private).unwrap().prologue(b"kelpie-link/1\x02").unwrap().build_responder().unwrap();
    resp.read_message(&first[FIRST_HEADER..], &mut msg).unwrap();
    let n = resp.write_message(b"surprise", &mut msg).unwrap();
    assert_eq!(init.finish(&msg[..n]).unwrap_err(), LinkError::Malformed);
}

#[test]
fn the_six_digits_are_hkdf_of_the_handshake_hash() {
    // HKDF-BLAKE2s (RFC 5869) by hand: extract with a zero salt, expand one block with info "kelpie-sas".
    let h = [7u8; 32];
    let mac = |k: &[u8], parts: &[&[u8]]| {
        let mut m = <SimpleHmac<Blake2s256> as Mac>::new_from_slice(k).unwrap();
        for p in parts {
            m.update(p);
        }
        m.finalize().into_bytes()
    };
    let prk = mac(&[], &[&h]);
    let t1 = mac(&prk, &[b"kelpie-sas", &[1]]);
    assert_eq!(pair_code(&h), format!("{:06}", u32::from_be_bytes([t1[0], t1[1], t1[2], t1[3]]) % 1_000_000));
    assert_ne!(pair_code(&h), pair_code(&[8u8; 32]));
}

// ---------- vectors shared with the phone (TypeScript) and the e2e scripts (Python) ----------

fn hx(b: &[u8]) -> String {
    hex::encode(b)
}

fn fixed(n: u8) -> [u8; 32] {
    let mut k = [0u8; 32];
    for (i, b) in k.iter_mut().enumerate() {
        *b = n.wrapping_mul(31).wrapping_add(i as u8);
    }
    k
}

fn make_vectors() -> serde_json::Value {
    let desktop = testing::keypair_from(fixed(1));
    let phone = testing::keypair_from(fixed(2));
    let (pe, de, psk) = (fixed(3), fixed(4), fixed(5));
    let mut out = serde_json::json!({
        "comment": "Generated by `KELPIE_LINK_WRITE_VECTORS=1 cargo test -p kelpie-link vectors`. Checked by kelpie-link (Rust), app/phone (noise.test.ts) and scripts/e2e/link_noise.py.",
        "desktop_private": hx(&desktop.private), "desktop_public": hx(&desktop.public),
        "phone_private": hx(&phone.private), "phone_public": hx(&phone.public),
        "phone_ephemeral": hx(&pe), "desktop_ephemeral": hx(&de), "psk": hx(&psk),
    });
    for (name, mode) in [("pair", MODE_PAIR), ("reconnect", MODE_RECONNECT)] {
        let psk_opt = (mode == MODE_PAIR).then_some(&psk);
        let (init, first) = testing::initiator_with_ephemeral(mode, &phone, &desktop.public, psk_opt, &pe).unwrap();
        let psks = if mode == MODE_PAIR { vec![fixed(9), psk] } else { vec![] };
        let opened = testing::read_first_with_ephemeral(&desktop, &first, &psks, &de).unwrap();
        let (mut desk, reply) = opened.accept().unwrap();
        let mut ph = init.finish(&reply).unwrap();
        desk.set_rekey_every(2);
        ph.set_rekey_every(2);
        let to_phone: Vec<String> = ["hello phone", "second", "third", "fourth", "fifth"].iter().map(|m| hx(&desk.seal(m.as_bytes()).unwrap()[0])).collect();
        let to_desktop: Vec<String> = ["hello desktop", "again", "third"].iter().map(|m| hx(&ph.seal(m.as_bytes()).unwrap()[0])).collect();
        out[name] = serde_json::json!({
            "first": hx(&first), "reply": hx(&reply),
            "hash": hx(desk.handshake_hash()), "code": desk.pair_code(),
            "rekey_every": 2, "to_phone": to_phone, "to_phone_plain": ["hello phone", "second", "third", "fourth", "fifth"],
            "to_desktop": to_desktop, "to_desktop_plain": ["hello desktop", "again", "third"],
        });
    }
    out
}

#[test]
fn vectors() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("vectors.json");
    let made = make_vectors();
    if std::env::var_os("KELPIE_LINK_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&made).unwrap() + "\n").unwrap();
    }
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("tests/vectors.json")).unwrap();
    assert_eq!(saved, made, "the shared vectors changed: regenerate them and check the phone and the e2e scripts");
}
