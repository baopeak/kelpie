//! The relay at the daemon, against a stand-in relay on this machine: what it is told first, what a slot is let do, and what it refuses.
//! The stand-in forwards nothing by itself: the test plays the relay and the phone on its side of the socket.

use super::*;
use crate::mobile::relay::{self as relay_side, KEYS};
use futures_util::{SinkExt, StreamExt};
use p256::ecdsa::signature::Verifier;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response as Handshake};
use tokio_tungstenite::tungstenite::Message as WsMessage;

const ORIGIN: &str = "https://desk.tail1234.ts.net:8443";
const ROOM: &str = "abababababababababababababababab";
type RelaySocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// A relay that takes desktops and hands each one's socket to the test, with the address it asked for. While `refuse` is set it turns every
/// desktop away (503), as a relay out of reach would.
struct FakeRelay {
    url: String,
    accepted: mpsc::Receiver<(String, RelaySocket)>,
    refuse: Arc<std::sync::atomic::AtomicBool>,
}

// The handshake callback's error type is tungstenite's own.
#[allow(clippy::result_large_err)]
async fn fake_relay() -> FakeRelay {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (tx, accepted) = mpsc::channel(4);
    let refuse = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let refusing = refuse.clone();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let asked = Arc::new(Mutex::new(String::new()));
            let seen = asked.clone();
            let refusing = refusing.clone();
            let callback = move |req: &Request, resp: Handshake| -> std::result::Result<Handshake, ErrorResponse> {
                if refusing.load(Ordering::SeqCst) {
                    let mut no = ErrorResponse::new(None);
                    *no.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::SERVICE_UNAVAILABLE;
                    return Err(no);
                }
                *seen.lock() = req.uri().to_string();
                Ok(resp)
            };
            if let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await {
                let asked = asked.lock().clone();
                let _ = tx.send((asked, ws)).await;
            }
        }
    });
    FakeRelay { url, accepted, refuse }
}

fn room_key() -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap()
}

/// The rig with the relay on at the stand-in (its phone paired on a secure origin when `secure`), and phone access running.
async fn relay_rig(fake: &FakeRelay, secure: bool, on: bool) -> NetworkRig {
    let r = rig();
    let m = &r.daemon.mobile;
    let key = m.mac_key(true).unwrap().unwrap();
    if secure {
        let mut c = m.config.lock();
        let phone = c.devices.iter_mut().find(|x| x.id == "phone").unwrap();
        phone.secure_origin = true;
        store::seal(phone, &key);
    }
    m.secrets.set(KEYS[0], &hex(&room_key().to_bytes())).unwrap();
    let mut settings = store::RelaySettings { on, url: fake.url.clone(), room: Some(ROOM.into()), ticket: Some(format!("{ROOM}.ticket")), origin: Some(ORIGIN.into()), ..Default::default() };
    store::seal_relay(&mut settings, &key);
    m.config.lock().relay = Some(settings);
    let dir = m.dir.clone();
    let mut network = NetworkRig { rig: r, dir, addr: "127.0.0.1:0".parse().unwrap() };
    network.restart().await;
    network
}

async fn desktop_socket(fake: &mut FakeRelay) -> (String, RelaySocket) {
    tokio::time::timeout(Duration::from_secs(5), fake.accepted.recv()).await.expect("the daemon dialled the relay").unwrap()
}

async fn next(ws: &mut RelaySocket) -> Option<WsMessage> {
    match tokio::time::timeout(Duration::from_secs(3), ws.next()).await {
        Ok(Some(Ok(WsMessage::Text(t)))) if t.as_str() == "ping" => Box::pin(next(ws)).await,
        Ok(Some(Ok(msg))) => Some(msg),
        _ => None,
    }
}

async fn next_text(ws: &mut RelaySocket) -> Value {
    match next(ws).await {
        Some(WsMessage::Text(t)) => serde_json::from_str(t.as_str()).unwrap(),
        other => panic!("expected the relay's word, got {other:?}"),
    }
}

