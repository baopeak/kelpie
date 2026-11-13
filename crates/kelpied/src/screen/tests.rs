use super::*;
use crate::auth::Role;
use kelpie_screen::geom::Rect;
use kelpie_screen::wire::Video;

/// How the app answers a start.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    Ok,
    Fail,
    /// After this many milliseconds.
    Slow(u64),
    /// As the real helper does: a frame streamed under the new screen first, then the answer after this many milliseconds.
    StreamsFirst(u64),
}

struct Rig {
    d: Arc<Daemon>,
    out: mpsc::Sender<String>,
    media: mpsc::Sender<Vec<u8>>,
    cursor: CursorSlot,
    phone: mpsc::Receiver<String>,
    frames: mpsc::Receiver<Vec<u8>>,
    notes: Arc<Mutex<Vec<Value>>>,
}

const APP: u64 = 901;

fn rig_with(lane: usize, start: Start) -> Rig {
    let dir = std::env::temp_dir().join(format!("kelpie-screen-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = Daemon::with_secrets(dir, std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
    // Devices given control, as the store has them: a hold rests on its record as well as on its grant.
    d.mobile.add_test_controller("phone");
    d.mobile.add_test_controller("tablet");
    let (tx, mut rx) = crate::session::client_channel();
    d.providers.register(APP, &Role::App, "screen", None, &tx).unwrap();
    let notes = Arc::new(Mutex::new(Vec::new()));
    let (app, kept) = (d.clone(), notes.clone());
    tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            let m: Value = serde_json::from_slice(&o.data[5..]).unwrap();
            if let Some(id) = m["preq"].as_u64() {
                let (app, method) = (app.clone(), m["method"].as_str().unwrap_or_default().to_string());
                let typed = m["params"]["text"].as_str().unwrap_or_default().to_string();
                let sc = m["params"]["sc"].as_str().unwrap_or_default().to_string();
                tokio::spawn(async move {
                    let answer = match method.as_str() {
                        "screen.targets" => json!({"result": {"targets": [
                            {"id": r"m:\\.\DISPLAY1", "kind": "monitor", "title": "Screen 1", "w": 2560, "h": 1440, "primary": true},
                            {"id": "w:66:7", "kind": "window", "title": "main.rs \u{7}— kelpie", "app": "VS Code", "shortcut": "ide", "w": 1200, "h": 800},
                            {"id": "nonsense", "kind": "window", "title": "x"},
                            {"id": "w:1:2", "kind": "toaster", "title": "x"},
                        ]}}),
                        "screen.start" => {
                            let ok = json!({"result": {"kind": "window", "title": "main.rs — kelpie", "exe": "Code.exe", "w": 1200, "h": 800, "encoder": "hw"}});
                            match start {
                                Start::Ok => ok,
                                Start::Fail => json!({"error": {"message": "that window was closed"}}),
                                Start::Slow(ms) => {
                                    tokio::time::sleep(Duration::from_millis(ms)).await;
                                    ok
                                }
                                Start::StreamsFirst(ms) => {
                                    let frame = base64::engine::general_purpose::STANDARD.encode(video(1, true, 100));
                                    app.screen.on_event(&app, APP, &json!({"sc": sc, "kind": "media", "data": frame}));
                                    tokio::time::sleep(Duration::from_millis(ms)).await;
                                    ok
                                }
                            }
                        }
                        // What the helper answers when the focus is elsewhere: by what is typed, so a test picks the case.
                        "screen.text" => match typed.as_str() {
                            "elsewhere" => json!({"error": {"message": "not_front: the shared window is not in front on the computer, so nothing was typed"}}),
                            "moved" => json!({"error": {"message": "not_front_partial: another window took the focus after 3 characters; the rest was not typed"}}),
                            "dialog" => json!({"error": {"message": "covered_dialog: a dialog of another program is in front of the shared window, so nothing was typed"}}),
                            // A long text: typed for a while.
                            "slow" => {
                                tokio::time::sleep(Duration::from_millis(400)).await;
                                json!({"result": true})
                            }
                            _ => json!({"result": true}),
                        },
                        _ => json!({"result": true}),
                    };
                    let mut reply = answer;
                    reply["preq"] = json!(id);
                    app.providers.reply(APP, &reply);
                });
            } else {
                kept.lock().push(m);
            }
        }
    });
    let (out, phone) = mpsc::channel(32);
    let (media, frames) = mpsc::channel(lane);
    Rig { d, out, media, cursor: CursorSlot::default(), phone, frames, notes }
}

fn rig() -> Rig {
    rig_with(8, Start::Ok)
}

fn video(seq: u32, key: bool, size: usize) -> Vec<u8> {
    let mut au = vec![0, 0, 0, 1, if key { 0x65 } else { 0x41 }];
    au.resize(size.max(5), 0x11);
    wire::video(&Video { key, seq, capture_ms: 0, src: (1200, 800), coded: (1200, 800), unpaint: vec![], unpaint_all: false }, &au)
}

fn tile(seq: u32) -> Vec<u8> {
    wire::tile(seq, 1, Rect::new(0, 0, 64, 64), wire::FORMAT_PNG, b"\x89PNG...")
}

