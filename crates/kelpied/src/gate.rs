//! The command gate: what a heavy command an agent is about to run will take, and whether it runs now, runs with less
//! parallelism, or waits a little.
//!
//! The agent's own hook asks before the command runs (Claude's `PreToolUse` on `Bash`). This module is the pure part:
//! - **Profile:** which commands are heavy, what each takes before anything is learned (a base plus a share per parallel job),
//!   and the flag that sets its parallelism.
//! - **Estimate:** what it takes now, from the command ledger (`resources/ledger.rs`) when it ran in this project before, from
//!   the profile otherwise.
//! - **Decide:** the cores and memory are shared by priority weight between it and the heavy commands already running. It
//!   runs as it is when its share holds it, runs with fewer jobs when a lower parallelism fits (the person allows rewriting
//!   per tier), and waits when even one job does not fit. Two heavy disk writers on a busy disk, or two GPU jobs, take
//!   turns. A command always runs when no other heavy command does, so nothing waits for nothing.
//!
//! The daemon bounds every wait and then lets the command run (with the least parallelism it allows), so a gate can slow
//! an agent but never stop it.

use crate::resources::ledger::{Entry, Fingerprint};

/// How a command's parallelism is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knob {
    /// `CARGO_BUILD_JOBS=N` before it, or its own `-j N` changed.
    Cargo,
    /// `-jN` after the program (make, ninja).
    MakeJobs,
    /// `-p N` after the subcommand (go).
    GoP,
    /// `--workers=N` after the subcommand (playwright test).
    Workers,
    /// `--maxWorkers=N` after the program or subcommand (vitest, jest).
    MaxWorkers,
}

/// What a kind of command takes before anything is learned about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Profile {
    pub heavy: bool,
    /// Memory it takes whatever its parallelism (a link, a bundler's graph), MiB.
    pub base_mb: f64,
    /// And per parallel job, MiB.
    pub per_job_mb: f64,
    /// Cores per job.
    pub cores_per_job: f64,
    /// Writes a lot to the disk (installs, image builds).
    pub io: bool,
    pub gpu: bool,
    pub knob: Option<Knob>,
    /// Jobs when nothing sets them: none means one per core.
    pub default_jobs: Option<u32>,
}

const LIGHT: Profile = Profile { heavy: false, base_mb: 0.0, per_job_mb: 0.0, cores_per_job: 0.0, io: false, gpu: false, knob: None, default_jobs: Some(1) };

const fn heavy(base_mb: f64, per_job_mb: f64, cores_per_job: f64, knob: Option<Knob>, default_jobs: Option<u32>) -> Profile {
    Profile { heavy: true, base_mb, per_job_mb, cores_per_job, io: false, gpu: false, knob, default_jobs }
}

/// The profile of a command by its fingerprint (`cargo build --release`, `pnpm build`, `playwright test`). These are
/// starting points only: the ledger's measurements replace them after one run in the project.
pub fn profile(f: &Fingerprint) -> Profile {
    let mut words = f.key.split_whitespace();
    let program = words.next().unwrap_or("");
    let sub = words.next().unwrap_or("");
    let release = f.key.contains("--release");
    match (program, sub) {
        ("cargo", "build" | "check" | "clippy" | "test" | "run" | "bench" | "doc" | "install" | "nextest") => {
            heavy(if release { 1500.0 } else { 700.0 }, if release { 600.0 } else { 350.0 }, 1.0, Some(Knob::Cargo), None)
        }
        ("rustc", _) => heavy(500.0, 0.0, 1.0, None, Some(1)),
        ("make" | "ninja", _) => heavy(200.0, 300.0, 1.0, Some(Knob::MakeJobs), Some(1)),
        ("go", "build" | "test" | "install" | "vet") => heavy(300.0, 250.0, 1.0, Some(Knob::GoP), None),
        ("pnpm" | "npm" | "yarn" | "bun", "install" | "i" | "ci" | "add") => Profile { io: true, ..heavy(600.0, 0.0, 1.0, None, Some(1)) },
        ("pnpm" | "npm" | "yarn" | "bun", "run" | "build" | "test") => heavy(1200.0, 0.0, 2.0, None, Some(1)),
        ("next" | "vite" | "webpack" | "turbo" | "nx", _) => heavy(1500.0, 0.0, 2.0, None, Some(1)),
        ("tsc", _) => heavy(800.0, 0.0, 1.0, None, Some(1)),
        ("playwright", "test") => heavy(300.0, 600.0, 1.0, Some(Knob::Workers), None),
        ("vitest", _) | ("jest", _) => heavy(300.0, 300.0, 1.0, Some(Knob::MaxWorkers), None),
        ("pytest", _) => heavy(400.0, 300.0, 1.0, None, Some(1)),
        ("gradle" | "gradlew" | "mvn" | "dotnet" | "xcodebuild", _) => heavy(1500.0, 0.0, 2.0, None, Some(1)),
        ("docker", "build" | "compose" | "pull") => Profile { io: true, ..heavy(1500.0, 0.0, 2.0, None, Some(1)) },
        ("ffmpeg", _) => Profile { gpu: f.key.contains("hwaccel"), ..heavy(500.0, 0.0, 2.0, None, Some(1)) },
        ("magick" | "convert" | "inkscape" | "gs" | "gswin64c" | "blender", _) => heavy(1000.0, 0.0, 1.0, None, Some(1)),
        _ => LIGHT,
    }
}

