//! Taking over a browser pane through a phone's own connection (`Conn::handle`), against the broker and an app that provides panes: what the
//! record, the transport and the lock allow, and what ends a hold.

use super::*;

const ORIGIN: &str = "https://desk.tail1234.ts.net:8443";
const APP: u64 = 700;

/// An app that provides browser panes on `d`: it answers every request, and keeps what it was told without one.
fn app(d: &Arc<Daemon>) -> Arc<Mutex<Vec<Value>>> {
    let (tx, mut rx) = crate::session::client_channel();
    d.providers.register(APP, &crate::auth::Role::App, "browser", None, &tx).unwrap();
    let notes = Arc::new(Mutex::new(Vec::new()));
    let (d, kept) = (d.clone(), notes.clone());
    tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            let m: Value = serde_json::from_slice(&o.data[5..]).unwrap();
            match m["preq"].as_u64() {
                Some(id) => {
                    let result = match m["method"].as_str() {
                        Some("browser.panes") => json!({"panes": [{"pane": "browser-a", "title": "Sign in", "host": "accounts.example", "project": "p1", "project_name": "Shop"}]}),
                        Some("browser.takeover_start") => json!({"title": "Sign in", "host": "accounts.example"}),
                        _ => json!(true),
                    };
                    d.providers.reply(APP, &json!({"preq": id, "result": result}));
                }
                None => kept.lock().push(m),
            }
        }
    });
    notes
}

async fn told(notes: &Mutex<Vec<Value>>, method: &str) -> Vec<Value> {
    tokio::time::sleep(Duration::from_millis(50)).await;
    notes.lock().iter().filter(|n| n["pnote"] == method).map(|n| n["params"].clone()).collect()
}