impl Rig {
    fn phone(&self) -> Phone<'_> {
        Phone { device: "phone", name: "Phone", conn: 1, out: &self.out, media: &self.media, cursor: &self.cursor }
    }

    async fn verb(&self, verb: &str, req: Value) -> Result<Option<Value>, Refused> {
        self.d.screen.phone_verb(&self.d, self.phone(), verb, &req).await
    }

    async fn start(&self) -> String {
        let reply = self.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap().unwrap();
        reply["sc"].as_str().unwrap().to_string()
    }

    fn send(&self, sc: &str, bytes: &[u8]) {
        self.d.screen.on_event(&self.d, APP, &json!({"sc": sc, "kind": "media", "data": base64::engine::general_purpose::STANDARD.encode(bytes)}));
    }

    async fn next(&mut self, kind: &str) -> Option<Value> {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            match tokio::time::timeout(Duration::from_millis(100), self.phone.recv()).await {
                Ok(Some(m)) => {
                    let v: Value = serde_json::from_str(&m).unwrap();
                    if v["type"] == kind {
                        return Some(v);
                    }
                }
                Ok(None) => return None,
                Err(_) => {}
            }
        }
        None
    }

    fn frames(&mut self) -> Vec<wire::Peek> {
        let mut out = Vec::new();
        while let Ok(f) = self.frames.try_recv() {
            out.push(wire::peek(&f).unwrap());
        }
        out
    }

    async fn told_now(&self, method: &str) -> Vec<Value> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.told(method)
    }

    fn told(&self, method: &str) -> Vec<Value> {
        self.notes.lock().iter().filter(|n| n["pnote"] == method).map(|n| n["params"].clone()).collect()
    }
}

#[tokio::test]
async fn targets_come_from_the_app_checked_and_the_stream_goes_on_the_media_lane() {
    let mut r = rig();
    let listed = r.verb("screens", json!({})).await.unwrap().unwrap();
    let rows = listed["targets"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "unknown ids and kinds are left out: {rows:?}");
    assert_eq!((rows[1]["title"].as_str(), rows[1]["shortcut"].as_str()), (Some("main.rs — kelpie"), Some("ide")));
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 2000));
    r.send(&sc, &video(2, false, 500));
    r.send(&sc, &tile(2));
    r.send(&sc, &wire::cursor(wire::Shape::Arrow, true, 5, 5));
    let got = r.frames();
    assert_eq!(got.iter().map(|p| (p.kind, p.seq)).collect::<Vec<_>>(), [(KIND_VIDEO, 1), (KIND_VIDEO, 2), (KIND_TILE, 2)]);
    assert_eq!(r.cursor.take(), Some(wire::cursor(wire::Shape::Arrow, true, 5, 5)), "the cursor in its own slot, ahead of the media lane");
    // The acknowledgement goes to the helper with its round trip and what the wire took.
    r.verb("sc_ack", json!({"sc": sc, "seq": 2})).await.unwrap();
    let flow = r.told_now("screen.flow").await;
    let last = flow.last().unwrap();
    assert_eq!(last["acked"], 2);
    assert!(last["wire"].as_u64().unwrap() > last["raw"].as_u64().unwrap(), "the link pads: {last}");
    assert!(r.next("sc_fhb").await.is_some(), "a heartbeat, frames or not");
    // State and stats pass through, with only what the phone knows.
    r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "state", "state": "elevated"}));
    assert_eq!(r.next("sc_state").await.unwrap()["state"], "elevated");
    r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "state", "state": "<script>"}));
    r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "stats", "stats": {"fps": 15, "w": 1280, "h": 720, "reason": "load", "degraded": true, "crisp": false, "secret": "x", "stages": {"capture_to_send_ms_p95": 41.26, "top_off": 3, "convert_ms_p95": "a lot", "secret": 7, "link_cuts_window": 1, "last_cut_link_full_ms": 1020}, "cuts": [{"rule": "window", "from_kbps": 1500, "to_kbps": 1275, "link_full_ms": 1020, "inflight": 3}, {"rule": "<b>"}]}}));
    let stats = r.next("sc_stats").await.unwrap();
    assert_eq!((stats["fps"].as_f64(), stats["reason"].as_str(), stats.get("secret")), (Some(15.0), Some("load"), None));
    assert_eq!(stats["crisp"], false, "crisp text off for now: the phone says so");
    let stages = &stats["stages"];
    assert_eq!((stages["capture_to_send_ms_p95"].as_f64(), stages["top_off"].as_f64(), &stages["convert_ms_p95"], stages.get("secret")), (Some(41.3), Some(3.0), &json!(null), None), "the stages' timings: known numbers only");
    assert_eq!((stages["link_cuts_window"].as_f64(), stages["last_cut_link_full_ms"].as_f64()), (Some(1.0), Some(1020.0)), "which rule cut, and what it saw");
    assert!(stats.get("cuts").is_none(), "the cuts themselves go to the daemon's log");
    // Input goes to the app with the computer's clock; stop from the phone tells the app; the audit log has the start, the end, nothing typed.
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 10, "y": 20}], "st": now_millis()})).await.unwrap();
    let input = r.told_now("screen.input").await;
    assert_eq!(input[0]["ops"][0]["b"], "left");
    r.verb("sc_stop", json!({"sc": sc})).await.unwrap();
    assert_eq!(r.next("sc_end").await.unwrap()["reason"], "phone");
    assert_eq!(r.told_now("screen.stop").await.len(), 1);
    let audit = r.d.mobile.audit_entries(20).to_string();
    assert!(audit.contains("screen_start") && audit.contains("window: Code.exe") && audit.contains("screen_stop") && !audit.contains("\"x\""), "{audit}");
    assert!(!audit.contains("main.rs"), "never the window's title: {audit}");
}

