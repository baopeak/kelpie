use super::*;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-window-agent-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A runner that records what it was asked to run and answers from `answer`.
fn fake(answer: impl Fn(&Invocation) -> Result<String, String> + Send + Sync + 'static) -> (Runner, Arc<Mutex<Vec<Invocation>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let runner: Runner = Arc::new(move |inv: &Invocation| {
        log.lock().push(inv.clone());
        answer(inv)
    });
    (runner, seen)
}

fn on(visible: bool) -> Policy {
    Policy { mode: Mode::On, visible }
}

const WINDOW: &str = "w:131074:4242";

#[test]
fn every_tool_has_a_class_and_nothing_else_does() {
    for t in ["list", "tree", "shot"] {
        assert_eq!(classify(t), Some(Class::Read), "{t}");
    }
    for t in ["launch", "act", "close"] {
        assert_eq!(classify(t), Some(Class::Act), "{t}");
    }
    assert_eq!(TOOLS.len(), 6);
    assert!(TOOLS.iter().all(|t| classify(t).is_some()));
    for t in ["", "kelpie_window_list", "List", "focus", "eval"] {
        assert_eq!(classify(t), None, "{t}");
    }
}

#[test]
fn the_gate_follows_the_policy() {
    for t in TOOLS {
        assert_eq!(gate(&Policy::default(), t).unwrap_err(), OFF, "{t}");
        assert_eq!(gate(&on(false), t), Ok(classify(t).unwrap()), "{t}");
    }
    assert!(gate(&on(false), "drag").unwrap_err().contains("unknown window tool"));
}

#[test]
fn the_policy_reads_and_writes_its_keys_and_a_bad_request_changes_nothing() {
    assert_eq!(Policy::from_file(None), Policy { mode: Mode::Off, visible: false });
    assert_eq!(Policy::from_file(Some(&json!({"window_agents": "on", "window_visible": true}))), on(true));
    // A value that is not valid in the file is the default, not an error.
    assert_eq!(Policy::from_file(Some(&json!({"window_agents": "ask", "window_visible": "yes"}))), Policy::default());
    let mut file = json!({"other": 1});
    on(true).write_into(&mut file);
    assert_eq!(file, json!({"other": 1, "window_agents": "on", "window_visible": true}));
    let now = on(false);
    assert_eq!(Policy::requested(&json!({}), &now).unwrap(), None);
    assert_eq!(Policy::requested(&json!({"window_visible": true}), &now).unwrap(), Some(on(true)));
    assert_eq!(Policy::requested(&json!({"window_agents": "off"}), &now).unwrap(), Some(Policy { mode: Mode::Off, visible: false }));
    for bad in [json!({"window_agents": "ask"}), json!({"window_agents": 1}), json!({"window_visible": "true"}), json!({"window_agents": "on", "window_visible": 1})] {
        assert!(Policy::requested(&bad, &now).is_err(), "{bad}");
    }
}

#[test]
fn a_window_target_and_an_id_have_the_shape_the_helper_prints() {
    for ok in [WINDOW, "w:1:2", "w:0:0"] {
        assert!(window_ok(ok), "{ok}");
    }
    let long_number = format!("w:{}:1", "9".repeat(21));
    for bad in ["", "w:", "w:1", "w:1:", "w::2", "w:1:2:3", "w:a:2", "w:-1:2", "w:1:2 --max", "m:\\\\.\\DISPLAY1", "w:1:2\n", long_number.as_str()] {
        assert!(!window_ok(bad), "{bad:?}");
    }
    for ok in ["42", "42.1312340.4.7", "-2147483647.1", "1.-1"] {
        assert!(id_ok(ok), "{ok}");
    }
    let too_deep = vec!["1"; MAX_ID_PARTS + 1].join(".");
    let too_long = "1".repeat(MAX_ID + 1);
    for bad in ["", ".", "1.", ".1", "1..2", "-", "--1", "1-2", "a", "1.a", "1 2", "1.2\n", "+1", too_deep.as_str(), too_long.as_str(), "123456789012"] {
        assert!(!id_ok(bad), "{bad:?}");
    }
}