/// Waits up to `limit` for the app to be told to stop.
async fn stopped(notes: &Mutex<Vec<Value>>, limit: Duration) -> Option<Value> {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if let Some(s) = notes.lock().iter().find(|n| n["pnote"] == "browser.takeover_stop") {
            return Some(s["params"].clone());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

/// The rig's phone with `permission` and `kind`, sealed.
fn record(r: &Rig, permission: Permission, kind: Kind, secure: bool, passkey: bool) {
    let key = r.daemon.mobile.mac_key(true).unwrap().unwrap();
    let mut c = r.daemon.mobile.config.lock();
    let d = c.devices.iter_mut().find(|d| d.id == "phone").unwrap();
    d.permission = permission;
    d.kind = kind;
    d.secure_origin = secure;
    d.lock = passkey.then(|| store::LockCredential { cred_id: "c".into(), spki: "s".into(), origin: ORIGIN.into(), sign_count: 0, created_at: 0, mac: String::new() });
    store::seal(d, &key);
}

/// The rig's connection as this computer's HTTPS page, unlocked by its passkey.
fn unlocked_page(r: &mut Rig) {
    r.conn.lock_path = LockPath::Passkey;
    r.conn.page_origin = Some(ORIGIN.into());
    r.conn.secure_page = Some(ORIGIN.into());
    r.conn.resume = Some(r.daemon.mobile.open_window_for_test("phone"));
    r.conn.locked = false;
}

async fn start(r: &mut Rig) -> String {
    let reply = r.ask(Permission::Control, json!({"type": "tk_start", "pane": "browser-a"})).await;
    reply["tk"].as_str().unwrap_or_else(|| panic!("no takeover: {reply}")).to_string()
}

#[tokio::test]
async fn a_phone_takes_a_pane_over_through_its_connection_and_nothing_it_types_is_noted() {
    let mut r = rig();
    record(&r, Permission::Control, Kind::App, false, false);
    let notes = app(&r.daemon);
    let panes = r.ask(Permission::Control, json!({"type": "panes"})).await;
    assert_eq!(panes["panes"][0]["pane"], "browser-a", "{panes}");
    let tk = start(&mut r).await;
    r.daemon.takeover.on_event(&r.daemon, APP, &json!({"kind": "frame", "tk": tk, "data": "jpeg", "meta": {"deviceWidth": 800, "deviceHeight": 600, "window_id": 3}}));
    let frame: Value = loop {
        let m: Value = serde_json::from_str(&tokio::time::timeout(Duration::from_secs(2), r.out.recv()).await.unwrap().unwrap()).unwrap();
        if m["type"] == "tk_frame" {
            break m;
        }
    };
    assert_eq!((frame["data"].as_str(), frame["meta"]["window_id"].as_u64()), (Some("jpeg"), Some(3)), "the frame comes on the connection's own lane");
    assert_eq!(r.ask(Permission::Control, json!({"type": "tk_ack", "tk": tk, "seq": frame["seq"]})).await["type"], "ack");
    r.ask(Permission::Control, json!({"type": "tk_input", "tk": tk, "ops": [{"k": "tap", "x": 40, "y": 50}], "st": now_millis(), "w": 3})).await;
    let sent = r.ask(Permission::Control, json!({"type": "tk_text", "tk": tk, "text": "hunter2-secret", "st": now_millis(), "w": 3})).await;
    assert_eq!(sent["result"], "sent", "{sent}");
    assert_eq!(told(&notes, "browser.takeover_input").await[0]["w"], 3);
    assert_eq!(r.ask(Permission::Control, json!({"type": "tk_stop", "tk": tk})).await["type"], "ack");
    let audit = r.daemon.mobile.audit_entries(50).to_string();
    assert!(audit.contains("takeover_start") && audit.contains("takeover_stop"), "{audit}");
    assert!(!audit.contains("hunter2") && !audit.contains("tk_input") && !audit.contains("tk_text"), "nothing typed, nor that it was: {audit}");
}

#[tokio::test]
async fn the_lock_ends_a_pages_hold_by_leaving_by_running_out_and_however_else_it_closes() {
    let mut r = rig();
    record(&r, Permission::Control, Kind::Pwa, true, true);
    let notes = app(&r.daemon);
    unlocked_page(&mut r);
    // Leaving the app.
    start(&mut r).await;
    r.daemon.mobile.lock_now("phone");
    assert_eq!(stopped(&notes, Duration::from_secs(1)).await.expect("the app lets go")["reason"], "revoked");
    notes.lock().clear();
    // The idle time running out, found by the sweep.
    unlocked_page(&mut r);
    start(&mut r).await;
    r.daemon.mobile.expire_window("phone");
    r.daemon.mobile.lock_sweep();
    assert!(stopped(&notes, Duration::from_secs(1)).await.is_some(), "the sweep ends it");
    notes.lock().clear();
    // The window gone with nothing to clear the grant: the hold reads the lock again as it goes.
    unlocked_page(&mut r);
    start(&mut r).await;
    r.daemon.mobile.expire_window("phone");
    assert!(stopped(&notes, Duration::from_secs(1)).await.is_some(), "the hold sees the lock itself");
}

#[tokio::test]
async fn a_page_with_its_lock_closed_or_no_passkey_gets_no_control_whatever_its_record_says() {
    let mut r = rig();
    record(&r, Permission::Control, Kind::Pwa, true, true);
    let _notes = app(&r.daemon);
    r.conn.lock_path = LockPath::Passkey;
    r.conn.page_origin = Some(ORIGIN.into());
    let locked = r.ask(Permission::Control, json!({"type": "panes"})).await;
    assert_eq!(code(&locked), Some("locked"), "no window: locked");
    record(&r, Permission::Control, Kind::Pwa, true, false);
    unlocked_page(&mut r);
    assert_eq!(code(&r.ask(Permission::Control, json!({"type": "panes"})).await), Some("no_control"), "no passkey on record");
}

#[tokio::test]
async fn an_app_record_used_from_a_browser_page_is_held_to_the_pages_rule() {
    let mut r = rig();
    record(&r, Permission::Control, Kind::App, false, false);
    let _notes = app(&r.daemon);
    // The same record from a page in a browser: no passkey, no secure origin, so no control however the connection came.
    r.conn.page_origin = Some("http://192.168.1.9:4817".into());
    assert_eq!(code(&r.ask(Permission::Control, json!({"type": "tk_start", "pane": "browser-a"})).await), Some("no_control"));
    // The native app's own connection keeps it.
    r.conn.page_origin = None;
    assert_eq!(r.ask(Permission::Control, json!({"type": "panes"})).await["result"], "ok");
}