#[test]
fn the_audit_log_names_a_program_by_its_file_name_only() {
    assert_eq!(program(Some(&json!("Code.exe"))), "Code.exe");
    assert_eq!(program(Some(&json!(r"C:\Users\me\AppData\Local\Programs\Code.exe"))), "Code.exe", "no path");
    assert_eq!(program(Some(&json!("evil\u{7}<b>.exe"))), "evilb.exe");
    assert_eq!(program(Some(&json!(""))), "unknown program");
    assert_eq!(program(None), "unknown program");
}

#[test]
fn what_a_message_takes_on_the_padded_link() {
    assert_eq!(wire_len(0), 256 + 16);
    assert_eq!(wire_len(200), 256 + 16);
    assert_eq!(wire_len(5_000), 16_384 + 16, "a 5 KB frame goes as 16 KB");
    let chunk = kelpie_link::MAX_CHUNK;
    assert_eq!(wire_len(chunk), chunk + 1 + 16, "one full chunk, no padded rest");
    assert!(wire_len(chunk + 10) > wire_len(chunk));
}

#[tokio::test]
async fn video_waits_for_its_first_keyframe_and_the_helper_hears_what_was_dropped() {
    let mut r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, false, 100));
    assert!(r.frames().is_empty(), "a decoder cannot start from a P-frame");
    assert_eq!(r.told_now("screen.keyframe").await.len(), 1, "a keyframe is asked for");
    r.send(&sc, &video(2, false, 100));
    assert_eq!(r.told_now("screen.keyframe").await.len(), 1, "at most one a second");
    // The next flow says the helper will hear nothing for frames 1 and 2, and that the tiles must be painted again.
    r.verb("sc_ack", json!({"sc": sc, "seq": 0})).await.unwrap();
    let flow = r.told_now("screen.flow").await;
    assert_eq!((flow.last().unwrap()["dropped_through"].as_u64(), flow.last().unwrap()["lost_tiles"].as_bool()), (Some(2), Some(true)));
    r.send(&sc, &video(3, true, 100));
    r.send(&sc, &video(4, false, 100));
    assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [3, 4]);
}

#[tokio::test]
async fn a_full_lane_drops_video_up_to_the_next_keyframe_and_tiles_never_take_video_s_room() {
    let mut r = rig_with(4, Start::Ok);
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.send(&sc, &tile(1));
    assert_eq!(r.frames().len(), 2, "room to spare: the tile goes");
    r.send(&sc, &video(2, false, 100));
    r.send(&sc, &video(3, false, 100));
    r.send(&sc, &tile(3));
    assert_eq!(r.frames().iter().map(|p| (p.kind, p.seq)).collect::<Vec<_>>(), [(KIND_VIDEO, 2), (KIND_VIDEO, 3)], "two places free: no tile");
    for seq in 4..=8 {
        r.send(&sc, &video(seq, false, 100));
    }
    assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [4, 5, 6, 7], "the lane held four");
    r.send(&sc, &video(9, false, 100));
    assert!(r.frames().is_empty(), "after a drop, nothing until a keyframe");
    r.send(&sc, &video(10, true, 100));
    assert_eq!(r.frames().iter().map(|p| (p.seq, p.key)).collect::<Vec<_>>(), [(10, true)]);
    assert!(!r.told_now("screen.keyframe").await.is_empty());
}

#[tokio::test]
async fn no_more_than_half_a_megabyte_waits_for_the_phone() {
    let mut r = rig_with(64, Start::Ok);
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 300 * 1024));
    r.send(&sc, &video(2, false, 300 * 1024));
    assert_eq!(r.frames().len(), 1, "the second would pass the bound");
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    r.send(&sc, &video(3, true, 300 * 1024));
    assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [3]);
}

#[tokio::test]
async fn a_revoke_stops_input_at_once_and_ends_the_screen_within_a_second() {
    let mut r = rig();
    let sc = r.start().await;
    let at = std::time::Instant::now();
    let _ = r.d.mobile.revoke("phone");
    let late = r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 1, "y": 1}], "st": now_millis()})).await;
    assert_eq!(late.unwrap_err().code, "sc_gone", "the grant is checked on every input");
    assert_eq!(r.next("sc_end").await.expect("told")["reason"], "revoked");
    assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
    assert!(r.told_now("screen.input").await.is_empty());
    assert_eq!(r.told("screen.stop").len(), 1, "the helper stops");
    assert_eq!(r.d.screen.count(), 0);
}