async fn open_slot(ws: &mut RelaySocket, slot: u16) {
    ws.send(WsMessage::text(json!({"t": "open", "s": slot, "via": "token"}).to_string())).await.unwrap();
}

async fn to_slot(ws: &mut RelaySocket, slot: u16, bytes: &[u8]) {
    let mut framed = slot.to_be_bytes().to_vec();
    framed.extend_from_slice(bytes);
    ws.send(WsMessage::binary(framed)).await.unwrap();
}

/// What came for `slot`: a frame, or the daemon's kick with its code.
enum ForSlot {
    Frame(Vec<u8>),
    Kick(u16),
}

/// The next thing the daemon says about `slot`; what it says about other slots, and its allow-lists, go to `others`.
async fn for_slot(ws: &mut RelaySocket, slot: u16, others: &mut Vec<Value>) -> Option<ForSlot> {
    loop {
        match next(ws).await? {
            WsMessage::Binary(b) if b.len() >= 2 && u16::from_be_bytes([b[0], b[1]]) == slot => return Some(ForSlot::Frame(b[2..].to_vec())),
            WsMessage::Text(t) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap();
                if v["t"] == "kick" && v["s"] == slot {
                    return Some(ForSlot::Kick(v["c"].as_u64().unwrap() as u16));
                }
                others.push(v);
            }
            _ => {}
        }
    }
}

/// A reconnect through the relay: the session, or the code the slot was closed with.
async fn reconnect(ws: &mut RelaySocket, slot: u16, keys: &kelpie_link::Keypair, desktop: &[u8; 32], others: &mut Vec<Value>) -> std::result::Result<kelpie_link::Session, u16> {
    open_slot(ws, slot).await;
    let (init, first) = kelpie_link::Initiator::reconnect(keys, desktop).unwrap();
    to_slot(ws, slot, &first).await;
    match for_slot(ws, slot, others).await {
        Some(ForSlot::Frame(reply)) => Ok(init.finish(&reply).unwrap()),
        Some(ForSlot::Kick(code)) => Err(code),
        None => Err(0),
    }
}

async fn say(ws: &mut RelaySocket, slot: u16, session: &mut kelpie_link::Session, value: Value) {
    for frame in session.seal(value.to_string().as_bytes()).unwrap() {
        to_slot(ws, slot, &frame).await;
    }
}

/// The next whole message the daemon sealed for `slot`, or the code it was closed with.
async fn hear(ws: &mut RelaySocket, slot: u16, session: &mut kelpie_link::Session, others: &mut Vec<Value>) -> std::result::Result<Value, u16> {
    loop {
        match for_slot(ws, slot, others).await {
            Some(ForSlot::Frame(f)) => {
                if let Some(bytes) = session.open(&f, now_millis()).unwrap() {
                    return Ok(serde_json::from_slice(&bytes).unwrap());
                }
            }
            Some(ForSlot::Kick(code)) => return Err(code),
            None => return Err(0),
        }
    }
}

#[tokio::test]
async fn the_relay_hears_first_who_may_come_in_and_from_where_and_the_desktop_signs_in_fresh() {
    let mut fake = fake_relay().await;
    let _network = relay_rig(&fake, true, true).await;
    let (asked, mut ws) = desktop_socket(&mut fake).await;
    let query: HashMap<String, String> = asked.split_once('?').unwrap().1.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    assert!(asked.starts_with("/v1/desktop?"), "{asked}");
    assert_eq!(query["t"], format!("{ROOM}.ticket"));
    let ts: u64 = query["ts"].parse().unwrap();
    assert!(now_millis().abs_diff(ts) < 60_000);
    let sig = p256::ecdsa::Signature::from_slice(&kelpie_link::unb64(&query["sig"]).unwrap()).unwrap();
    assert!(room_key().verifying_key().verify(relay_side::statement(ROOM, ts).as_bytes(), &sig).is_ok(), "signed with the room key");
    assert_eq!(next_text(&mut ws).await, json!({"t": "allow", "h": []}), "nobody has a token yet");
    assert_eq!(next_text(&mut ws).await, json!({"t": "origin", "o": ORIGIN}));
    // Once more, as after a drop: a new signature, later than the last, and the list first again.
    drop(ws);
    let (again, mut ws) = desktop_socket(&mut fake).await;
    let later: u64 = again.split("ts=").nth(1).unwrap().split('&').next().unwrap().parse().unwrap();
    assert!(later > ts);
    assert_eq!(next_text(&mut ws).await["t"], "allow");
}

