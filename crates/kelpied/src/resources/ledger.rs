//! What each command an agent ran has cost, learned run by run: `command-ledger.json`.
//!
//! A command is known by its project and its fingerprint (`cargo test --workspace`, `pnpm build`, `next build`) and the
//! parallelism it ran with (`-j 2`): the same build costs about the same each time in the same project. Each run adds its
//! demand: the most memory, CPU, disk traffic and GPU the session's processes took above what they held when it started.
//! The figures are kept as a moving average with its spread (an EWMA), plus the last few memory peaks, which is what a
//! later estimate is built from. Nothing here holds a command's text beyond its fingerprint, nor any of its output.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Weight of the newest run in the averages.
const ALPHA: f64 = 0.3;
/// Memory peaks kept per command.
const PEAKS_KEPT: usize = 5;
/// Commands kept in all; the ones run longest ago go first.
const MAX_ENTRIES: usize = 500;

/// Programs that only prepare the line for the command that does the work.
const PREFIXES: [&str; 8] = ["env", "sudo", "time", "nice", "nohup", "command", "exec", "call"];
/// A segment that is only one of these does no work worth measuring.
const TRIVIAL: [&str; 10] = ["cd", "pushd", "popd", "export", "set", "echo", "true", ":", "source", "."];
/// Programs whose first word after them names what they do (`cargo test`, `pnpm build`).
const HAS_SUBCOMMAND: [&str; 20] = ["cargo", "pnpm", "npm", "yarn", "bun", "go", "docker", "git", "uv", "pip", "dotnet", "gradle", "gradlew", "make", "turbo", "nx", "playwright", "vitest", "next", "vite"];
/// Flags that change what a command costs, kept in the fingerprint.
const KEY_FLAGS: [&str; 7] = ["--release", "--workspace", "--all", "--all-targets", "--all-features", "--prod", "--production"];

/// Words of a command line, quotes respected (not a shell: enough to tell programs and flags apart).
fn words(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote, mut any) = (Vec::new(), String::new(), None::<char>, false);
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The command line's segments, split at `&&`, `||`, `;` and `|` outside quotes.
fn segments(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quote) = (Vec::new(), String::new(), None::<char>);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, '&' | '|') if chars.peek() == Some(&c) => {
                chars.next();
                out.push(std::mem::take(&mut cur));
            }
            (None, ';' | '|' | '\n') => out.push(std::mem::take(&mut cur)),
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// `C:\tools\cargo.exe` → `cargo`.
fn program(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    name
}

/// A command as the ledger knows it: what runs and how much of it runs at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// `cargo test --workspace -p kelpied`: the program, its subcommand and the flags that change the cost.
    pub key: String,
    /// The parallelism it asked for (`-j 2`, `CARGO_BUILD_JOBS=2`, `--workers=2`), or none (the tool's default).
    pub jobs: Option<u32>,
}