#[tokio::test]
async fn the_phone_locking_ends_the_screen_through_its_task() {
    let mut r = rig();
    let sc = r.start().await;
    r.d.mobile.lock_now_for_test("phone");
    assert_eq!(r.next("sc_end").await.expect("told")["reason"], "revoked");
    assert_eq!(r.d.screen.count(), 0);
    assert!(r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 1, "y": 1}], "st": now_millis()})).await.is_err());
}

#[tokio::test]
async fn a_screen_nobody_touches_ends_after_ten_minutes() {
    let mut r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    {
        let hold = r.d.screen.hold(&sc).unwrap();
        hold.state.lock().idle = Duration::from_millis(1200);
        hold.wake.notify_one();
    }
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap();
    }
    assert_eq!(r.d.screen.count(), 1, "input starts the time over");
    assert_eq!(r.next("sc_end").await.expect("told")["reason"], "idle");
}

#[tokio::test]
async fn a_dropped_link_pauses_the_helper_and_keeps_the_screen_for_the_same_device() {
    let mut r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.frames();
    // The same connection asking again (a hello after an unlock) changes nothing.
    r.verb("sc_rebind", json!({"sc": sc})).await.unwrap();
    r.d.screen.link_closed(&r.d, 1);
    assert_eq!(r.told_now("screen.pause").await[0]["paused"], true);
    let (out2, mut phone2) = mpsc::channel(8);
    let (media2, mut frames2) = mpsc::channel(8);
    let other = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2, media: &media2, cursor: &CursorSlot::default() };
    assert_eq!(r.d.screen.phone_verb(&r.d, other, "sc_rebind", &json!({"sc": sc})).await.unwrap_err().code, "sc_gone", "another device cannot take it");
    let back = Phone { device: "phone", name: "Phone", conn: 3, out: &out2, media: &media2, cursor: &CursorSlot::default() };
    r.d.screen.phone_verb(&r.d, back, "sc_rebind", &json!({"sc": sc})).await.unwrap();
    assert_eq!(r.told_now("screen.pause").await.last().unwrap()["paused"], false);
    // Never a frame from before the gap: a P-frame waits for the keyframe the helper sends first.
    r.send(&sc, &video(2, false, 100));
    r.send(&sc, &video(3, true, 100));
    assert_eq!(wire::peek(&frames2.try_recv().unwrap()).unwrap().seq, 3);
    assert!(tokio::time::timeout(Duration::from_secs(2), phone2.recv()).await.is_ok(), "the heartbeat goes to the new connection");
    // Gone for longer than the window: ended.
    {
        let hold = r.d.screen.hold(&sc).unwrap();
        hold.state.lock().rebind = Duration::from_millis(600);
    }
    r.d.screen.link_closed(&r.d, 3);
    tokio::time::sleep(Duration::from_millis(600) + WATCH * 3).await;
    assert_eq!(r.d.screen.count(), 0);
}

#[tokio::test]
async fn a_link_that_closes_during_a_start_leaves_a_screen_waiting_for_the_device() {
    let r = rig_with(8, Start::Slow(300));
    let (out, out_rx) = mpsc::channel(8);
    let (media, media_rx) = mpsc::channel(8);
    let dying = Phone { device: "phone", name: "Phone", conn: 5, out: &out, media: &media, cursor: &CursorSlot::default() };
    let d = r.d.clone();
    let req = json!({"target": "w:66:7"});
    let starting = d.screen.phone_verb(&d, dying, "sc_start", &req);
    let closing = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(out_rx);
        drop(media_rx);
    };
    let (started, ()) = tokio::join!(starting, closing);
    let sc = started.unwrap().unwrap()["sc"].as_str().unwrap().to_string();
    assert!(r.told_now("screen.pause").await.iter().any(|p| p["sc"] == sc.as_str() && p["paused"] == true), "the helper waits too");
    // The device's next connection takes it, as after any dropped link.
    r.d.screen.phone_verb(&r.d, r.phone(), "sc_rebind", &json!({"sc": sc})).await.unwrap();
}

#[tokio::test]
async fn a_failed_start_tells_the_app_to_let_go() {
    let r = rig_with(8, Start::Fail);
    assert_eq!(r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap_err().code, "no_screen");
    let stops = r.told_now("screen.stop").await;
    assert_eq!(stops.len(), 1, "a start the app may still finish is stopped");
    assert_eq!(stops[0]["reason"], "failed");
    assert_eq!(r.d.screen.count(), 0);
    // The reservation went with it: a new start may come.
    assert!(!*r.d.screen.starting.lock());
}

#[tokio::test]
async fn two_starts_at_once_the_first_keeps_the_screen_and_the_second_is_told_it_is_in_use() {
    let r = rig_with(8, Start::Slow(300));
    let (out2, _rx2) = mpsc::channel(8);
    let (media2, _m2) = mpsc::channel(8);
    let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2, media: &media2, cursor: &CursorSlot::default() };
    let d = r.d.clone();
    let first = r.verb("sc_start", json!({"target": "w:66:7"}));
    let second = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        d.screen.phone_verb(&d, tablet, "sc_start", &json!({"target": "w:66:7"})).await
    };
    let (first, second) = tokio::join!(first, second);
    assert!(first.unwrap().is_some());
    assert_eq!(second.unwrap_err().code, "sc_taken");
    assert_eq!(r.d.screen.count(), 1);
    // The same device starting twice at once: the second is refused too, never two holds.
    let again = r.verb("sc_start", json!({"target": "w:66:7"}));
    let twice = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        r.d.screen.phone_verb(&r.d, Phone { device: "phone", name: "Phone", conn: 9, out: &r.out, media: &r.media, cursor: &r.cursor }, "sc_start", &json!({"target": "w:66:7"})).await
    };
    let (again, twice) = tokio::join!(again, twice);
    assert!(again.is_ok());
    assert_eq!(twice.unwrap_err().code, "sc_taken");
    assert_eq!(r.d.screen.count(), 1);
}