#[tokio::test]
async fn with_the_relay_off_nothing_dials_out() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, false).await;
    assert!(tokio::time::timeout(Duration::from_secs(2), fake.accepted.recv()).await.is_err(), "no socket while it is off");
    assert!(network.rig.daemon.mobile.relay_endpoint().is_none());
    let told: Value = serde_json::from_str(&addrs_message(&network.rig.daemon.mobile.addrs.borrow())).unwrap();
    assert!(!told["endpoints"].as_array().unwrap().iter().any(|e| e["t"] == "relay"), "phones are not told of a relay that is off: {told}");
}

#[tokio::test]
async fn garbage_on_a_slot_is_dropped_unanswered_and_costs_no_diffie_hellman() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    open_slot(&mut ws, 1).await;
    to_slot(&mut ws, 1, &[0x01, 0x02, 0x03].repeat(40)).await;
    let mut first = vec![kelpie_link::VERSION, kelpie_link::MODE_RECONNECT];
    first.extend_from_slice(&[9u8; 16 + 48]);
    open_slot(&mut ws, 2).await;
    to_slot(&mut ws, 2, &first).await;
    assert!(tokio::time::timeout(Duration::from_millis(600), ws.next()).await.is_err(), "no reply, no kick, nothing");
    let budget = network.rig.daemon.mobile.handshake_budget.lock().tokens;
    assert!(budget >= HANDSHAKES_PER_SECOND - 0.5, "the budget was not spent: {budget}");
}

#[tokio::test]
async fn a_page_paired_over_plain_http_is_turned_away_at_the_relay() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, false, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    let desktop = network.rig.daemon.mobile.link_key().unwrap().public;
    let mut others = Vec::new();
    assert_eq!(reconnect(&mut ws, 0, &network.rig.phone, &desktop, &mut others).await.err(), Some(link::RELAY_REFUSED), "refused before any reply");
}

#[tokio::test]
async fn a_secure_phone_works_through_a_slot_gets_its_token_and_is_closed_when_removed() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    let desktop = network.rig.daemon.mobile.link_key().unwrap().public;
    let phone = network.rig.phone.clone();
    let mut others = Vec::new();

    // A client with no heartbeat has no place on the relay.
    let mut session = reconnect(&mut ws, 3, &phone, &desktop, &mut others).await.unwrap();
    say(&mut ws, 3, &mut session, json!({"caps": ["ack"]})).await;
    assert_eq!(hear(&mut ws, 3, &mut session, &mut others).await.err(), Some(link::RELAY_REFUSED));

    // One that answers heartbeats and can use the relay: the hello, where to reach the computer, then its token.
    let mut session = reconnect(&mut ws, 4, &phone, &desktop, &mut others).await.unwrap();
    say(&mut ws, 4, &mut session, json!({"caps": ["ack", "hb", "relay"]})).await;
    let hello = hear(&mut ws, 4, &mut session, &mut others).await.unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["device"]["permission"], "write", "a secure device types through the relay: {hello}");
    let addrs = hear(&mut ws, 4, &mut session, &mut others).await.unwrap();
    assert!(addrs["endpoints"].as_array().unwrap().iter().any(|e| e["t"] == "relay" && e["r"] == format!("{ROOM}.ticket")), "{addrs}");
    let given = hear(&mut ws, 4, &mut session, &mut others).await.unwrap();
    assert_eq!(given["type"], "relay");
    let token = kelpie_link::unb64(given["token"].as_str().unwrap()).unwrap();
    let hash = relay_side::token_hash(&token);
    // The relay is told the new list.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([hash])) && Instant::now() < deadline {
        if let Some(WsMessage::Text(t)) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await.ok().flatten().and_then(|m| m.ok()) {
            others.push(serde_json::from_str(t.as_str()).unwrap());
        }
    }
    assert!(others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([hash])), "{others:?}");

    // Removed on the computer: it is told inside the session and its slot closed, and only then does the relay get the list without it. A
    // real relay closes a socket whose token left the list at once, so the other order would lose the sealed `revoked`.
    others.clear();
    network.rig.daemon.mobile.revoke("phone").unwrap();
    let mut told_revoked = false;
    loop {
        match hear(&mut ws, 4, &mut session, &mut others).await {
            Ok(v) if v["type"] == "revoked" => told_revoked = true,
            Ok(_) => {}
            Err(code) => {
                assert_eq!(code, NOT_PAIRED);
                break;
            }
        }
        assert!(!others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([])), "the list went first: {others:?}");
    }
    assert!(told_revoked);
    assert!(!others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([])), "the list went before the slot was closed: {others:?}");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([])) && Instant::now() < deadline {
        if let Some(WsMessage::Text(t)) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await.ok().flatten().and_then(|m| m.ok()) {
            others.push(serde_json::from_str(t.as_str()).unwrap());
        }
    }
    assert!(others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([])), "{others:?}");
}