/// What a command will take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub ram_mb: f64,
    pub cores: f64,
    pub jobs: u32,
    /// From the ledger (a run of it here was measured), not the profile.
    pub learned: bool,
}

/// The jobs it would run with as written on a machine of `cores`.
pub fn jobs_as_written(f: &Fingerprint, p: &Profile, cores: u32) -> u32 {
    f.jobs.or(p.default_jobs).unwrap_or(cores).max(1)
}

/// What it takes at `jobs`: the ledger's plan for this command at this parallelism when there is one, else the profile.
pub fn estimate(p: &Profile, jobs: u32, learned: Option<&Entry>) -> Estimate {
    if let Some(e) = learned.filter(|e| e.runs > 0) {
        return Estimate { ram_mb: e.plan_ram_mb(), cores: e.peak_cores.mean.max(1.0), jobs, learned: true };
    }
    Estimate { ram_mb: p.base_mb + p.per_job_mb * f64::from(jobs), cores: (p.cores_per_job * f64::from(jobs)).max(1.0), jobs, learned: false }
}

/// A heavy command running now, as the gate charged it.
#[derive(Debug, Clone, PartialEq)]
pub struct Running {
    pub session: String,
    pub fingerprint: String,
    pub ram_mb: f64,
    pub cores: f64,
    pub io: bool,
    pub gpu: bool,
    pub weight: f64,
}

/// The computer as the gate sees it.
#[derive(Debug, Clone, Default)]
pub struct Machine {
    pub cores: u32,
    /// Memory available now (none: cannot tell, and memory is not checked), MiB.
    pub avail_mb: Option<f64>,
    /// Kept free for the system, and reserved for others with a live session, MiB.
    pub floor_mb: f64,
    pub reserved_others_mb: f64,
    /// What commands that started in the last half minute will still take (the reading does not show it yet), MiB.
    pub pending_mb: f64,
    /// The disk is busy or saturated.
    pub disk_busy: bool,
}

/// What the agent asks to run.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    pub fingerprint: &'a Fingerprint,
    pub profile: Profile,
    pub learned: Option<&'a Entry>,
    pub weight: f64,
    /// Critical or boosted: it may use what is kept free for others.
    pub critical: bool,
    /// The person lets Kelpie lower this tier's parallelism, and the agent's mode accepts a changed command.
    pub may_rewrite: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// As written.
    Run(Estimate),
    /// With this many jobs instead.
    Rewrite(Estimate),
    /// Not yet, and why.
    Hold(String),
}

