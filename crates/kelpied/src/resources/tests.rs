use super::ledger::{fingerprint, Fingerprint};
use super::*;

fn proc_(pid: u32, name: &str, private_mb: u64, cpu: f32, io: u64) -> Proc {
    Proc { pid, ppid: None, name: name.into(), start_time: 1, exe: None, mem: private_mb << 20, cpu, private: private_mb << 20, io }
}

fn calm() -> MachineReading {
    MachineReading {
        cores: 4,
        cpu_pct: Some(20.0),
        cpu_queue: Some(1.0),
        mem_total_mb: Some(8000),
        mem_avail_mb: Some(4000),
        commit_limit_mb: Some(12000),
        commit_avail_mb: Some(6000),
        disks: vec![DiskReading { name: "0 C:".into(), busy_pct: Some(10.0), queue: Some(0.1), mbps: Some(5.0) }],
        volumes: vec![VolumeReading { path: "C:\\".into(), free_mb: 100_000, total_mb: 400_000 }],
        gpus: vec![GpuReading { id: "a".into(), util_pct: Some(3.0), ..GpuReading::default() }],
        ..MachineReading::default()
    }
}

fn session(id: &str, procs: Vec<Proc>) -> SessionSample {
    SessionSample { tree: TreeUsage { id: id.into(), procs }, project: Some("p".into()), peak_commit: None }
}

#[test]
fn each_resource_reads_its_own_signals() {
    assert_eq!(wants(&calm()), [Some(State::Ok); 5]);
    let mut r = calm();
    r.cpu_pct = Some(97.0);
    assert_eq!(wants(&r)[0], Some(State::Saturated));
    r.cpu_pct = Some(72.0);
    r.cpu_queue = Some(9.0);
    assert_eq!(wants(&r)[0], Some(State::Busy), "a long run queue on a busy machine");
    r.cpu_pct = Some(20.0);
    assert_eq!(wants(&r)[0], Some(State::Ok), "a long run queue on an idle machine is noise");

    let mut r = calm();
    r.mem_avail_mb = Some(1000);
    assert_eq!(wants(&r)[1], Some(State::Busy), "12.5 % free");
    r.low_memory = true;
    assert_eq!(wants(&r)[1], Some(State::Saturated));
    let mut r = calm();
    r.commit_avail_mb = Some(400);
    assert_eq!(wants(&r)[1], Some(State::Saturated), "the commit is almost used up");

    let mut r = calm();
    r.disks[0].busy_pct = Some(99.0);
    r.disks[0].queue = Some(4.0);
    assert_eq!(wants(&r)[2], Some(State::Saturated));
    r.disks.push(DiskReading { name: "1 D:".into(), busy_pct: Some(0.0), ..DiskReading::default() });
    assert_eq!(wants(&r)[2], Some(State::Saturated), "the busiest disk counts");

    let mut r = calm();
    r.volumes[0] = VolumeReading { path: "C:\\".into(), free_mb: 4000, total_mb: 40_000 };
    assert_eq!(wants(&r)[3], Some(State::Busy), "under 5 GiB free");
    r.volumes[0].free_mb = 1000;
    assert_eq!(wants(&r)[3], Some(State::Saturated));

    let mut r = calm();
    r.gpus[0].util_pct = Some(96.0);
    assert_eq!(wants(&r)[4], Some(State::Saturated));

    let mut r = calm();
    r.pressure = Pressure { cpu: Some(25.0), memory: Some(12.0), io: Some(40.0) };
    assert_eq!(&wants(&r)[..3], &[Some(State::Busy), Some(State::Saturated), Some(State::Saturated)], "Linux's pressure figures");
}

#[test]
fn what_the_machine_cannot_tell_is_unknown_not_idle() {
    let r = MachineReading { cores: 4, ..MachineReading::default() };
    assert_eq!(wants(&r), [None; 5]);
    let mut g = Gauge::default();
    assert!(!g.feed(0, None));
    assert!(g.feed(0, Some(State::Ok)));
    assert!(g.feed(1, None), "a counter that went away");
    assert_eq!(g.state, None);
}

#[test]
fn a_gauge_rises_after_two_samples_and_falls_after_ten_seconds() {
    let mut g = Gauge::default();
    g.feed(0, Some(State::Ok));
    assert!(!g.feed(2_000, Some(State::Saturated)), "one spike is not load");
    assert!(g.feed(4_000, Some(State::Busy)), "two in a row: the least they asked");
    assert_eq!(g.state, Some(State::Busy));
    assert!(!g.feed(6_000, Some(State::Ok)));
    assert!(!g.feed(12_000, Some(State::Ok)));
    assert!(!g.feed(14_000, Some(State::Busy)), "back up: the countdown starts over");
    assert!(!g.feed(16_000, Some(State::Ok)));
    assert!(!g.feed(24_000, Some(State::Ok)));
    assert!(g.feed(26_000, Some(State::Ok)));
    assert_eq!(g.state, Some(State::Ok));
}

