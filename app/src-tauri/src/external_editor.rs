//! "Open in …" for a project folder: the code editors installed on this machine.

use std::path::PathBuf;

use serde::Serialize;

use crate::fs_ops::CmdResult;

struct Editor {
    id: &'static str,
    label: &'static str,
    /// Command-line launcher (Windows / Linux).
    cli: &'static str,
    /// Application bundle name (macOS, where GUI apps do not see the shell PATH).
    mac_app: &'static str,
}

const EDITORS: &[Editor] = &[
    Editor { id: "vscode", label: "VS Code", cli: "code", mac_app: "Visual Studio Code" },
    Editor { id: "cursor", label: "Cursor", cli: "cursor", mac_app: "Cursor" },
    Editor { id: "windsurf", label: "Windsurf", cli: "windsurf", mac_app: "Windsurf" },
    Editor { id: "zed", label: "Zed", cli: "zed", mac_app: "Zed" },
    Editor { id: "sublime", label: "Sublime Text", cli: "subl", mac_app: "Sublime Text" },
    Editor { id: "idea", label: "IntelliJ IDEA", cli: "idea", mac_app: "IntelliJ IDEA" },
];

/// PATH lookup honouring PATHEXT (`code` is `code.cmd` on Windows).
fn which(program: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT".into()).split(';').map(|e| e.to_ascii_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|dir| {
        exts.iter().map(|e| dir.join(format!("{program}{e}"))).find(|p| p.is_file())
    })
}

fn mac_app_path(app: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    [PathBuf::from("/Applications"), home.join("Applications")].into_iter().map(|d| d.join(format!("{app}.app"))).find(|p| p.exists())
}

#[derive(Serialize)]
pub struct EditorInfo {
    id: &'static str,
    label: &'static str,
}

#[tauri::command]
pub fn editors_list() -> Vec<EditorInfo> {
    EDITORS
        .iter()
        .filter(|e| if cfg!(target_os = "macos") { mac_app_path(e.mac_app).is_some() } else { which(e.cli).is_some() })
        .map(|e| EditorInfo { id: e.id, label: e.label })
        .collect()
}

#[tauri::command]
pub fn editor_open(editor: String, path: String) -> CmdResult<()> {
    let e = EDITORS.iter().find(|e| e.id == editor).ok_or("unknown editor")?;
    // A remote project opens through the editor's own SSH remoting (VS Code and its forks).
    let remote = crate::remote::parse_root(&path);
    if remote.is_some() && !matches!(e.id, "vscode" | "cursor" | "windsurf") {
        return Err(format!("{} cannot open a folder on another machine; use VS Code, Cursor or Windsurf", e.label));
    }
    if remote.is_none() && !std::path::Path::new(&path).exists() {
        return Err(format!("{path} does not exist"));
    }
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        match &remote {
            Some(r) => {
                let host = r.port.map_or(r.dest.clone(), |p| format!("{}:{p}", r.dest));
                c.args(["-a", e.mac_app, "--args", "--remote", &format!("ssh-remote+{host}"), &r.path]);
            }
            None => {
                c.args(["-a", e.mac_app, &path]);
            }
        }
        c
    } else {
        // `code` is `code.cmd` on Windows. Spawned directly (not via `cmd /c`), std escapes the
        // argument for cmd.exe and refuses ones it cannot pass safely — a folder named `R&D`
        // must never become a second command.
        let exe = which(e.cli).ok_or_else(|| format!("{} is not on PATH", e.label))?;
        let mut c = std::process::Command::new(exe);
        crate::daemon_runtime::clean_env(&mut c);
        match &remote {
            Some(r) => {
                let host = match r.port {
                    Some(p) => format!("{}:{p}", r.dest),
                    None => r.dest.clone(),
                };
                c.args(["--remote", &format!("ssh-remote+{host}"), &r.path]);
            }
            None => {
                c.arg(&path);
            }
        }
        c
    };
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn().map(|_| ()).map_err(|err| format!("could not start {}: {err}", e.label))
}
