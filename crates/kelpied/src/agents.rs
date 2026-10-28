//! Turns a session spec (kind + user args) into the argv actually spawned on the PTY.
//!
//! - Injects status hooks: Claude Code via `--settings <file>`, Codex via `-c notify=[...]`.
//! - Adds resume flags when an agent session id is known (restart / chat → terminal switch).
//! - Resolves executables the way the user's shell would: Windows PATH+PATHEXT with npm
//!   `.cmd` shims unwrapped to `node <script>`, Unix via a login shell (GUI apps on macOS do not
//!   inherit the shell PATH).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Fully resolved spawn description.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub argv: Vec<String>,
    /// First prompt to type once the program is ready (agents that take no prompt argument,
    /// startup commands of shells).
    pub typed_prompt: Option<TypedPrompt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedPrompt {
    pub text: String,
    /// Deliver as a bracketed paste (agent TUIs: a multi-line prompt stays one message);
    /// otherwise as keystrokes (shells, where older line editors print the paste markers).
    pub paste: bool,
}

/// How an agent takes a first prompt at launch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PromptMode {
    /// Trailing positional argument.
    Argv,
    /// `<flag> <prompt>`, e.g. Gemini's `-i` (prompt, then stay interactive).
    Flag(&'static str),
    /// Typed into the TUI once it is ready.
    Typed,
}

/// A coding-agent CLI Kelpie knows how to start. Flags come from each CLI's documentation
/// (cross-checked with Orca's agent table).
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    /// Executable looked up on PATH.
    pub bin: &'static str,
    /// Arguments that start the interactive UI (e.g. `chat --tui`).
    pub launch: &'static [&'static str],
    /// Skip-every-permission-prompt flags (Settings → Agents → Permissions: Yolo).
    pub yolo: &'static [&'static str],
    /// Continue the most recent conversation in the folder (restart / hibernation resume).
    pub resume_latest: &'static [&'static str],
    pub prompt: PromptMode,
    /// Arguments that start it as an Agent Client Protocol agent over stdio (chat view), for
    /// plugin agents; the app knows the built-in ones (`app/src/lib/agents.ts`).
    pub acp: &'static [&'static str],
}

const fn preset(id: &'static str, label: &'static str, bin: &'static str) -> Preset {
    Preset { id, label, bin, launch: &[], yolo: &[], resume_latest: &[], prompt: PromptMode::Typed, acp: &[] }
}

pub const PRESETS: &[Preset] = &[
    Preset { yolo: &["--dangerously-skip-permissions"], resume_latest: &["--continue"], prompt: PromptMode::Argv, ..preset("claude", "Claude Code", "claude") },
    Preset { yolo: &["--dangerously-bypass-approvals-and-sandbox"], resume_latest: &["resume", "--last"], prompt: PromptMode::Argv, ..preset("codex", "Codex", "codex") },
    Preset { resume_latest: &["--continue"], prompt: PromptMode::Argv, ..preset("omp", "OMP", "omp") },
    Preset { yolo: &["--yolo"], resume_latest: &["--resume", "latest"], prompt: PromptMode::Flag("-i"), ..preset("gemini", "Gemini", "gemini") },
    Preset { yolo: &["--yolo"], resume_latest: &["--continue"], prompt: PromptMode::Argv, ..preset("cursor", "Cursor CLI", "cursor-agent") },
    Preset { resume_latest: &["--continue"], prompt: PromptMode::Flag("--prompt"), ..preset("opencode", "OpenCode", "opencode") },
    Preset { yolo: &["--yolo"], resume_latest: &["--continue"], prompt: PromptMode::Flag("-i"), ..preset("copilot", "GitHub Copilot", "copilot") },
    Preset { yolo: &["--permission-mode", "bypassPermissions"], prompt: PromptMode::Argv, ..preset("grok", "Grok", "grok") },
    Preset { yolo: &["--approval-mode", "yolo"], resume_latest: &["--continue"], ..preset("qwen", "Qwen Code", "qwen") },
    Preset { yolo: &["--auto", "high"], prompt: PromptMode::Argv, ..preset("droid", "Droid", "droid") },
    Preset { yolo: &["--yes-always"], resume_latest: &["--restore-chat-history"], ..preset("aider", "Aider", "aider") },
    Preset { yolo: &["--dangerously-allow-all"], ..preset("amp", "Amp", "amp") },
    Preset { launch: &["session"], resume_latest: &["-r"], ..preset("goose", "Goose", "goose") },
    Preset { yolo: &["--yolo"], resume_latest: &["-C"], ..preset("kimi", "Kimi", "kimi") },
    Preset { yolo: &["--yolo"], resume_latest: &["-C"], ..preset("crush", "Crush", "crush") },
    Preset { yolo: &["--auto-approve", "true"], ..preset("cline", "Cline", "cline") },
    Preset { launch: &["chat", "--tui"], yolo: &["--trust-all-tools"], ..preset("kiro", "Kiro", "kiro-cli") },
    Preset { yolo: &["--allow", "*"], ..preset("continue", "Continue", "cn") },
    Preset { yolo: &["--permission-mode", "bypass"], prompt: PromptMode::Argv, ..preset("devin", "Devin", "devin") },
    preset("auggie", "Auggie", "auggie"),
    Preset { yolo: &["--dangerously-skip-permissions"], prompt: PromptMode::Argv, ..preset("codebuddy", "CodeBuddy", "codebuddy") },
    Preset { yolo: &["--agent", "auto-approve"], ..preset("vibe", "Mistral Vibe", "vibe") },
    Preset { resume_latest: &["--continue"], prompt: PromptMode::Argv, ..preset("pi", "Pi", "pi") },
];