#[test]
fn a_session_is_the_sum_of_its_tree_and_io_is_measured_between_samples() {
    let res = Resources::new(None);
    let tree = |io: u64| vec![proc_(10, "claude.exe", 300, 50.0, io), proc_(11, "cargo.exe", 700, 150.0, 0)];
    res.record(0, calm(), &tree(1 << 20), vec![session("s", tree(1 << 20))], 2.0);
    res.record(2_000, calm(), &tree(21 << 20), vec![session("s", tree(21 << 20))], 2.0);
    let snap = res.snapshot(true, None);
    let now = &snap["sessions"][0]["now"];
    assert_eq!(now["private_mb"], 1000.0);
    assert_eq!(now["cores"], 2.0);
    assert_eq!(now["io_mbps"], 10.0, "20 MiB in two seconds");
    assert_eq!(now["procs"], 2);
    assert_eq!(snap["sessions"][0]["history"].as_array().unwrap().len(), 2);
    assert_eq!(snap["gauges"]["cpu"], "ok");
    // A closed session is forgotten at the next sample.
    res.record(4_000, calm(), &[], vec![], 2.0);
    assert!(res.snapshot(false, None)["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn a_command_is_charged_what_the_tree_took_above_its_start() {
    let res = Resources::new(None);
    let agent = proc_(10, "claude.exe", 300, 5.0, 0);
    res.record(0, calm(), &[agent.clone()], vec![session("s", vec![agent.clone()])], 2.0);
    res.command_started("s", "call-1", "cd crates && CARGO_BUILD_JOBS=2 cargo build --release -p kelpied", "proj", 1_000);
    for (t, build_mb, cpu) in [(2_000, 900, 180.0), (4_000, 1500, 200.0)] {
        let tree = vec![agent.clone(), proc_(11, "rustc.exe", build_mb, cpu, 0)];
        res.record(t, calm(), &tree, vec![session("s", tree.clone())], 2.0);
    }
    let running = &res.snapshot(false, Some("s"))["sessions"][0]["commands"][0];
    assert_eq!(running["fingerprint"], "cargo build --release -p kelpied");
    assert_eq!(running["jobs"], 2);
    res.command_ended("s", "call-1", false, 5_000);
    let learned = res.ledger(Some("proj"));
    let e = &learned[0];
    assert_eq!(e["fingerprint"], "cargo build --release -p kelpied");
    assert_eq!(e["runs"], 1);
    assert_eq!(e["ram_mb"], 1500.0, "the agent's own 300 MiB are not the build's");
    assert!((e["cores"].as_f64().unwrap() - 2.0).abs() < 0.11, "{}", e["cores"]);
    assert_eq!(e["secs"], 4.0);
    // A command no sample saw is not learned, and one with no end is ended by the turn.
    res.command_started("s", "call-2", "cargo test", "proj", 5_100);
    res.command_ended("s", "call-2", false, 5_200);
    res.command_started("s", "call-3", "pnpm build", "proj", 5_300);
    let tree = vec![agent.clone(), proc_(12, "node.exe", 400, 100.0, 0)];
    res.record(6_000, calm(), &tree, vec![session("s", tree.clone())], 2.0);
    res.turn_ended("s", 7_000);
    let keys: Vec<String> = res.ledger(None).as_array().unwrap().iter().map(|e| e["fingerprint"].as_str().unwrap().to_string()).collect();
    assert_eq!(keys, ["pnpm build", "cargo build --release -p kelpied"]);
}

#[test]
fn a_contended_resource_names_its_biggest_users() {
    let res = Resources::new(None);
    let mut hot = calm();
    hot.cpu_pct = Some(99.0);
    let tree = vec![proc_(10, "cargo.exe", 100, 300.0, 0)];
    let all = [tree.clone(), vec![proc_(20, "chrome.exe", 500, 40.0, 0), proc_(21, "chrome.exe", 300, 30.0, 0), proc_(30, "idle.exe", 1, 0.0, 0)]].concat();
    res.record(0, calm(), &all, vec![session("s", tree.clone())], 2.0);
    res.record(2_000, hot.clone(), &all, vec![session("s", tree.clone())], 2.0);
    res.record(4_000, hot, &all, vec![session("s", tree)], 2.0);
    let snap = res.snapshot(false, None);
    assert_eq!(snap["gauges"]["cpu"], "saturated");
    let users = snap["users"]["cpu"].as_array().unwrap();
    assert_eq!(users[0]["session"], "s");
    assert_eq!(users[0]["value"], 3.0);
    assert_eq!(users[1]["program"], "chrome.exe");
    assert_eq!(users[1]["count"], 2);
    assert!((users[1]["value"].as_f64().unwrap() - 0.7).abs() < 1e-9);
    assert_eq!(users.len(), 2, "a program using none of it is not listed");
    assert!(snap["users"].get("ram").is_none(), "only contended resources");
}

#[test]
fn sub_sessions_stop_only_when_memory_is_saturated() {
    let res = Resources::new(None);
    let mut short = calm();
    short.low_memory = true;
    res.record(0, calm(), &[], vec![], 2.0);
    assert_eq!(res.caps_level(), Level::L0);
    res.record(2_000, short.clone(), &[], vec![], 2.0);
    res.record(4_000, short, &[], vec![], 2.0);
    assert_eq!(res.caps_level(), Level::L3);
    let mut cpu = calm();
    cpu.cpu_pct = Some(100.0);
    let res = Resources::new(None);
    res.record(0, cpu.clone(), &[], vec![], 2.0);
    assert_eq!(res.caps_level(), Level::L0, "a busy CPU queues work, it does not refuse it");
}

fn fp(line: &str) -> Option<(String, Option<u32>)> {
    fingerprint(line).map(|Fingerprint { key, jobs }| (key, jobs))
}

#[test]
fn a_command_is_known_by_what_does_the_work() {
    let k = |key: &str, jobs: Option<u32>| Some((key.to_string(), jobs));
    assert_eq!(fp("cargo build"), k("cargo build", None));
    assert_eq!(fp("cd app && FOO=1 pnpm build 2>&1 | tee log"), k("pnpm build", None));
    assert_eq!(fp("CARGO_BUILD_JOBS=2 cargo +nightly test --workspace -p b -p a --release"), k("cargo test --workspace --release -p a -p b", Some(2)));
    assert_eq!(fp("cargo build -j 3"), k("cargo build", Some(3)));
    assert_eq!(fp("cargo build -j3"), k("cargo build", Some(3)));
    assert_eq!(fp("cargo build --jobs=4"), k("cargo build", Some(4)));
    assert_eq!(fp("npx playwright test --workers=2"), k("playwright test", Some(2)));
    assert_eq!(fp("pnpm exec vitest run --maxWorkers 2"), k("vitest run", Some(2)));
    assert_eq!(fp("python -m pytest -n 4 tests/"), k("pytest", Some(4)));
    assert_eq!(fp("uv run pytest"), k("pytest", None));
    assert_eq!(fp("go build -p 2 ./..."), k("go build", Some(2)));
    assert_eq!(fp("C:\\Users\\x\\.cargo\\bin\\cargo.exe check"), k("cargo check", None));
    assert_eq!(fp("\"C:/Program Files/nodejs/node.exe\" build.js"), k("node", None));
    assert_eq!(fp("git commit -n -m 'x'"), k("git commit", None), "-n with no number is not a parallelism");
    assert_eq!(fp("sudo nice make -j8 all"), k("make all", Some(8)));
    assert_eq!(fp("make -j 8 all"), k("make all", Some(8)));
    assert_eq!(fp("next build"), k("next build", None));
    assert_eq!(fp("cd foo; echo hi"), None, "nothing in it does work");
    assert_eq!(fp(""), None);
    assert_eq!(fp("echo 'a && b' && cargo fmt"), k("cargo fmt", None), "quotes keep their separators");
}

#[test]
fn the_ledger_learns_runs_and_keeps_what_it_learned() {
    let dir = std::env::temp_dir().join(format!("kelpie-ledger-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("command-ledger.json");
    let f = fingerprint("cargo build").unwrap();
    let mut l = Ledger::open(Some(path.clone()));
    for (i, ram) in [1000.0, 1200.0, 1100.0, 3000.0].into_iter().enumerate() {
        l.learn("p", &f, &Demand { ram_mb: ram, cores: 2.0, secs: 30.0, ..Demand::default() }, i == 3, 1_000 + i as u64);
    }
    l.save();
    l.learn("q", &f, &Demand::default(), false, 2_000);
    l.save();
    let again = Ledger::open(Some(path.clone()));
    let e = again.get("p", &f).unwrap();
    assert_eq!((e.runs, e.failed), (4, 1));
    assert_eq!(e.ram_peaks, [1000.0, 1200.0, 1100.0, 3000.0]);
    assert!(e.ram_mb.mean > 1100.0 && e.ram_mb.mean < 3000.0);
    assert_eq!(e.plan_ram_mb(), 3000.0, "the largest recent peak when it is above the spread");
    assert!(again.get("other", &f).is_none(), "per project");
    assert!(again.get("p", &fingerprint("cargo build -j 2").unwrap()).is_none(), "per parallelism");
    // A damaged file is not saved over by an empty ledger.
    std::fs::write(&path, b"{not json").unwrap();
    let mut empty = Ledger::open(Some(path.clone()));
    assert_eq!(empty.len(), 1, "restored from the previous version (before `q`)");
    empty.save();
    let _ = std::fs::remove_dir_all(&dir);
}
