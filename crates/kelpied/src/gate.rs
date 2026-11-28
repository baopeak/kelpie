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
    /// `-j N` after the subcommand (before any `--`), or its own `-j` changed.
    Cargo,
    /// `-jN` at the end of the make or ninja command, or its own `-j` changed.
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
    // The cores are shared by lowering the parallelism: a command that cannot be lowered is never held for cores alone.
    let can_lower = ask.may_rewrite && p.knob.is_some();
    let cores_fit = |e: &Estimate| !can_lower || e.jobs <= core_share || running.is_empty();
    if fits(&as_written) && cores_fit(&as_written) {
        return Verdict::Run(as_written);
    }
    // Fewer jobs, when the command has a knob and the person allows it: the most that fits both shares.
    if can_lower {
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
    let need = if can_lower { estimate(p, 1, None).ram_mb } else { as_written.ram_mb };
    Verdict::Hold(match free {
        Some(f) => format!("it needs about {need:.0} MiB and {:.0} MiB is free beyond what is kept for the system and others", f.max(0.0)),
        None => "the cores are shared with the heavy commands running".to_string(),
    })
}

/// A word of a command line: where it is (quotes included) and what it says (quotes taken off).
#[derive(Debug)]
struct Tok {
    start: usize,
    end: usize,
    text: String,
}

/// The words of `line` (one segment, no separators), quote-aware; none when a quote is left open.
fn tokens(line: &str, base: usize) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let (mut start, mut text, mut quote, mut open) = (0, String::new(), None::<char>, false);
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => text.push(c),
            (None, '"' | '\'') => {
                if !open {
                    (start, open) = (i, true);
                }
                quote = Some(c);
            }
            (None, c) if c.is_whitespace() => {
                if open {
                    out.push(Tok { start: base + start, end: base + i, text: std::mem::take(&mut text) });
                    open = false;
                }
            }
            (None, c) => {
                if !open {
                    (start, open) = (i, true);
                }
                text.push(c);
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if open {
        out.push(Tok { start: base + start, end: base + line.len(), text });
    }
    Some(out)
}

/// The byte ranges of `command`'s segments, split at `&&`, `||`, `;` and `|` outside quotes; none for a command of
/// several lines or with a quote left open (it is not changed then).
fn segment_ranges(command: &str) -> Option<Vec<(usize, usize)>> {
    let mut out = Vec::new();
    let (mut start, mut quote) = (0, None::<char>);
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '\n' | '\r') => return None,
            (None, '&' | '|') if bytes.get(i + 1) == Some(&bytes[i]) => {
                out.push((start, i));
                i += 1;
                start = i + 1;
            }
            (None, ';' | '|') => {
                out.push((start, i));
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    out.push((start, command.len()));
    Some(out)
}

/// `C:/tools/cargo.exe` → `cargo`.
fn stem(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    [".exe", ".cmd", ".bat"].iter().find_map(|e| name.strip_suffix(e).map(str::to_string)).unwrap_or(name)
}

/// Runners whose next word is the program (`npx playwright`, `pnpm exec vitest`).
fn is_runner(words: &[&str]) -> bool {
    matches!(words, ["npx"] | ["pnpx"] | ["bunx"]) || matches!(words, [pm, "exec" | "dlx" | "x"] if matches!(*pm, "pnpm" | "yarn" | "npm" | "bun"))
}