pub fn preset_for(kind: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == kind).or_else(|| CUSTOM.read().iter().copied().find(|p| p.id == kind))
}

/// Agents added by plugins (`agent.set_custom`), kept with the built-in presets so launching,
/// resuming and the installed check treat them the same. Each set leaks a few small strings —
/// it happens when plugins change, not per session.
static CUSTOM: parking_lot::RwLock<Vec<&'static Preset>> = parking_lot::RwLock::new(Vec::new());

/// A plugin's agent, as the app sends it.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct AgentDef {
    pub id: String,
    pub label: String,
    pub bin: String,
    #[serde(default)]
    pub launch: Vec<String>,
    #[serde(default)]
    pub yolo: Vec<String>,
    #[serde(default)]
    pub resume_latest: Vec<String>,
    /// "argv" | "typed" | "flag:<flag>"
    #[serde(default)]
    pub prompt: String,
    /// Arguments that start it as an ACP agent (chat view); empty: terminal only.
    #[serde(default)]
    pub acp: Vec<String>,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn leak_all(v: &[String]) -> &'static [&'static str] {
    Box::leak(v.iter().map(|s| leak(s)).collect::<Vec<_>>().into_boxed_slice())
}

/// Replaces the plugin agents. Ids must be new (not a built-in) and plain.
pub fn set_custom(defs: &[AgentDef]) -> Result<()> {
    let mut out = Vec::new();
    for d in defs {
        anyhow::ensure!(!d.id.is_empty() && d.id.len() <= 48 && d.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "invalid agent id {:?}", d.id);
        anyhow::ensure!(!PRESETS.iter().any(|p| p.id == d.id) && !matches!(d.id.as_str(), "shell" | "custom"), "agent id {} is taken", d.id);
        anyhow::ensure!(!d.bin.trim().is_empty() && !d.bin.starts_with('-'), "agent {} needs a program", d.id);
        let prompt = match d.prompt.as_str() {
            "argv" => PromptMode::Argv,
            p if p.starts_with("flag:") && p.len() > 5 => PromptMode::Flag(leak(&p[5..])),
            _ => PromptMode::Typed,
        };
        out.push(&*Box::leak(Box::new(Preset { id: leak(&d.id), label: leak(&d.label), bin: leak(&d.bin), launch: leak_all(&d.launch), yolo: leak_all(&d.yolo), resume_latest: leak_all(&d.resume_latest), prompt, acp: leak_all(&d.acp) })));
    }
    *CUSTOM.write() = out;
    Ok(())
}

/// All presets, built-in then plugin ones.
pub fn all_presets() -> Vec<&'static Preset> {
    PRESETS.iter().chain(CUSTOM.read().iter().copied()).collect()
}

/// Which preset agents are installed (on PATH; through the login shell on Unix).
pub fn installed_presets() -> Vec<&'static str> {
    #[cfg(windows)]
    {
        all_presets().into_iter().filter(|p| which(p.bin).is_some()).map(|p| p.id).collect()
    }
    #[cfg(unix)]
    {
        let presets = all_presets();
        let bins: Vec<&str> = presets.iter().map(|p| p.bin).filter(|b| b.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))).collect();
        let script = format!("for b in {}; do command -v \"$b\" >/dev/null 2>&1 && echo \"$b\"; done", bins.join(" "));
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let found = std::process::Command::new(shell).args(["-l", "-c", &script]).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let found: Vec<&str> = found.lines().collect();
        presets.into_iter().filter(|p| found.contains(&p.bin)).map(|p| p.id).collect()
    }
}

/// Default interactive shell for plain terminal sessions.
pub fn default_shell() -> Vec<String> {
    if cfg!(windows) {
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if let Some(p) = which(candidate) {
                return vec![p.to_string_lossy().into_owned(), "-NoLogo".into()];
            }
        }
        vec![std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into())]
    } else {
        vec![std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()), "-l".into()]
    }
}

/// Whether Claude Code has saved conversation `id`: a `<id>.jsonl` under `projects/` of the
/// session's config home (`CLAUDE_CONFIG_DIR` from its environment, else the default).
pub fn claude_conversation_saved(id: &str, env: &std::collections::BTreeMap<String, String>) -> bool {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return false;
    }
    let home = env
        .get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from))
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")));
    let Some(projects) = home.map(|h| h.join("projects")) else { return false };
    let file = format!("{id}.jsonl");
    std::fs::read_dir(projects).into_iter().flatten().filter_map(|e| e.ok()).any(|d| d.path().join(&file).is_file())
}