/// Whether `ask` runs now, among `running` on `m`.
pub fn decide(ask: &Ask, running: &[Running], m: &Machine) -> Verdict {
    let p = &ask.profile;
    let written = jobs_as_written(ask.fingerprint, p, m.cores);
    let as_written = estimate(p, written, ask.learned);
    if !p.heavy || running.is_empty() && !ask.may_rewrite {
        return Verdict::Run(as_written);
    }
    if let Some(other) = running.iter().find(|r| p.gpu && r.gpu) {
        return Verdict::Hold(format!("the GPU is busy with `{}`", other.fingerprint));
    }
    if let Some(other) = running.iter().find(|r| p.io && r.io && m.disk_busy) {
        return Verdict::Hold(format!("the disk is busy with `{}`", other.fingerprint));
    }
    // Its share of the cores, by weight against the heavy commands running.
    let total_weight = ask.weight + running.iter().map(|r| r.weight).sum::<f64>();
    let core_share = ((f64::from(m.cores) * ask.weight / total_weight).floor() as u32).max(1);
    // The memory it may take: what is available beyond what is kept free and what recent starts will still take.
    let free = m.avail_mb.map(|a| a - m.pending_mb - if ask.critical { m.floor_mb / 2.0 } else { m.floor_mb + m.reserved_others_mb });
    let fits = |e: &Estimate| free.is_none_or(|f| e.ram_mb <= f);
    let cores_fit = |e: &Estimate| e.jobs <= core_share || p.knob.is_none() || running.is_empty();
    if fits(&as_written) && cores_fit(&as_written) {
        return Verdict::Run(as_written);
    }
    // Fewer jobs, when the command has a knob and the person allows it: the most that fits both shares.
    if ask.may_rewrite && p.knob.is_some() {
        let by_memory = |jobs: u32| fits(&estimate(p, jobs, None));
        let mut jobs = written.min(core_share);
        while jobs > 1 && !by_memory(jobs) {
            jobs -= 1;
        }
        // The ledger knows this command at its written parallelism only: the profile scales it down.
        let lower = estimate(p, jobs, None);
        if jobs < written && fits(&lower) {
            return Verdict::Rewrite(lower);
        }
    }
    if running.is_empty() {
        return Verdict::Run(as_written);
    }
    let need = if p.knob.is_some() && ask.may_rewrite { estimate(p, 1, None).ram_mb } else { as_written.ram_mb };
    Verdict::Hold(match free {
        Some(f) => format!("it needs about {need:.0} MiB and {:.0} MiB is free beyond what is kept for the system and others", f.max(0.0)),
        None => "the cores are shared with the heavy commands running".to_string(),
    })
}

/// `command` with `jobs` set by `knob` in the segment that does the work (`f`'s program); none when the segment is not found.
pub fn rewrite(command: &str, f: &Fingerprint, knob: Knob, jobs: u32) -> Option<String> {
    let mut words = f.key.split_whitespace();
    let program = words.next()?;
    let sub = words.next();
    // The program's word in the command (as written: `cargo`, `C:/…/cargo.exe`, `npx playwright`).
    let at = find_word(command, program)?;
    let after_program = at + word_len(command, at);
    let after_sub = sub.and_then(|s| find_word(&command[after_program..], s).map(|i| after_program + i + s.len())).unwrap_or(after_program);
    let insert = |pos: usize, text: &str| format!("{}{}{}", &command[..pos], text, &command[pos..]);
    match knob {
        Knob::Cargo => {
            let segment_end = segment_end(command, at);
            if let Some(re) = flag_value(&command[at..segment_end], &["-j", "--jobs"]) {
                return Some(format!("{}{}{}", &command[..at + re.0], jobs, &command[at + re.1..]));
            }
            // Before the program: `CARGO_BUILD_JOBS=2 cargo build`, so arguments after `--` stay the tests'.
            Some(insert(at, &format!("CARGO_BUILD_JOBS={jobs} ")))
        }
        Knob::MakeJobs => {
            let segment_end = segment_end(command, at);
            if let Some(re) = flag_value(&command[at..segment_end], &["-j", "--jobs"]) {
                return Some(format!("{}{}{}", &command[..at + re.0], jobs, &command[at + re.1..]));
            }
            Some(insert(after_program, &format!(" -j{jobs}")))
        }
        Knob::GoP => Some(insert(after_sub, &format!(" -p {jobs}"))),
        Knob::Workers | Knob::MaxWorkers => {
            let flag = if knob == Knob::Workers { "--workers" } else { "--maxWorkers" };
            let segment_end = segment_end(command, at);
            if let Some(re) = flag_value(&command[at..segment_end], &[flag]) {
                return Some(format!("{}{}{}", &command[..at + re.0], jobs, &command[at + re.1..]));
            }
            Some(insert(after_sub, &format!(" {flag}={jobs}")))
        }
    }
}

