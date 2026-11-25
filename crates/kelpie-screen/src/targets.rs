//! What a phone may pick: each monitor, and the top-level windows a person would call windows (a title, on screen or minimized, not a tool
//! palette, not owned by another window, not the shell's own). IDEs and the Kelpie window come first, marked as shortcuts.
//!
//! A target's id says what it is and is checked again when a stream starts: `m:\\.\DISPLAY1` for a monitor, `w:<hwnd>:<pid>` for a window
//! (a window handle can be reused once its window is gone: with the process id, a reused one is not taken for the old).

use serde::Serialize;

use crate::geom::{Monitor, Rect};

/// What the native side reads of one top-level window.
#[derive(Clone, Debug, Default)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
    pub class: String,
    /// The process's executable, file name only (`Code.exe`).
    pub exe: String,
    /// `DWMWA_EXTENDED_FRAME_BOUNDS`, physical pixels.
    pub rect: Rect,
    pub visible: bool,
    /// Hidden by DWM (another virtual desktop, a suspended UWP app).
    pub cloaked: bool,
    /// `WS_EX_TOOLWINDOW` without `WS_EX_APPWINDOW`.
    pub tool: bool,
    /// Has an owner window (a dialog of another window is reached through that one).
    pub owned: bool,
    pub minimized: bool,
}

/// Windows that are part of the shell or of this helper, never offered.
const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow", "Windows.UI.Core.CoreWindow", "XamlExplorerHostIslandWindow", crate::BORDER_CLASS];

/// IDEs by executable, with the name the phone shows.
const IDES: &[(&str, &str)] = &[
    ("code.exe", "VS Code"),
    ("code - insiders.exe", "VS Code Insiders"),
    ("cursor.exe", "Cursor"),
    ("windsurf.exe", "Windsurf"),
    ("zed.exe", "Zed"),
    ("devenv.exe", "Visual Studio"),
    ("idea64.exe", "IntelliJ IDEA"),
    ("pycharm64.exe", "PyCharm"),
    ("webstorm64.exe", "WebStorm"),
    ("rider64.exe", "Rider"),
    ("goland64.exe", "GoLand"),
    ("clion64.exe", "CLion"),
    ("rustrover64.exe", "RustRover"),
    ("phpstorm64.exe", "PhpStorm"),
    ("rubymine64.exe", "RubyMine"),
    ("datagrip64.exe", "DataGrip"),
    ("studio64.exe", "Android Studio"),
    ("fleet.exe", "Fleet"),
];

/// The IDE an executable is, if it is one.
pub fn ide_name(exe: &str) -> Option<&'static str> {
    let exe = exe.to_ascii_lowercase();
    IDES.iter().find(|(e, _)| *e == exe).map(|(_, n)| *n)
}

fn is_kelpie(exe: &str) -> bool {
    exe.eq_ignore_ascii_case("kelpie.exe")
}

/// The smallest window offered (a sliver is not something to control).
const MIN_SIDE: i32 = 80;