/// Whether Codex saved thread `id` (so `codex resume <id>` finds it). Codex also runs unsaved
/// background threads of its own (suggestions, skill selection) that report through `notify`
/// too; their ids must not replace the conversation's. Rollouts live in
/// `sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`, under the day the thread started: a UUIDv7 id
/// carries that time, so only the days around it are looked at.
pub fn codex_thread_saved(id: &str, codex_home: &std::path::Path) -> bool {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return false;
    }
    let sessions = codex_home.join("sessions");
    let suffix = format!("-{id}.jsonl");
    let has = |dir: PathBuf| {
        std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().ends_with(&suffix))
    };
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    let Some(ms) = (hex.len() == 32 && hex.as_bytes()[12] == b'7').then(|| u64::from_str_radix(&hex[..12], 16).ok()).flatten() else {
        // Not time-ordered: look through every day.
        return walk_days(&sessions).into_iter().any(has);
    };
    // The folder uses the local date; a day either side of the UTC one covers every zone.
    let day = (ms / 86_400_000) as i64;
    (day - 1..=day + 1).any(|d| {
        let (y, m, dd) = civil_from_days(d);
        has(sessions.join(format!("{y:04}")).join(format!("{m:02}")).join(format!("{dd:02}")))
    })
}

fn walk_days(sessions: &std::path::Path) -> Vec<PathBuf> {
    let sub = |p: &std::path::Path| -> Vec<PathBuf> { std::fs::read_dir(p).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect() };
    sub(sessions).iter().flat_map(|y| sub(y)).flat_map(|m| sub(&m)).collect()
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// POSIX shell single quoting.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

const ROOT_BYPASS_NOTE: &str = "[kelpie] Logged in as root: Claude Code refuses to skip permission prompts as root, so it starts asking for them. To skip them, run it as a regular user (Kelpie: SSH hosts → Set up a regular user…).";

/// Claude's argv to start as root: without the flags that skip permission prompts (all refused
/// as root), and in the default permission mode unless it names another one, since a
/// `defaultMode: bypassPermissions` in its settings is refused as well.
fn claude_as_root(argv: &[String]) -> Vec<String> {
    let mut out = without_claude_bypass(argv);
    if !out.iter().any(|a| a == "--permission-mode" || a.starts_with("--permission-mode=")) {
        out.extend(["--permission-mode".to_string(), "default".to_string()]);
    }
    out
}

/// Claude's argv without the flags that skip permission prompts.
fn without_claude_bypass(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut it = argv.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions" | "--permission-mode=bypassPermissions" => {}
            "--permission-mode" if it.peek().is_some_and(|v| *v == "bypassPermissions") => {
                it.next();
            }
            _ => out.push(a.clone()),
        }
    }
    out
}

