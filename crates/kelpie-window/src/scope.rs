//! Which windows an agent reaches: those of the programs it started with `launch`, and of their child processes.
//!
//! `launch` writes down each program it starts: the process id and when the process was created. A process id is reused once its process
//! is gone, so an entry counts only while a process with that id **and** that creation time is alive. A window is in reach when its
//! process is such a program or descends from one through parent links, each link checked the same way (a child is never older than its
//! parent: a parent id that now names a younger process is a reused id, and the chain ends there).
//!
//! When the daemon names a job object (`KELPIE_WINDOW_JOB`), the job decides instead: `launch` puts each program in it before the program
//! runs any code, and a window is in reach when its process is in the job, however it got there. The list then only gives a window its
//! program's name.

use serde::{Deserialize, Serialize};

/// Programs remembered at once. The oldest are forgotten first; dead ones are dropped whenever the list is read.
pub const MAX_LAUNCHED: usize = 32;
/// Parent links followed from a window's process.
pub const MAX_CHAIN: usize = 32;

/// A program `launch` started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launched {
    pub pid: u32,
    /// The process's creation time (a `FILETIME` as one number).
    pub created: u64,
    /// The program, file name only, for the list an agent reads.
    pub program: String,
}

/// One running process as the native side reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    /// 0 when it could not be read (a process of another user).
    pub created: u64,
}

fn find(procs: &[Proc], pid: u32) -> Option<&Proc> {
    procs.iter().find(|p| p.pid == pid)
}

/// The entries whose process is still the one that was started.
pub fn alive(launched: &[Launched], procs: &[Proc]) -> Vec<Launched> {
    launched
        .iter()
        .filter(|l| find(procs, l.pid).is_some_and(|p| p.created != 0 && p.created == l.created))
        .cloned()
        .collect()
}

/// The program an agent started that `pid` is or descends from.
pub fn owner<'a>(pid: u32, launched: &'a [Launched], procs: &[Proc]) -> Option<&'a Launched> {
    let mut at = find(procs, pid)?;
    for _ in 0..MAX_CHAIN {
        if at.created == 0 {
            return None;
        }
        if let Some(l) = launched
            .iter()
            .find(|l| l.pid == at.pid && l.created == at.created)
        {
            return Some(l);
        }
        let parent = find(procs, at.parent)?;
        // A parent that was created after its child is another process that took the id.
        if parent.created == 0 || parent.created > at.created || parent.pid == at.pid {
            return None;
        }
        at = parent;
    }
    None
}

/// The list with one more program: dead entries dropped, the oldest forgotten when it is full.
pub fn remember(launched: &[Launched], procs: &[Proc], new: Launched) -> Vec<Launched> {
    let mut out = alive(launched, procs);
    out.retain(|l| l.pid != new.pid);
    out.push(new);
    let extra = out.len().saturating_sub(MAX_LAUNCHED);
    out.drain(..extra);
    out
}

/// A program and its arguments as the one line Windows starts a process from, quoted so that the program reads back exactly these
/// arguments (the C runtime's rules: an argument with a space, a tab or a quote goes in quotes; backslashes count only before a quote).
pub fn command_line(program: &str, args: &[String]) -> String {
    fn quoted(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.to_string();
        }
        let mut out = String::from('"');
        let mut slashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => slashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                    slashes = 0;
                }
                _ => {
                    out.extend(std::iter::repeat_n('\\', slashes));
                    slashes = 0;
                }
            }
            if c != '\\' {
                out.push(c);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }
    std::iter::once(program)
        .chain(args.iter().map(String::as_str))
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why Kelpie itself may not be started, if it may not. A Kelpie window an agent started is in the agent's reach, and a Kelpie window
/// approves what agents ask: so Kelpie is started only as a separate one, with its own daemon endpoint and its own data folder (a
/// development build under test), never as another window of the person's Kelpie.
pub fn kelpie_refusal(
    program: &str,
    endpoint: Option<&str>,
    data_dir: Option<&str>,
    session_endpoint: Option<&str>,
) -> Option<&'static str> {
    let file = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let stem = file.strip_suffix(".exe").unwrap_or(&file);
    if !matches!(stem, "kelpie" | "kelpied") {
        return None;
    }
    let set = |v: Option<&str>| v.is_some_and(|s| !s.trim().is_empty());
    if !set(endpoint) || !set(data_dir) {
        return Some("Kelpie is started only as a separate one: set KELPIE_ENDPOINT and KELPIE_DATA_DIR to its own daemon and data folder");
    }
    if endpoint.map(str::trim) == session_endpoint.map(str::trim) {
        return Some("that is the endpoint of the Kelpie this session runs in: a Kelpie under test needs its own");
    }
    None
}

