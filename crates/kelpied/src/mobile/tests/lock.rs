//! The app lock at the daemon: real signatures from a software authenticator, over the real verbs and, where it matters, a real socket. Nothing here trusts a page.

use super::*;
use crate::mobile::risk;
use crate::mobile::webauthn::soft::Authenticator;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;

const ORIGIN: &str = "https://desk.tail1234.ts.net:8443";

fn b(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// The rig's phone as one that paired on a secure origin, on a page there: locked, with no passkey yet.
fn secure(d: &Daemon) {
    let key = d.mobile.mac_key(true).unwrap().unwrap();
    let mut c = d.mobile.config.lock();
    let phone = c.devices.iter_mut().find(|x| x.id == "phone").unwrap();
    phone.secure_origin = true;
    store::seal(phone, &key);
}

fn lock_rig() -> Rig {
    let mut r = rig();
    secure(&r.daemon);
    r.conn.lock_path = LockPath::Passkey;
    r.conn.secure_page = Some(ORIGIN.to_string());
    r.conn.locked = true;
    r.conn.lock_told = "enrol";
    r
}

fn device_of(r: &Rig) -> Device {
    r.daemon.mobile.config.lock().devices.iter().find(|x| x.id == "phone").unwrap().clone()
}

async fn challenge(r: &mut Rig) -> Vec<u8> {
    let reply = r.ask_write(json!({"type": "lock_challenge"})).await;
    B64.decode(reply["lock"]["challenge"].as_str().unwrap_or_else(|| panic!("no challenge in {reply}"))).unwrap()
}

fn enrol_message(a: &Authenticator, challenge: &[u8]) -> Value {
    let (client, auth) = a.create(challenge);
    json!({"type": "lock_enrol", "cred_id": b(&a.id), "spki": b(&a.spki()), "auth_data": b(&auth), "client_data": b(&client)})
}

fn unlock_message(a: &mut Authenticator, challenge: &[u8]) -> Value {
    let (client, auth, signature) = a.get(challenge);
    json!({"type": "lock_unlock", "auth_data": b(&auth), "client_data": b(&client), "signature": b(&signature)})
}

/// The phone with a passkey, unlocked by having just set it up.
async fn enrolled() -> (Rig, Authenticator) {
    let mut r = lock_rig();
    let a = Authenticator::new(ORIGIN);
    let c = challenge(&mut r).await;
    let reply = r.ask_write(enrol_message(&a, &c)).await;
    assert_eq!(reply["result"], "ok", "{reply}");
    let device = device_of(&r);
    r.conn.lock_change(&device, Permission::Write).await.expect("a new hello");
    (r, a)
}

async fn unlock(r: &mut Rig, a: &mut Authenticator) -> Value {
    let c = challenge(r).await;
    r.ask_write(unlock_message(a, &c)).await
}

const NOT_WHILE_LOCKED: [&str; 16] = ["list", "sub", "unsub", "chat_sub", "input", "key", "answer", "chat_answer", "chat_send", "chat_cancel", "chat_stop", "chat_resume", "push_info", "push_sub", "push_unsub", "push_test"];

#[tokio::test]
async fn while_locked_only_keeping_the_line_up_and_the_locks_own_verbs_are_answered() {
    let mut r = lock_rig();
    for name in NOT_WHILE_LOCKED {
        let reply = r.ask_write(json!({"type": name, "id": "t1", "key": "enter", "text": "hi", "cid": format!("c-{name}"), "waiting_since": 1, "endpoint": "x", "p256dh": "x", "auth": "x"})).await;
        assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("locked")), "{name}: {reply}");
        assert!(r.typed.writes().is_empty(), "{name}");
    }
    assert!(r.conn.followed.is_empty() && r.conn.watched.is_empty());
    for name in ["hb", "ack", "lock_challenge", "lock_now", "lock_touch", "lock_enrol", "lock_unlock"] {
        let reply = r.ask_write(json!({"type": name, "id": "t1", "rev": 1})).await;
        assert_ne!(code(&reply), Some("locked"), "{name}: {reply}");
    }
    // A name that is no verb is still that, locked or not.
    assert_eq!(code(&r.ask_write(json!({"type": "session.input"})).await), Some("unknown_verb"));
    // A retry of something done before the lock gets no first reply back: that reply is a session's.
    let (mut r, mut a) = enrolled().await;
    let since = crate::session::fake::wait_on_user(&r.term);
    let answer = json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1", "cid": "same"});
    assert_eq!(r.ask_write(answer.clone()).await["result"], "ok");
    r.daemon.mobile.lock_now("phone");
    let device = device_of(&r);
    r.conn.lock_change(&device, Permission::Write).await.expect("told");
    assert_eq!(code(&r.ask_write(answer.clone()).await), Some("locked"));
    assert_eq!(unlock(&mut r, &mut a).await["result"], "ok");
    assert_eq!(r.ask_write(answer).await["dup"], true, "unlocked, it is the repeat it was");
}