/// A phone through the relay, connected and greeted on `slot`: its session.
async fn greeted(ws: &mut RelaySocket, network: &NetworkRig, slot: u16, others: &mut Vec<Value>) -> kelpie_link::Session {
    let desktop = network.rig.daemon.mobile.link_key().unwrap().public;
    let mut session = reconnect(ws, slot, &network.rig.phone, &desktop, others).await.unwrap();
    say(ws, slot, &mut session, json!({"caps": ["ack", "hb", "relay"]})).await;
    assert_eq!(hear(ws, slot, &mut session, others).await.unwrap()["type"], "hello");
    session
}

/// What reaches the relay until its socket closes: the relay's word, and frames as `{"frame": slot}`.
async fn until_closed(ws: &mut RelaySocket) -> Vec<Value> {
    let mut seen = Vec::new();
    while let Some(msg) = next(ws).await {
        match msg {
            WsMessage::Text(t) => seen.push(serde_json::from_str(t.as_str()).unwrap()),
            WsMessage::Binary(b) if b.len() >= 2 => seen.push(json!({"frame": u16::from_be_bytes([b[0], b[1]])})),
            WsMessage::Close(_) => break,
            _ => {}
        }
    }
    seen
}

#[tokio::test]
async fn the_kill_switch_tells_relay_phones_first_then_lets_nobody_in() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    let mut others = Vec::new();
    let mut session = greeted(&mut ws, &network, 5, &mut others).await;
    for _ in 0..2 {
        hear(&mut ws, 5, &mut session, &mut others).await.unwrap();
    }
    network.rig.daemon.mobile.revoke_all();
    let mut told_revoked = false;
    let code = loop {
        match hear(&mut ws, 5, &mut session, &mut others).await {
            Ok(v) => told_revoked |= v["type"] == "revoked",
            Err(code) => break code,
        }
    };
    assert!(told_revoked, "the phone hears it was removed, inside its session");
    assert_eq!(code, NOT_PAIRED);
    let rest = until_closed(&mut ws).await;
    assert_eq!(rest.iter().rfind(|v| v["t"] == "allow"), Some(&json!({"t": "allow", "h": []})), "then nobody is let in: {rest:?}");
    assert!(!rest.iter().any(|v| v["t"] == "drop"), "turning off keeps the room");
}

