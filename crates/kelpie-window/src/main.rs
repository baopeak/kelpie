//! `kelpie-window`: an agent starts an app and uses its windows, one command a call.
//!
//! ```text
//! kelpie-window launch <program> [arguments…]     start a program; its windows come into reach
//! kelpie-window list                              the windows in reach: target, program, title, size
//! kelpie-window tree <target> [--max N] [--rects] the window as an outline; [id] marks what can be acted on
//! kelpie-window act <target> <id> <action> [text] invoke | set <text> | toggle | expand | collapse | select | focus | show
//! kelpie-window shot <target> <file.png>          a picture of the window
//! kelpie-window close <target>                    ask the window to close
//! ```
//!
//! Only windows of programs started with `launch` (and of their child processes) are in reach; see the library's notes. A web view shows
//! its page in the tree only with accessibility on: `launch` asks WebView2 apps for it, and a Chromium or Electron program takes
//! `--force-renderer-accessibility` as an argument. `KELPIE_WINDOW_SCOPE` names the agent session, so that sessions do not reach each
//! other's programs.

#[cfg(not(windows))]
fn main() {
    eprintln!("kelpie-window runs on Windows");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::run(&args) {
        Ok(out) => print!("{out}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

#[cfg(windows)]
mod cli {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use kelpie_screen::targets::{TargetId, WindowInfo};
    use kelpie_screen::win::system::init_process;
    use kelpie_screen::win::targets_win::{exists, hwnd, windows};
    use kelpie_window::scope::{
        alive, command_line, kelpie_refusal, owner, remember, scope_name, Launched, Proc,
    };
    use kelpie_window::tree::{content_pending, outline, parse_id, prune, Action, MAX_NODES};
    use kelpie_window::win::desk;
    use kelpie_window::win::procs::{created, processes};
    use kelpie_window::win::shot;
    use kelpie_window::win::uia::Uia;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    const USAGE: &str = "usage: kelpie-window launch <program> [arguments…] | list | tree <target> [--max N] [--rects] | act <target> <id> <action> [text] | shot <target> <file.png> | close <target>\n";
    /// `SW_SHOWNOACTIVATE`: the program's first window shows without taking the focus from the person.
    const SHOW_NO_ACTIVATE: u16 = 4;
    /// How long `launch` waits for the program's first window before it answers without one.
    const WINDOW_WAIT: Duration = Duration::from_secs(15);
    /// How many more times, and how far apart, a window is read while its page is not in the tree yet.
    const PAGE_READS: usize = 4;
    const PAGE_WAIT: Duration = Duration::from_millis(700);
    /// The smallest window listed.
    const MIN_SIDE: i32 = 24;
    const ACCESSIBLE: &str = "--force-renderer-accessibility";
    const NOT_YOURS: &str = "not a window of a program you started: start the app with `kelpie-window launch`, then `kelpie-window list`";

    /// What `launch` started, for this agent session.
    struct Registry {
        path: PathBuf,
    }

    impl Registry {
        fn open() -> Registry {
            let dir = std::env::temp_dir().join("kelpie-window");
            let _ = std::fs::create_dir_all(&dir);
            Registry {
                path: dir.join(format!(
                    "{}.json",
                    scope_name(std::env::var("KELPIE_WINDOW_SCOPE").ok().as_deref())
                )),
            }
        }

        fn read(&self, procs: &[Proc]) -> Vec<Launched> {
            let stored: Vec<Launched> = std::fs::read(&self.path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            alive(&stored, procs)
        }

        fn add(&self, new: Launched) -> Result<(), String> {
            let procs = processes();
            let stored: Vec<Launched> = std::fs::read(&self.path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            let next = remember(&stored, &procs, new);
            let tmp = self.path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec(&next).map_err(|e| e.to_string())?)
                .and_then(|_| std::fs::rename(&tmp, &self.path))
                .map_err(|e| format!("the list of started programs could not be written: {e}"))
        }
    }

    /// A window a person or an agent could use: shown, with a size. Dialogs (owned windows) are included: an app's questions are asked
    /// in them.
    fn usable(w: &WindowInfo) -> bool {
        // An app's helper windows (a 16x16 one behind a tray icon) are not something to use.
        w.visible && !w.cloaked && (w.minimized || (w.rect.w >= MIN_SIDE && w.rect.h >= MIN_SIDE))
    }

    fn in_reach(launched: &[Launched], procs: &[Proc]) -> Vec<(WindowInfo, Launched)> {
        windows()
            .into_iter()
            .filter(usable)
            .filter_map(|w| owner(w.pid, launched, procs).cloned().map(|l| (w, l)))
            .collect()
    }

    fn line(w: &WindowInfo, l: &Launched) -> String {
        let id = TargetId::Window {
            hwnd: w.hwnd,
            pid: w.pid,
        };
        let title: String = w
            .title
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect();
        let kind = if w.owned { " dialog" } else { "" };
        let size = if w.minimized {
            "minimized".to_string()
        } else {
            format!("{}x{}", w.rect.w, w.rect.h)
        };
        format!("{id}  {}{kind}  \"{title}\"  {size}\n", l.program)
    }

    /// The window a target names, when it is in reach.
    fn target(arg: Option<&String>) -> Result<windows::Win32::Foundation::HWND, String> {
        let Some(TargetId::Window { hwnd: h, pid }) = arg.and_then(|a| TargetId::parse(a)) else {
            return Err(format!("a target looks like w:<number>:<number>, as `kelpie-window list` prints it\n{USAGE}"));
        };
        let h = hwnd(h);
        if !exists(h, pid) {
            return Err("that window is gone: `kelpie-window list` shows what is there now".into());
        }
        let procs = processes();
        let launched = Registry::open().read(&procs);
        owner(pid, &launched, &procs)
            .map(|_| h)
            .ok_or_else(|| NOT_YOURS.to_string())
    }

    pub fn run(args: &[String]) -> Result<String, String> {
        init_process();
        // `KELPIE_WINDOW_DESKTOP=hidden`: the app runs on the agent's own desktop, where it cannot come in front of the person's windows.
        let hidden = std::env::var("KELPIE_WINDOW_DESKTOP").is_ok_and(|v| v == "hidden");
        if hidden {
            desk::enter()?;
        }
        match args.first().map(String::as_str) {
            Some("launch") => launch(&args[1..], hidden),
            Some("list") => {
                let procs = processes();
                let launched = Registry::open().read(&procs);
                let lines: String = in_reach(&launched, &procs)
                    .iter()
                    .map(|(w, l)| line(w, l))
                    .collect();
                Ok(if lines.is_empty() {
                    "no windows in reach: start an app with `kelpie-window launch <program>`\n"
                        .into()
                } else {
                    lines
                })
            }
            Some("tree") => {
                let h = target(args.get(1))?;
                let mut max = MAX_NODES;
                let mut rects = false;
                let mut rest = args[2..].iter();
                while let Some(a) = rest.next() {
                    match a.as_str() {
                        "--rects" => rects = true,
                        "--max" => {
                            max = rest
                                .next()
                                .and_then(|n| n.parse().ok())
                                .filter(|n| (1..=5000).contains(n))
                                .ok_or("--max takes a number from 1 to 5000")?
                        }
                        other => return Err(format!("unknown option {other}\n{USAGE}")),
                    }
                }
                let origin = shot::frame(h).map(|f| (f.x, f.y)).unwrap_or_default();
                let uia = Uia::new()?;
                let mut nodes = uia.nodes(h, origin)?;
                // A web view builds its page's tree once it has been asked: ask again while a large container is still empty.
                for _ in 0..PAGE_READS {
                    if !content_pending(&nodes) {
                        break;
                    }
                    std::thread::sleep(PAGE_WAIT);
                    nodes = uia.nodes(h, origin)?;
                }
                let nodes = prune(nodes);
                Ok(outline(&nodes, max, rects))
            }
            Some("act") => {
                let h = target(args.get(1))?;
                let id = args.get(2).and_then(|s| parse_id(s)).ok_or(
                    "the id is the numbers in [brackets] in the tree, such as 42.1312340.4.7",
                )?;
                let action = Action::parse(
                    args.get(3).map(String::as_str).ok_or(USAGE)?,
                    args.get(4).map(String::as_str),
                )?;
                if args.len() > 5 {
                    return Err("one text only: quote it".into());
                }
                Uia::new()?
                    .act(h, &id, action)
                    .map(|done| format!("{done}\n"))
            }
            Some("shot") => {
                let h = target(args.get(1))?;
                let path = args
                    .get(2)
                    .filter(|p| p.to_ascii_lowercase().ends_with(".png"))
                    .ok_or("shot takes the file to write, ending in .png")?;
                let (w, hgt, png) = shot::png(h)?;
                std::fs::write(path, png)
                    .map_err(|e| format!("{path} could not be written: {e}"))?;
                Ok(format!("{w}x{hgt} {path}\n"))
            }
            Some("close") => {
                let h = target(args.get(1))?;
                unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) }
                    .map_err(|e| format!("the window did not take the request: {}", e.message()))?;
                Ok("asked to close; the app may ask a question in a dialog first\n".into())
            }
            _ => Err(USAGE.into()),
        }
    }

    fn launch(args: &[String], hidden: bool) -> Result<String, String> {
        let program = args.first().ok_or(USAGE)?;
        // A WebView2 app (a Tauri app is one) puts its page in the tree only when its web view runs with accessibility on.
        let web_args = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        let web_args = if web_args.contains(ACCESSIBLE) {
            web_args
        } else {
            format!("{web_args} {ACCESSIBLE}").trim().to_string()
        };
        let env = |name: &str| std::env::var(name).ok();
        if let Some(why) = kelpie_refusal(
            program,
            env("KELPIE_ENDPOINT").as_deref(),
            env("KELPIE_DATA_DIR").as_deref(),
            env("KELPIE_SESSION_ENDPOINT").as_deref(),
        ) {
            return Err(why.into());
        }
        // The creation time is read while the process's handle is open, so that it and the id name the same process.
        let (pid, made) = if hidden {
            // The program inherits this process's environment; no other thread of this process runs yet.
            unsafe { std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &web_args) };
            desk::start(&command_line(program, &args[1..]))
                .map_err(|e| format!("{program}: {e}"))?
        } else {
            let mut cmd = Command::new(program);
            cmd.args(&args[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .show_window(SHOW_NO_ACTIVATE)
                .env("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", &web_args);
            // The child is not waited for: it lives on after this command.
            let child = cmd
                .spawn()
                .map_err(|e| format!("{program} could not be started: {e}"))?;
            (child.id(), created(child.id()))
        };
        if made == 0 {
            return Err(format!("{program} ended as soon as it started"));
        }
        let name = std::path::Path::new(program)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| program.clone());
        let registry = Registry::open();
        registry.add(Launched {
            pid,
            created: made,
            program: name,
        })?;
        let until = Instant::now() + WINDOW_WAIT;
        loop {
            let procs = processes();
            let launched = registry.read(&procs);
            let mine: String = in_reach(&launched, &procs)
                .iter()
                .filter(|(_, l)| l.pid == pid)
                .map(|(w, l)| line(w, l))
                .collect();
            if !mine.is_empty() {
                return Ok(mine);
            }
            if !launched.iter().any(|l| l.pid == pid) {
                return Err(format!("{program} ended without showing a window (a program that hands over to one already running is not in reach)"));
            }
            if Instant::now() >= until {
                return Ok(format!("started (process {pid}); no window yet: `kelpie-window list` shows it when it is there\n"));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}