/// What the plan's first criterion says, at the daemon and over a real socket: a program holding the phone's Noise key, saying whatever it likes about its origin.
#[tokio::test]
async fn a_copied_key_on_a_real_socket_reaches_nothing_and_the_passkey_opens_it() {
    let network = NetworkRig::new().await;
    secure(&network.rig.daemon);
    let keys = network.rig.phone.clone();
    let mut owner = Authenticator::new(ORIGIN);
    let held = network.rig.daemon.clone();
    let m = &held.mobile;
    let c = m.lock_info("phone", "enrol", Some(ORIGIN))["challenge"].as_str().map(|c| B64.decode(c).unwrap()).unwrap();
    m.lock_enrol("phone", &enrol_message(&owner, &c), Some(ORIGIN)).map_err(|r| r.code).unwrap();
    m.lock_now("phone");

    let mut thief = network.socket_as(keys.clone(), false).await;
    thief.reconnect().await.expect("a paired key is answered");
    phone_send(&mut thief, json!({"caps": ["ack", "hb"]})).await.unwrap();
    let hello = phone_recv(&mut thief).await.unwrap();
    assert_eq!((hello["type"].as_str(), hello["locked"].as_bool(), hello["lock"]["state"].as_str()), (Some("hello"), Some(true), Some("unlock")));
    assert_eq!((hello["sessions"].clone(), hello["projects"].clone(), hello["rev"].clone(), hello["push"].clone()), (json!([]), json!({}), json!(0), Value::Null));
    // The random parts could hold anything, and the rest holds no session's name.
    let mut bare = hello.clone();
    for random in ["lock", "daemon_epoch"] {
        bare.as_object_mut().unwrap().remove(random);
    }
    let text = bare.to_string();
    assert!(!text.contains("t1") && !text.contains("c1"), "no session in it: {text}");
    assert_eq!(phone_recv(&mut thief).await.unwrap()["type"], "addrs");
    for (i, name) in NOT_WHILE_LOCKED.iter().enumerate() {
        phone_send(&mut thief, json!({"type": name, "id": "t1", "text": "x", "req": i})).await.unwrap();
        let reply = phone_recv(&mut thief).await.unwrap();
        assert_eq!((reply["code"].as_str(), reply["req"].as_u64()), (Some("locked"), Some(i as u64)), "{name}: {reply}");
    }
    // Its own passkey does not pass, on the challenge it was given.
    let c = B64.decode(hello["lock"]["challenge"].as_str().unwrap()).unwrap();
    let mut other = Authenticator::new(ORIGIN);
    phone_send(&mut thief, json!({"req": "forged", "type": "lock_unlock", "client_data": b(&other.get(&c).0), "auth_data": b(&other.get(&c).1), "signature": b(&other.get(&c).2)})).await.unwrap();
    assert_eq!(phone_recv(&mut thief).await.unwrap()["code"], "lock_refused");
    // Nor a new passkey in place of the owner's.
    phone_send(&mut thief, json!({"req": "again", "type": "lock_enrol", "cred_id": b(&other.id), "spki": b(&other.spki())})).await.unwrap();
    assert_eq!(phone_recv(&mut thief).await.unwrap()["code"], "insecure", "no page that could be one");
    assert!(!m.is_unlocked("phone"));
    assert_eq!(m.config.lock().devices[0].lock.as_ref().unwrap().spki, b(&owner.spki()));

    // The owner's signature, on the real connection: a fresh hello follows, with the sessions.
    phone_send(&mut thief, json!({"req": "new", "type": "lock_challenge"})).await.unwrap();
    let c = B64.decode(phone_recv(&mut thief).await.unwrap()["lock"]["challenge"].as_str().unwrap()).unwrap();
    let mut good = unlock_message(&mut owner, &c);
    good["req"] = json!("good");
    phone_send(&mut thief, good).await.unwrap();
    assert_eq!(phone_recv(&mut thief).await.unwrap()["result"], "ok");
    let fresh = phone_recv(&mut thief).await.unwrap();
    assert_eq!((fresh["type"].as_str(), fresh["locked"].as_bool(), fresh["lock"]["state"].as_str()), (Some("hello"), Some(false), Some("open")));
    assert_eq!(fresh["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(fresh["device"]["can_write"], true);

    // Going away and back inside the window asks nothing: a flap of the network.
    drop(thief);
    let mut back = network.socket_as(keys.clone(), false).await;
    back.reconnect().await.unwrap();
    phone_send(&mut back, json!({"caps": ["ack", "hb"]})).await.unwrap();
    let hello = phone_recv(&mut back).await.unwrap();
    assert_eq!((hello["locked"].as_bool(), hello["sessions"].as_array().unwrap().len()), (Some(false), 2));
    assert_eq!(phone_recv(&mut back).await.unwrap()["type"], "addrs");

    // Leaving locks at once, on this connection, and it stops hearing.
    phone_send(&mut back, json!({"type": "lock_now", "req": "left"})).await.unwrap();
    assert_eq!(phone_recv(&mut back).await.unwrap()["type"], "ack");
    let locked = phone_recv(&mut back).await.unwrap();
    assert_eq!((locked["type"].as_str(), locked["lock"]["state"].as_str()), (Some("locked"), Some("unlock")));
    assert!(locked["lock"]["challenge"].is_string(), "a new challenge, for the next try");
    drop(back);

    // After the window: locked again, on a new connection, with no other word from the phone.
    let c = m.lock_info("phone", "unlock", None)["challenge"].as_str().map(|c| B64.decode(c).unwrap()).unwrap();
    m.lock_unlock("phone", &unlock_message(&mut owner, &c)).map_err(|r| r.code).unwrap();
    assert!(m.is_unlocked("phone"));
    m.expire_window("phone");
    let mut late = network.socket_as(keys, false).await;
    late.reconnect().await.unwrap();
    phone_send(&mut late, json!({"caps": ["ack", "hb"]})).await.unwrap();
    assert_eq!(phone_recv(&mut late).await.unwrap()["locked"], true);
}

#[tokio::test]
async fn the_idle_time_stops_what_it_follows_and_controls_and_a_touch_moves_it() {
    let (mut r, mut a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    let device = device_of(&r);
    assert!(m.is_unlocked("phone"), "setting up a passkey unlocks");
    assert!(r.ask_write(json!({"type": "sub", "id": "t1"})).await.get("code").is_none());
    assert!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await.get("code").is_none());
    assert_eq!((r.conn.followed.len(), r.conn.watched.len()), (1, 1));
    let grant = m.grant("phone");
    assert!(r.conn.lock_change(&device, Permission::Write).await.is_none(), "nothing changed");

    // A touch moves the end of the window; on a locked phone it does nothing.
    m.expire_window("phone");
    m.lock_touch("phone");
    assert!(!m.is_unlocked("phone"), "a touch never unlocks");

    let told = r.conn.lock_change(&device, Permission::Write).await.expect("told");
    assert_eq!((told["type"].as_str(), told["lock"]["state"].as_str()), (Some("locked"), Some("unlock")));
    assert!(!grant.load(Ordering::SeqCst), "what it controls stops");
    assert!(r.conn.followed.is_empty() && r.conn.watched.is_empty() && r.conn.owed.is_empty(), "follows and chat watches stop");
    assert!(r.conn.publish(true).is_none(), "and the list goes nowhere");
    assert_eq!(code(&r.ask_write(json!({"type": "list"})).await), Some("locked"));

    // Unlocked again, the phone starts from a hello; the list has a new `rev` from there.
    assert_eq!(unlock(&mut r, &mut a).await["result"], "ok");
    let hello = r.conn.lock_change(&device, Permission::Write).await.expect("a new hello");
    assert_eq!((hello["type"].as_str(), hello["locked"].as_bool()), (Some("hello"), Some(false)));
    assert!(hello["sessions"].as_array().is_some_and(|s| s.len() == 2));
    m.lock_touch("phone");
    assert!(m.is_unlocked("phone"));

    // With no connection to notice, the sweep stops what it controls.
    let grant = m.grant("phone");
    m.expire_window("phone");
    m.lock_sweep();
    assert!(!grant.load(Ordering::SeqCst));
    assert!(!m.lock_memory_holds("phone"));
    m.lock_sweep();
}

#[tokio::test]
async fn a_challenge_is_used_once_and_for_a_minute() {
    let (mut r, mut a) = enrolled().await;
    r.daemon.mobile.lock_now("phone");
    let c = challenge(&mut r).await;
    let mut message = unlock_message(&mut a, &c);
    r.daemon.mobile.age_challenge("phone");
    assert_eq!(code(&r.ask_write(message.clone()).await), Some("challenge"), "too old");
    let c = challenge(&mut r).await;
    message = unlock_message(&mut a, &c);
    assert_eq!(r.ask_write(message.clone()).await["result"], "ok");
    r.daemon.mobile.lock_now("phone");
    assert_eq!(code(&r.ask_write(message).await), Some("challenge"), "used");
    // Asking again leaves only the newest.
    let first = challenge(&mut r).await;
    let _second = challenge(&mut r).await;
    assert_eq!(code(&r.ask_write(unlock_message(&mut a, &first)).await), Some("lock_refused"), "not the one it was asked for");
    assert!(!r.daemon.mobile.is_unlocked("phone"));
    // What failed counts, and a phone that keeps failing is told to wait.
    for _ in 0..9 {
        let c = challenge(&mut r).await;
        let mut bad = unlock_message(&mut a, &c);
        bad["signature"] = json!(b(b"nope"));
        assert_eq!(code(&r.ask_write(bad).await), Some("lock_refused"));
    }
    let c = challenge(&mut r).await;
    assert_eq!(code(&r.ask_write(unlock_message(&mut a, &c)).await), Some("too_many"));
    // And it is in the audit log, with nothing of what was sent.
    let log = r.daemon.mobile.audit_entries(50).to_string();
    assert!(log.contains("lock_refused") && log.contains("signature"));
    assert!(!log.contains(&b(&a.spki())));
}

#[tokio::test]
async fn replacing_a_passkey_needs_the_desktop_and_the_old_one_stops_when_it_does() {
    let (mut r, mut old) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    m.lock_now("phone");
    let device = device_of(&r);
    r.conn.lock_change(&device, Permission::Write).await.expect("told");
    let mine = Authenticator::new(ORIGIN);
    let c = challenge(&mut r).await;
    assert_eq!(code(&r.ask_write(enrol_message(&mine, &c)).await), Some("needs_desktop"));
    assert_eq!(m.config.lock().devices[0].lock.as_ref().unwrap().spki, b(&old.spki()));

    // The desktop opens the window: the phone hears, and a new passkey takes the old one's place. The phone stays locked until one passes.
    m.reset_lock("phone");
    let told = r.conn.lock_change(&device, Permission::Write).await.expect("told");
    assert_eq!(told["lock"]["state"], "enrol");
    assert_eq!(told["lock"]["rp_id"], "desk.tail1234.ts.net");
    assert_eq!(m.status(true)["devices"][0]["lock_reset"], true);
    let c = challenge(&mut r).await;
    assert_eq!(r.ask_write(enrol_message(&mine, &c)).await["result"], "ok");
    assert_eq!(m.status(true)["devices"][0]["lock_reset"], false, "once");
    m.lock_now("phone");
    assert_eq!(code(&r.ask_write(json!({"type": "list"})).await), Some("locked"));
    assert_eq!(code(&unlock(&mut r, &mut old).await), Some("lock_refused"), "the old passkey");

    // The window is used up: the next one is refused again.
    let mut mine = mine;
    assert_eq!(unlock(&mut r, &mut mine).await["result"], "ok");
    m.lock_now("phone");
    let third = Authenticator::new(ORIGIN);
    let c = challenge(&mut r).await;
    assert_eq!(code(&r.ask_write(enrol_message(&third, &c)).await), Some("needs_desktop"));
}

#[tokio::test]
async fn the_lock_is_never_shed_at_the_worst_load() {
    let (mut r, mut a) = enrolled().await;
    r.daemon.mobile.load.set_for_test(Level::L3);
    r.daemon.mobile.lock_now("phone");
    assert_eq!(code(&r.ask_write(json!({"type": "list"})).await), Some("locked"));
    assert_eq!(unlock(&mut r, &mut a).await["result"], "ok");
    assert!(r.ask_write(json!({"type": "list"})).await["sessions"].is_array());
    assert_eq!(r.ask_write(json!({"type": "lock_now"})).await["type"], "ack");
    assert!(!r.daemon.mobile.is_unlocked("phone"));
}

#[tokio::test]
async fn a_passkey_that_cannot_be_saved_is_not_taken_and_the_one_before_stands() {
    let (mut r, mut a) = enrolled().await;
    let spki = b(&a.spki());
    let m = &r.daemon.mobile;
    m.lock_now("phone");
    m.reset_lock("phone");
    // A folder where the file goes: nothing can be written over it.
    let path = store::path(&m.dir);
    std::fs::remove_file(&path).ok();
    std::fs::create_dir_all(&path).unwrap();
    let held = r.daemon.clone();
    let m = &held.mobile;
    let new = Authenticator::new(ORIGIN);
    let c = challenge(&mut r).await;
    assert_eq!(code(&r.ask_write(enrol_message(&new, &c)).await), Some("not_kept"));
    assert_eq!(m.config.lock().devices[0].lock.as_ref().unwrap().spki, spki);
    std::fs::remove_dir_all(&path).unwrap();
    assert_eq!(unlock(&mut r, &mut a).await["result"], "ok");
}

#[tokio::test]
async fn a_passkey_goes_with_a_page_that_can_have_one() {
    let mut r = lock_rig();
    let a = Authenticator::new("https://127.0.0.1:8443");
    r.conn.secure_page = Some("https://127.0.0.1:8443".into());
    let device = device_of(&r);
    r.conn.lock_told = "";
    assert_eq!(r.conn.lock_change(&device, Permission::Write).await.expect("told")["lock"]["state"], "no_name");
    assert!(r.ask_write(json!({"type": "lock_challenge"})).await["lock"].get("challenge").is_none(), "nothing to answer");
    assert_eq!(code(&r.ask_write(enrol_message(&a, &[1; 32])).await), Some("insecure"));
    r.conn.secure_page = None;
    assert_eq!(code(&r.ask_write(enrol_message(&a, &[1; 32])).await), Some("insecure"));
    // A device that paired over plain HTTP has no passkey to set up, wherever it says it is.
    let (key, device) = (r.daemon.mobile.mac_key(true).unwrap().unwrap(), "phone");
    {
        let mut c = r.daemon.mobile.config.lock();
        let phone = c.devices.iter_mut().find(|x| x.id == device).unwrap();
        phone.secure_origin = false;
        store::seal(phone, &key);
    }
    assert_eq!(r.daemon.mobile.lock_enrol(device, &json!({}), Some(ORIGIN)).map_err(|e| e.code).unwrap_err(), "insecure");
}

#[tokio::test]
async fn a_saved_passkey_that_does_not_verify_never_unlocks_and_says_the_desktop_can_help() {
    let (mut r, mut a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    m.lock_now("phone");
    m.config.lock().devices[0].lock.as_mut().unwrap().sign_count = 77;
    let device = device_of(&r);
    assert_eq!(r.conn.lock_change(&device, Permission::Write).await.expect("told")["lock"]["state"], "broken");
    assert_eq!(code(&unlock_with_a_challenge_of_its_own(&mut r, &mut a).await), Some("lock_broken"));
    assert!(!m.is_unlocked("phone"));
    // A key that cannot be read says nothing good about it either.
    *m.mac_key.lock() = None;
    m.secrets.delete(K_MAC).unwrap();
    assert_eq!(m.lock_state("phone", LockPath::Passkey, Gate::Locked, Some(ORIGIN)), "broken");
    assert_eq!(code(&unlock_with_a_challenge_of_its_own(&mut r, &mut a).await), Some("lock_broken"));
    assert!(!m.is_unlocked("phone"));
}

/// A phone in the `broken` state is given no challenge: this one is what a page that ignores that could send.
async fn unlock_with_a_challenge_of_its_own(r: &mut Rig, a: &mut Authenticator) -> Value {
    let held = r.daemon.clone();
    let m = &held.mobile;
    let c = m.lock_info("phone", "unlock", None)["challenge"].as_str().map(|c| B64.decode(c).unwrap()).unwrap();
    r.ask_write(unlock_message(a, &c)).await
}

#[tokio::test]
async fn lock_settings_edited_in_the_file_count_for_nothing() {
    let r = rig();
    let held = r.daemon.clone();
    let m = &held.mobile;
    assert!(m.lock_options().on, "on by default");
    // Written by hand: off, an hour, and no tag.
    m.config.lock().lock = Some(store::LockOptions { on: false, idle_s: 3600, recheck: false, plain_read: true, mac: String::new() });
    m.verify_devices();
    assert_eq!(m.lock_options(), store::LockOptions::tightest());
    m.config.lock().lock = None;
    // Set here it holds, across a read of the file.
    let mut next = m.lock_options();
    next.on = false;
    m.set_lock(next).unwrap();
    m.verify_devices();
    assert!(!m.lock_options().on);
    let saved = std::fs::read_to_string(store::path(&m.dir)).unwrap();
    let mut flipped: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(flipped["lock"]["on"], false);
    flipped["lock"]["idle_s"] = json!(3600);
    let again = Mobile::load(&m.dir, m.secrets.clone());
    assert!(!again.lock_options().on, "what was saved is what a new daemon reads");
    std::fs::write(store::path(&m.dir), flipped.to_string()).unwrap();
    let edited = Mobile::load(&m.dir, m.secrets.clone());
    assert_eq!(edited.lock_options(), store::LockOptions::tightest(), "an edit, and the tightest applies");
    // The key gone: the same.
    m.secrets.delete(K_MAC).unwrap();
    assert_eq!(Mobile::load(&m.dir, m.secrets.clone()).lock_options(), store::LockOptions::tightest());
}

/// The person chose more than the defaults, and a program of theirs edits the file or takes the settings out of it. A restart must not hand back less than they chose.
#[tokio::test]
async fn settings_tampered_with_never_come_back_weaker_than_the_person_set_them() {
    let (r, _a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    let path = store::path(&m.dir);
    assert_eq!(m.lock_options(), store::LockOptions::default(), "nobody has changed anything");
    let untouched = Mobile::load(&m.dir, m.secrets.clone());
    let defaults = store::LockOptions::default();
    assert!(!untouched.lock_options().weakened_by(&defaults) && !defaults.weakened_by(&untouched.lock_options()), "the first passkey wrote the settings, so a passkey with the defaults is no sign of an edit");
    assert!(m.config.lock().lock.is_some());

    let chosen = store::LockOptions { on: true, idle_s: 60, recheck: true, plain_read: false, mac: String::new() };
    m.set_lock(chosen.clone()).unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (what, edit) in [("a value changed", json!({"recheck": false})), ("another changed", json!({"plain_read": true})), ("a longer time", json!({"idle_s": 3600}))] {
        let mut changed = saved.clone();
        for (k, v) in edit.as_object().unwrap() {
            changed["lock"][k] = v.clone();
        }
        std::fs::write(&path, changed.to_string()).unwrap();
        let after = Mobile::load(&m.dir, m.secrets.clone()).lock_options();
        assert!(!chosen.weakened_by(&after), "{what}: {after:?}");
        assert!(after.on && after.recheck && !after.plain_read && after.idle_s == 60, "{what}: {after:?}");
    }
    // The settings taken out altogether, with a passkey on record.
    let mut gone = saved.clone();
    gone.as_object_mut().unwrap().remove("lock");
    std::fs::write(&path, gone.to_string()).unwrap();
    assert_eq!(Mobile::load(&m.dir, m.secrets.clone()).lock_options(), store::LockOptions::tightest());
    // The person can still go on from there, with the check that weakening has.
    std::fs::write(&path, saved.to_string()).unwrap();
    assert_eq!(Mobile::load(&m.dir, m.secrets.clone()).lock_options().idle_s, 60, "what was saved holds");
}

/// A phone paired from the link the daemon gives on a certificate listener, at a number: no passkey can exist there, so it must read rather than wait at a lock nothing can open.
#[tokio::test]
async fn a_phone_paired_at_a_number_reads_and_is_not_held_at_the_lock() {
    let mut r = rig();
    let held = r.daemon.clone();
    let m = &held.mobile;
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, HeaderValue::from_static("https://192.168.1.9:4817"));
    let peer = Peer::direct("192.168.1.20:50000".parse().unwrap(), true).with_headers(&headers, None);
    assert!(!peer.secure_origin, "what a pairing is recorded with");
    let key = m.mac_key(true).unwrap().unwrap();
    {
        let mut c = m.config.lock();
        let phone = c.devices.iter_mut().find(|x| x.id == "phone").unwrap();
        phone.secure_origin = peer.secure_origin;
        store::seal(phone, &key);
    }
    let device = device_of(&r);
    r.conn.lock_path = m.lock_path(&device, &peer);
    assert_eq!(r.conn.lock_path, LockPath::Plain);
    let hello = r.conn.hello(&device, Permission::Write).await;
    assert_eq!((hello["locked"].as_bool(), hello["lock"]["state"].as_str(), hello["device"]["can_write"].as_bool()), (Some(false), Some("read_only"), Some(false)));
    assert_eq!(hello["sessions"].as_array().unwrap().len(), 2);
    // Its record says it may type, so sending it to Settings would be no help: what stops it is where it is.
    let refused = r.ask_write(json!({"type": "input", "id": "t1", "text": "hi"})).await;
    assert_eq!(code(&refused), Some("read_only"));
    assert!(!refused["message"].as_str().unwrap().contains("Settings"), "{refused}");
    // At a name it is what it was: a passkey is asked for.
    let mut named = HeaderMap::new();
    named.insert(header::ORIGIN, HeaderValue::from_static("https://desk.example:4817"));
    assert!(Peer::direct("192.168.1.20:50000".parse().unwrap(), true).with_headers(&named, None).secure_origin);
    // A phone that paired at a number before this was known is told what helps, and turning the lock off lets it in.
    secure(&r.daemon);
    let stuck = device_of(&r);
    assert_eq!(m.lock_path(&stuck, &peer), LockPath::Passkey);
    assert_eq!(m.lock_state("phone", LockPath::Passkey, Gate::Locked, Some("https://192.168.1.9:4817")), "no_name");
    let mut off = m.lock_options();
    off.on = false;
    m.set_lock(off).unwrap();
    assert_eq!(m.lock_gate("phone", LockPath::Passkey), Gate::Open);
}

#[tokio::test]
async fn the_daemons_link_says_whether_a_phone_paired_at_it_can_have_a_passkey() {
    let network = NetworkRig::new().await;
    let made = network.rig.daemon.mobile.pair_start(false).unwrap();
    assert_eq!((made["tls"].as_bool(), made["passkey"].as_bool()), (Some(false), Some(false)), "plain HTTP");
}

fn hook(event: &str, tool: &str, command: &str) -> Value {
    json!({"session_id": "t1", "agent": "claude", "payload": {"hook_event_name": event, "tool_name": tool, "tool_input": {"command": command}}})
}

fn bare_hook(event: &str, extra: Value) -> Value {
    let mut payload = extra;
    payload["hook_event_name"] = json!(event);
    json!({"session_id": "t1", "agent": "claude", "payload": payload})
}

/// Through the daemon's own hook path, in the orders an agent with parallel tools can send them.
#[tokio::test]
async fn a_waits_label_stays_with_the_requests_it_is_about() {
    let r = rig();
    let d = &r.daemon;
    let wait = || r.term.screen_view(10).wait.expect("a wait");
    let level = || wait().risk.level;
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    crate::session::fake::wait_on_user(&r.term);
    assert_eq!((level(), wait().command.as_deref().map(str::to_string)), (risk::Level::Dangerous, Some("rm -rf build".to_string())));

    // A second request does not take the first one's place, whichever is milder.
    d.hook(&hook("PermissionRequest", "Bash", "ls")).unwrap();
    assert_eq!((level(), wait().command.as_deref().map(str::to_string)), (risk::Level::Dangerous, Some("rm -rf build\nls".to_string())));
    assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build\nls"), "what an assertion is over is what the card shows");

    // Another tool's work, in parallel or in a subagent, says nothing of the dialog that is open.
    d.hook(&hook("PreToolUse", "Bash", "npm test")).unwrap();
    d.hook(&hook("PostToolUse", "Bash", "npm test")).unwrap();
    d.hook(&bare_hook("Notification", json!({"notification_type": "permission_prompt"}))).unwrap();
    crate::session::fake::wait_on_user(&r.term);
    assert_eq!(level(), risk::Level::Dangerous, "the wait came back through a hook with no tool in it");

    // A tool that has run is no longer asked about, and it is that one that goes.
    d.hook(&hook("PostToolUse", "Bash", "ls")).unwrap();
    crate::session::fake::wait_on_user(&r.term);
    assert_eq!(wait().command.as_deref(), Some("rm -rf build"));

    // Nor does time: long after the hook, with the screen printing since, the status goes by the output, and the request is still open.
    crate::session::fake::wait_went_stale(&r.term);
    assert_eq!(r.term.info().status, crate::status::Status::Working, "what makes this the case in question");
    assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build"));

    // The end of the turn does.
    d.hook(&bare_hook("Stop", json!({}))).unwrap();
    assert_eq!(r.term.dangerous_wait(), None);
    let since = crate::session::fake::wait_on_user(&r.term);
    assert_eq!((wait().command, wait().risk.level, since > 0), (None, risk::Level::Unknown, true), "nothing left over for the next wait");

    // So does a new turn, for an agent stopped with no word.
    d.hook(&hook("PermissionRequest", "Bash", "git push")).unwrap();
    d.hook(&bare_hook("UserPromptSubmit", json!({}))).unwrap();
    assert_eq!(r.term.dangerous_wait(), None);
}

/// An answer does not say which dialog it was for. Whichever came up first, and whatever order the hooks reached the daemon in, the dangerous one keeps its label until its own hook settles it.
#[tokio::test]
async fn an_answer_cannot_take_the_label_off_the_dialog_still_open() {
    for (first, second) in [("ls", "rm -rf build"), ("rm -rf build", "ls")] {
        let mut r = rig();
        r.daemon.hook(&hook("PermissionRequest", "Bash", first)).unwrap();
        r.daemon.hook(&hook("PermissionRequest", "Bash", second)).unwrap();
        let since = crate::session::fake::wait_on_user(&r.term);
        let both = format!("{first}\n{second}");
        assert_eq!(r.term.screen_view(10).wait.unwrap().command.as_deref(), Some(both.as_str()), "both are on the card");
        // The harmless one's dialog is answered, in either order of arrival.
        assert_eq!(r.ask_write(json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1"})).await["result"], "ok");
        assert_eq!(r.term.dangerous_wait().as_deref(), Some(both.as_str()), "the answer settles nothing");
        // Its tool ran: that one goes, and the other stays dangerous, with what it is.
        r.daemon.hook(&hook("PostToolUse", "Bash", "ls")).unwrap();
        crate::session::fake::wait_on_user(&r.term);
        let wait = r.term.screen_view(10).wait.unwrap();
        assert_eq!((wait.risk.level, wait.command.as_deref()), (risk::Level::Dangerous, Some("rm -rf build")));
        assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build"));
    }
}

/// A prompt no hook spoke of comes first, with a request behind it. Answering that prompt says nothing of the request.
#[tokio::test]
async fn answering_some_other_prompt_leaves_a_request_behind_it_labelled() {
    let mut r = rig();
    r.daemon.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    let since = crate::session::fake::wait_on_user(&r.term);
    assert_eq!(r.ask_write(json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1"})).await["result"], "ok");
    assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build"));
}

#[tokio::test]
async fn a_tool_that_failed_is_done_with_and_a_tool_whose_report_does_not_match_stays() {
    let r = rig();
    let d = &r.daemon;
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    d.hook(&hook("PostToolUse", "Bash", "rm -rf something-else")).unwrap();
    assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build"), "not this one: it stays, which errs high");
    d.hook(&hook("PostToolUseFailure", "Bash", "rm -rf build")).unwrap();
    assert_eq!(r.term.dangerous_wait(), None);
}

#[tokio::test]
async fn the_same_request_twice_is_two_until_each_has_run() {
    let r = rig();
    let d = &r.daemon;
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    d.hook(&hook("PostToolUse", "Bash", "rm -rf build")).unwrap();
    assert_eq!(r.term.dangerous_wait().as_deref(), Some("rm -rf build"), "another agent's identical command has run; this dialog is still open");
    d.hook(&hook("PostToolUse", "Bash", "rm -rf build")).unwrap();
    assert_eq!(r.term.dangerous_wait(), None);
}

/// Built to make reading it explode. It reaches the card, and past the list's limit, and what comes of it is a label that says it was not read, soon.
#[tokio::test]
async fn a_command_built_to_be_slow_holds_nothing_up() {
    let r = rig();
    let began = std::time::Instant::now();
    r.daemon.hook(&hook("PermissionRequest", "Bash", &"find . -exec ".repeat(40))).unwrap();
    for i in 0..16 {
        r.daemon.hook(&hook("PermissionRequest", "Bash", &format!("{}{i}", "xargs ".repeat(10900)))).unwrap();
    }
    crate::session::fake::wait_on_user(&r.term);
    assert_eq!(r.term.screen_view(10).wait.unwrap().risk.level, risk::Level::Unknown);
    assert_eq!(r.term.dangerous_wait(), None);
    assert!(began.elapsed() < std::time::Duration::from_secs(30), "{:?}", began.elapsed());
}

#[tokio::test]
async fn a_full_list_of_requests_keeps_the_dangerous_one_whenever_it_came() {
    let r = rig();
    let d = &r.daemon;
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    for i in 0..40 {
        d.hook(&hook("PermissionRequest", "Bash", &format!("ls a{i}"))).unwrap();
    }
    assert!(r.term.dangerous_wait().is_some_and(|c| c.contains("rm -rf build")), "first in, and not pushed out");
    d.hook(&bare_hook("Stop", json!({}))).unwrap();
    for i in 0..40 {
        d.hook(&hook("PermissionRequest", "Bash", &format!("ls a{i}"))).unwrap();
    }
    d.hook(&hook("PermissionRequest", "Bash", "rm -rf build")).unwrap();
    assert!(r.term.dangerous_wait().is_some_and(|c| c.contains("rm -rf build")), "last in, with the list full");
}

#[tokio::test]
async fn weakening_the_lock_is_for_the_verified_app_and_the_person_and_tightening_needs_no_prompt() {
    let r = rig();
    let d = r.daemon.clone();
    let held = d.clone();
    let m = &held.mobile;
    let (tx, mut replies) = app_client(&d, 74);
    let (user_tx, _user_rx) = crate::session::client_channel();
    d.set_client_role(75, &crate::auth::Role::User);
    no_prompt_expected(&d);
    secure(&d);
    for method in ["mobile.set_lock", "mobile.reset_lock"] {
        assert!(crate::auth::APP_ONLY.contains(&method), "{method}");
        assert!(crate::auth::user_may(&crate::auth::Role::User, method).is_err(), "{method}");
        assert!(d.dispatch(75, &user_tx, &json!(1), method, json!({"on": false, "id": "phone"})).unwrap_err().to_string().contains("Kelpie app"), "{method}");
    }
    assert!(m.lock_options().on);
    // Tightening asks nobody.
    assert_eq!(d.dispatch(74, &tx, &json!(2), "mobile.set_lock", json!({"idle_s": 60, "recheck": true, "plain_read": false})).unwrap().unwrap()["idle_s"], 60);
    assert_eq!(m.status(true)["lock"], json!({"on": true, "idle_s": 60, "recheck": true, "plain_read": false}));
    // Values not on offer are refused with no prompt.
    assert!(d.dispatch(74, &tx, &json!(3), "mobile.set_lock", json!({"idle_s": 7})).is_err());
    assert!(d.dispatch(74, &tx, &json!(3), "mobile.set_lock", json!({"on": "off"})).is_err());
    // Refused by the person, nothing changes. Twice: a third in a row stops the prompts for a while.
    for weaker in [json!({"on": false}), json!({"idle_s": 900})] {
        let before = m.lock_options();
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        assert!(ask_app(&d, &tx, &mut replies, 74, "mobile.set_lock", weaker.clone()).await["error"].is_object(), "{weaker}");
        assert_eq!(m.lock_options(), before, "{weaker}");
    }
    // Each way of weakening asks, and passed, it holds.
    d.presence.inject(|reason| if reason.contains("app lock") { Ok(()) } else { Err(reason.to_string()) });
    for (weaker, field, value) in [(json!({"idle_s": 900}), "idle_s", json!(900)), (json!({"recheck": false}), "recheck", json!(false)), (json!({"plain_read": true}), "plain_read", json!(true)), (json!({"on": false}), "on", json!(false))] {
        assert_eq!(ask_app(&d, &tx, &mut replies, 74, "mobile.set_lock", weaker.clone()).await["result"][field], value, "{weaker}");
        assert_eq!(m.status(true)["lock"][field], value, "{weaker}");
    }
    // The reset, and its prompt: the phone by name, with no way to one the daemon does not hold.
    let unknown_phone = d.dispatch(74, &tx, &json!(4), "mobile.reset_lock", json!({"id": "nobody"}));
    assert!(unknown_phone.is_err());
    d.presence.inject(|reason| if reason.contains("Phone") && reason.contains("passkey") { Ok(()) } else { Err(reason.to_string()) });
    assert_eq!(ask_app(&d, &tx, &mut replies, 74, "mobile.reset_lock", json!({"id": "phone"})).await["result"], true);
    assert_eq!(m.status(true)["devices"][0]["lock_reset"], true);
}

#[tokio::test]
async fn a_setting_applies_on_the_next_message() {
    let (mut r, _a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    let device = device_of(&r);
    // A shorter idle time holds for the window open now.
    let mut next = m.lock_options();
    next.idle_s = 60;
    m.set_lock(next).unwrap();
    assert!(m.is_unlocked("phone"));
    // Off: every message passes, and the phone is told it has no lock to pass.
    let mut off = m.lock_options();
    off.on = false;
    m.set_lock(off.clone()).unwrap();
    assert_eq!(r.conn.lock_change(&device, Permission::Write).await.expect("told")["lock"]["state"], "none");
    assert!(r.ask_write(json!({"type": "list"})).await["sessions"].is_array());
    // On again: locked at once, the window forgotten.
    off.on = true;
    m.set_lock(off).unwrap();
    assert_eq!(code(&r.ask_write(json!({"type": "list"})).await), Some("locked"));
    assert_eq!(r.conn.lock_change(&device, Permission::Write).await.expect("told")["type"], "locked");
}

#[tokio::test]
async fn a_device_that_never_saw_a_secure_origin_may_read_at_most_over_the_network_and_that_can_be_closed() {
    let mut r = rig();
    let held = r.daemon.clone();
    let m = &held.mobile;
    let device = device_of(&r);
    // From this machine over plain HTTP: as it would be with no lock.
    assert_eq!(m.lock_gate("phone", LockPath::Local), Gate::Open);
    // Over the network: a write permission counts for reading only, though the transport would allow it.
    r.conn.lock_path = LockPath::Plain;
    assert_eq!(m.lock_gate("phone", LockPath::Plain), Gate::ReadOnly);
    let hello = r.conn.hello(&device, Permission::Write).await;
    assert_eq!((hello["locked"].as_bool(), hello["lock"]["state"].as_str(), hello["device"]["can_write"].as_bool(), hello["device"]["capped"].as_bool()), (Some(false), Some("read_only"), Some(false), Some(true)));
    assert!(hello["lock"].get("challenge").is_none());
    assert!(r.ask_write(json!({"type": "list"})).await["sessions"].is_array());
    assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "hi"})).await), Some("read_only"));
    assert!(r.typed.writes().is_empty());
    assert_eq!(code(&r.ask_write(json!({"type": "lock_enrol"})).await), Some("insecure"));
    // Closed by the switch: nothing reads, and there is no ceremony to try.
    let mut closed = m.lock_options();
    closed.plain_read = false;
    m.set_lock(closed).unwrap();
    let told = r.conn.lock_change(&device, Permission::Write).await.expect("told");
    assert_eq!((told["type"].as_str(), told["lock"]["state"].as_str()), (Some("locked"), Some("closed")));
    assert_eq!(code(&r.ask_write(json!({"type": "list"})).await), Some("locked"));
    assert_eq!(code(&r.ask_write(json!({"type": "push_test"})).await), Some("locked"));
    // A connection is judged by its record, not by what it says: a page paired on a secure origin is locked even over plain HTTP from this machine.
    let peer = Peer::direct("127.0.0.1:1".parse().unwrap(), false);
    let plain = device_of(&r);
    assert_eq!(m.lock_path(&plain, &peer), LockPath::Local);
    secure(&r.daemon);
    assert_eq!(m.lock_path(&device_of(&r), &peer), LockPath::Passkey);
    let tailnet = Peer::direct("100.64.1.2:1".parse().unwrap(), false);
    assert_eq!(m.lock_path(&plain, &tailnet), LockPath::Plain, "a Tailscale address is not a secure context");
    assert_eq!(m.lock_path(&plain, &Peer::direct("127.0.0.1:1".parse().unwrap(), true)), LockPath::Plain);
}

#[tokio::test]
async fn revoking_a_reset_or_turning_access_off_forgets_the_window() {
    let (r, _a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    assert!(m.is_unlocked("phone"));
    m.disable();
    assert!(!m.is_unlocked("phone"), "access off");
    m.config.lock().enabled = true;
    m.open_window_for_test("phone");
    m.revoke("phone").unwrap();
    assert!(!m.is_unlocked("phone"), "revoked");
}

fn permission_request(id: &str, command: &str) -> String {
    json!({"type": "control_request", "request_id": id, "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": command, "description": "nothing to see here"}}}).to_string()
}

fn allow_line(id: &str) -> String {
    json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow"}}}).to_string()
}

fn proof(a: &mut Authenticator, request: &str, command: &str, expiry: u64) -> Value {
    let (client, auth, signature) = a.get(&webauthn::recheck_challenge(request, command, expiry));
    json!({"expiry": expiry, "auth_data": b(&auth), "client_data": b(&client), "signature": b(&signature)})
}

#[tokio::test]
async fn with_the_recheck_on_a_dangerous_approval_needs_a_fresh_assertion_over_what_is_approved() {
    let (mut r, mut a) = enrolled().await;
    let held = r.daemon.clone();
    let m = &held.mobile;
    let command = "rm -rf build";
    r.chat.ingest_for_test(permission_request("r1", command));
    let answer = |id: &str, proof: Option<Value>| {
        let mut v = json!({"type": "chat_answer", "id": "c1", "request_id": id, "line": allow_line(id)});
        if let Some(p) = proof {
            v["recheck"] = p;
        }
        v
    };
    // Off, as it is by default: no second look.
    r.chat.ingest_for_test(permission_request("off", command));
    assert_eq!(r.ask_write(answer("off", None)).await["result"], "ok");

    let mut on = m.lock_options();
    on.recheck = true;
    m.set_lock(on).unwrap();
    let refused = r.ask_write(answer("r1", None)).await;
    assert_eq!((code(&refused), refused["result"].as_str()), (Some("recheck"), Some("refused")), "{refused}");
    let soon = now_millis() + 60_000;
    // Each thing the challenge is over.
    assert_eq!(code(&r.ask_write(answer("r1", Some(proof(&mut a, "r1", "rm -rf other", soon)))).await), Some("lock_refused"), "another command");
    assert_eq!(code(&r.ask_write(answer("r1", Some(proof(&mut a, "r2", command, soon)))).await), Some("lock_refused"), "another request");
    let mut said_later = proof(&mut a, "r1", command, soon);
    said_later["expiry"] = json!(soon + 1);
    assert_eq!(code(&r.ask_write(answer("r1", Some(said_later))).await), Some("lock_refused"), "another time than it was signed for");
    assert_eq!(code(&r.ask_write(answer("r1", Some(proof(&mut a, "r1", command, now_millis() - 1)))).await), Some("recheck"), "past");
    assert_eq!(code(&r.ask_write(answer("r1", Some(proof(&mut a, "r1", command, now_millis() + 3_600_000)))).await), Some("recheck"), "too far ahead to be fresh");
    assert_eq!(code(&r.ask_write(answer("r1", Some(proof(&mut Authenticator::new(ORIGIN), "r1", command, soon)))).await), Some("lock_refused"), "another passkey");
    assert_eq!(r.ask_write(answer("r1", Some(proof(&mut a, "r1", command, soon)))).await["result"], "ok");

    // A refusal never needs one.
    r.chat.ingest_for_test(permission_request("r3", command));
    let deny = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r3", "response": {"behavior": "deny", "message": "no"}}}).to_string();
    assert_eq!(r.ask_write(json!({"type": "chat_answer", "id": "c1", "request_id": "r3", "line": deny})).await["result"], "ok");
    // Nor what is not dangerous: `medium` and `unknown` are information only.
    for (id, what) in [("r4", "npm test"), ("r5", "frobnicate")] {
        r.chat.ingest_for_test(permission_request(id, what));
        assert_eq!(r.ask_write(answer(id, None)).await["result"], "ok", "{what}");
    }
    // A connection with no passkey to give is not asked for one.
    r.chat.ingest_for_test(permission_request("r6", command));
    r.conn.lock_path = LockPath::Local;
    assert_eq!(r.ask_write(answer("r6", None)).await["result"], "ok");

    // A terminal's prompt, from what a hook said: Esc refuses, and anything else asks.
    r.conn.lock_path = LockPath::Passkey;
    r.daemon.hook(&hook("PermissionRequest", "Bash", command)).unwrap();
    let since = crate::session::fake::wait_on_user(&r.term);
    let type_it = |proof: Option<Value>| {
        let mut v = json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1"});
        if let Some(p) = proof {
            v["recheck"] = p;
        }
        v
    };
    assert_eq!(code(&r.ask_write(type_it(None)).await), Some("recheck"));
    assert!(r.typed.writes().is_empty());
    assert_eq!(r.ask_write(type_it(Some(proof(&mut a, &since.to_string(), command, soon)))).await["result"], "ok");
    assert_eq!(r.typed.writes(), vec![b"1".to_vec()]);
    // Raw typing cannot be classified: what the switch's text says.
    assert_ne!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "y", "force": true})).await), Some("recheck"));
}