/// Whether a window is offered. `own_pid` is this helper's.
pub fn listed(w: &WindowInfo, own_pid: u32) -> bool {
    w.visible
        && !w.cloaked
        && !w.tool
        && !w.owned
        && w.pid != own_pid
        && !w.title.trim().is_empty()
        && !SHELL_CLASSES.contains(&w.class.as_str())
        && (w.minimized || (w.rect.w >= MIN_SIDE && w.rect.h >= MIN_SIDE))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Target {
    pub id: String,
    /// `monitor` or `window`.
    pub kind: &'static str,
    pub title: String,
    /// The program, for a window (`VS Code`, or the executable without `.exe`).
    pub app: Option<String>,
    /// `ide` or `kelpie`: shown first, as shortcuts.
    pub shortcut: Option<&'static str>,
    pub w: i32,
    pub h: i32,
    pub minimized: bool,
    pub primary: bool,
}

const MAX_TITLE: usize = 120;

fn short(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(MAX_TITLE).collect()
}

/// What a phone is offered: monitors (the primary first), then the Kelpie window and IDEs, then the other windows in the order given (the
/// native side gives them front to back). At most `MAX_TARGETS`.
pub fn targets(monitors: &[Monitor], windows: &[WindowInfo], own_pid: u32) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    let mut ms: Vec<&Monitor> = monitors.iter().collect();
    ms.sort_by_key(|m| !m.primary);
    for (i, m) in ms.iter().enumerate() {
        out.push(Target { id: TargetId::Monitor(m.name.clone()).to_string(), kind: "monitor", title: format!("Screen {}", i + 1), app: None, shortcut: None, w: m.rect.w, h: m.rect.h, minimized: false, primary: m.primary });
    }
    let mut shown: Vec<(u8, Target)> = windows
        .iter()
        .filter(|w| listed(w, own_pid))
        .map(|w| {
            let (rank, shortcut, app) = if is_kelpie(&w.exe) {
                (0, Some("kelpie"), "Kelpie".to_string())
            } else if let Some(ide) = ide_name(&w.exe) {
                (1, Some("ide"), ide.to_string())
            } else {
                (2, None, w.exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string())
            };
            (rank, Target { id: TargetId::Window { hwnd: w.hwnd, pid: w.pid }.to_string(), kind: "window", title: short(&w.title), app: Some(short(&app)), shortcut, w: w.rect.w, h: w.rect.h, minimized: w.minimized, primary: false })
        })
        .collect();
    // Stable: within a rank, front to back as given.
    shown.sort_by_key(|(rank, _)| *rank);
    out.extend(shown.into_iter().map(|(_, t)| t));
    out.truncate(MAX_TARGETS);
    out
}

pub const MAX_TARGETS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetId {
    Monitor(String),
    Window { hwnd: u64, pid: u32 },
}

impl std::fmt::Display for TargetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetId::Monitor(name) => write!(f, "m:{name}"),
            TargetId::Window { hwnd, pid } => write!(f, "w:{hwnd}:{pid}"),
        }
    }
}

impl TargetId {
    pub fn parse(s: &str) -> Option<TargetId> {
        if s.len() > 80 {
            return None;
        }
        if let Some(name) = s.strip_prefix("m:") {
            // `\\.\DISPLAY<n>` is all DXGI names a desktop output.
            let n = name.strip_prefix(r"\\.\DISPLAY")?;
            return (!n.is_empty() && n.len() <= 3 && n.bytes().all(|b| b.is_ascii_digit())).then(|| TargetId::Monitor(name.to_string()));
        }
        let rest = s.strip_prefix("w:")?;
        let (hwnd, pid) = rest.split_once(':')?;
        let hwnd: u64 = hwnd.parse().ok().filter(|h| *h != 0)?;
        let pid: u32 = pid.parse().ok().filter(|p| *p != 0)?;
        Some(TargetId::Window { hwnd, pid })
    }