/// The fingerprint of the segment that does the work in `line`: the first one that is not a `cd`, an `export` or an `echo`.
pub fn fingerprint(line: &str) -> Option<Fingerprint> {
    for segment in segments(line) {
        let mut w = words(&segment);
        let mut jobs = None;
        // Leading `VAR=value` assignments and wrappers: the jobs variables are the one thing read from them.
        loop {
            let Some(first) = w.first() else { break };
            if let Some((name, value)) = first.split_once('=').filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
                if matches!(name, "CARGO_BUILD_JOBS" | "MAKEFLAGS" | "NUMBER_OF_PROCESSORS") {
                    jobs = jobs.or(value.trim_start_matches("-j").parse().ok());
                }
                w.remove(0);
            } else if PREFIXES.contains(&program(first).as_str()) {
                w.remove(0);
            } else {
                break;
            }
        }
        let Some(first) = w.first() else { continue };
        let mut prog = program(first);
        if TRIVIAL.contains(&prog.as_str()) {
            continue;
        }
        let mut rest: Vec<String> = w[1..].to_vec();
        // `npx vite build`, `pnpm exec tsc`, `python -m pytest`, `uv run pytest`: the program is the one they run.
        let runner = match prog.as_str() {
            "npx" | "bunx" | "pnpx" => Some(0),
            "pnpm" | "yarn" | "npm" | "bun" if rest.first().is_some_and(|a| a == "exec" || a == "dlx" || a == "x") => Some(1),
            "uv" | "poetry" | "pipenv" if rest.first().is_some_and(|a| a == "run") => Some(1),
            "python" | "python3" | "py" if rest.first().is_some_and(|a| a == "-m") => Some(1),
            _ => None,
        };
        if let Some(skip) = runner {
            let mut r = rest.into_iter().skip(skip).skip_while(|a| a.starts_with('-'));
            let Some(next) = r.next() else { continue };
            prog = program(&next);
            rest = r.collect();
        }
        let mut key = vec![prog.clone()];
        if HAS_SUBCOMMAND.contains(&prog.as_str()) {
            // The first word that is not a flag (cargo's `+nightly` toolchain is not one either); flags before it still count.
            if let Some(at) = rest.iter().position(|a| !a.starts_with('-') && !a.starts_with('+') && !a.chars().all(|c| c.is_ascii_digit())) {
                key.push(rest.remove(at).to_ascii_lowercase());
            }
        }
        let mut packages: Vec<String> = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            let a = rest[i].as_str();
            let next = rest.get(i + 1).map(|s| s.as_str());
            let value_of = |flag: &str| a.strip_prefix(flag).and_then(|v| v.strip_prefix('='));
            match a {
                "-j" | "--jobs" | "--workers" | "--maxWorkers" | "--max-workers" | "--concurrency" | "-n" => {
                    // A number after it is the parallelism (`-j 2`, pytest-xdist's `-n 4`); anything else is left alone.
                    if let Some(n) = next.and_then(|n| n.parse().ok()) {
                        jobs = jobs.or(Some(n));
                        i += 1;
                    }
                }
                // `go build -p 4` runs four builds at once; cargo's `-p` names a package.
                "-p" if prog == "go" => {
                    if let Some(n) = next.and_then(|n| n.parse().ok()) {
                        jobs = jobs.or(Some(n));
                        i += 1;
                    }
                }
                "-p" | "--package" if prog == "cargo" => {
                    if let Some(p) = next {
                        packages.push(format!("-p {p}"));
                    }
                    i += 1;
                }
                _ if KEY_FLAGS.contains(&a) => key.push(a.to_string()),
                _ => {
                    if let Some(n) = ["--jobs", "--workers", "--maxWorkers", "--max-workers", "--concurrency"].iter().find_map(|f| value_of(f)) {
                        jobs = jobs.or(n.parse().ok());
                    } else if let Some(n) = a.strip_prefix("-j").filter(|n| !n.is_empty()) {
                        jobs = jobs.or(n.parse().ok());
                    } else if let Some(p) = a.strip_prefix("--package=") {
                        packages.push(format!("-p {p}"));
                    }
                }
            }
            i += 1;
        }
        packages.sort();
        key.extend(packages);
        return Some(Fingerprint { key: key.join(" "), jobs });
    }
    None
}

/// What one run of a command took, above what the session held when it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Demand {
    /// Most memory (private bytes of the whole tree), MiB.
    pub ram_mb: f64,
    /// Mean CPU over the run, in cores (100 % of one core = 1).
    pub cores: f64,
    /// Most CPU in one sample, in cores.
    pub peak_cores: f64,
    /// Disk and pipe traffic, MiB in all and the most in one second.
    pub io_mb: f64,
    pub peak_io_mbps: f64,
    /// Most GPU use (percent of the busiest engine) and GPU memory, MiB.
    pub gpu_pct: f64,
    pub gpu_mb: f64,
    pub secs: f64,
}

/// A running average and its spread.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Avg {
    pub mean: f64,
    pub var: f64,
}

impl Avg {
    fn add(&mut self, x: f64, first: bool) {
        if first {
            *self = Avg { mean: x, var: 0.0 };
            return;
        }
        let d = x - self.mean;
        self.mean += ALPHA * d;
        self.var = (1.0 - ALPHA) * (self.var + ALPHA * d * d);
    }

    pub fn sd(&self) -> f64 {
        self.var.max(0.0).sqrt()
    }
}