/// A session on another machine: `ssh -t` running the program there in the remote folder,
/// through the user's login shell (so PATH has their tools), inside tmux when asked and
/// available (`tmux new-session -A` re-attaches when that session still runs). Agents get no
/// hook settings (hooks would call a local executable) and report status by output only.
pub fn build_remote_launch(kind: &str, user_args: &[String], resume: Resume, prompt: Option<&str>, remote: &crate::session::RemoteSpec, cwd: &str, session_id: &str) -> Result<Launch> {
    anyhow::ensure!(!remote.dest.is_empty() && !remote.dest.starts_with('-'), "invalid ssh destination");
    let startup = || prompt.filter(|t| !t.trim().is_empty()).map(|t| TypedPrompt { text: t.to_string(), paste: false });
    let mut typed_prompt = None;
    let quote = |argv: &[String]| argv.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ");
    // A shell `if` that picks the command on the host, when only the host can tell.
    let mut choice: Option<String> = None;
    let program: Option<Vec<String>> = match kind {
        "shell" => (!user_args.is_empty()).then(|| user_args.to_vec()),
        "custom" => {
            anyhow::ensure!(!user_args.is_empty(), "custom session needs a command");
            Some(user_args.to_vec())
        }
        other => {
            let p = preset_for(other).ok_or_else(|| anyhow::anyhow!("unknown session kind {other}"))?;
            let mut rest: Vec<String> = user_args.to_vec();
            if let Some(text) = prompt.filter(|t| !t.trim().is_empty()) {
                let fits = text.len() <= 8000;
                match p.prompt {
                    PromptMode::Argv if fits && !text.starts_with('-') => rest.push(text.to_string()),
                    PromptMode::Flag(flag) if fits => rest.extend([flag.to_string(), text.to_string()]),
                    _ => typed_prompt = Some(TypedPrompt { text: text.to_string(), paste: true }),
                }
            }
            let with = |r: &Resume, as_root: bool| -> Vec<String> {
                let base = std::iter::once(p.bin).chain(p.launch.iter().copied()).map(str::to_string);
                let argv: Vec<String> = base.chain(resume_args(other, r, Some(p))).chain(rest.iter().cloned()).collect();
                if as_root { claude_as_root(&argv) } else { argv }
            };
            // Claude's id is Kelpie's own (`--session-id`), so it may name a conversation that
            // was never saved (no message sent): resume it when the host has its transcript,
            // else start it under that id.
            let exec = |as_root: bool| match resume {
                Resume::Id(id) if other == "claude" && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => format!(
                    r#"if ls "${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}"/projects/*/{id}.jsonl >/dev/null 2>&1; then exec {}; else exec {}; fi"#,
                    quote(&with(&Resume::Id(id), as_root)),
                    quote(&with(&Resume::New(id), as_root))
                ),
                _ => format!("exec {}", quote(&with(&resume, as_root))),
            };
            // Claude refuses every bypass flag as root (unless the host sets IS_SANDBOX=1) and
            // exits at once, so as root it starts asking for permission instead, and says why.
            let full = with(&resume, false);
            choice = Some(if other == "claude" && without_claude_bypass(&full) != full {
                format!(
                    r#"if [ "$(id -u)" = 0 ] && [ "${{IS_SANDBOX:-}}" != 1 ]; then printf '%s\n' {} >&2; {}; else {}; fi"#,
                    sh_quote(ROOT_BYPASS_NOTE),
                    exec(true),
                    exec(false)
                )
            } else {
                exec(false)
            });
            Some(full)
        }
    };
    if matches!(kind, "shell" | "custom") {
        typed_prompt = startup();
    }
    let run = match &program {
        Some(argv) => quote(argv),
        None => r#""${SHELL:-/bin/sh}" -l"#.to_string(),
    };
    let dir = sh_quote(cwd);
    // sshd prints the login banner (MOTD) only when it runs no command, so a plain shell prints
    // it itself; `~/.hushlogin` silences it as it does for sshd. Agents start without it.
    let banner = (kind == "shell" && program.is_none()).then_some(r#"[ -e "$HOME/.hushlogin" ] || cat /run/motd.dynamic /etc/motd 2>/dev/null"#);
    let direct = match (banner, &choice) {
        (Some(b), _) => format!("cd {dir} && {{ {b}; exec {run}; }}"),
        (None, Some(c)) => format!("cd {dir} && {c}"),
        (None, None) => format!("cd {dir} && exec {run}"),
    };
    // tmux runs this only when it starts the session: re-attaching shows the screen as it was.
    let in_tmux = match (banner, &choice) {
        (Some(b), _) => format!("{b}; exec {run}"),
        (None, Some(c)) => c.clone(),
        (None, None) => run.clone(),
    };
    let body = if remote.tmux {
        let id: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
        // Sessions started before the rename to Kelpie run in `erka-<id>`: re-attach to that one
        // when it is still there instead of starting the agent a second time.
        let (name, legacy) = (format!("kelpie-{id}"), format!("erka-{id}"));
        format!(
            "if command -v tmux >/dev/null 2>&1; then n={}; tmux has-session -t {} 2>/dev/null && n={}; exec tmux new-session -A -s \"$n\" -c {dir} {}; else {direct}; fi",
            sh_quote(&name),
            sh_quote(&format!("={legacy}")),
            sh_quote(&legacy),
            sh_quote(&in_tmux)
        )
    } else {
        direct
    };
    let mut argv = vec![system_ssh(), "-t".into()];
    if let Some(c) = remote.config.as_deref().filter(|c| !c.is_empty()) {
        argv.extend(["-F".to_string(), c.to_string()]);
    }
    if let Some(port) = remote.port {
        argv.extend(["-p".to_string(), port.to_string()]);
    }
    argv.extend(["-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=4"].map(str::to_string));
    argv.extend([remote.dest.clone(), "--".into(), format!(r#"exec "${{SHELL:-/bin/sh}}" -lc {}"#, sh_quote(&body))]);
    Ok(Launch { argv: wrap_for_platform(argv)?, typed_prompt })
}

/// Windows' own OpenSSH client when present (as the app uses): one from Git Bash earlier on PATH
/// cannot reach the Windows ssh-agent service, so agent forwarding would silently not work.
fn system_ssh() -> String {
    if cfg!(windows) {
        let p = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("OpenSSH").join("ssh.exe");
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    "ssh".into()
}

/// What to resume when (re)starting an agent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resume<'a> {
    None,
    /// The agent's own conversation id (from hooks / transcripts).
    Id(&'a str),
    /// A new conversation under an id Kelpie chose (Claude Code's `--session-id`), so it is
    /// known from the start — even if no hook ever reports it. Other agents start plainly.
    New(&'a str),
    /// The most recent conversation in the folder, for agents that report no id.
    Latest,
}

/// Builds the argv for `kind`. `user_args` are extra flags from the user/profile; `prompt` is
/// an optional first message (typed once the agent is ready when resuming or too long).
pub fn build_launch(
    kind: &str,
    user_args: &[String],
    resume: Resume,
    prompt: Option<&str>,
    hook_exe: &Path,
    data_dir: &Path,
) -> Result<Launch> {
    // Shells and plain commands take `prompt` as a startup command, typed as plain keys.
    let startup = || prompt.filter(|t| !t.trim().is_empty()).map(|t| TypedPrompt { text: t.to_string(), paste: false });
    let mut argv: Vec<String> = match kind {
        "shell" => return Ok(Launch { argv: if user_args.is_empty() { default_shell() } else { resolve(user_args)? }, typed_prompt: startup() }),
        "custom" => {
            anyhow::ensure!(!user_args.is_empty(), "custom session needs a command");
            return Ok(Launch { argv: wrap_for_platform(user_args.to_vec())?, typed_prompt: startup() });
        }
        "claude" => {
            let settings = write_claude_hook_settings(hook_exe, data_dir)?;
            vec!["claude".to_string(), "--settings".into(), settings.to_string_lossy().into_owned()]
        }
        "codex" => {
            let exe = hook_exe.to_string_lossy().replace('\\', "/");
            vec!["codex".to_string(), "-c".into(), format!("notify=['{exe}','hook','codex']")]
        }
        other => {
            let p = preset_for(other).ok_or_else(|| anyhow::anyhow!("unknown session kind {other}"))?;
            std::iter::once(p.bin).chain(p.launch.iter().copied()).map(str::to_string).collect()
        }
    };
    let preset = preset_for(kind);
    argv.extend(resume_args(kind, &resume, preset));
    argv.extend(user_args.iter().cloned());
    let mut typed_prompt = None;
    if let Some(text) = prompt.filter(|t| !t.trim().is_empty()) {
        // A long prompt (a hand-off transcript) is typed instead: Windows command lines stop at
        // 32 K characters, and batch-file launchers mangle newlines. A resumed conversation takes
        // the argument too (`claude --resume <id> "…"`, `codex resume <id> "…"` run it there), so
        // the agent submits it itself instead of racing a typed Enter against its start-up.
        let fits = text.len() <= 8000;
        match preset.map_or(PromptMode::Typed, |p| p.prompt) {
            // A prompt that looks like an option would be parsed as one.
            PromptMode::Argv if fits && !text.starts_with('-') => argv.push(text.to_string()),
            PromptMode::Flag(flag) if fits => argv.extend([flag.to_string(), text.to_string()]),
            _ => typed_prompt = Some(TypedPrompt { text: text.to_string(), paste: true }),
        }
    }
    Ok(Launch { argv: wrap_for_platform(argv)?, typed_prompt })
}

/// The arguments that pick the conversation: `codex resume <id>`, `--resume <id>`, Claude's
/// `--session-id <id>` for a new one, or the preset's "latest in this folder".
fn resume_args(kind: &str, resume: &Resume, preset: Option<&Preset>) -> Vec<String> {
    match *resume {
        Resume::Id(id) if kind == "codex" => vec!["resume".into(), id.into()],
        Resume::Id(id) => vec!["--resume".into(), id.into()],
        Resume::New(id) if kind == "claude" => vec!["--session-id".into(), id.into()],
        Resume::Latest => preset.map_or(&[][..], |p| p.resume_latest).iter().map(|a| a.to_string()).collect(),
        Resume::New(_) | Resume::None => Vec::new(),
    }
}

/// The user's own Claude arguments already choose the conversation (`--resume`, `--continue`,
/// `--session-id`, `--fork-session`…): Kelpie then names none, as Claude rejects two choices.
pub fn claude_args_pick_conversation(args: &[String]) -> bool {
    args.iter().any(|a| {
        let flag = a.split('=').next().unwrap_or(a);
        matches!(flag, "--resume" | "-r" | "--continue" | "-c" | "--session-id" | "--fork-session" | "--from-pr")
    })
}

/// Claude Code merges `--settings` hooks with the user's own settings, so we never touch
/// `~/.claude/settings.json`. One command handles every event (name comes in the payload).
fn write_claude_hook_settings(hook_exe: &Path, data_dir: &Path) -> Result<PathBuf> {
    let exe = hook_exe.to_string_lossy().replace('\\', "/");
    let command = format!("\"{exe}\" hook claude");
    let entry = serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]);
    let mut hooks = serde_json::Map::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "StopFailure",
        "SubagentStart",
        "PostCompact",
    ] {
        hooks.insert(event.into(), entry.clone());
    }
    let settings = serde_json::json!({ "hooks": hooks });
    let path = data_dir.join("claude-hooks.json");
    let body = serde_json::to_vec_pretty(&settings)?;
    if std::fs::read(&path).ok().as_deref() != Some(&body[..]) {
        std::fs::create_dir_all(data_dir)?;
        crate::persist::write_atomic(&path, &body).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(path)
}

/// Platform-correct argv for a non-PTY child (chat mode) or a PTY agent.
pub fn platform_argv(argv: Vec<String>) -> Result<Vec<String>> {
    wrap_for_platform(argv)
}

#[cfg(windows)]
fn wrap_for_platform(argv: Vec<String>) -> Result<Vec<String>> {
    resolve(&argv)
}

/// Runs the agent through the user's login shell so PATH (nvm, bun, homebrew…) matches what
/// they get in Terminal.app. `exec` keeps the agent as the PTY's direct child.
#[cfg(unix)]
fn wrap_for_platform(argv: Vec<String>) -> Result<Vec<String>> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let joined = argv.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ");
    Ok(vec![shell, "-l".into(), "-c".into(), format!("exec {joined}")])
}

#[cfg(unix)]
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty() && arg.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c)) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Windows: resolve argv[0] through PATH/PATHEXT; unwrap npm cmd-shims to `node script.js`;
/// other batch files go through `cmd.exe /d /c`.
#[cfg(windows)]
fn resolve(argv: &[String]) -> Result<Vec<String>> {
    let program = &argv[0];
    let Some(path) = which(program) else {
        anyhow::bail!("`{program}` not found on PATH");
    };
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let rest = argv[1..].iter().cloned();
    if ext == "cmd" || ext == "bat" {
        if let Some((node, script)) = parse_npm_shim(&path) {
            return Ok([node, script].into_iter().chain(rest).collect());
        }
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        return Ok([comspec, "/d".into(), "/c".into(), path.to_string_lossy().into_owned()].into_iter().chain(rest).collect());
    }
    Ok(std::iter::once(path.to_string_lossy().into_owned()).chain(rest).collect())
}

#[cfg(unix)]
fn resolve(argv: &[String]) -> Result<Vec<String>> {
    Ok(argv.to_vec())
}

/// Recognises the standard npm `cmd-shim` output:
/// `"%_prog%"  "%dp0%\node_modules\pkg\bin\cli.js" %*`
#[cfg(windows)]
fn parse_npm_shim(path: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let dir = path.parent()?;
    let script = dir.join(npm_shim_script(&text)?);
    if !script.exists() {
        return None;
    }
    let local_node = dir.join("node.exe");
    let node = if local_node.exists() { local_node } else { which("node")? };
    Some((node.to_string_lossy().into_owned(), script.to_string_lossy().into_owned()))
}

/// The script is the last `"%dp0%\…"` token ending in a JS extension; earlier tokens point at
/// `node.exe`.
#[cfg_attr(not(windows), allow(dead_code))]
fn npm_shim_script(text: &str) -> Option<&str> {
    text.rmatch_indices("\"%dp0%\\").find_map(|(start, m)| {
        let rest = &text[start + m.len()..];
        let rel = &rest[..rest.find('"')?];
        [".js", ".mjs", ".cjs"].iter().any(|ext| rel.ends_with(ext)).then_some(rel)
    })
}

/// PATH lookup honouring PATHEXT on Windows (extension-less files are skipped there because
/// npm/bun also drop POSIX shell scripts next to the real shims).
pub fn which(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.is_absolute() || program.contains(['/', '\\']) {
        return candidate.exists().then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let has_ext = candidate.extension().is_some();
        let mut list: Vec<String> = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect();
        if has_ext {
            list.insert(0, String::new());
        }
        list
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let full = dir.join(format!("{program}{ext}"));
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_hook_settings_are_published_as_complete_json() {
        use std::sync::{atomic::{AtomicUsize, Ordering}, Barrier};

        let dir = std::env::temp_dir().join(format!("kelpie-hooks-atomic-{}", uuid::Uuid::new_v4()));
        let path = write_claude_hook_settings(Path::new("/fixture/initial/kelpied"), &dir).unwrap();
        let start = Barrier::new(3);
        let done = AtomicUsize::new(0);
        let mut incomplete = false;
        std::thread::scope(|scope| {
            for writer in 0..2 {
                let (dir, start, done) = (&dir, &start, &done);
                scope.spawn(move || {
                    start.wait();
                    for revision in 0..128 {
                        let exe = PathBuf::from(format!("/fixture/{writer}/{revision}/kelpied"));
                        write_claude_hook_settings(&exe, dir).unwrap();
                    }
                    done.fetch_add(1, Ordering::Release);
                });
            }
            start.wait();
            loop {
                let bytes = std::fs::read(&path).unwrap();
                if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
                    incomplete = true;
                    break;
                }
                if done.load(Ordering::Acquire) == 2 {
                    break;
                }
            }
        });
        std::fs::remove_dir_all(dir).unwrap();
        assert!(!incomplete, "a consumer observed torn hook settings during concurrent launches");
    }


    #[test]
    fn user_args_that_choose_the_conversation() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(claude_args_pick_conversation(&a(&["--model", "opus", "--continue"])));
        assert!(claude_args_pick_conversation(&a(&["-r"])));
        assert!(claude_args_pick_conversation(&a(&["--session-id=1234"])));
        assert!(claude_args_pick_conversation(&a(&["--resume", "x", "--fork-session"])));
        assert!(!claude_args_pick_conversation(&a(&["--model", "opus", "--dangerously-skip-permissions"])));
    }

    #[test]
    fn finds_npm_shim_script() {
        let shim = "IF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n)\r\n\
                    \"%_prog%\"  \"%dp0%\\pkgs\\@openai\\codex\\bin\\codex.js\" %*\r\n";
        assert_eq!(npm_shim_script(shim), Some("pkgs\\@openai\\codex\\bin\\codex.js"));
        assert_eq!(npm_shim_script("\"%dp0%\\node.exe\""), None);
    }


}