    pub fn is_window(&self) -> bool {
        matches!(self, TargetId::Window { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: u64, pid: u32, title: &str, exe: &str) -> WindowInfo {
        WindowInfo { hwnd, pid, title: title.into(), class: "Chrome_WidgetWin_1".into(), exe: exe.into(), rect: Rect::new(10, 10, 1200, 800), visible: true, ..Default::default() }
    }

    #[test]
    fn only_windows_a_person_would_call_windows_are_offered() {
        let me = 4242;
        let ok = win(1, 10, "main.rs — kelpie", "Code.exe");
        assert!(listed(&ok, me));
        let cases: Vec<(WindowInfo, &str)> = vec![
            (WindowInfo { visible: false, ..ok.clone() }, "hidden"),
            (WindowInfo { cloaked: true, ..ok.clone() }, "on another virtual desktop"),
            (WindowInfo { tool: true, ..ok.clone() }, "a tool palette"),
            (WindowInfo { owned: true, ..ok.clone() }, "a dialog owned by another window"),
            (WindowInfo { pid: me, ..ok.clone() }, "this helper's own"),
            (WindowInfo { title: "  ".into(), ..ok.clone() }, "no title"),
            (WindowInfo { class: "Progman".into(), ..ok.clone() }, "the desktop"),
            (WindowInfo { class: crate::BORDER_CLASS.into(), ..ok.clone() }, "the sharing border"),
            (WindowInfo { rect: Rect::new(0, 0, 40, 600), ..ok.clone() }, "a sliver"),
        ];
        for (w, why) in cases {
            assert!(!listed(&w, me), "{why}");
        }
        assert!(listed(&WindowInfo { minimized: true, rect: Rect::new(-32000, -32000, 0, 0), ..ok }, me), "a minimized window is offered: picking it says it is minimized");
    }

    #[test]
    fn monitors_then_kelpie_and_ides_then_the_rest_front_to_back() {
        let monitors = vec![
            Monitor { name: r"\\.\DISPLAY2".into(), rect: Rect::new(-1920, 0, 1920, 1080), dpi: 96, primary: false },
            Monitor { name: r"\\.\DISPLAY1".into(), rect: Rect::new(0, 0, 2560, 1440), dpi: 144, primary: true },
        ];
        let windows = vec![win(5, 50, "Inbox — Mail", "olk.exe"), win(6, 60, "lib.rs — RustRover", "rustrover64.exe"), win(7, 70, "Kelpie", "kelpie.exe"), win(8, 80, "notes.txt - Notepad", "notepad.exe"), win(9, 90, "app.ts — Cursor", "Cursor.exe")];
        let t = targets(&monitors, &windows, 1);
        let ids: Vec<&str> = t.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, [r"m:\\.\DISPLAY1", r"m:\\.\DISPLAY2", "w:7:70", "w:6:60", "w:9:90", "w:5:50", "w:8:80"]);
        assert_eq!((t[0].title.as_str(), t[0].primary, t[0].w), ("Screen 1", true, 2560));
        assert_eq!((t[2].shortcut, t[2].app.as_deref()), (Some("kelpie"), Some("Kelpie")));
        assert_eq!((t[3].shortcut, t[3].app.as_deref()), (Some("ide"), Some("RustRover")));
        assert_eq!((t[5].shortcut, t[5].app.as_deref()), (None, Some("olk")));
    }

    #[test]
    fn titles_are_short_and_plain() {
        let mut w = win(1, 2, &format!("a\u{7}b{}", "x".repeat(500)), "x.exe");
        w.title.push('\n');
        let t = targets(&[], &[w], 0);
        assert_eq!(t[0].title.chars().count(), MAX_TITLE);
        assert!(t[0].title.starts_with("abx"));
    }

    #[test]
    fn target_ids_parse_back_and_nothing_else_does() {
        for id in [TargetId::Monitor(r"\\.\DISPLAY12".into()), TargetId::Window { hwnd: 0x1_0042, pid: 7 }] {
            assert_eq!(TargetId::parse(&id.to_string()), Some(id));
        }
        for bad in ["", "m:", r"m:\\.\DISPLAY", r"m:\\.\DISPLAY1x", "m:C:\\Windows", "w:", "w:12", "w:0:5", "w:12:0", "w:x:1", "w:-1:2", "x:1:2"] {
            assert_eq!(TargetId::parse(bad), None, "{bad:?}");
        }
        assert_eq!(TargetId::parse(&format!("w:1:{}", "9".repeat(90))), None, "too long");
        assert!(TargetId::parse("w:1:2").unwrap().is_window());
    }

    #[test]
    fn ides_are_known_by_their_executable_in_any_case() {
        assert_eq!(ide_name("CODE.EXE"), Some("VS Code"));
        assert_eq!(ide_name("idea64.exe"), Some("IntelliJ IDEA"));
        assert_eq!(ide_name("code"), None);
        assert_eq!(ide_name("notepad.exe"), None);
    }
}