/// Where `word` starts as a whole word in `s` (or as the stem of a path to it: `C:/…/cargo.exe`).
fn find_word(s: &str, word: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut from = 0;
    while let Some(i) = s[from..].to_ascii_lowercase().find(word).map(|i| i + from) {
        let before_ok = i == 0 || matches!(bytes[i - 1], b' ' | b'\t' | b'/' | b'\\' | b'"' | b'\'' | b';' | b'&' | b'|' | b'(');
        let end = i + word.len();
        let after_ok = end == s.len() || matches!(bytes[end], b' ' | b'\t' | b'.' | b'"' | b'\'' | b';' | b'&' | b'|' | b')');
        if before_ok && after_ok {
            // Back to the start of a path or quote, so an inserted prefix goes before the whole program word.
            let mut start = i;
            while start > 0 && !matches!(bytes[start - 1], b' ' | b'\t' | b';' | b'&' | b'|' | b'(') {
                start -= 1;
            }
            return Some(start);
        }
        from = i + 1;
    }
    None
}

/// The length of the word starting at `at` (through a quoted path).
fn word_len(s: &str, at: usize) -> usize {
    let rest = &s[at..];
    if let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
        return rest[1..].find(q).map_or(rest.len(), |i| i + 2);
    }
    rest.find([' ', '\t', ';', '&', '|', ')']).unwrap_or(rest.len())
}

/// Where the command segment that starts at `at` ends (the next `&&`, `||`, `;`, `|`).
fn segment_end(s: &str, at: usize) -> usize {
    s[at..].find(['&', '|', ';']).map_or(s.len(), |i| at + i)
}

