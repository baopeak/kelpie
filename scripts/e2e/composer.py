"""GUI e2e for the composer: @-mentions per agent, inert delivery, pictures, selection menus, history.

    python -X utf8 scripts/e2e/composer.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver, on free ports
(also the native driver's: another run may hold 4445), and stops both on exit.

The agents are stand-ins on PATH (`claude`, `codex`, `omp`, `gemini`; scripts/e2e/fake_tui_agent.py and
fake_acp_agent.py): the TUI ones log every character they are typed, bracketed-paste markers included, so
the log shows exactly what a real program behind the terminal would have read.

1. Ctrl+Shift+I opens and closes the composer, the pane button too; Esc keeps the draft.
2. Enter sends, Shift+Enter adds a line; the Enter that submits comes after the paste.
3. @path agents (Claude, Codex) get `@path` for files, folders, names with spaces and unicode; Codex submits.
4. Other programs get the file fenced and labelled; caps (200 lines) give a path.
5. A file holding Escape [201~ and a command is inserted inert: one paste, one terminator, the command never
   runs (agent); a shell or other program gets paths only, quoted for it and left at the prompt, never
   submitted: names with `$(...)`, `;`, `&`, a backtick or a quote, a script and an .exe prove that nothing
   runs; an agent that has not asked for bracketed paste gets paths, and no multi-line text.
6. @selection, @diff, @note, @terminal (redacted), @issue (nothing sent when it is not found).
7. Send selection to agent from the editor, the diff and the terminal menus.
8. A pasted picture becomes a path for a TUI and an image block for a chat; a drop does the same.
9. Up arrow walks the project's earlier prompts; the palette finds them; another project has none.
"""
import base64
import glob
import importlib.util
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