/// The file's name for a scope: letters, digits, `-` and `_` of what the caller names, so one agent session does not reach another's
/// programs. Anything else is the shared `default`.
pub fn scope_name(raw: Option<&str>) -> String {
    let s: String = raw
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(64)
        .collect();
    if s.is_empty() {
        "default".into()
    } else {
        s
    }
}

/// The longest job name taken.
pub const MAX_JOB_NAME: usize = 160;

/// The job object the daemon named (`KELPIE_WINDOW_JOB`). `Ok(None)`: none is named, and reach is by the list of started programs. A name
/// that is not one the daemon would make (letters, digits, `-` and `_`) is an error and never falls back to the list: a call that was
/// meant to be held to a job must not quietly be held to something wider.
pub fn job_name(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(name) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let plain = name.len() <= MAX_JOB_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        Ok(Some(name.to_string()))
    } else {
        Err("KELPIE_WINDOW_JOB is not a job name Kelpie makes".into())
    }
}

/// Whether a window's process is in reach. `in_job` is `Some(member)` when a job is named (whether the process is in it) and `None`
/// when none is: the list's `owner` decides then.
pub fn reaches(in_job: Option<bool>, owner: Option<&Launched>) -> bool {
    in_job.unwrap_or(owner.is_some())
}

/// The program name a window is listed under: the one `launch` wrote down for its process or an ancestor, else the file name of the
/// process's image (a program a started one handed over to, which only the job knows).
pub fn program_label(owner: Option<&Launched>, image: Option<&str>) -> String {
    if let Some(l) = owner {
        return l.program.clone();
    }
    image
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .filter(|n| !n.is_empty())
        .map_or_else(|| "program".to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, created: u64) -> Proc {
        Proc {
            pid,
            parent,
            created,
        }
    }

    fn l(pid: u32, created: u64) -> Launched {
        Launched {
            pid,
            created,
            program: "app.exe".into(),
        }
    }

    #[test]
    fn a_window_of_the_program_or_of_its_children_is_in_reach() {
        let procs = [
            p(4, 0, 1),
            p(100, 4, 10),
            p(200, 100, 20),
            p(300, 200, 30),
            p(400, 4, 15),
        ];
        let launched = [l(100, 10)];
        assert_eq!(owner(100, &launched, &procs).map(|l| l.pid), Some(100));
        assert_eq!(owner(300, &launched, &procs).map(|l| l.pid), Some(100));
        assert_eq!(owner(400, &launched, &procs), None);
        assert_eq!(owner(4, &launched, &procs), None);
        assert_eq!(owner(999, &launched, &procs), None);
    }

    #[test]
    fn a_reused_process_id_is_not_the_program_that_was_started() {
        // 100 was started at 10 and is gone; another process took the id at 50.
        let procs = [p(100, 4, 50), p(200, 100, 60)];
        let launched = [l(100, 10)];
        assert_eq!(owner(100, &launched, &procs), None);
        assert_eq!(owner(200, &launched, &procs), None);
        assert!(alive(&launched, &procs).is_empty());
    }

    #[test]
    fn a_parent_id_taken_by_a_younger_process_ends_the_chain() {
        // 300's parent was 200, which is gone; the id 200 now names a process created later, started by the agent.
        let procs = [p(200, 4, 90), p(300, 200, 30)];
        assert_eq!(owner(300, &[l(200, 90)], &procs), None);
    }

    #[test]
    fn a_process_that_cannot_be_read_is_never_in_reach() {
        let procs = [p(100, 4, 0), p(200, 100, 20)];
        assert_eq!(owner(100, &[l(100, 0)], &procs), None);
        assert_eq!(owner(200, &[l(100, 0)], &procs), None);
        assert!(alive(&[l(100, 0)], &procs).is_empty());
    }

    #[test]
    fn a_loop_of_parent_links_ends() {
        let procs = [p(100, 200, 10), p(200, 100, 10)];
        assert_eq!(owner(100, &[l(5, 1)], &procs), None);
        assert_eq!(owner(7, &[l(5, 1)], &[p(7, 7, 3)]), None);
    }

    #[test]
    fn the_list_drops_the_dead_and_forgets_the_oldest() {
        let procs: Vec<Proc> = (1..=40).map(|i| p(i, 0, u64::from(i))).collect();
        let mut list = vec![l(900, 5)];
        for i in 1..=40 {
            list = remember(&list, &procs, l(i, u64::from(i)));
        }
        assert_eq!(list.len(), MAX_LAUNCHED);
        assert_eq!((list[0].pid, list[MAX_LAUNCHED - 1].pid), (9, 40));
        // Started again under the same id: one entry, the new one.
        let again = remember(&[l(3, 3)], &[p(3, 0, 3)], l(3, 3));
        assert_eq!(again, vec![l(3, 3)]);
    }

    #[test]
    fn a_command_line_reads_back_as_the_same_arguments() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(command_line("notepad.exe", &[]), "notepad.exe");
        assert_eq!(
            command_line(r"C:\Program Files\App\app.exe", &args(&["--flag", "a b"])),
            r#""C:\Program Files\App\app.exe" --flag "a b""#
        );
        assert_eq!(command_line("app", &args(&[""])), r#"app """#);
        assert_eq!(
            command_line("app", &args(&[r#"say "hi""#])),
            r#"app "say \"hi\"""#
        );
        // Backslashes before a quote are doubled; a trailing one before the closing quote too; others stay.
        assert_eq!(
            command_line("app", &args(&[r"C:\dir with space\"])),
            r#"app "C:\dir with space\\""#
        );
        assert_eq!(command_line("app", &args(&[r#"a\"b"#])), r#"app "a\\\"b""#);
        assert_eq!(
            command_line("app", &args(&[r"C:\plain\path\"])),
            r"app C:\plain\path\"
        );
    }

    #[test]
    fn kelpie_is_started_only_as_a_separate_one() {
        let ok = |p, e, d, s| kelpie_refusal(p, e, d, s).is_none();
        assert!(ok("notepad.exe", None, None, None));
        assert!(ok(r"C:\apps\kelpie-notes.exe", None, None, None));
        for program in [
            "kelpie",
            "Kelpie.exe",
            r"C:\Program Files\Kelpie\KELPIE.EXE",
            "build/debug/kelpied.exe",
        ] {
            assert!(!ok(program, None, None, None), "{program}");
            assert!(!ok(program, Some("kelpie-dev"), None, None), "{program}");
            assert!(!ok(program, Some(" "), Some("d"), None), "{program}");
            assert!(
                !ok(program, Some("kelpie"), Some("d"), Some("kelpie")),
                "{program}"
            );
            assert!(
                ok(program, Some("kelpie-dev"), Some("d"), Some("kelpie")),
                "{program}"
            );
            assert!(
                ok(program, Some("kelpie-dev"), Some("d"), None),
                "{program}"
            );
        }
    }

    #[test]
    fn a_scope_name_is_a_plain_file_name() {
        assert_eq!(scope_name(None), "default");
        assert_eq!(scope_name(Some("")), "default");
        assert_eq!(scope_name(Some("../..\\x:y")), "xy");
        assert_eq!(scope_name(Some("sess-12_a")), "sess-12_a");
        assert_eq!(scope_name(Some(&"a".repeat(100))).len(), 64);
    }

    #[test]
    fn a_job_is_named_by_the_daemon_or_not_at_all() {
        assert_eq!(job_name(None), Ok(None));
        assert_eq!(job_name(Some("")), Ok(None));
        assert_eq!(
            job_name(Some("kelpie-win-s_1-0a1B")),
            Ok(Some("kelpie-win-s_1-0a1B".to_string()))
        );
        assert!(job_name(Some("a".repeat(MAX_JOB_NAME).as_str())).is_ok());
        // Anything else is refused, never read as "no job": the namespace prefixes, a path, a space, a longer name.
        let long = "a".repeat(MAX_JOB_NAME + 1);
        for bad in [
            r"Global\x",
            r"Local\x",
            "a b",
            "../x",
            "x\0",
            "é",
            long.as_str(),
        ] {
            assert!(job_name(Some(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_named_job_decides_reach_and_the_list_only_when_there_is_none() {
        let listed = l(100, 10);
        // With a job, being in it is all that counts: a program the list does not know is in reach, one the list knows but the job
        // does not hold is not.
        assert!(reaches(Some(true), None));
        assert!(reaches(Some(true), Some(&listed)));
        assert!(!reaches(Some(false), Some(&listed)));
        assert!(!reaches(Some(false), None));
        // With none, the list's answer stands.
        assert!(reaches(None, Some(&listed)));
        assert!(!reaches(None, None));
    }

    #[test]
    fn a_window_is_listed_under_its_program_s_name() {
        assert_eq!(program_label(Some(&l(1, 1)), Some(r"C:\x\other.exe")), "app.exe");
        assert_eq!(
            program_label(None, Some(r"C:\Program Files\Edge\msedge.exe")),
            "msedge.exe"
        );
        assert_eq!(program_label(None, Some("/opt/app")), "app");
        assert_eq!(program_label(None, Some(r"C:\dir\")), "program");
        assert_eq!(program_label(None, None), "program");
    }
}