#[test]
fn arguments_are_checked_per_tool_and_unknown_ones_refused() {
    assert_eq!(check_args("list", &Value::Null).unwrap(), json!({}));
    assert_eq!(check_args("list", &json!({"visible": false})).unwrap(), json!({"visible": false}));
    assert_eq!(check_args("shot", &json!({"window": WINDOW})).unwrap(), json!({"window": WINDOW}));
    assert_eq!(check_args("close", &json!({"window": WINDOW, "visible": true})).unwrap(), json!({"window": WINDOW, "visible": true}));
    assert_eq!(check_args("tree", &json!({"window": WINDOW, "max": 1500})).unwrap(), json!({"window": WINDOW, "max": 1500}));
    for bad in [json!({}), json!({"window": "w:1"}), json!({"window": 5}), json!({"window": WINDOW, "max": 0}), json!({"window": WINDOW, "max": 1501}), json!({"window": WINDOW, "max": "5"}), json!({"window": WINDOW, "visible": "yes"})] {
        assert!(check_args("tree", &bad).is_err(), "{bad}");
    }
    let e = check_args("list", &json!({"window": WINDOW})).unwrap_err();
    assert!(e.contains("takes no `window`") && e.contains("visible"), "{e}");
    assert!(check_args("close", &json!({"window": WINDOW, "text": "x"})).is_err());
    assert!(check_args("list", &json!([1])).unwrap_err().contains("object"));
    assert!(check_args("drag", &json!({})).unwrap_err().contains("unknown window tool"));
}