/// Everything learned about one command.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub runs: u32,
    pub failed: u32,
    pub ram_mb: Avg,
    pub cores: Avg,
    pub peak_cores: Avg,
    pub io_mb: Avg,
    pub peak_io_mbps: Avg,
    pub gpu_pct: Avg,
    pub gpu_mb: Avg,
    pub secs: Avg,
    /// The latest memory peaks, newest last.
    pub ram_peaks: Vec<f64>,
    /// When it last ran, ms since the epoch.
    pub last: u64,
}

impl Entry {
    fn add(&mut self, d: &Demand, failed: bool, at: u64) {
        let first = self.runs == 0;
        self.runs += 1;
        self.failed += u32::from(failed);
        self.ram_mb.add(d.ram_mb, first);
        self.cores.add(d.cores, first);
        self.peak_cores.add(d.peak_cores, first);
        self.io_mb.add(d.io_mb, first);
        self.peak_io_mbps.add(d.peak_io_mbps, first);
        self.gpu_pct.add(d.gpu_pct, first);
        self.gpu_mb.add(d.gpu_mb, first);
        self.secs.add(d.secs, first);
        self.ram_peaks.push(d.ram_mb);
        if self.ram_peaks.len() > PEAKS_KEPT {
            self.ram_peaks.remove(0);
        }
        self.last = at;
    }

    /// The memory to plan for: the mean plus one and a half spreads, or the largest recent peak when that is more.
    pub fn plan_ram_mb(&self) -> f64 {
        let by_spread = self.ram_mb.mean + 1.5 * self.ram_mb.sd();
        self.ram_peaks.iter().copied().fold(by_spread, f64::max)
    }
}

/// `project · key · jobs`: how an entry is stored.
pub fn entry_key(project: &str, f: &Fingerprint) -> String {
    format!("{project}\u{1f}{}\u{1f}{}", f.key, f.jobs.map_or("default".to_string(), |j| j.to_string()))
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    version: u32,
    entries: BTreeMap<String, Entry>,
}

pub struct Ledger {
    path: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    dirty: bool,
}

impl Ledger {
    /// The ledger in `path`. A file that cannot be read is kept as it is and the ledger starts empty without
    /// saving over it until a new run is learned (`persist::load_state` keeps a copy of a damaged one).
    pub fn open(path: Option<PathBuf>) -> Ledger {
        let entries = path
            .as_deref()
            .and_then(|p| crate::persist::load_state::<File>(p).ok().flatten())
            .map(|f| f.entries.into_iter().collect())
            .unwrap_or_default();
        Ledger { path, entries, dirty: false }
    }

    #[cfg(test)]
    pub fn get(&self, project: &str, f: &Fingerprint) -> Option<&Entry> {
        self.entries.get(&entry_key(project, f))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Learns one run.
    pub fn learn(&mut self, project: &str, f: &Fingerprint, d: &Demand, failed: bool, at: u64) {
        self.entries.entry(entry_key(project, f)).or_default().add(d, failed, at);
        if self.entries.len() > MAX_ENTRIES {
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, e)| e.last).map(|(k, _)| k.clone()) {
                self.entries.remove(&oldest);
            }
        }
        self.dirty = true;
    }

    /// Writes the ledger when something was learned since the last write.
    pub fn save(&mut self) {
        let Some(path) = self.path.as_deref().filter(|_| self.dirty) else { return };
        let file = File { version: 1, entries: self.entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect() };
        match serde_json::to_vec_pretty(&file).map_err(anyhow::Error::from).and_then(|b| crate::persist::save_state(path, &b)) {
            Ok(()) => self.dirty = false,
            Err(e) => tracing::warn!("command ledger not saved: {e}"),
        }
    }

    /// The entries of a project (all with `None`), as `(project, key, jobs, entry)`, most recently run first.
    pub fn list(&self, project: Option<&str>) -> Vec<(String, String, Option<u32>, Entry)> {
        let mut out: Vec<(String, String, Option<u32>, Entry)> = self
            .entries
            .iter()
            .filter_map(|(k, e)| {
                let mut parts = k.split('\u{1f}');
                let (p, key, jobs) = (parts.next()?, parts.next()?, parts.next()?);
                project.is_none_or(|want| want == p).then(|| (p.to_string(), key.to_string(), jobs.parse().ok(), e.clone()))
            })
            .collect();
        out.sort_by(|a, b| b.3.last.cmp(&a.3.last));
        out
    }
}