#[tokio::test]
async fn the_kill_switch_while_the_relay_is_out_of_reach_lets_nobody_in_once_it_answers() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, ws) = desktop_socket(&mut fake).await;
    // The relay goes out of reach: the connection drops, and the next ones are turned away.
    fake.refuse.store(true, Ordering::SeqCst);
    drop(ws);
    tokio::time::sleep(Duration::from_millis(400)).await;
    network.rig.daemon.mobile.revoke_all();
    fake.refuse.store(false, Ordering::SeqCst);
    let (asked, mut ws) = desktop_socket(&mut fake).await;
    assert!(asked.starts_with(&format!("/v1/desktop?t={ROOM}.ticket&")), "signed into the same room: {asked}");
    let said = until_closed(&mut ws).await;
    assert_eq!(said.iter().rfind(|v| v["t"] == "allow"), Some(&json!({"t": "allow", "h": []})), "nobody is let in: {said:?}");
    assert!(!said.iter().any(|v| v["t"] == "drop"), "and the room stays");
    assert!(tokio::time::timeout(Duration::from_millis(1500), fake.accepted.recv()).await.is_err(), "said once, then nothing dials out");
}

/// A phone through the relay with a token: its session, and the hash the relay was told.
async fn greeted_with_token(ws: &mut RelaySocket, network: &NetworkRig, slot: u16, others: &mut Vec<Value>) -> (kelpie_link::Session, String) {
    let mut session = greeted(ws, network, slot, others).await;
    assert!(hear(ws, slot, &mut session, others).await.unwrap()["endpoints"].is_array());
    let given = hear(ws, slot, &mut session, others).await.unwrap();
    assert_eq!(given["type"], "relay");
    let hash = relay_side::token_hash(&kelpie_link::unb64(given["token"].as_str().unwrap()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(3);
    while !others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([hash])) && Instant::now() < deadline {
        if let Some(WsMessage::Text(t)) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await.ok().flatten().and_then(|m| m.ok()) {
            others.push(serde_json::from_str(t.as_str()).unwrap());
        }
    }
    assert!(others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([hash])), "{others:?}");
    (session, hash)
}

#[tokio::test]
async fn a_list_asked_for_just_after_a_removal_waits_until_the_removed_phone_heard_it() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    let mut others = Vec::new();
    let (mut session, _) = greeted_with_token(&mut ws, &network, 6, &mut others).await;
    others.clear();
    let m = &network.rig.daemon.mobile;
    m.revoke("phone").unwrap();
    // As another phone's new token would: the list is asked for at once.
    m.relay.nudge();
    loop {
        let heard = hear(&mut ws, 6, &mut session, &mut others).await;
        assert!(!others.iter().any(|v| v["t"] == "allow" && v["h"] == json!([])), "the list went first: {others:?}");
        if let Err(code) = heard {
            assert_eq!(code, NOT_PAIRED);
            break;
        }
    }
}

/// A phone's first message on `slot`, which came in with `cred`, answered by the daemon; then the phone closes the socket with `code`.
async fn answered_then_closed(ws: &mut RelaySocket, phone: &kelpie_link::Keypair, desktop: &[u8; 32], slot: u16, cred: &str, code: u16) {
    ws.send(WsMessage::text(json!({"t": "open", "s": slot, "via": "token", "k": cred}).to_string())).await.unwrap();
    let (_, first) = kelpie_link::Initiator::reconnect(phone, desktop).unwrap();
    to_slot(ws, slot, &first).await;
    assert!(matches!(for_slot(ws, slot, &mut Vec::new()).await, Some(ForSlot::Frame(_))), "the reply");
    ws.send(WsMessage::text(json!({"t": "close", "s": slot, "c": code}).to_string())).await.unwrap();
}