#[test]
fn an_action_is_one_the_helper_offers_to_agents_and_text_goes_only_with_set() {
    for action in ["invoke", "toggle", "expand", "collapse", "select", "show"] {
        let ok = check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": action})).unwrap();
        assert_eq!(ok, json!({"window": WINDOW, "id": "4.2", "action": action}));
        assert!(check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": action, "text": "x"})).unwrap_err().contains("only with the action `set`"), "{action}");
    }
    // The person's focus is not an agent's to take.
    for action in ["focus", "Invoke", "click", ""] {
        assert!(check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": action})).unwrap_err().contains("`action` is one of"), "{action:?}");
    }
    let set = check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": "hello"})).unwrap();
    assert_eq!(set["text"], "hello");
    // An empty text empties a field; a missing one is a mistake.
    assert_eq!(check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": ""})).unwrap()["text"], "");
    assert!(check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set"})).unwrap_err().contains("needs `text`"));
    let at_limit = "é".repeat(MAX_TEXT_BYTES / 2);
    assert!(check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": at_limit})).is_ok());
    let over = "x".repeat(MAX_TEXT_BYTES + 1);
    let e = check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": over})).unwrap_err();
    assert!(e.contains("longer than 4096 bytes") && !e.contains("xxxx"), "the reason never repeats the text");
    let e = check_args("act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": "a\0b"})).unwrap_err();
    assert!(e.contains("NUL"), "{e}");
    for (key, bad) in [("window", "w:1"), ("id", "4;2"), ("id", "$(x)"), ("id", "")] {
        let mut args = json!({"window": WINDOW, "id": "4.2", "action": "invoke"});
        args[key] = json!(bad);
        assert!(check_args("act", &args).is_err(), "{key} {bad}");
    }
}

#[test]
fn a_program_and_its_arguments_cannot_become_an_option_of_the_helper() {
    let ok = check_args("launch", &json!({"program": "  C:\\Program Files\\App\\app.exe ", "args": ["--port", "8080", "a b", "-x"]})).unwrap();
    assert_eq!(ok, json!({"program": "C:\\Program Files\\App\\app.exe", "args": ["--port", "8080", "a b", "-x"]}));
    assert_eq!(check_args("launch", &json!({"program": "notepad"})).unwrap(), json!({"program": "notepad", "args": []}));
    // The program is the first thing after `launch`, so a leading `-` would be read as an option by anything that parsed it.
    for bad in [json!({}), json!({"program": ""}), json!({"program": "   "}), json!({"program": "-x"}), json!({"program": "--max"}), json!({"program": "a\nb"}), json!({"program": "a\u{7}b"}), json!({"program": 5}), json!({"program": "x".repeat(MAX_PROGRAM + 1)})] {
        assert!(check_args("launch", &bad).is_err(), "{bad}");
    }
    for option in ["--max", "--rects", "--max=1", "--rects=yes"] {
        let e = check_args("launch", &json!({"program": "a.exe", "args": ["ok", option]})).unwrap_err();
        assert!(e.contains("own option"), "{option}: {e}");
    }
    // Only the exact options: an ordinary argument that begins alike is the program's.
    assert!(check_args("launch", &json!({"program": "a.exe", "args": ["--maximize", "--rectangle"]})).is_ok());
    let many: Vec<String> = (0..=MAX_ARGS).map(|n| n.to_string()).collect();
    assert!(check_args("launch", &json!({"program": "a.exe", "args": many})).unwrap_err().contains("at most 32"));
    assert!(check_args("launch", &json!({"program": "a.exe", "args": ["x".repeat(MAX_ARG + 1)]})).is_err());
    assert!(check_args("launch", &json!({"program": "a.exe", "args": ["a\0b"]})).is_err());
    assert!(check_args("launch", &json!({"program": "a.exe", "args": [1]})).is_err());
    assert!(check_args("launch", &json!({"program": "a.exe", "args": "x"})).is_err());
    let long: Vec<String> = (0..MAX_ARGS).map(|_| "y".repeat(MAX_ARG)).collect();
    assert!(check_args("launch", &json!({"program": "a.exe", "args": long})).unwrap_err().contains("too long in all"));
}

#[test]
fn the_command_line_of_each_tool() {
    let helper = Path::new("C:\\Kelpie\\kelpie-window.exe");
    let shot = Path::new("C:\\data\\window-shots\\s1\\1-0.png");
    let line = |tool: &str, args: Value, shot: Option<&Path>| command(helper, tool, &args, shot);
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(line("list", json!({}), None), (helper.to_path_buf(), args(&["list"])));
    assert_eq!(line("launch", json!({"program": "C:\\a b\\app.exe", "args": ["--flag", "x y"]}), None).1, args(&["launch", "C:\\a b\\app.exe", "--flag", "x y"]));
    assert_eq!(line("launch", json!({"program": "notepad", "args": []}), None).1, args(&["launch", "notepad"]));
    assert_eq!(line("tree", json!({"window": WINDOW}), None).1, args(&["tree", WINDOW]));
    assert_eq!(line("tree", json!({"window": WINDOW, "max": 50}), None).1, args(&["tree", WINDOW, "--max", "50"]));
    assert_eq!(line("act", json!({"window": WINDOW, "id": "4.2", "action": "invoke"}), None).1, args(&["act", WINDOW, "4.2", "invoke"]));
    assert_eq!(line("act", json!({"window": WINDOW, "id": "4.2", "action": "set", "text": "--max 5"}), None).1, args(&["act", WINDOW, "4.2", "set", "--max 5"]));
    assert_eq!(line("shot", json!({"window": WINDOW}), Some(shot)).1, args(&["shot", WINDOW, &shot.to_string_lossy()]));
    assert_eq!(line("close", json!({"window": WINDOW}), None).1, args(&["close", WINDOW]));
}

#[test]
fn the_helper_runs_with_the_session_as_its_scope_on_the_hidden_desktop_unless_told() {
    let helper = Path::new("kelpie-window");
    let hidden = invocation(helper, "list", &json!({}), None, "s-1", false, None, Duration::from_secs(20));
    assert_eq!(hidden.env, [("KELPIE_WINDOW_SCOPE".to_string(), "s-1".to_string()), ("KELPIE_WINDOW_DESKTOP".to_string(), "hidden".to_string())]);
    let visible = invocation(helper, "list", &json!({}), None, "s-1", true, None, Duration::from_secs(20));
    assert_eq!(visible.env, [("KELPIE_WINDOW_SCOPE".to_string(), "s-1".to_string())]);
    // What names this Kelpie or speaks for a session is not passed on to the helper, nor to what it starts.
    for name in ["KELPIE_ENDPOINT", "KELPIE_DATA_DIR", "KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"] {
        assert!(hidden.env_remove.contains(&name), "{name}");
    }
    assert_eq!((limit("launch"), limit("tree"), limit("shot")), (Duration::from_secs(30), Duration::from_secs(20), Duration::from_secs(20)));
}

#[test]
fn the_helper_is_looked_for_beside_the_daemon_then_beside_the_app() {
    let own = Path::new("C:\\data\\daemon\\a");
    let found = helper_candidates(Some(own), Some(" C:\\Program Files\\Kelpie \n"), Some("C:\\dev\\kelpie-window.exe"), false);
    assert_eq!(found, [own.join(HELPER_NAME), Path::new("C:\\Program Files\\Kelpie").join(HELPER_NAME)]);
    // A release build takes no path from the environment, and no development folder.
    assert!(!found.iter().any(|p| p.to_string_lossy().contains("dev")));
    let debug = helper_candidates(Some(Path::new("/repo/target/debug")), None, Some("/x/kelpie-window"), true);
    assert_eq!(debug, [PathBuf::from("/x/kelpie-window"), Path::new("/repo/target/debug").join(HELPER_NAME), Path::new("/repo/target").join("release").join(HELPER_NAME)]);
    assert!(helper_candidates(None, Some(""), None, false).is_empty());
}

#[test]
fn nothing_runs_while_the_policy_is_off_and_a_refusal_never_reaches_the_helper() {
    let dir = dir("off");
    let (runner, seen) = fake(|_| Ok("x".into()));
    let off = State::with_runner(Policy::default(), &dir, Path::new("kelpie-window"), runner.clone());
    for tool in TOOLS {
        assert_eq!(execute(&off, "s1", "agent", tool, &json!({}), 1).unwrap_err(), OFF, "{tool}");
    }
    assert!(seen.lock().is_empty());
    assert!(off.audit.tail(10).is_empty(), "a refusal leaves no audit line");
    let allowed = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner.clone());
    // A bad argument, an unknown tool and a session id that is not one the daemon makes.
    assert!(execute(&allowed, "s1", "agent", "act", &json!({"window": "x"}), 1).is_err());
    assert!(execute(&allowed, "s1", "agent", "drag", &json!({}), 1).is_err());
    assert!(execute(&allowed, "..\\evil", "agent", "list", &json!({}), 1).is_err());
    assert!(execute(&allowed, "", "agent", "list", &json!({}), 1).is_err());
    // A computer that cannot run the helper says so.
    let mut elsewhere = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    elsewhere.supported = false;
    assert_eq!(execute(&elsewhere, "s1", "agent", "list", &json!({}), 1).unwrap_err(), WINDOWS_ONLY);
    assert!(seen.lock().is_empty());
    assert!(allowed.audit.tail(10).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_call_runs_the_helper_for_its_session_and_brings_back_its_text() {
    let dir = dir("call");
    let (runner, seen) = fake(|_| Ok("w:131074:4242  notepad.exe  \"Untitled\"  800x600\n".into()));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let reply = execute(&state, "s1", "agent", "list", &json!({}), 7).unwrap();
    assert_eq!(reply, json!({"text": "w:131074:4242  notepad.exe  \"Untitled\"  800x600"}));
    let ran = seen.lock().clone();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].args, ["list"]);
    assert!(ran[0].env.contains(&("KELPIE_WINDOW_SCOPE".to_string(), "s1".to_string())));
    assert!(ran[0].env.contains(&("KELPIE_WINDOW_DESKTOP".to_string(), "hidden".to_string())));
    // A helper's complaint is the tool's error, scrubbed like any text a program wrote.
    let (runner, _) = fake(|_| Err("not a window of a program you started: https://a.test/p?token=S3CRET".into()));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let e = execute(&state, "s1", "agent", "tree", &json!({"window": WINDOW}), 8).unwrap_err();
    assert_eq!(e, "not a window of a program you started: https://a.test/p");
    // A reply is bounded.
    let (runner, _) = fake(|_| Ok("x".repeat(MAX_REPLY_CHARS * 2)));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let text = execute(&state, "s1", "agent", "list", &json!({}), 9).unwrap()["text"].as_str().unwrap().to_string();
    assert!(text.chars().count() <= MAX_REPLY_CHARS);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_person_s_desktop_needs_the_policy_and_is_remembered_for_the_session() {
    let dir = dir("visible");
    let (runner, seen) = fake(|_| Ok("ok".into()));
    let hidden_only = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner.clone());
    assert_eq!(execute(&hidden_only, "s1", "agent", "launch", &json!({"program": "app.exe", "visible": true}), 1).unwrap_err(), VISIBLE_NOT_ALLOWED);
    assert!(seen.lock().is_empty() && !hidden_only.has_session("s1"));
    let state = State::with_runner(on(true), &dir, Path::new("kelpie-window"), runner);
    let desktop = |n: usize| seen.lock()[n].env.iter().any(|(k, v)| k == "KELPIE_WINDOW_DESKTOP" && v == "hidden");
    execute(&state, "s1", "agent", "launch", &json!({"program": "app.exe", "visible": true}), 1).unwrap();
    assert!(!desktop(0), "started on the person's desktop");
    // The next call of the session means the same desktop unless it says otherwise; another session's does not.
    execute(&state, "s1", "agent", "list", &json!({}), 2).unwrap();
    execute(&state, "s1", "agent", "list", &json!({"visible": false}), 3).unwrap();
    execute(&state, "s2", "agent", "list", &json!({}), 4).unwrap();
    assert_eq!((desktop(1), desktop(2), desktop(3)), (false, true, true));
    assert!(state.has_session("s1") && !state.has_session("s2"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn acting_calls_are_audited_without_the_text_and_reading_ones_are_not() {
    let dir = dir("audit");
    let (runner, _) = fake(|_| Ok("done".into()));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    execute(&state, "s1", "a\nb agent", "launch", &json!({"program": "C:\\secret folder\\app.exe", "args": ["--token=hunter2"]}), 10).unwrap();
    execute(&state, "s1", "agent", "list", &json!({}), 11).unwrap();
    execute(&state, "s1", "agent", "tree", &json!({"window": WINDOW}), 12).unwrap();
    execute(&state, "s1", "agent", "act", &json!({"window": WINDOW, "id": "4.2", "action": "set", "text": "hunter2-password"}), 13).unwrap();
    execute(&state, "s1", "agent", "close", &json!({"window": WINDOW}), 14).unwrap();
    let lines = state.audit.tail(10);
    assert_eq!(lines.len(), 3, "launch, act and close only: {lines:?}");
    assert_eq!(lines[0]["tool"], "launch");
    assert_eq!(lines[0]["target"], "app.exe", "the program's file name, not its path or arguments");
    assert_eq!((lines[1]["tool"].clone(), lines[1]["target"].clone(), lines[1]["action"].clone()), (json!("act"), json!(WINDOW), json!("set")));
    assert_eq!(lines[2]["tool"], "close");
    assert!(lines.iter().all(|l| l["ok"] == true && l["session"] == "s1"));
    let file = std::fs::read_to_string(dir.join(AUDIT_FILE)).unwrap();
    for secret in ["hunter2", "secret folder"] {
        assert!(!file.contains(secret), "{secret}: {file}");
    }
    // A failure is recorded as one, with its reason scrubbed and bounded.
    let (runner, _) = fake(|_| Err(format!("the window is gone {}", "x".repeat(500))));
    let failing = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    execute(&failing, "s1", "agent", "close", &json!({"window": WINDOW}), 20).unwrap_err();
    let last = failing.audit.tail(1).remove(0);
    assert_eq!(last["ok"], false);
    assert!(last["error"].as_str().unwrap().chars().count() <= 120);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_has_a_rate_and_an_hourly_share_of_starts() {
    let dir = dir("rate");
    let (runner, seen) = fake(|_| Ok("ok".into()));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let results: Vec<_> = (0..25).map(|n| execute(&state, "s1", "agent", "list", &json!({}), n)).collect();
    // The bucket holds 20 and refills 10 a second: the first 20 pass, and the end of the burst is refused.
    assert!(results[..20].iter().all(Result::is_ok));
    let refused = results.iter().filter(|r| r.as_ref().is_err_and(|e| e == TOO_MANY)).count();
    assert!(refused >= 3 && results[24].is_err(), "{refused}");
    assert_eq!(seen.lock().len(), 25 - refused, "a refused call costs the helper nothing");
    // Another session is not slowed by it.
    assert!(execute(&state, "s2", "agent", "list", &json!({}), 99).is_ok());
    let mut launches = Launches::default();
    let t0 = Instant::now();
    for n in 0..LAUNCHES_PER_HOUR {
        assert!(launches.take("s1", t0 + Duration::from_secs(n as u64)), "{n}");
    }
    assert!(!launches.take("s1", t0 + Duration::from_secs(60)));
    assert!(launches.take("s2", t0 + Duration::from_secs(60)));
    assert!(launches.take("s1", t0 + HOUR + Duration::from_secs(1)), "an hour later the first start is out of the count");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_is_written_where_the_daemon_chose_and_comes_back_as_an_image() {
    let dir = dir("shot");
    let png = b"\x89PNG\r\n\x1a\nnot really a picture".to_vec();
    let bytes = png.clone();
    let (runner, seen) = fake(move |inv| {
        let path = inv.args.last().unwrap().clone();
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        Ok(format!("640x480 {path}\n"))
    });
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let reply = execute(&state, "s1", "agent", "shot", &json!({"window": WINDOW}), 5).unwrap();
    let path = PathBuf::from(reply["image"]["path"].as_str().unwrap());
    assert!(path.starts_with(dir.join("window-shots").join("s1")), "{path:?}");
    assert_eq!(seen.lock()[0].args[..2], ["shot".to_string(), WINDOW.to_string()]);
    assert_eq!((reply["image"]["mime"].clone(), reply["image"]["width"].clone(), reply["image"]["height"].clone()), (json!("image/png"), json!(640), json!(480)));
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(reply["image"]["data"].as_str().unwrap()).unwrap(), png);
    // The agent names no path: `path` is not an argument.
    assert!(check_args("shot", &json!({"window": WINDOW, "path": "C:\\x.png"})).unwrap_err().contains("takes no `path`"));
    // The folder holds the newest pictures only.
    let folder = dir.join("window-shots").join("s1");
    for n in 0..KEEP_SHOTS + 5 {
        std::fs::write(folder.join(format!("old-{n}.png")), b"x").unwrap();
    }
    trim_shots(&folder);
    let left = std::fs::read_dir(&folder).unwrap().count();
    assert_eq!(left, KEEP_SHOTS);
    // A picture too large to pass on stays in its file and is named.
    let big = vec![0u8; MAX_SHOT_BYTES + 1];
    let big_png = dir.join("big.png");
    std::fs::write(&big_png, &big).unwrap();
    let reply = shot_reply(&big_png, "9x9 big").unwrap();
    assert!(reply.get("image").is_none() && reply["text"].as_str().unwrap().contains("too large"), "{reply}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn too_many_helpers_at_once_are_refused() {
    let dir = dir("busy");
    let (runner, _) = fake(|_| Ok("ok".into()));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let counter = AtomicUsize::new(0);
    let held: Vec<_> = (0..MAX_RUNNING).map(|_| Running::start(&counter).unwrap()).collect();
    assert!(Running::start(&counter).is_none());
    drop(held);
    assert!(Running::start(&counter).is_some());
    state.running.store(MAX_RUNNING, Ordering::SeqCst);
    assert_eq!(execute(&state, "s1", "agent", "list", &json!({}), 1).unwrap_err(), BUSY);
    assert_eq!(state.running.load(Ordering::SeqCst), MAX_RUNNING, "a refused call leaves the count as it was");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn when_a_session_ends_the_windows_it_started_are_asked_to_close() {
    let dir = dir("end");
    let (runner, seen) = fake(|inv| {
        Ok(match inv.args[0].as_str() {
            "list" => "w:10:20  app.exe  \"Main\"  800x600\nw:11:20  app.exe dialog  \"Sure?\"  300x100\nsomething else\n".into(),
            _ => "asked to close".into(),
        })
    });
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    execute(&state, "s1", "agent", "launch", &json!({"program": "app.exe"}), 1).unwrap();
    std::fs::create_dir_all(dir.join("window-shots").join("s1")).unwrap();
    seen.lock().clear();
    end_session(&state, "s1");
    let ran: Vec<Vec<String>> = seen.lock().iter().map(|i| i.args.clone()).collect();
    assert_eq!(ran, [vec!["list".to_string()], vec!["close".to_string(), "w:10:20".to_string()], vec!["close".to_string(), "w:11:20".to_string()]]);
    assert!(seen.lock().iter().all(|i| i.env.contains(&("KELPIE_WINDOW_SCOPE".to_string(), "s1".to_string())) && i.limit <= Duration::from_secs(5)));
    assert!(!state.has_session("s1") && !dir.join("window-shots").join("s1").exists());
    // A session that started nothing costs no process; a second end does nothing.
    seen.lock().clear();
    end_session(&state, "s2");
    end_session(&state, "s1");
    assert!(seen.lock().is_empty());
    assert_eq!(listed_targets("w:1:2 x\n  w:3:4\nw:x:1\n\nw:5:6"), ["w:1:2", "w:3:4", "w:5:6"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_helper_is_run_as_a_process_with_bounded_output() {
    assert_eq!(read_bounded(&b"abcdef"[..], 4), b"abcd");
    assert_eq!(read_bounded(&b"ab"[..], 4), b"ab");
    let missing = Invocation { program: PathBuf::from("kelpie-window-that-does-not-exist"), args: Vec::new(), env: Vec::new(), env_remove: Vec::new(), limit: Duration::from_secs(1) };
    assert!(run_helper(&missing).unwrap_err().contains("could not be started"));
}

#[test]
fn a_job_name_is_one_the_helper_takes() {
    let sid = "s".repeat(64);
    let name = job::job_name(&sid, &uuid::Uuid::new_v4().simple().to_string());
    assert!(name.len() <= 160 && name.starts_with("kelpie-win-s"), "{name}");
    assert!(name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{name}");
    assert_ne!(job::job_name("s1", "aa"), job::job_name("s1", "bb"));
    assert_ne!(job::job_name("s1", "aa"), job::job_name("s2", "aa"));
}

#[test]
fn a_session_that_starts_a_program_gets_one_job_that_its_calls_and_its_end_name() {
    let dir = dir("job");
    let (runner, seen) = fake(|inv| Ok(if inv.args[0] == "list" { "w:10:20  app.exe  \"Main\"  800x600\n".into() } else { "ok".into() }));
    let state = State::with_runner(on(false), &dir, Path::new("kelpie-window"), runner);
    let job_of = |n: usize| seen.lock()[n].env.iter().find(|(k, _)| k == "KELPIE_WINDOW_JOB").map(|(_, v)| v.clone());
    // Before a start there is nothing to name; a start makes the job and names it, and the same name goes with every later call.
    execute(&state, "s1", "agent", "list", &json!({}), 1).unwrap();
    execute(&state, "s1", "agent", "launch", &json!({"program": "app.exe"}), 2).unwrap();
    execute(&state, "s1", "agent", "launch", &json!({"program": "app.exe"}), 3).unwrap();
    execute(&state, "s1", "agent", "list", &json!({}), 4).unwrap();
    let name = job_of(1).expect("a start names the job");
    assert_eq!((job_of(0), job_of(2), job_of(3)), (None, Some(name.clone()), Some(name.clone())));
    assert!(name.starts_with("kelpie-win-s1-"));
    // Another session has a job of its own.
    execute(&state, "s2", "agent", "launch", &json!({"program": "app.exe"}), 5).unwrap();
    let other = job_of(4).unwrap();
    assert!(other.starts_with("kelpie-win-s2-") && other != name);
    // The end of the session asks about its windows within its own job, then lets the job go.
    seen.lock().clear();
    end_session(&state, "s1");
    let calls = seen.lock().len();
    assert!(calls > 0 && (0..calls).all(|n| job_of(n).as_deref() == Some(name.as_str())));
    assert!(state.job_of("s1").is_none() && state.job_of("s2").as_deref() == Some(other.as_str()));
    let _ = std::fs::remove_dir_all(dir);
}