#[tokio::test]
async fn late_stale_or_flooding_input_is_dropped_and_only_input_a_screen_takes_goes() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    let old = now_millis() - 1500;
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}], "st": old})).await.unwrap();
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}]})).await.unwrap();
    assert!(r.told_now("screen.input").await.is_empty(), "a click after a gap never clicks");
    assert_eq!(r.verb("sc_text", json!({"sc": sc, "text": "hunter2", "st": old})).await.unwrap_err().code, "not_sent");
    for bad in [json!([]), json!([{"k": "eval"}]), json!([{"k": "key", "key": "LWin"}]), json!([{"k": "move", "x": -1, "y": 0}])] {
        assert_eq!(r.verb("sc_input", json!({"sc": sc, "ops": bad, "st": now_millis()})).await.unwrap_err().code, "bad_request", "{bad}");
    }
    let sent = r.verb("sc_text", json!({"sc": sc, "text": "hunter2", "st": now_millis()})).await.unwrap().unwrap();
    assert_eq!(sent["result"], "sent");
    for _ in 0..(INPUT_PER_SECOND + 20) {
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap();
    }
    assert!(r.told_now("screen.input").await.len() <= INPUT_PER_SECOND as usize + 1, "a flood is cut");
    let other = Phone { device: "phone", name: "Phone", conn: 9, out: &r.out, media: &r.media, cursor: &r.cursor };
    assert_eq!(r.d.screen.phone_verb(&r.d, other, "sc_input", &json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap_err().code, "sc_elsewhere");
}

#[tokio::test]
async fn input_onto_a_picture_that_stopped_is_not_sent() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    let hold = r.d.screen.hold(&sc).unwrap();
    // A frame waited two seconds for its acknowledgement.
    hold.state.lock().sent.front_mut().unwrap().1 = Instant::now() - STALE_AFTER;
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}], "st": now_millis()})).await.unwrap();
    assert!(r.told_now("screen.input").await.is_empty());
    // Everything delivered was acknowledged, but a keyframe has been owed for two seconds (the frames after it were dropped): still stale.
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    {
        let mut st = hold.state.lock();
        st.need_key = true;
        st.need_key_since = Instant::now() - STALE_AFTER;
    }
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}], "st": now_millis()})).await.unwrap();
    assert!(r.told_now("screen.input").await.is_empty());
}

#[tokio::test]
async fn a_busy_computer_starts_no_screen_one_device_at_a_time_and_none_without_the_app() {
    let r = rig();
    r.d.mobile.load.set_for_test(Level::L2);
    assert_eq!(r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap_err().code, "host_busy");
    r.d.mobile.load.set_for_test(Level::L0);
    assert_eq!(r.verb("sc_start", json!({"target": "C:\\Windows"})).await.unwrap_err().code, "bad_request");
    let first = r.start().await;
    let (out2, _rx) = mpsc::channel(4);
    let (media2, _frames) = mpsc::channel(4);
    let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2, media: &media2, cursor: &CursorSlot::default() };
    assert_eq!(r.d.screen.phone_verb(&r.d, tablet, "sc_start", &json!({"target": "w:66:7"})).await.unwrap_err().code, "sc_taken");
    let second = r.start().await;
    assert_ne!(first, second);
    assert_eq!(r.d.screen.count(), 1, "the same device's new screen replaces its old one");
    r.d.screen.provider_gone(&r.d, APP);
    assert_eq!(r.d.screen.count(), 0, "the app went: its screens end");
    r.d.providers.client_gone(APP);
    assert_eq!(r.verb("screens", json!({})).await.unwrap_err().code, "no_app");
    assert_eq!(r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap_err().code, "no_app");
}

#[tokio::test]
async fn only_the_hold_s_own_provider_is_heard_and_what_it_ends_is_ended() {
    let mut r = rig();
    let sc = r.start().await;
    r.d.screen.on_event(&r.d, APP + 1, &json!({"sc": sc, "kind": "ended", "reason": "hotkey"}));
    assert_eq!(r.d.screen.count(), 1, "another connection cannot end it");
    r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "state", "state": "not_front"}));
    assert_eq!(r.next("sc_state").await.unwrap()["state"], "not_front");
    r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "ended", "reason": "hotkey"}));
    assert_eq!(r.next("sc_end").await.unwrap()["reason"], "hotkey");
    assert!(r.told_now("screen.stop").await.is_empty(), "the helper ended it itself");
    assert!(Broker::owns_event(&json!({"sc": "x"})) && !Broker::owns_event(&json!({"tk": "x"})));
}