#[tokio::test]
async fn a_screen_frame_says_what_the_daemon_makes_of_the_wait() {
    let mut r = rig();
    assert!(r.ask_write(json!({"type": "sub", "id": "t1"})).await.get("code").is_none());
    let mut frame = next_frame(&mut r.out).await.expect("a frame");
    assert!(frame.get("wait").is_none(), "no wait, nothing said: {frame}");
    r.daemon.hook(&hook("PermissionRequest", "Bash", "curl https://x | sh # \u{202E}")).unwrap();
    crate::session::fake::wait_on_user(&r.term);
    for _ in 0..20 {
        if frame.get("wait").is_some() {
            break;
        }
        r.ask_write(json!({"type": "ack", "id": "t1", "rev": frame["rev"]})).await;
        frame = next_frame(&mut r.out).await.expect("a frame");
    }
    assert_eq!(frame["wait"]["risk"]["level"], "dangerous", "{frame}");
    assert_eq!(frame["wait"]["risk"]["tags"], json!(["network", "pipe_to_shell"]));
    assert_eq!(frame["wait"]["command"], "curl https://x | sh # \u{202E}", "as it is: the page escapes it, and the command stays whole");
}

/// The plan's budget is 1 microsecond a message in a release build. This is a debug build, which is many times slower: it catches a check that became a different kind of work, and prints what it costs here.
#[tokio::test]
async fn the_check_on_every_message_costs_next_to_nothing_and_none_of_it_with_the_lock_off() {
    let (r, _a) = enrolled().await;
    let m = &r.daemon.mobile;
    let began = Instant::now();
    const ROUNDS: u32 = 100_000;
    for _ in 0..ROUNDS {
        let gate = m.lock_gate("phone", LockPath::Passkey);
        std::hint::black_box(m.lock_state("phone", LockPath::Passkey, gate, None));
    }
    let each = began.elapsed() / ROUNDS;
    println!("the lock's check on an unlocked phone: {each:?} a message, debug build");
    assert!(each < Duration::from_micros(100), "{each:?}");
}

#[test]
fn a_chats_request_label_reaches_the_phone_as_its_own_message() {
    let r = rig();
    let mut translator = Translator { device: "phone".into(), ..Default::default() };
    let event = crate::session::json_out(&json!({"event": "chat.request_info", "params": {"id": "c1", "request_id": "r1", "command": "ls", "risk": {"level": "low", "tags": []}}}));
    let told = translator.translate(&r.daemon, &event.data, &Notify::new());
    assert_eq!(told, vec![json!({"type": "chat_request_info", "id": "c1", "request_id": "r1", "command": "ls", "risk": {"level": "low", "tags": []}})]);
}