#[tokio::test]
async fn a_relay_phone_whose_other_route_won_is_no_failed_attempt_and_a_silent_one_counts_against_its_token() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    let d = network.rig.daemon.clone();
    let desktop = d.mobile.link_key().unwrap().public;
    let cred = "ab".repeat(32);
    let ip = relay_side::phone_ip(0, Some(&cred));
    let failures = || d.mobile.failures.lock().get(&ip).map_or(0, VecDeque::len);
    // One phone's sockets, each on a slot of its own: answered, then closed with the phone's code, which the relay passes on.
    answered_then_closed(&mut ws, &network.rig.phone, &desktop, 1, &cred, link::ROUTE_LOST).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(failures(), 0, "another route won: nothing held a place for nothing");
    answered_then_closed(&mut ws, &network.rig.phone, &desktop, 2, &cred, 1000).await;
    let deadline = Instant::now() + Duration::from_secs(3);
    while failures() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(failures(), 1, "counted against the token it came in with, not against its slot");
}

#[tokio::test]
async fn the_relays_addresses_never_empty_the_lockouts_of_the_network() {
    let r = rig();
    let m = &r.daemon.mobile;
    for i in 0..LIMITER_ADDRESSES {
        m.pairing_failed(IpAddr::from([10, 1, (i / 256) as u8, (i % 256) as u8]));
    }
    for slot in 0..50 {
        assert!(!m.pairing_failed(relay_side::slot_ip(slot)), "no place, and no lockout");
    }
    let failures = m.failures.lock();
    assert_eq!(failures.len(), LIMITER_ADDRESSES);
    assert!(failures.contains_key(&IpAddr::from([10, 1, 0, 0])), "every address from the network is still counted");
}

#[tokio::test]
async fn a_reset_removes_the_room_with_the_keys() {
    let mut fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let (_, mut ws) = desktop_socket(&mut fake).await;
    next_text(&mut ws).await;
    next_text(&mut ws).await;
    network.rig.daemon.mobile.reset();
    let rest = until_closed(&mut ws).await;
    assert!(rest.iter().any(|v| v["t"] == "drop"), "{rest:?}");
    assert!(network.rig.daemon.mobile.relay_room().is_none(), "nothing of the room is kept here either");
}

#[tokio::test]
async fn a_key_typed_with_no_room_to_make_is_not_kept() {
    let fake = fake_relay().await;
    let network = relay_rig(&fake, true, true).await;
    let m = &network.rig.daemon.mobile;
    let asked = relay_side::check_turn_on(m, &json!({"on": true, "url": fake.url, "operator_key": "typed-but-never-checked"})).unwrap();
    relay_side::turn_on(&network.rig.daemon, asked).unwrap();
    assert_eq!(m.secrets.read(KEYS[1]).unwrap(), None, "no room was made, so the relay never saw the key");
    assert!(m.relay_live().is_some());
}

#[tokio::test]
async fn a_phone_whose_other_route_won_is_no_failed_attempt_and_a_silent_one_still_is() {
    let network = NetworkRig::new().await;
    let d = network.rig.daemon.clone();
    d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
    let failures = |d: &Daemon| d.mobile.failures.lock().get(&network.addr.ip()).map_or(0, VecDeque::len);
    let desktop = d.mobile.link_key().unwrap().public;
    // The loser of the phone's race: answered, then closed with the code that says so.
    let (_, first) = kelpie_link::Initiator::reconnect(&network.rig.phone, &desktop).unwrap();
    let mut loser = network.websocket().await;
    ws_send(&mut loser.tcp, 2, &first).await.unwrap();
    assert_eq!(ws_recv(&mut loser.tcp).await.unwrap().0, 2);
    ws_send(&mut loser.tcp, 8, &link::ROUTE_LOST.to_be_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(failures(&d), 0, "another route won: nothing held a place for nothing");
    // Closed with any other code after the reply, it still counts.
    let (_, first) = kelpie_link::Initiator::reconnect(&network.rig.phone, &desktop).unwrap();
    let mut quiet = network.websocket().await;
    ws_send(&mut quiet.tcp, 2, &first).await.unwrap();
    assert_eq!(ws_recv(&mut quiet.tcp).await.unwrap().0, 2);
    ws_send(&mut quiet.tcp, 8, &1000u16.to_be_bytes()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while failures(&d) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(failures(&d), 1);
}