ENDPOINT = f"kelpie-composer-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-composer-data-")
WORK = os.path.join(scratch, f"composer-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
PROJ = os.path.join(WORK, "proj")
PROJ_B = os.path.join(WORK, "other")
for d_ in (BIN, PROJ, PROJ_B):
    os.makedirs(d_, exist_ok=True)
FAKE_TUI = os.path.join(HERE, "fake_tui_agent.py")
FAKE_ACP = os.path.join(HERE, "fake_acp_agent.py")
ACP_LOG = os.path.join(WORK, "gemini-client.log")


def log_path(name):
    return os.path.join(WORK, name + ".log")


# Stand-in agents: `claude`, `codex`, `omp` log what they are typed; `gemini` speaks ACP (chat view).
for name, extra in (("claude", ""), ("codex", " --codex"), ("omp", ""), ("opencode", " --no-bracketed")):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{FAKE_TUI}" "{log_path(name)}"{extra} %*\r\n')
with open(os.path.join(BIN, "gemini.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE_ACP}" --image --log "{ACP_LOG}" %*\r\n')

os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "CLAUDE_CONFIG_DIR": os.path.join(WORK, "claude-home"), "CODEX_HOME": os.path.join(WORK, "codex-home"),
    "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
NATIVE_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver")
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver on PATH and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

procs = {}
S = "window.__kelpie.store"
TOKEN = "ghp_0123456789abcdefghijABCDEFGHIJ012345"
# A 1x1 PNG.
PNG_B64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="


def until(fn, timeout=30, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


HOSTILE = [
    "x$(New-Item pwned-sub.marker).txt", "a;New-Item pwned-semi.marker;.txt", "a&New-Item pwned-amp.marker&.txt",
    "a`New-Item pwned-tick.marker`.txt", "it's.txt", "deploy.ps1", "setup.exe",
]


def pwsh_word(name):
    """The name as one PowerShell word the way the composer writes it."""
    return name if re.fullmatch(r"[\w./\\:+-]+", name) and not name.startswith("-") else "'" + name.replace("'", "''") + "'"


def make_project():
    write(os.path.join(PROJ, "hello.txt"), "hello world\n")
    write(os.path.join(PROJ, "src", "app.ts"), "export function add(a: number, b: number) {\n  return a + b;\n}\nexport const answer = add(40, 2);\n")
    write(os.path.join(PROJ, "docs", "guide.md"), "# Guide\nhow to use it\n")
    write(os.path.join(PROJ, "my docs", "read me.md"), "# Read me\nfile with a space in its folder and name\n")
    write(os.path.join(PROJ, "Đồ án", "báo cáo.md"), "nội dung báo cáo\n")
    write(os.path.join(PROJ, "big.txt"), "".join(f"line {i}\n" for i in range(1, 301)))
    # Escape [201~ ends a bracketed paste early; what follows would then be typed as keystrokes.
    # Names a shell would take for code, a script and a program: mentioned, they must never run.
    for name in HOSTILE:
        write(os.path.join(PROJ, name), "New-Item pwned-script.marker\n")
    write(os.path.join(PROJ, "inject.txt"), "before\n\x1b[201~New-Item -ItemType File -Path pwned-inject.marker\nafter\n")
    write(os.path.join(PROJ_B, "readme.txt"), "other project\n")
    git0 = ["git", "-c", "user.name=e2e", "-c", "user.email=e2e@example.com"]
    subprocess.run(git0 + ["init", "-q"], cwd=PROJ_B, check=True)
    subprocess.run(git0 + ["add", "-A"], cwd=PROJ_B, check=True)
    subprocess.run(git0 + ["commit", "-q", "-m", "fixture"], cwd=PROJ_B, check=True)
    git = ["git", "-c", "user.name=e2e", "-c", "user.email=e2e@example.com"]
    subprocess.run(git + ["init", "-q"], cwd=PROJ, check=True)
    subprocess.run(git + ["add", "-A"], cwd=PROJ, check=True)
    subprocess.run(git + ["commit", "-q", "-m", "fixture"], cwd=PROJ, check=True)
    # Uncommitted changes for @diff and the diff pane: one staged, one not.
    write(os.path.join(PROJ, "src", "app.ts"), "export function add(a: number, b: number) {\n  return a + b + 0;\n}\nexport const answer = add(40, 2);\n")
    subprocess.run(git + ["add", "src/app.ts"], cwd=PROJ, check=True)
    write(os.path.join(PROJ, "hello.txt"), "hello world edited\n")


# ---------- page helpers ----------


def read_log(name):
    try:
        with open(log_path(name), encoding="utf-8") as f:
            return f.read()
    except OSError:
        return ""


def new_session(d, pid, kind, name, args=None):
    opts = {"name": name}
    if args:
        opts["args"] = args
    info = js_await(d, f"{S}.newSession(arguments[0], arguments[1], arguments[2])", pid, kind, opts)
    if not info:
        raise RuntimeError(f"{name} did not start")
    return info


def reveal(d, sid):
    d.js(f"{S}.revealSession(arguments[0])", sid)
    time.sleep(0.4)


def composer_open(d, sid=None):
    return d.js("const c = document.querySelector('[data-composer]'); return c ? c.dataset.composer : null")


def open_composer(d, sid):
    """Shows the session, opens its composer with Ctrl+Shift+I and waits for it to hold the keyboard."""
    reveal(d, sid)
    if composer_open(d) != sid:
        d.keys("ctrl+shift+i")
    d.wait("return !!document.querySelector('[data-composer-input]') && document.activeElement === document.querySelector('[data-composer-input]')", 20)
    return True


def set_draft(d, text):
    d.js("""const t = document.querySelector('[data-composer-input]'); t.focus(); t.value = arguments[0];
            t.setSelectionRange(t.value.length, t.value.length); t.dispatchEvent(new Event('input', { bubbles: true }));""", text)


def draft(d):
    return d.js("return document.querySelector('[data-composer-input]')?.value ?? null")


def send(d, text):
    set_draft(d, text)
    d.keys("enter")


def toast(d):
    return d.js("return document.querySelector('.toast')?.innerText ?? ''")


def menu_items(d):
    return d.js("return [...document.querySelectorAll('.ctx-menu .ctx-item')].map(i => i.innerText.trim())")


def click_menu(d, label):
    ok = d.js("const i = [...document.querySelectorAll('.ctx-menu .ctx-item')].find(x => x.innerText.trim().startsWith(arguments[0])); if (!i) return false; i.click(); return true;", label)
    if not ok:
        raise RuntimeError(f"no menu item {label!r} in {menu_items(d)}")


def context_menu(d, selector):
    d.js("""const el = document.querySelector(arguments[0]); const r = el.getBoundingClientRect();
            el.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: r.left + 40, clientY: r.top + 40 }));""", selector)
    time.sleep(0.3)


def paste_picture(d):
    d.js("""const bytes = Uint8Array.from(atob(arguments[0]), c => c.charCodeAt(0));
            const dt = new DataTransfer(); dt.items.add(new File([bytes], 'shot.png', { type: 'image/png' }));
            document.querySelector('[data-composer-input]').dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));""", PNG_B64)


def paste_block(log, after=0):
    """The text between the bracketed-paste markers in the log, from position `after`."""
    text = log[after:]
    start = text.find("<1b>[200~")
    end = text.rfind("<1b>[201~")
    return text[start + 9:end] if start >= 0 and end > start else None


def scenario(run):
    d = run.d
    check = run.check
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js(f"{S}.setSettings({{ shortcutHints: false }})")
    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    pid_b = d.js(f"return {S}.addProject(arguments[0])", PROJ_B)
    d.js(f"{S}.switchProject(arguments[0])", pid)

    sessions = {}
    sessions["claude"] = new_session(d, pid, "claude", "fclaude")
    sessions["codex"] = new_session(d, pid, "codex", "fcodex")
    sessions["omp"] = new_session(d, pid, "omp", "fomp")
    sessions["opencode"] = new_session(d, pid, "opencode", "fopencode")
    sessions["shell"] = new_session(d, pid, "shell", "fshell")
    sessions["tui"] = new_session(d, pid, "custom", "ftui", [sys.executable, FAKE_TUI, log_path("tui")])
    sessions["plain"] = new_session(d, pid, "custom", "fplain", [sys.executable, FAKE_TUI, log_path("plain"), "--no-bracketed"])
    for key in ("claude", "codex", "omp", "opencode", "tui", "plain"):
        ok = until(lambda: "fake agent ready" in daemon(d, "session.read", {"handle": sessions[key]["handle"], "lines": 20})["text"], 30)
        check(f"the stand-in {key} is running", bool(ok))
    shell_ready = until(lambda: "PS " in daemon(d, "session.read", {"handle": sessions["shell"]["handle"], "lines": 20})["text"], 40)
    check("the shell shows its prompt", bool(shell_ready))
    sid = {k: v["id"] for k, v in sessions.items()}

    # ---------- 1. open and close ----------
    reveal(d, sid["claude"])
    check("no composer exists until one is opened", composer_open(d) is None)
    chords = d.js("return window.__kelpie.commands.filter(c => (c.win || '').toLowerCase() === 'mod+shift+i' || (c.mac || '').toLowerCase() === 'mod+shift+i').map(c => c.id)")
    check("Ctrl+Shift+I is bound to the composer alone", chords == ["composer.open"], str(chords))
    open_composer(d, sid["claude"])
    check("Ctrl+Shift+I opens the focused session's composer with the keyboard in it", composer_open(d) == sid["claude"])
    run.shot("01-composer-open")
    check("the pane button shows it is open", bool(d.js("return !!document.querySelector('.pane [data-compose].active')")))
    d.keys("ctrl+shift+i")
    check("Ctrl+Shift+I again closes it", until(lambda: composer_open(d) is None, 5) is not None)
    d.js("document.querySelector('.pane [data-compose]').click()")
    check("the pane's Compose button opens it", until(lambda: composer_open(d) == sid["claude"], 10) is not None)
    set_draft(d, "half-written thought")
    d.keys("escape")
    check("Esc closes it", until(lambda: composer_open(d) is None, 5) is not None)
    open_composer(d, sid["claude"])
    check("…and the draft is still there when it opens again", draft(d) == "half-written thought", str(draft(d)))
    set_draft(d, "")
    d.keys("escape")

    # ---------- 2. Enter and Shift+Enter ----------
    open_composer(d, sid["claude"])
    mark = len(read_log("claude"))
    set_draft(d, "first")
    d.keys("shift+enter")
    d.keys("second")
    check("Shift+Enter adds a line and sends nothing", draft(d) == "first\nsecond" and len(read_log("claude")) == mark, repr(draft(d)))
    d.keys("enter")
    got = until(lambda: "<SUBMIT>" in read_log("claude")[mark:], 15)
    check("Enter sends: one bracketed paste with both lines, then the Enter", read_log("claude")[mark:] == "<1b>[200~first<CR>second<1b>[201~<SUBMIT>\n", repr(read_log("claude")[mark:]))
    check("the composer closes after sending", until(lambda: composer_open(d) is None, 5) is not None)

    # ---------- 3. agents that read @path ----------
    open_composer(d, sid["claude"])
    mark = len(read_log("claude"))
    want = 'read @src/app.ts, @docs/ and @"my docs/read me.md" and @"Đồ án/báo cáo.md" please'
    send(d, want)
    until(lambda: "<SUBMIT>" in read_log("claude")[mark:], 15)
    block = paste_block(read_log("claude"), mark)
    check("Claude gets @path for a file, a folder and names with spaces and unicode", block == want, repr(block))
    check("…and none of the content", "export function" not in read_log("claude")[mark:] and "nội dung" not in read_log("claude")[mark:])

    # The stand-in judges the pause by when the bytes reach it: on a starved machine the paste and the
    # Enter can arrive together, so a miss is tried again (up to three times) before it counts.
    for attempt in range(3):
        open_composer(d, sid["codex"])
        mark = len(read_log("codex"))
        send(d, "check @src/app.ts")
        until(lambda: "<SUBMIT>" in read_log("codex")[mark:] or "<NEWLINE>" in read_log("codex")[mark:], 15)
        if "<SUBMIT>" in read_log("codex")[mark:]:
            break
        daemon(d, "session.input", {"handle": sessions["codex"]["handle"], "data": "\x15"})
    check("Codex gets @path and the Enter submits instead of inserting a newline", read_log("codex")[mark:] == "<1b>[200~check @src/app.ts<1b>[201~<SUBMIT>\n", f"attempt {attempt + 1}: {read_log('codex')[mark:]!r}")
    mark = len(read_log("codex"))
    daemon(d, "session.input", {"handle": sessions["codex"]["handle"], "data": "x\r"})
    until(lambda: read_log("codex")[mark:].endswith("<NEWLINE>") or read_log("codex")[mark:].endswith("<SUBMIT>\n"), 10)
    check("(control) an Enter right behind the text is a newline for the same program", read_log("codex")[mark:] == "x<NEWLINE>", repr(read_log("codex")[mark:]))

    # ---------- 4. other programs get the content ----------
    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    send(d, 'explain @src/app.ts and @"my docs/read me.md" and @"Đồ án/báo cáo.md"')
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    block = paste_block(read_log("omp"), mark) or ""
    check("OMP gets the files fenced and labelled with their paths", all(x in block for x in ("File `src/app.ts`:", "```ts", "export function add", "File `my docs/read me.md`:", "file with a space in its folder and name", "File `Đồ án/báo cáo.md`:", "nội dung báo cáo")), repr(block[:400]))
    check("…and the Enter submits it", read_log("omp").endswith("<SUBMIT>\n"))

    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    send(d, "look at @big.txt")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    block = paste_block(read_log("omp"), mark) or ""
    check("a file over 200 lines is a path, not content", block == "look at big.txt", repr(block[:200]))

    # ---------- 5. inert delivery ----------
    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    send(d, "show @inject.txt")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    seen = read_log("omp")[mark:]
    block = paste_block(read_log("omp"), mark) or ""
    check("a file with Escape [201~ and a command is one paste: no escape inside, one terminator", seen.count("<1b>") == 2 and seen.count("<1b>[201~") == 1 and "<1b>" not in block, repr(seen[:300]))
    check("…the command is only text in the paste, and the Enter comes after the terminator", "New-Item -ItemType File -Path pwned-inject.marker" in block and seen.index("<1b>[201~") < seen.index("<SUBMIT>"), repr(block[:200]))

    # A program that is not an agent never gets content, only the path, and the Enter is left to the user.
    open_composer(d, sid["tui"])
    mark = len(read_log("tui"))
    send(d, "show @inject.txt")
    time.sleep(2)
    seen = read_log("tui")[mark:]
    check("a program that asked for bracketed paste gets the path in one paste, no content", seen == "<1b>[200~show inject.txt<1b>[201~", repr(seen[:300]))
    check("…and it is left at the prompt, not submitted", "<SUBMIT>" not in seen)

    open_composer(d, sid["plain"])
    mark = len(read_log("plain"))
    send(d, "show @inject.txt")
    time.sleep(2)
    seen = read_log("plain")[mark:]
    check("a program without bracketed paste gets the path as keystrokes, not submitted", seen == "show inject.txt", repr(seen))

    # An agent that has not asked for bracketed paste: paths instead of content, and no multi-line text.
    open_composer(d, sid["opencode"])
    mark = len(read_log("opencode"))
    send(d, "review @diff")
    until(lambda: "<SUBMIT>" in read_log("opencode")[mark:], 15)
    seen = read_log("opencode")[mark:]
    check("an agent not ready for a paste gets a path for the diff, never its lines", re.fullmatch(r"review .*kelpie-composer.*diff-.*\.txt<SUBMIT>\n", seen) is not None and "diff --git" not in seen, repr(seen[:300]))
    open_composer(d, sid["opencode"])
    mark = len(read_log("opencode"))
    set_draft(d, "first")
    d.keys("shift+enter")
    d.keys("second")
    d.keys("enter")
    said = until(lambda: "multi-line" in toast(d), 10)
    check("…and a multi-line text is refused with a reason", bool(said) and len(read_log("opencode")) == mark, toast(d))
    check("…the composer stays open with the text", composer_open(d) == sid["opencode"] and draft(d) == "first\nsecond")
    set_draft(d, "")
    d.keys("escape")

    # Every shell: paths quoted as one word, left at the prompt.
    sh = sessions["shell"]["handle"]

    def prompt_line():
        """The last prompt with what is typed after it, rows the terminal wrapped joined again."""
        text = daemon(d, "session.read", {"handle": sh, "lines": 60})["text"]
        return text[text.rfind("PS "):].replace("\r", "").replace("\n", "").rstrip() if "PS " in text else ""

    def clear_line():
        daemon(d, "session.input", {"handle": sh, "data": "\x03"})
        time.sleep(0.6)

    open_composer(d, sid["shell"])
    send(d, "@inject.txt")
    time.sleep(3)
    check("PowerShell: the command in the file did not run", not os.path.exists(os.path.join(PROJ, "pwned-inject.marker")))
    line = prompt_line()
    check("…the path is at the prompt and was not submitted", line.startswith("PS ") and line.endswith("> inject.txt"), line[-120:])
    clear_line()
    bad = []
    for name in HOSTILE:
        open_composer(d, sid["shell"])
        send(d, f'echo @"{name}"')
        until(lambda: composer_open(d) is None, 10)
        time.sleep(1.2)
        line = prompt_line()
        if not (line.startswith("PS ") and line.endswith("> echo " + pwsh_word(name))):
            bad.append((name, line[-100:]))
        clear_line()
    check("hostile file names ($(...), ;, &, backtick, quote, .ps1, .exe) are single-quoted words at the prompt, nothing submitted", not bad, str(bad))
    ran = glob.glob(os.path.join(PROJ, "pwned-*.marker")) + glob.glob(os.path.join(PROJ, "*", "pwned-*.marker"))
    check("…and nothing ran: no marker file anywhere", not ran, str(ran))
    open_composer(d, sid["shell"])
    send(d, "echo composer-" + "ok-123")
    ran = until(lambda: daemon(d, "session.read", {"handle": sessions["shell"]["handle"], "lines": 60})["text"].count("composer-ok-123") >= 2, 15)
    check("a plain command typed in the composer runs in the shell", bool(ran))

    # ---------- 6. selection, diff, note, terminal, issue ----------
    d.js(f"{S}.addNote(arguments[0], {{ path: 'src/app.ts', line: 2, side: 'new', text: 'return a + b + 0;', note: 'drop the + 0' }})", pid)
    daemon(d, "session.input", {"handle": sessions["shell"]["handle"], "data": f"echo {TOKEN}\r"})
    until(lambda: TOKEN in daemon(d, "session.read", {"handle": sessions["shell"]["handle"], "lines": 40})["text"], 15)
    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    send(d, "summarise @diff and @note and @terminal:fshell")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 20)
    block = paste_block(read_log("omp"), mark) or ""
    check("@diff is inlined and labelled", "Uncommitted changes (git diff):" in block and "return a + b + 0;" in block, repr(block[:300]))
    check("@note lists the review note with its file and line", "src/app.ts:2" in block and "drop the + 0" in block)
    check("@terminal is inlined and labelled with the session", "Recent output of terminal `fshell`:" in block)
    check("…and its credential is masked before it is sent", TOKEN not in read_log("omp") and "[redacted]" in block, "token leaked" if TOKEN in read_log("omp") else "")
    check("@diff holds the staged and the unstaged change", "return a + b + 0;" in block and "+hello world edited" in block, repr(block[:300]))

    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    set_draft(d, "ask @terminal:fc")
    time.sleep(0.4)
    d.keys("escape")  # puts the completions away; Enter would pick one
    d.keys("enter")
    said = until(lambda: "no session named" in toast(d), 15)
    check("a session name that fits several sessions names none of them", bool(said) and len(read_log("omp")) == mark, toast(d))
    set_draft(d, "")
    d.keys("escape")

    open_composer(d, sid["omp"])
    mark = len(read_log("omp"))
    send(d, "fix @issue:ABC-1")
    said = until(lambda: "ABC-1" in toast(d), 15)
    check("@issue that no tracker has sends nothing and says so", len(read_log("omp")) == mark and bool(said), toast(d))
    check("…and the composer stays open for a fix", composer_open(d) == sid["omp"])
    check("…and says that Enter again sends it as written", "Enter again" in toast(d), toast(d))
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    check("…which then goes out as typed", paste_block(read_log("omp"), mark) == "fix @issue:ABC-1", repr(read_log("omp")[mark:]))

    # ---------- completions ----------
    open_composer(d, sid["claude"])
    set_draft(d, "see @doc")
    names = until(lambda: d.js("return [...document.querySelectorAll('.composer-menu .composer-item .name')].map(n => n.innerText)") or None, 10) or []
    check("@ completes folders as well as files", "docs/" in names, str(names))
    d.keys("tab")
    check("Tab takes the first one", draft(d).startswith("see @") and draft(d).endswith("/ ") or draft(d).startswith("see @docs"), repr(draft(d)))
    set_draft(d, "open @read")
    names = until(lambda: d.js("return [...document.querySelectorAll('.composer-menu .composer-item .name')].map(n => n.innerText)") or None, 10) or []
    check("…and finds names with spaces", "my docs/read me.md" in names, str(names))
    d.keys("tab")
    check("…inserting them quoted", draft(d) == 'open @"my docs/read me.md" ', repr(draft(d)))
    set_draft(d, "ask @term")
    names = until(lambda: d.js("return [...document.querySelectorAll('.composer-menu .composer-item .name')].map(n => n.innerText)") or None, 10) or []
    check("@terminal: is offered", "@terminal:" in names, str(names))
    set_draft(d, "ask @terminal:fsh")
    names = until(lambda: d.js("return [...document.querySelectorAll('.composer-menu .composer-item .name')].map(n => n.innerText)") or None, 10) or []
    check("…with the project's other sessions", names == ["fshell"], str(names))
    run.shot("02-completions")
    set_draft(d, "")
    d.keys("escape")

    # ---------- 7. Send selection to agent: editor, diff, terminal ----------
    d.js(f"{S}.openFile(arguments[0], 'src/app.ts')", pid)
    d.wait("return !!document.querySelector('.cm-content')", 20)
    time.sleep(1)
    d.click(".cm-content")
    d.keys("ctrl+a")
    context_menu(d, ".cm-host")
    items = menu_items(d)
    check("the editor's menu offers its selection to an agent", any(i.startswith("Send Selection to Agent") for i in items), str(items))
    mark = len(read_log("omp"))
    click_menu(d, "Send Selection to Agent")
    check("…from a list of the project's running agents", any(i.startswith("Send to ") for i in menu_items(d)), str(menu_items(d)))
    click_menu(d, "Send to fomp")
    ok = until(lambda: composer_open(d) is not None and draft(d) == "@selection ", 20)
    check("choosing one opens its composer with @selection ready", bool(ok), repr(draft(d)))
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    block = paste_block(read_log("omp"), mark) or ""
    check("the editor's selection arrives labelled with file and lines", "Selection from `src/app.ts:1-4`:" in block and "export const answer" in block, repr(block[:300]))

    d.js(f"{S}.openFile(arguments[0], 'src/app.ts', {{ diff: true }})", pid)
    d.wait("return document.querySelectorAll('.viewer .cm-merge-b .cm-line').length > 1", 20)
    time.sleep(1)
    # A diff cannot be edited: its text is selected with the mouse.
    r = d.js("""const l = [...document.querySelectorAll('.viewer .cm-merge-b .cm-line')]; const a = l[0].getBoundingClientRect(), b = l[l.length - 1].getBoundingClientRect();
                return {x0: a.left + 3, y0: a.top + 5, x1: b.right - 3, y1: b.bottom - 3}""")
    d.drag(r["x0"], r["y0"], r["x1"], r["y1"])
    time.sleep(0.5)
    context_menu(d, ".viewer .cm-host")
    items = menu_items(d)
    check("the diff's menu offers the selection too", any(i.startswith("Send Selection to Agent") for i in items), str(items))
    mark = len(read_log("omp"))
    click_menu(d, "Send Selection to Agent")
    click_menu(d, "Send to fomp")
    until(lambda: composer_open(d) is not None and draft(d) == "@selection ", 20)
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    block = paste_block(read_log("omp"), mark) or ""
    check("…and the diff's selection arrives labelled with the file", "Selection from `src/app.ts`:" in block and "return a + b" in block, repr(block[:300]))

    reveal(d, sid["shell"])
    time.sleep(0.5)
    d.js("window.__kelpie.peekView(arguments[0]).term.selectAll()", sid["shell"])
    context_menu(d, f"[data-session='{sid['shell']}']")
    items = menu_items(d)
    check("the terminal's menu offers its selection", any(i.startswith("Send Selection to Agent") for i in items), str(items))
    mark = len(read_log("omp"))
    click_menu(d, "Send Selection to Agent")
    click_menu(d, "Send to fomp")
    until(lambda: composer_open(d) is not None and draft(d) == "@selection ", 20)
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("omp")[mark:], 15)
    block = paste_block(read_log("omp"), mark) or ""
    check("…and the terminal's selection arrives labelled as a selection", block.startswith("the selection below") and "Selection:" in block and "composer-ok-123" in block, repr(block[:200]))
    check("…with its credentials masked", TOKEN not in read_log("omp")[mark:] and "[redacted]" in block, "token leaked" if TOKEN in read_log("omp")[mark:] else "")

    # ---------- 8. pictures ----------
    open_composer(d, sid["claude"])
    mark = len(read_log("claude"))
    paste_picture(d)
    chip = until(lambda: d.js("const c = document.querySelector('.composer-image'); return c ? c.title : null"), 15)
    check("a pasted picture appears as an attachment", bool(chip), str(chip))
    set_draft(d, "what is in this?")
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("claude")[mark:], 15)
    block = paste_block(read_log("claude"), mark) or ""
    check("a TUI gets the picture as a path", chip and block == f"what is in this? @{chip}", repr(block))
    check("…of a real temporary file holding the picture", bool(chip) and os.path.exists(chip) and open(chip, "rb").read() == base64.b64decode(PNG_B64), str(chip))

    chat = js_await(d, "window.__kelpie.chat.newChat(arguments[0], 'gemini')", pid)
    check("an ACP chat session starts", bool(chat))
    if chat:
        sid["chat"] = chat["id"]
        ready = until(lambda: d.js("const c = window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return c.state.ready && !!c.state.conversationId", chat["id"]), 40)
        check("…and is ready", bool(ready))
        open_composer(d, chat["id"])
        d.js("""const dt = new DataTransfer(); dt.items.add(new File([new Uint8Array(6 * 1024 * 1024)], 'big.png', { type: 'image/png' }));
                document.querySelector('[data-composer-input]').dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));""")
        said = until(lambda: "5 MB" in toast(d), 10)
        check("a picture over 5 MB is refused for a chat, with the limit named", bool(said) and not d.js("return !!document.querySelector('.composer-image')"), toast(d))
        paste_picture(d)
        until(lambda: d.js("return !!document.querySelector('.composer-image')"), 15)
        set_draft(d, "describe the picture")
        d.keys("enter")
        until(lambda: any('"session/prompt"' in x for x in open(ACP_LOG, encoding="utf-8").read().splitlines()) if os.path.exists(ACP_LOG) else False, 20)
        prompts = [json.loads(x) for x in open(ACP_LOG, encoding="utf-8").read().splitlines() if '"session/prompt"' in x]
        blocks = prompts[-1]["params"]["prompt"] if prompts else []
        check("a chat gets the picture as an image block", any(b.get("type") == "image" and b.get("mimeType") == "image/png" and b.get("data") == PNG_B64 for b in blocks), json.dumps(blocks)[:200])
        check("…next to the text", any(b.get("type") == "text" and b.get("text") == "describe the picture" for b in blocks))
        user = until(lambda: d.js("const c = window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return c.state.items.filter(i => i.kind === 'user').map(i => i.text)", chat["id"]), 10)
        check("…and the transcript names the picture", bool(user) and "describe the picture" in user[-1] and ".png]" in user[-1], str(user))
        run.shot("03-chat-picture")

    # A picture dropped from the file manager: the app receives the path from the window.
    drop_png = os.path.join(WORK, "dropped.png")
    with open(drop_png, "wb") as f:
        f.write(base64.b64decode(PNG_B64))
    open_composer(d, sid["claude"])
    mark = len(read_log("claude"))
    pos = d.js("const r = document.querySelector('[data-composer]').getBoundingClientRect(); return {x: (r.left + 40) * devicePixelRatio, y: (r.top + 40) * devicePixelRatio}")
    d.js_async("const done = arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke('plugin:event|emit', {event: 'tauri://drag-drop', payload: {paths: [arguments[0], arguments[1]], position: arguments[2]}}).then(() => done(true), e => done(String(e)));", drop_png, os.path.join(PROJ, "hello.txt"), pos)
    got = until(lambda: d.js("return document.querySelector('.composer-image')?.title ?? null"), 10)
    check("a dropped picture is attached by its own path", got == drop_png, str(got))
    check("…and a dropped file becomes a mention", "@hello.txt" in (draft(d) or ""), repr(draft(d)))
    d.keys("enter")
    until(lambda: "<SUBMIT>" in read_log("claude")[mark:], 15)
    block = paste_block(read_log("claude"), mark) or ""
    check("…and both reach the program", f"@hello.txt" in block and drop_png in block, repr(block))

    # ---------- 9. history ----------
    open_composer(d, sid["claude"])
    check("a fresh composer is empty", draft(d) == "")
    d.keys("up")
    last = draft(d)
    check("Up brings back the last prompt as typed (mentions not expanded)", last.startswith("what is in this?") or "@hello.txt" in last, repr(last))
    d.keys("up")
    older = draft(d)
    check("…Up again an older one", older != last and older != "", repr(older))
    d.keys("down")
    check("…Down comes back", draft(d) == last, repr(draft(d)))
    d.keys("down")
    check("…to the empty draft", draft(d) == "", repr(draft(d)))
    d.keys("escape")
    other = new_session(d, pid_b, "claude", "bclaude")
    until(lambda: "fake agent ready" in daemon(d, "session.read", {"handle": other["handle"], "lines": 20})["text"], 30)
    open_composer(d, other["id"])
    d.keys("up")
    check("another project has its own (empty) history", draft(d) == "", repr(draft(d)))
    mark = len(read_log("claude"))
    send(d, "what changed? @diff")
    said = until(lambda: "no uncommitted changes" in toast(d), 15)
    check("@diff on a clean repository says there is nothing, and sends nothing", bool(said) and "<SUBMIT>" not in read_log("claude")[mark:], toast(d))
    set_draft(d, "")
    d.keys("escape")
    d.js(f"{S}.switchProject(arguments[0])", pid)
    time.sleep(0.5)
    d.js(f"{S}.setOverlay('palette')")
    d.wait("return !!document.querySelector('.palette-input input')", 10)
    d.js("const i = document.querySelector('.palette-input input'); i.value = '!big'; i.dispatchEvent(new Event('input', { bubbles: true }));")
    hit = until(lambda: d.js("return [...document.querySelectorAll('.palette .item .label')].map(x => x.innerText)"), 10) or []
    check("the palette searches the prompt history", any("look at @big.txt" in h for h in hit), str(hit))
    run.shot("04-palette-prompts")
    reveal(d, sid["claude"])
    d.js(f"{S}.setOverlay('palette')")
    d.wait("return !!document.querySelector('.palette-input input')", 10)
    d.js("const i = document.querySelector('.palette-input input'); i.value = '!big'; i.dispatchEvent(new Event('input', { bubbles: true }));")
    time.sleep(0.5)
    d.keys("enter")
    ok = until(lambda: composer_open(d) is not None and "big.txt" in (draft(d) or ""), 15)
    check("choosing one reopens it in the focused session's composer", bool(ok), repr(draft(d)))
    d.keys("escape")  # the first Esc puts the completions away, the next closes
    d.keys("escape")
    check("a closed composer leaves nothing in the page", until(lambda: composer_open(d) is None, 5) is not None)


def main():
    make_project()
    spawn("daemon", [kelpied, "serve"])
    c = None
    for _ in range(150):
        try:
            c = kc.Conn()
            break
        except OSError:
            time.sleep(0.1)
    if c is None:
        raise SystemExit("the daemon did not start")
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", NATIVE])
    if not until(port_open, 30):
        raise SystemExit("tauri-driver did not open its port")

    run = None
    try:
        run = Run(shots, app=app)
        scenario(run)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        teardown(run)
    run.finish()


def teardown(run):
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then
    the test daemon with its sessions, then the driver tree. Removes this run's token and folders."""
    if run is not None:
        try:
            run.d.quit()
        except Exception:
            pass
    if "driver" in procs:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(procs["driver"].pid), "/T", "/F"], capture_output=True)
        else:
            procs["driver"].kill()
    try:
        c = kc.Conn()
        c.call("daemon.shutdown", {"kill_sessions": True})
    except Exception:
        pass
    for p in procs.values():
        try:
            p.wait(timeout=5)
        except Exception:
            p.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