/// `command` with its parallelism set to `jobs` by `knob`, in the one segment that does the work (`f`), so that the command
/// still starts with the same words (a person's allow rule such as `Bash(cargo build:*)` still matches it). None, and the
/// command is left as it is, whenever that cannot be done for sure: several lines, a wrapper before the program (`timeout`,
/// `nice`, `env`), the program not where the fingerprint says, or a flag only after `--` (the program's own arguments).
pub fn rewrite(command: &str, f: &Fingerprint, knob: Knob, jobs: u32) -> Option<String> {
    let mut want = f.key.split_whitespace();
    let program = want.next()?;
    let sub = want.next().filter(|w| !w.starts_with('-'));
    for (s, e) in segment_ranges(command)? {
        let seg = &command[s..e];
        if crate::resources::ledger::fingerprint(seg).as_ref().map(|g| &g.key) != Some(&f.key) {
            continue;
        }
        let toks = tokens(seg, s)?;
        // Leading `VAR=value` assignments are kept; any other wrapper means the command is not changed.
        let first = toks.iter().position(|t| !(t.text.contains('=') && t.text.split('=').next().is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))))?;
        // The program, right away or after a runner (`npx`, `pnpm exec`).
        let p = (first..toks.len().min(first + 3)).find(|i| stem(&toks[*i].text) == program)?;
        let before: Vec<String> = toks[first..p].iter().map(|t| stem(&t.text)).collect();
        if p > first && !is_runner(&before.iter().map(String::as_str).collect::<Vec<_>>()) {
            return None;
        }
        let dash = toks.iter().position(|t| t.text == "--").unwrap_or(toks.len());
        let at_sub = match sub {
            Some(sub) => (p + 1..dash).find(|i| toks[*i].text.eq_ignore_ascii_case(sub))?,
            None => p,
        };
        let flags: &[&str] = match knob {
            Knob::Cargo | Knob::MakeJobs => &["-j", "--jobs"],
            Knob::GoP => &["-p"],
            Knob::Workers => &["--workers"],
            Knob::MaxWorkers => &["--maxWorkers", "--max-workers"],
        };
        // A parallelism the command already sets (before `--`) is changed in place.
        for i in p + 1..dash {
            let t = &toks[i];
            for flag in flags {
                let attached = t.text.strip_prefix(flag).and_then(|r| r.strip_prefix('=').or((!r.is_empty() && flag.len() == 2).then_some(r)));
                if let Some(n) = attached.filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty()) {
                    let at = t.end - n.len();
                    return Some(format!("{}{}{}", &command[..at], jobs, &command[t.end..]));
                }
                if t.text == *flag {
                    if let Some(next) = toks.get(i + 1).filter(|n| i + 1 < dash && n.text.chars().all(|c| c.is_ascii_digit()) && !n.text.is_empty()) {
                        return Some(format!("{}{}{}", &command[..next.start], jobs, &command[next.end..]));
                    }
                }
            }
        }
        let (at, flag) = match knob {
            Knob::Cargo => (toks[at_sub].end, format!(" -j {jobs}")),
            Knob::MakeJobs => (toks[dash - 1].end, format!(" -j{jobs}")),
            Knob::GoP => (toks[at_sub].end, format!(" -p {jobs}")),
            Knob::Workers => (toks[at_sub].end, format!(" --workers={jobs}")),
            Knob::MaxWorkers => (toks[at_sub].end, format!(" --maxWorkers={jobs}")),
        };
        return Some(format!("{}{}{}", &command[..at], flag, &command[at..]));
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
    fn the_parallelism_is_set_where_the_tool_reads_it_and_the_command_keeps_its_first_words() {
        let rw = |c: &str, jobs| rewrite(c, &fp(c), profile(&fp(c)).knob.unwrap(), jobs);
        let ok = |c: &str, jobs| rw(c, jobs).unwrap();
        assert_eq!(ok("cargo build --release", 2), "cargo build -j 2 --release");
        assert_eq!(ok("cd crates && cargo test -p kelpied -- --nocapture", 1), "cd crates && cargo test -j 1 -p kelpied -- --nocapture");
        assert_eq!(ok("cargo build -j 8 && echo ok", 2), "cargo build -j 2 && echo ok");
        assert_eq!(ok("cargo build -j8", 2), "cargo build -j2");
        assert_eq!(ok("cargo build --jobs=8", 2), "cargo build --jobs=2");
        assert_eq!(ok("\"C:/Program Files/x/cargo.exe\" check", 3), "\"C:/Program Files/x/cargo.exe\" check -j 3", "a quoted path with a space stays whole");
        assert_eq!(ok("cargo +nightly build", 2), "cargo +nightly build -j 2");
        assert_eq!(ok("RUST_LOG=info cargo test", 2), "RUST_LOG=info cargo test -j 2");
        assert_eq!(ok("make -j8 all", 2), "make -j2 all");
        assert_eq!(ok("make all", 2), "make all -j2");
        assert_eq!(ok("go test ./...", 2), "go test -p 2 ./...");
        assert_eq!(ok("go test -p 8 ./...", 2), "go test -p 2 ./...");
        assert_eq!(ok("npx playwright test e2e/", 1), "npx playwright test --workers=1 e2e/");
        assert_eq!(ok("npx playwright test --workers=4", 2), "npx playwright test --workers=2");
        assert_eq!(ok("pnpm exec vitest run", 2), "pnpm exec vitest run --maxWorkers=2");
        // The work's own segment, never an earlier mention of the program.
        assert_eq!(ok("cd vendor/cargo && cargo build", 2), "cd vendor/cargo && cargo build -j 2");
        assert_eq!(ok("echo 'cargo build now' && cargo build", 2), "echo 'cargo build now' && cargo build -j 2");
        // A flag after `--` is the program's own; none set before it: one is added before `--`.
        assert_eq!(ok("cargo run -- -j 16", 2), "cargo run -j 2 -- -j 16");
        // Left alone when it cannot be done for sure.
        assert_eq!(rw("timeout 900 cargo test", 2), None, "a wrapper");
        assert_eq!(rw("nice cargo build", 2), None);
        assert_eq!(rw("cargo build\ncargo test", 2), None, "several lines");
        assert_eq!(rw("cargo build 'unclosed", 2), None);
    }
}