/// The byte range of the number after one of `flags` in `s` (`-j 4`, `-j4`, `--jobs=4`).
fn flag_value(s: &str, flags: &[&str]) -> Option<(usize, usize)> {
    for flag in flags {
        let mut from = 0;
        while let Some(i) = s[from..].find(flag).map(|i| i + from) {
            let mut j = i + flag.len();
            if s[j..].starts_with('=') || s[j..].starts_with(' ') {
                j += 1;
            }
            let digits = s[j..].bytes().take_while(u8::is_ascii_digit).count();
            let boundary = i == 0 || s.as_bytes()[i - 1] == b' ';
            if digits > 0 && boundary {
                return Some((j, j + digits));
            }
            from = i + 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ledger::fingerprint;

    fn fp(c: &str) -> Fingerprint {
        fingerprint(c).unwrap()
    }

    fn machine(avail: f64) -> Machine {
        Machine { cores: 4, avail_mb: Some(avail), floor_mb: 1000.0, ..Machine::default() }
    }

    fn ask<'a>(f: &'a Fingerprint, weight: f64) -> Ask<'a> {
        Ask { fingerprint: f, profile: profile(f), learned: None, weight, critical: false, may_rewrite: true }
    }

    fn build(weight: f64) -> Running {
        Running { session: "other".into(), fingerprint: "cargo build".into(), ram_mb: 2000.0, cores: 2.0, io: false, gpu: false, weight }
    }

    #[test]
    fn heavy_commands_are_known_and_light_ones_pass() {
        assert!(profile(&fp("cargo build --release")).heavy);
        assert_eq!(profile(&fp("pnpm install")).io, true);
        assert_eq!(profile(&fp("npx playwright test")).knob, Some(Knob::Workers));
        assert!(!profile(&fp("git status")).heavy);
        assert!(!profile(&fp("ls -la")).heavy);
        let f = fp("git status");
        assert!(matches!(decide(&ask(&f, 2.0), &[build(2.0)], &machine(100.0)), Verdict::Run(_)), "a light command never waits");
    }

    #[test]
    fn a_command_alone_runs_even_when_memory_is_short_but_with_fewer_jobs_when_it_can() {
        let f = fp("cargo build");
        // Four jobs: 700 + 4 x 350 = 2100 MiB; 2200 available - 1000 kept = 1200 free: two jobs (1400) do not fit, one (1050) does.
        match decide(&ask(&f, 2.0), &[], &machine(2200.0)) {
            Verdict::Rewrite(e) => assert_eq!(e.jobs, 1),
            v => panic!("{v:?}"),
        }
        let none = Ask { may_rewrite: false, ..ask(&f, 2.0) };
        assert!(matches!(decide(&none, &[], &machine(500.0)), Verdict::Run(_)), "alone, it always runs");
    }

    #[test]
    fn the_cores_are_shared_by_weight_with_what_runs() {
        let f = fp("cargo build");
        // Weight 2 against a running build of weight 2: half of 4 cores.
        match decide(&ask(&f, 2.0), &[build(2.0)], &machine(8000.0)) {
            Verdict::Rewrite(e) => assert_eq!(e.jobs, 2),
            v => panic!("{v:?}"),
        }
        // A boosted command (weight 8) against a background build (1): 3 of 4 cores.
        match decide(&ask(&f, 8.0), &[build(1.0)], &machine(8000.0)) {
            Verdict::Rewrite(e) => assert_eq!(e.jobs, 3),
            v => panic!("{v:?}"),
        }
        // A command that sets its own -j 1 already fits.
        let one = fp("cargo build -j 1");
        assert!(matches!(decide(&ask(&one, 2.0), &[build(2.0)], &machine(8000.0)), Verdict::Run(_)));
    }

    #[test]
    fn a_command_that_cannot_fit_waits_and_the_disk_and_the_gpu_take_turns() {
        let f = fp("cargo build");
        let v = decide(&ask(&f, 2.0), &[build(2.0)], &machine(1500.0));
        assert!(matches!(&v, Verdict::Hold(why) if why.contains("1050 MiB")), "{v:?}");
        let install = fp("pnpm install");
        let writer = Running { io: true, fingerprint: "docker build".into(), ..build(1.0) };
        let busy = Machine { disk_busy: true, ..machine(8000.0) };
        assert!(matches!(decide(&ask(&install, 4.0), &[writer.clone()], &busy), Verdict::Hold(why) if why.contains("disk")));
        assert!(!matches!(decide(&ask(&install, 4.0), &[writer], &machine(8000.0)), Verdict::Hold(_)), "a calm disk is shared");
        // Critical work may use what is kept free for others.
        let critical = Ask { critical: true, ..ask(&f, 4.0) };
        assert!(!matches!(decide(&critical, &[build(2.0)], &Machine { reserved_others_mb: 3000.0, ..machine(4000.0) }), Verdict::Hold(_)));
    }

    #[test]
    fn the_parallelism_is_set_where_the_tool_reads_it() {
        let rw = |c: &str, jobs| rewrite(c, &fp(c), profile(&fp(c)).knob.unwrap(), jobs).unwrap();
        assert_eq!(rw("cargo build --release", 2), "CARGO_BUILD_JOBS=2 cargo build --release");
        assert_eq!(rw("cd crates && cargo test -p kelpied -- --nocapture", 1), "cd crates && CARGO_BUILD_JOBS=1 cargo test -p kelpied -- --nocapture");
        assert_eq!(rw("cargo build -j 8 && echo ok", 2), "cargo build -j 2 && echo ok");
        assert_eq!(rw("\"C:/Users/x/.cargo/bin/cargo.exe\" check", 3), "CARGO_BUILD_JOBS=3 \"C:/Users/x/.cargo/bin/cargo.exe\" check");
        assert_eq!(rw("make -j8 all", 2), "make -j2 all");
        assert_eq!(rw("make all", 2), "make -j2 all");
        assert_eq!(rw("go test ./...", 2), "go test -p 2 ./...");
        assert_eq!(rw("npx playwright test e2e/", 1), "npx playwright test --workers=1 e2e/");
        assert_eq!(rw("npx playwright test --workers=4", 2), "npx playwright test --workers=2");
        assert_eq!(rw("pnpm exec vitest run", 2), "pnpm exec vitest run --maxWorkers=2");
    }
}