#[tokio::test]
async fn a_press_is_let_go_when_its_late_up_is_dropped_but_the_pointer_does_not_move() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    // A drag starts on the picture as it is.
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "down", "x": 10, "y": 10, "b": "left"}], "st": now_millis()})).await.unwrap();
    assert_eq!(r.told_now("screen.input").await.len(), 1);
    // Its end comes too late: dropped, but the button is let go where the pointer is.
    let old = now_millis() - 1500;
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}, {"k": "up", "x": 2, "y": 2, "b": "left"}], "st": old})).await.unwrap();
    let sent = r.told_now("screen.input").await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["ops"], json!([{"k": "release"}]), "a release with no place: a stale picture never says where the pointer goes");
    // Nothing is held now: another late up sends nothing.
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "up", "x": 2, "y": 2}], "st": old})).await.unwrap();
    assert_eq!(r.told_now("screen.input").await.len(), 2);
}

#[tokio::test]
async fn a_flood_of_late_ups_is_one_release_at_most_and_none_without_a_press() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    let old = now_millis() - 1500;
    let ups: Vec<Value> = (0..32).map(|i| json!({"k": "up", "x": i, "y": i})).collect();
    for _ in 0..50 {
        r.verb("sc_input", json!({"sc": sc, "ops": ups, "st": old})).await.unwrap();
    }
    assert!(r.told_now("screen.input").await.is_empty(), "no press outstanding: nothing at all");
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "down", "x": 10, "y": 10}], "st": now_millis()})).await.unwrap();
    for _ in 0..50 {
        r.verb("sc_input", json!({"sc": sc, "ops": ups, "st": old})).await.unwrap();
    }
    let sent = r.told_now("screen.input").await;
    assert_eq!(sent.len(), 2, "the press, then one release: {sent:?}");
    assert_eq!(sent[1]["ops"], json!([{"k": "release"}]));
    assert!(sent[1]["ops"][0].get("x").is_none() && sent[1]["ops"][0].get("y").is_none());
}

#[tokio::test]
async fn pauses_and_resumes_are_numbered_so_that_an_older_one_arriving_late_is_ignored() {
    let r = rig();
    let sc = r.start().await;
    r.d.screen.link_closed(&r.d, 1);
    r.d.screen.phone_verb(&r.d, r.phone(), "sc_rebind", &json!({"sc": sc})).await.unwrap();
    r.d.screen.link_closed(&r.d, 1);
    let told = r.told_now("screen.pause").await;
    let versions: Vec<u64> = told.iter().map(|p| p["v"].as_u64().unwrap()).collect();
    assert_eq!(versions, [1, 2, 3], "{told:?}");
    assert_eq!(told.last().unwrap()["paused"], true, "the newest says pause");
}
#[tokio::test]
async fn typing_the_focus_refused_says_why_to_the_phone() {
    let r = rig();
    let sc = r.start().await;
    let refusal = |text: &'static str| {
        let (d, sc) = (r.d.clone(), sc.clone());
        let (out, media) = (r.out.clone(), r.media.clone());
        async move {
            let phone = Phone { device: "phone", name: "Phone", conn: 1, out: &out, media: &media, cursor: &CursorSlot::default() };
            d.screen.phone_verb(&d, phone, "sc_text", &json!({"sc": sc, "text": text, "st": now_millis()})).await.unwrap_err()
        }
    };
    let e = refusal("elsewhere").await;
    assert_eq!(e.code, "not_front");
    assert!(e.message.starts_with("Not typed"), "{}", e.message);
    let e = refusal("moved").await;
    assert_eq!(e.code, "not_front");
    assert!(e.message.starts_with("Stopped partway"), "part of it went: the phone does not say nothing was typed ({})", e.message);
    let e = refusal("dialog").await;
    assert_eq!(e.code, "covered");
    assert!(e.message.contains("Share the whole monitor to type into this dialog"), "{}", e.message);
    assert_eq!(r.verb("sc_text", json!({"sc": sc, "text": "fine", "st": now_millis()})).await.unwrap().unwrap()["result"], "sent");
}

#[tokio::test]
async fn an_acknowledgement_is_never_taken_past_the_frames_sent() {
    let mut r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.frames();
    // A number from a stream before this one (its frames were still on their way while this one started).
    r.verb("sc_ack", json!({"sc": sc, "seq": 999})).await.unwrap();
    let flow = r.told_now("screen.flow").await;
    assert_eq!(flow.last().unwrap()["acked"], 1, "{flow:?}");
}

/// The window the helper's controller cuts for is the link's, counted here from a frame leaving for the phone to its acknowledgement
/// arriving: frames the helper is still getting to the daemon, or acknowledgements on their way back to a busy helper, are not in it.
#[tokio::test]
async fn the_helper_hears_how_long_the_link_s_window_has_been_full_as_counted_here() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.send(&sc, &video(2, false, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 2})).await.unwrap();
    assert_eq!(r.told_now("screen.flow").await.last().unwrap()["link_full_ms"], 0, "every frame answered: not full");
    for seq in 3..=5 {
        r.send(&sc, &video(seq, false, 100));
    }
    // Three wait for the phone: with no acknowledgement to bring a flow, the hold's task says how long at each look.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let full = r.told("screen.flow").last().unwrap()["link_full_ms"].as_u64().unwrap();
    assert!((250..=1000).contains(&full), "{full} ms");
    r.verb("sc_ack", json!({"sc": sc, "seq": 3})).await.unwrap();
    assert_eq!(r.told_now("screen.flow").await.last().unwrap()["link_full_ms"], 0, "one answered: the window has room again");
}