#[cfg(test)]
mod conversation_tests {
    use super::*;

    #[test]
    fn claude_conversation_saved_looks_in_the_sessions_config_home() {
        let home = std::env::temp_dir().join(format!("kelpie-claude-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join("projects").join("C--some-project")).unwrap();
        std::fs::write(home.join("projects").join("C--some-project").join("abc-123.jsonl"), "{}\n").unwrap();
        let env: std::collections::BTreeMap<String, String> = [("CLAUDE_CONFIG_DIR".to_string(), home.to_string_lossy().into_owned())].into();
        assert!(claude_conversation_saved("abc-123", &env));
        assert!(!claude_conversation_saved("never-saved", &env));
        assert!(!claude_conversation_saved("../abc-123", &env));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn only_a_saved_codex_thread_counts() {
        let home = std::env::temp_dir().join(format!("kelpie-codex-home-{}", std::process::id()));
        let day = home.join("sessions").join("2026").join("10").join("23");
        std::fs::create_dir_all(&day).unwrap();
        // The conversation (saved) and a background thread of the same process 50 s later (not).
        std::fs::write(day.join("rollout-2026-10-23T14-52-03-01a167be-cb4f-7773-b37a-95835bbafc28.jsonl"), "{}\n").unwrap();
        assert!(codex_thread_saved("01a167be-cb4f-7773-b37a-95835bbafc28", &home));
        assert!(!codex_thread_saved("01a167bf-8fbc-7cc3-aa31-c874dd6af1d9", &home));
        // An id that is not time-ordered is looked for in every day.
        std::fs::write(day.join("rollout-2026-10-23T09-00-00-5a5f996e-bc9d-4b62-95c9-787b89fd1454.jsonl"), "{}\n").unwrap();
        assert!(codex_thread_saved("5a5f996e-bc9d-4b62-95c9-787b89fd1454", &home));
        assert!(!codex_thread_saved("../x", &home));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_749), (2026, 10, 23));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}

/// The command a launch runs as plain words joined by spaces. On Unix the argv is wrapped in the
/// login shell (`$SHELL -l -c "exec …"`, quoted by `shell_quote`); that layer is undone here so
/// tests read the same on every platform.
#[cfg(test)]
fn command_line(argv: &[String]) -> String {
    match argv {
        [_, l, c, cmd] if cfg!(unix) && l == "-l" && c == "-c" => match cmd.strip_prefix("exec ") {
            Some(rest) => shell_words(rest).join(" "),
            None => argv.join(" "),
        },
        _ => argv.join(" "),
    }
}

/// Splits a POSIX shell string of single-quoted and `\`-escaped words (what `shell_quote` writes).
#[cfg(test)]
fn shell_words(s: &str) -> Vec<String> {
    let (mut words, mut cur) = (Vec::new(), String::new());
    let (mut quoted, mut escaped, mut in_word) = (false, false, false);
    for c in s.chars() {
        if escaped {
            cur.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if !quoted => (escaped, in_word) = (true, true),
            '\'' => (quoted, in_word) = (!quoted, true),
            ' ' if !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            _ => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

#[cfg(test)]
mod remote_tests {
    use super::*;
    use crate::session::RemoteSpec;

    #[test]
    fn shell_words_undo_shell_quote() {
        let args = ["ssh", "-t", "a b", "it's", r#"exec "${SHELL:-/bin/sh}" -lc 'cd '\''/x y'\'''"#];
        let quoted = args.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect::<Vec<_>>().join(" ");
        assert_eq!(shell_words(&quoted), args);
    }

    fn remote(tmux: bool) -> RemoteSpec {
        RemoteSpec { dest: "me@box".into(), port: Some(2222), config: None, tmux }
    }

    #[test]
    fn remote_shell_and_agent_commands() {
        let l = build_remote_launch("shell", &[], Resume::None, None, &remote(false), "/srv/my app", "abc").unwrap();
        let j = command_line(&l.argv);
        assert!(j.contains("-t") && j.contains("-p 2222") && j.contains("me@box --"), "{j}");
        assert!(j.ends_with(r#"exec "${SHELL:-/bin/sh}" -lc 'cd '\''/srv/my app'\'' && { [ -e "$HOME/.hushlogin" ] || cat /run/motd.dynamic /etc/motd 2>/dev/null; exec "${SHELL:-/bin/sh}" -l; }'"#), "{j}");
        let l = build_remote_launch("claude", &["--model".into(), "haiku".into()], Resume::Latest, None, &remote(false), "/srv/x", "abc").unwrap();
        let j = command_line(&l.argv);
        assert!(!j.contains("motd"), "agents start without the login banner: {j}");
        assert!(j.contains("exec '\\''claude'\\'' '\\''--continue'\\'' '\\''--model'\\'' '\\''haiku'\\''"), "{j}");
        assert!(!j.contains("--settings"), "no local hook settings on a remote host: {j}");
    }

    #[test]
    fn remote_tmux_reattaches_by_session() {
        let l = build_remote_launch("claude", &[], Resume::None, Some("fix it"), &remote(true), "/srv/x", "1234-5678-abcd").unwrap();
        let j = command_line(&l.argv);
        assert!(j.contains("tmux new-session -A -s \"$n\""), "{j}");
        // Quoted once more inside the remote shell's -c argument.
        assert!(j.contains(r"n='\''kelpie-12345678abcd'\''"), "{j}");
        assert!(j.contains(r"tmux has-session -t '\''=erka-12345678abcd'\'' 2>/dev/null && n='\''erka-12345678abcd'\''"), "re-attaches a pre-rename session: {j}");
        assert!(j.contains("command -v tmux"), "{j}");
        assert!(j.contains("fix it"), "{j}");
        assert!(!j.contains("motd"), "{j}");
    }

    #[test]
    fn remote_shell_shows_the_login_banner_once() {
        let l = build_remote_launch("shell", &[], Resume::None, None, &remote(true), "/srv/x", "abc").unwrap();
        let j = command_line(&l.argv);
        // Both as the tmux session's first command and in the no-tmux fallback.
        assert_eq!(j.matches("cat /run/motd.dynamic /etc/motd").count(), 2, "{j}");
        assert!(j.contains("hushlogin"), "{j}");
        let l = build_remote_launch("shell", &["htop".into()], Resume::None, None, &remote(true), "/srv/x", "abc").unwrap();
        assert!(!command_line(&l.argv).contains("motd"), "a shell running a command has no banner");
    }

    #[test]
    fn remote_claude_resumes_its_own_id_or_starts_it() {
        let id = "5a5f996e-bc9d-4b62-95c9-787b89fd1454";
        for tmux in [false, true] {
            let j = command_line(&build_remote_launch("claude", &["--model".into(), "haiku".into()], Resume::Id(id), None, &remote(tmux), "/srv/x", "abc").unwrap().argv);
            // The host decides: its transcript of that id exists, or not yet (nothing sent).
            assert!(j.contains(&format!("projects/*/{id}.jsonl")), "{j}");
            assert!(j.contains(&format!(r"exec '\''claude'\'' '\''--resume'\'' '\''{id}'\'' '\''--model'\''")), "{j}");
            assert!(j.contains(&format!(r"else exec '\''claude'\'' '\''--session-id'\'' '\''{id}'\''")), "{j}");
            assert!(!j.contains("&& exec if"), "{j}");
        }
        let j = command_line(&build_remote_launch("claude", &[], Resume::New(id), None, &remote(false), "/srv/x", "abc").unwrap().argv);
        assert!(j.contains("--session-id") && !j.contains("--resume") && !j.contains("jsonl"), "{j}");
        // Codex resumes the id it reported; no choice needed.
        let j = command_line(&build_remote_launch("codex", &[], Resume::Id(id), None, &remote(false), "/srv/x", "abc").unwrap().argv);
        assert!(j.contains("'\\''resume'\\''") && !j.contains("jsonl"), "{j}");
    }

    #[test]
    fn remote_claude_as_root_drops_the_bypass_flags() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(without_claude_bypass(&a(&["claude", "--dangerously-skip-permissions", "--model", "opus"])), a(&["claude", "--model", "opus"]));
        assert_eq!(without_claude_bypass(&a(&["claude", "--permission-mode", "bypassPermissions", "--allow-dangerously-skip-permissions"])), a(&["claude"]));
        assert_eq!(without_claude_bypass(&a(&["claude", "--permission-mode=bypassPermissions", "--permission-mode", "plan"])), a(&["claude", "--permission-mode", "plan"]));
        // Settings may say bypassPermissions too: as root it starts in the default mode.
        assert_eq!(claude_as_root(&a(&["claude", "--dangerously-skip-permissions"])), a(&["claude", "--permission-mode", "default"]));
        assert_eq!(claude_as_root(&a(&["claude", "--dangerously-skip-permissions", "--permission-mode", "plan"])), a(&["claude", "--permission-mode", "plan"]));
        let id = "5a5f996e-bc9d-4b62-95c9-787b89fd1454";
        for tmux in [false, true] {
            let j = command_line(&build_remote_launch("claude", &["--dangerously-skip-permissions".into()], Resume::Id(id), None, &remote(tmux), "/srv/x", "abc").unwrap().argv);
            // The host decides: root without IS_SANDBOX=1 runs it without the flag, saying why.
            assert!(j.contains(r#"if [ "$(id -u)" = 0 ] && [ "${IS_SANDBOX:-}" != 1 ]; then printf"#), "{j}");
            assert!(j.contains("Logged in as root"), "{j}");
            assert!(j.contains(&format!(r"then exec '\''claude'\'' '\''--resume'\'' '\''{id}'\'' '\''--permission-mode'\'' '\''default'\''; else")), "as root, no flag: {j}");
            assert!(j.contains(r"'\''--dangerously-skip-permissions'\''; fi; fi"), "kept for other users: {j}");
        }
        let j = command_line(&build_remote_launch("claude", &["--model".into(), "haiku".into()], Resume::None, None, &remote(false), "/srv/x", "abc").unwrap().argv);
        assert!(!j.contains("id -u"), "no check without a bypass flag: {j}");
        let j = command_line(&build_remote_launch("codex", &["--dangerously-skip-permissions".into()], Resume::None, None, &remote(false), "/srv/x", "abc").unwrap().argv);
        assert!(!j.contains("id -u"), "only Claude refuses root: {j}");
    }

    #[test]
    fn remote_rejects_option_like_destinations() {
        let r = RemoteSpec { dest: "-oProxyCommand=calc".into(), ..Default::default() };
        assert!(build_remote_launch("shell", &[], Resume::None, None, &r, "/", "a").is_err());
    }
}

#[cfg(test)]
mod custom_tests {
    use super::*;

    #[test]
    fn plugin_agents_launch_and_resume_like_presets() {
        // A program that exists everywhere (Windows resolves the binary on PATH at launch).
        let bin = if cfg!(windows) { "cmd" } else { "sh" };
        let def = AgentDef { id: "acme-bot".into(), label: "Acme Bot".into(), bin: bin.into(), launch: vec!["chat".into()], yolo: vec!["--yes".into()], resume_latest: vec!["--continue".into()], prompt: "flag:--ask".into(), acp: vec!["acp".into()] };
        set_custom(std::slice::from_ref(&def)).unwrap();
        let p = preset_for("acme-bot").unwrap();
        assert_eq!((p.bin, p.launch, p.resume_latest), (bin, &["chat"][..], &["--continue"][..]));
        let dir = std::env::temp_dir();
        let l = build_launch("acme-bot", &[], Resume::None, Some("do it"), Path::new("kelpied"), &dir).unwrap();
        let j = command_line(&l.argv);
        assert!(j.contains("chat --ask do it"), "{j}");
        let l = build_launch("acme-bot", &[], Resume::Latest, None, Path::new("kelpied"), &dir).unwrap();
        assert!(command_line(&l.argv).contains("chat --continue"));
        assert!(set_custom(&[AgentDef { id: "claude".into(), ..def.clone() }]).is_err(), "built-in ids are taken");
        assert!(set_custom(&[AgentDef { id: "a b".into(), ..def }]).is_err());
        set_custom(&[]).unwrap();
        assert!(preset_for("acme-bot").is_none());
    }
}