#[tokio::test]
async fn frames_that_come_before_the_hold_exists_bring_a_keyframe_request_once_it_does() {
    // The helper streams as soon as it starts: its first frame reaches the daemon while the app's answer is still on its way, and is dropped.
    let r = rig_with(8, Start::StreamsFirst(200));
    let sc = r.start().await;
    let asked = r.told_now("screen.keyframe").await;
    assert_eq!(asked.len(), 1, "a keyframe asked for as soon as the hold exists: {asked:?}");
    assert_eq!(asked[0]["sc"], sc.as_str());
    // A start that streamed nothing first asks for nothing.
    let quiet = rig();
    quiet.start().await;
    assert!(quiet.told_now("screen.keyframe").await.is_empty());
}

#[tokio::test]
async fn the_helper_s_window_sizes_the_link_s_full_mark_and_the_bytes_that_may_wait_within_bounds() {
    let mut r = rig_with(64, Start::Ok);
    let sc = r.start().await;
    let window = |r: &Rig, frames: u64, bytes: u64| r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "window", "frames": frames, "bytes": bytes}));
    // The flow says this daemon takes a window.
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    assert_eq!(r.told_now("screen.flow").await.last().unwrap()["win"], true);
    window(&r, 6, 400 * 1024);
    for seq in 2..=6 {
        r.send(&sc, &video(seq, false, 100));
    }
    assert!(r.d.screen.hold(&sc).unwrap().state.lock().full_since.is_none(), "five of six: not full");
    r.send(&sc, &video(7, false, 100));
    assert!(r.d.screen.hold(&sc).unwrap().state.lock().full_since.is_some(), "six: full");
    // Bytes: a frame goes while fewer than the window's 400 KiB wait, whatever its size (as the helper sends it); once they wait, the next does not.
    r.verb("sc_ack", json!({"sc": sc, "seq": 7})).await.unwrap();
    r.frames();
    r.send(&sc, &video(8, false, 300 * 1024));
    r.send(&sc, &video(9, false, 150 * 1024));
    r.send(&sc, &video(10, false, 10 * 1024));
    assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [8, 9]);
    // Whatever the helper says, within 3..=12 frames and 192 KiB..=2 MiB.
    window(&r, 100, 100 * 1024 * 1024);
    assert_eq!(r.d.screen.hold(&sc).unwrap().state.lock().window, (MAX_WINDOW, MAX_WINDOW_BYTES));
    window(&r, 0, 0);
    assert_eq!(r.d.screen.hold(&sc).unwrap().state.lock().window, (MIN_WINDOW, MIN_WINDOW_BYTES));
}

#[tokio::test]
async fn the_cursor_never_waits_behind_video_and_only_its_latest_place_is_kept() {
    let mut r = rig_with(2, Start::Ok);
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.send(&sc, &video(2, false, 100));
    // The lane is full: a cursor still goes, to its own slot.
    r.send(&sc, &wire::cursor(wire::Shape::Arrow, true, 10, 20));
    r.send(&sc, &wire::cursor(wire::Shape::Arrow, true, 30, 40));
    assert_eq!(r.frames().len(), 2, "the video only on the media lane");
    assert_eq!(r.cursor.take(), Some(wire::cursor(wire::Shape::Arrow, true, 30, 40)));
    assert_eq!(r.cursor.take(), None);
}

#[tokio::test]
async fn input_that_comes_while_text_is_typed_goes_after_it_and_acknowledgements_do_not_wait() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    let typing = r.d.screen.text_begin(&r.d, r.phone(), &json!({"sc": sc, "text": "slow", "st": now_millis()})).unwrap();
    assert_eq!(r.told_now("screen.text").await.len(), 0, "a request, not a notice");
    // While it is typed: an acknowledgement is taken at once, and Enter waits.
    let at = std::time::Instant::now();
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "key", "key": "Enter", "mods": []}], "st": now_millis()})).await.unwrap();
    assert!(at.elapsed() < Duration::from_millis(200), "{:?}", at.elapsed());
    assert!(r.told_now("screen.input").await.is_empty(), "Enter waits for the text");
    assert_eq!(r.d.screen.text_begin(&r.d, r.phone(), &json!({"sc": sc, "text": "again", "st": now_millis()})).err().map(|e| e.code), Some("not_sent"), "one text at a time");
    assert_eq!(typing.await.unwrap()["result"], "sent");
    let input = r.told_now("screen.input").await;
    assert_eq!(input.len(), 1, "then it goes");
    assert_eq!(input[0]["ops"][0]["key"], "Enter");
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap();
    assert_eq!(r.told_now("screen.input").await.len(), 2, "and input goes at once again");
}

#[tokio::test]
async fn sharpen_goes_to_the_helper_at_most_every_three_seconds_and_needs_a_part_of_the_picture() {
    let r = rig();
    let sc = r.start().await;
    let rect = json!({"x": 0, "y": 0, "w": 600, "h": 400});
    r.verb("sc_sharpen", json!({"sc": sc, "rect": rect})).await.unwrap();
    r.verb("sc_sharpen", json!({"sc": sc, "rect": rect})).await.unwrap();
    let told = r.told_now("screen.sharpen").await;
    assert_eq!(told.len(), 1, "once in three seconds");
    assert_eq!(told[0]["rect"], rect);
    for bad in [json!({"x": 0, "y": 0, "w": 0, "h": 4}), json!({"x": 0, "y": 0, "w": 70000, "h": 4}), json!(null)] {
        assert_eq!(r.verb("sc_sharpen", json!({"sc": sc, "rect": bad})).await.unwrap_err().code, "bad_request");
    }
}

#[tokio::test]
async fn the_start_says_the_stream_s_clock_and_a_dropped_link_keeps_the_screen_forty_five_seconds() {
    let r = rig();
    let reply = r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap().unwrap();
    assert!(reply.as_object().unwrap().contains_key("t0"), "null from an app that does not say it");
    let sc = reply["sc"].as_str().unwrap();
    assert_eq!(r.d.screen.hold(sc).unwrap().state.lock().rebind, SCREEN_REBIND_WINDOW);
    assert_eq!(SCREEN_REBIND_WINDOW, Duration::from_secs(45));
    assert!(crate::takeover::REBIND_WINDOW < SCREEN_REBIND_WINDOW, "a pane's takeover keeps its own");
}

#[tokio::test]
async fn input_that_waited_too_long_behind_a_text_is_dropped_and_a_press_in_it_let_go() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    let typing = r.d.screen.text_begin(&r.d, r.phone(), &json!({"sc": sc, "text": "slow", "st": now_millis()})).unwrap();
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "down", "x": 1, "y": 1, "b": "left"}], "st": now_millis()})).await.unwrap();
    // As if it had waited over a second.
    r.d.screen.hold(&sc).unwrap().state.lock().after_text[0].0 -= Duration::from_millis(1500);
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 2, "y": 2}], "st": now_millis()})).await.unwrap();
    typing.await.unwrap();
    let sent: Vec<Value> = r.told_now("screen.input").await.iter().map(|n| n["ops"][0]["k"].clone()).collect();
    assert_eq!(sent, [json!("move"), json!("release")], "the old press dropped and let go; the recent move goes");
    assert!(!r.d.screen.hold(&sc).unwrap().state.lock().held);
}

#[tokio::test]
async fn while_text_is_typed_a_late_up_never_overtakes_its_press_and_a_full_queue_changes_nothing() {
    let r = rig();
    let sc = r.start().await;
    r.send(&sc, &video(1, true, 100));
    r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
    let typing = r.d.screen.text_begin(&r.d, r.phone(), &json!({"sc": sc, "text": "slow", "st": now_millis()})).unwrap();
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "down", "x": 1, "y": 1, "b": "left"}], "st": now_millis()})).await.unwrap();
    // A late `up`: its release waits behind the press.
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "up", "x": 1, "y": 1, "b": "left"}], "st": now_millis() - 5000})).await.unwrap();
    assert!(r.told_now("screen.input").await.is_empty());
    {
        let hold = r.d.screen.hold(&sc).unwrap();
        let mut st = hold.state.lock();
        while st.after_text.len() < MAX_AFTER_TEXT {
            st.after_text.push((Instant::now(), vec![json!({"k": "move", "x": 3, "y": 3})]));
        }
    }
    r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "down", "x": 1, "y": 1, "b": "left"}], "st": now_millis()})).await.unwrap();
    assert!(!r.d.screen.hold(&sc).unwrap().state.lock().held, "a press past the queue's bound is dropped whole");
    typing.await.unwrap();
    let kinds: Vec<Value> = r.told_now("screen.input").await.iter().map(|n| n["ops"][0]["k"].clone()).collect();
    assert_eq!(&kinds[..2], [json!("down"), json!("release")]);
    assert_eq!(kinds.len(), MAX_AFTER_TEXT);
}

#[tokio::test]
async fn a_newer_connection_of_the_device_takes_the_screen_over_at_once() {
    let r = rig();
    let sc = r.start().await;
    crate::mobile::connection_opened_for_test(&r.d, "phone", 1);
    // The device connects again while its older connection is not closed yet (a stalled socket): the older one's hold waits for it at once.
    crate::mobile::connection_opened_for_test(&r.d, "phone", 3);
    assert_eq!(r.told_now("screen.pause").await[0]["paused"], true);
    let (out2, _phone2) = mpsc::channel(8);
    let (media2, _frames2) = mpsc::channel(8);
    let back = Phone { device: "phone", name: "Phone", conn: 3, out: &out2, media: &media2, cursor: &CursorSlot::default() };
    r.d.screen.phone_verb(&r.d, back, "sc_rebind", &json!({"sc": sc})).await.unwrap();
    // The older connection ending later changes nothing for it.
    r.d.screen.link_closed(&r.d, 1);
    assert_eq!(r.d.screen.hold(&sc).unwrap().state.lock().conn.as_ref().map(|l| l.conn), Some(3));
}
