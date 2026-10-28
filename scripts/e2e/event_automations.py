"""GUI e2e for event-triggered automations (daemon automations.rs + daemon/automation.rs, the window's watcher).

    python -X utf8 scripts/e2e/event_automations.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie and its automations file are never reached.
Agents are stand-ins (an `amp` shim on PATH running fake_agent below); every repository is a fixture under
the scratch folder; there is no network. KELPIE_AUTOMATION_GAP_MS shortens the daemon's one-minute gap between
two runs of an automation to 3 s so the script does not wait minutes (the minute itself is held by unit tests).

 1. A file trigger on `*.md` (from the real watcher): an unapproved command does not run and says so; approved in
    the settings page it runs once per change, however often the file is saved, and again after the gap; a file
    name made of `$(...)`, backticks, `;`, quotes, `&` and `%PATH%` reaches the program literally (argv and
    environment) and nothing is executed; a dry run records what it would do and runs nothing.
 2. An agent started by an automation edits files that match its own trigger: it is not started again while it
    works or just after, and a later save starts it again; a quick-command action types into the session it started
    (never one the user opened, never right after something was typed).
 3. Agent-turn trigger; commit trigger: fires on HEAD moving (made by hand or by an agent), not for the turn
    checkpoints' refs under refs/kelpie, not for a checkout.
 4. Daily cap, the global switch in the status bar, finished / pull-request events from the window.
 5. Who may do what: a script with the user's token cannot save a command, approve it or report events (the app
    can); a session's programs and a claimed session cannot touch automations at all.
 6. The settings editor: triggers, honest scope text, a command with its hash and approval.
 7. Storage: a damaged file is set aside and never overwritten; the old format migrates with a .bak and keeps
    its schedules.
"""
import importlib.util
import json
import os
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

ENDPOINT = f"kelpie-evauto-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-evauto-data-")
WORK = os.path.join(scratch, f"evauto-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
REPO = os.path.join(WORK, "repo")
RECORDS = os.path.join(WORK, "records.jsonl")
LAUNCHES = os.path.join(WORK, "launches.jsonl")
for d_ in (BIN, REPO):
    os.makedirs(d_, exist_ok=True)

# Stand-in agent. Each typed line starts with a JSON turn {"work": s, "files": {path: text}, "run": [[argv], ...]} (what
# follows it, such as the note an automation appends, is ignored), or
# `EDIT:{"span": s, "files": {...}}`: edit the files in two bursts spread over `span` seconds of "work" (what a
# launched automation session does with its prompt). It is a kind that takes its first prompt as typed text
# (a batch-file launcher would mangle the prompt's newlines).
FAKE = os.path.join(WORK, "fake_agent.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(
        "import json, msvcrt, os, subprocess, sys, time\n"
        "LOG = os.environ['EVENT_AUTO_LAUNCHES']\n"
        "def log(**kw):\n"
        "    with open(LOG, 'a', encoding='utf-8') as f:\n"
        "        f.write(json.dumps(dict(kw, cwd=os.getcwd())) + '\\n')\n"
        "def write_files(files, n=0):\n"
        "    for p, text in files.items():\n"
        "        os.makedirs(os.path.dirname(p) or '.', exist_ok=True)\n"
        "        open(p, 'w', encoding='utf-8', newline='').write(text + str(n))\n"
        "def busy(seconds, word):\n"
        "    end = time.time() + seconds\n"
        "    while time.time() < end:\n"
        "        print(word + ' ' + str(time.time()), flush=True)\n"
        "        time.sleep(0.2)\n"
        "print('fake agent ready', flush=True)\n"
        "log(started=True)\n"
        "buf = ''\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch == '\\x1b':\n"
        "        seq = ''\n"
        "        while True:\n"
        "            c = msvcrt.getwch()\n"
        "            seq += c\n"
        "            if c.isalpha() or c == '~':\n"
        "                break\n"
        "        continue\n"
        "    if ch not in '\\r\\n':\n"
        "        buf += ch\n"
        "        continue\n"
        "    line, buf = buf.strip(), ''\n"
        "    if line.startswith('EDIT:'):\n"
        "        spec = json.JSONDecoder().raw_decode(line[5:])[0]\n"
        "        log(edit=spec)\n"
        "        span = spec.get('span', 4)\n"
        "        busy(1, 'thinking')\n"
        "        write_files(spec['files'], 1)\n"
        "        busy(max(span - 2, 0), 'working')\n"
        "        write_files(spec['files'], 2)\n"
        "        busy(1, 'finishing')\n"
        "        print('done', flush=True)\n"
        "        continue\n"
        "    try:\n"
        "        turn = json.JSONDecoder().raw_decode(line)[0]\n"
        "    except ValueError:\n"
        "        continue\n"
        "    if not isinstance(turn, dict):\n"
        "        continue\n"
        "    log(turn=turn)\n"
        "    busy(turn.get('delay', 1.0), 'thinking')\n"
        "    write_files(turn.get('files', {}))\n"
        "    for argv in turn.get('run', []):\n"
        "        subprocess.run(argv, cwd=os.getcwd(), capture_output=True)\n"
        "    busy(turn.get('work', 1), 'working')\n"
        "    print('done', flush=True)\n"
    )
with open(os.path.join(BIN, "amp.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" %*\r\n')

# The program of command actions: records what it was given.
RECORDER = os.path.join(WORK, "recorder.py")
with open(RECORDER, "w", encoding="utf-8") as f:
    f.write(
        "import json, os, sys\n"
        "keys = ['KELPIE_EVENT', 'KELPIE_AUTOMATION', 'KELPIE_ROOT', 'KELPIE_FILE', 'KELPIE_FILES', 'KELPIE_COMMIT', 'KELPIE_BRANCH', 'KELPIE_PR_NUMBER']\n"
        "rec = {'argv': sys.argv[1:], 'cwd': os.getcwd(), 'session_vars': [k for k in os.environ if k.startswith('KELPIE_SESSION')]}\n"
        "rec.update({k: os.environ.get(k) for k in keys})\n"
        "with open(os.environ['EVENT_AUTO_RECORDS'], 'a', encoding='utf-8') as f:\n"
        "    f.write(json.dumps(rec) + '\\n')\n"
    )

# Programs a Windows user would write: a batch file next to the project, one found by name on PATH (PATHEXT), and a
# PowerShell script. Each appends what it was given, quoted, to a file of its own (they run at the same moment).
HOOKS = os.path.join(WORK, "hooks.txt")
with open(os.path.join(BIN, "evhook.cmd"), "w", encoding="utf-8") as f:
    f.write('@echo off\r\n>>"%EVENT_AUTO_HOOKS%.bare" echo bare:"%~1"\r\n')
with open(os.path.join(REPO, "hook.cmd"), "w", encoding="utf-8") as f:
    f.write('@echo off\r\n>>"%EVENT_AUTO_HOOKS%.relative" echo relative:"%~1"\r\n')
# Scripts an interpreter may be given event data for live in the project (the daemon refuses others), and the
# approval covers their contents.
shutil.copy(RECORDER, os.path.join(REPO, "recorder.py"))
HOOK_PS1 = os.path.join(REPO, "hook.ps1")
with open(HOOK_PS1, "w", encoding="utf-8") as f:
    f.write("Add-Content -LiteralPath ($env:EVENT_AUTO_HOOKS + '.ps1') -Value ('ps1:\"' + $args[0] + '\"')\r\n")

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "KELPIE_AUTOMATION_GAP_MS": "3000",
                   "EVENT_AUTO_HOOKS": HOOKS, "EVENT_AUTO_RECORDS": RECORDS, "EVENT_AUTO_LAUNCHES": LAUNCHES, "PATH": BIN + os.pathsep + os.environ.get("PATH", "")})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
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
assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT

procs = {}
S = "window.__kelpie.store"
GAP = 3.0  # seconds, KELPIE_AUTOMATION_GAP_MS


def git(*args, cwd=REPO):
    r = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *args], cwd=cwd, capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0", "LC_ALL": "C"})
    if r.returncode:
        raise RuntimeError(f"git {args}: {r.stderr}")
    return r.stdout.strip()


def until(fn, timeout=30, every=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return None


def write(path, text):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def lines(path):
    try:
        with open(path, encoding="utf-8") as f:
            return [json.loads(x) for x in f if x.strip()]
    except OSError:
        return []


def records(name=None):
    return [r for r in lines(RECORDS) if name is None or r["KELPIE_AUTOMATION"] == name]


def edits():
    return [x for x in lines(LAUNCHES) if "edit" in x]


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def start_agent(d, pid, name):
    info = js_await(d, f"{S}.newSession(arguments[0], 'amp', arguments[1])", pid, {"name": name})
    if not info:
        raise RuntimeError(f"{name} did not start")
    if not until(lambda: "fake agent ready" in daemon(d, "session.read", {"handle": info["handle"]})["text"], 90):
        raise RuntimeError(f"{name}: the stand-in agent never showed its banner")
    return info


def dones(d, sess):
    return daemon(d, "session.read", {"handle": sess["handle"]})["text"].count("done")


def turn(d, sess, files=None, run=None, work=1):
    """One agent turn; returns once the stand-in reports it done."""
    n = dones(d, sess)
    daemon(d, "session.input", {"handle": sess["handle"], "data": json.dumps({"work": work, "files": files or {}, "run": run or []}) + "\r"})
    if not until(lambda: dones(d, sess) > n, 60):
        raise RuntimeError("the stand-in never finished its turn")


def scenario(run):
    d = run.d
    check = run.check
    run.check = lambda name, ok, detail="": check(name, ok, " ".join(str(detail)[:200].split()))  # a failing check's detail, not a page of it
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 15, 30)
    run.check("the app talks to a daemon with event automations (API 15)", d.js(f"return {S}.daemonApi()") >= 15, str(d.js(f"return {S}.daemonApi()")))
    d.js("window.confirm = () => true")
    # Turn snapshots are off by default; the commit scenario needs their refs to exist.
    d.js(f"{S}.setSettings({{ checkpoints: true }})")
    until(lambda: daemon(d, "checkpoint.status")["enabled"], 10)

    # ---------- the fixture repository ----------
    git("init", "-q", "-b", "main")
    write(os.path.join(REPO, "README.md"), "# fixture\n")
    write(os.path.join(REPO, "spec", "api.md"), "api\n")
    git("add", "-A")
    git("commit", "-qm", "init")
    pid = d.js(f"return {S}.addProject(arguments[0])", REPO)
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.branch", pid) == "main", 30)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    root = d.js(f"return {S}.state.projects.find(p => p.id === arguments[0]).path", pid)
    time.sleep(2)  # the watcher is up

    def save(name, **kw):
        base = {"id": name, "name": name, "enabled": True, "cwd": root, "project_id": pid, "kind": "", "prompt": "", "every": "daily"}
        return daemon(d, "automation.save", {**base, **kw})

    def command(*argv):
        return {"action": "shell", "shell": {"program": sys.executable, "argv": ["recorder.py", *argv]}}

    def approve(name):
        a = next(x for x in daemon(d, "automation.list") if x["id"] == name)
        return daemon(d, "automation.approve", {"id": name, "hash": a["command_hash"]})

    def auto(name):
        return next((x for x in daemon(d, "automation.list") if x["id"] == name), None)

    def drop(*names):
        for n in names:
            daemon(d, "automation.delete", {"id": n})

    # ---------- 1. file trigger, approval, literal event data, once, dry run ----------
    save("md", trigger={"event": "file", "glob": "*.md", "daily_cap": 50}, **command("--", "{file}", "{files}", "literal $(x) `y`"))
    saved = auto("md")
    run.check("a saved command is not approved, and the daemon shows its hash", saved["shell"]["approved_hash"] is None and len(saved["command_hash"]) == 64)
    write(os.path.join(REPO, "notes.md"), "one\n")
    time.sleep(3)
    run.check("an unapproved command does not run", not records("md"), str(records("md")))
    run.check("…and the automation says why", "not been approved" in (auto("md")["last_error"] or ""), str(auto("md")["last_error"]))
    d.js(f"{S}.openSettings('automations')")
    d.wait("return !!document.querySelector('.automation-item')", 15)
    run.shot("01-unapproved-command")
    clicked = d.js("const b = [...document.querySelectorAll('.automation-item button')].find(b => b.textContent.trim() === 'Approve command'); if (b) b.click(); return !!b")
    run.check("the settings page offers to approve the command", clicked)
    run.check("approving it there records the hash that was shown", bool(until(lambda: auto("md")["shell"]["approved_hash"] == auto("md")["command_hash"], 10)))

    hostile = "n $(copy NUL INJECTED.txt) `copy NUL INJECTED2.txt`; it's & %PATH% ^x.md"
    time.sleep(GAP + 0.5)
    write(os.path.join(REPO, hostile), "a\n")
    write(os.path.join(REPO, hostile), "b\n")
    got = until(lambda: records("md"), 25)
    run.check("saving a .md file runs the command", bool(got), str(records("md")))
    r = records("md")[0] if records("md") else {}
    run.check("the file name arrives literally as an argument", r.get("argv", [None, None])[1:2] == [hostile], str(r.get("argv")))
    run.check("…as the environment's file list", r.get("KELPIE_FILE") == hostile and hostile in (r.get("KELPIE_FILES") or ""), str(r.get("KELPIE_FILE")))
    run.check("…literal text in the arguments stays literal", r.get("argv", [])[-1:] == ["literal $(x) `y`"] and r.get("argv", [""])[0] == "--", str(r.get("argv")))
    run.check("…and the event kind and folder are told", r.get("KELPIE_EVENT") == "file" and r.get("cwd", "").lower() == root.lower(), f"{r.get('KELPIE_EVENT')} {r.get('cwd')}")
    run.check("the program is not given a session's identity", r.get("session_vars") == [], str(r.get("session_vars")))
    time.sleep(1.5)
    injected = [p for base in (REPO, WORK, os.getcwd()) for p in os.listdir(base) if p.startswith("INJECTED")]
    run.check("nothing in the file name was executed", not injected, str(injected))
    run.check("two saves of the file within the gap ran it once", len(records("md")) == 1, str(len(records("md"))))
    write(os.path.join(REPO, "notes.md"), "two\n")
    time.sleep(1)
    run.check("…and a change inside the gap is dropped", len(records("md")) == 1)
    time.sleep(GAP)
    write(os.path.join(REPO, "notes.md"), "three\n")
    run.check("after the gap the next save runs it again", bool(until(lambda: len(records("md")) == 2, 20)), str(len(records("md"))))
    write(os.path.join(REPO, "other.txt"), "x\n")
    time.sleep(GAP + 1)
    run.check("a file that does not match the pattern runs nothing", len(records("md")) == 2)
    # Dry run: matched and noted, nothing runs.
    save("md", trigger={"event": "file", "glob": "*.md", "daily_cap": 50}, dry_run=True, **command("--", "{file}"))
    time.sleep(GAP + 0.5)
    write(os.path.join(REPO, "notes.md"), "four\n")
    noted = until(lambda: (auto("md")["last_note"] or "").startswith("dry run:"), 20)
    run.check("a dry-run automation says what it would run", bool(noted), str(auto("md")["last_note"]))
    time.sleep(1)
    run.check("…and runs nothing", len(records("md")) == 2, str(len(records("md"))))
    shown = daemon(d, "automation.run", {"id": "md", "dry_run": True})
    run.check("the Dry run button shows the literal command line", shown["detail"]["args"][:3] == ["recorder.py", "--", "example.md"], str(shown["detail"]["args"]))
    drop("md")
    run.shot("02-after-file-trigger")

    # ---------- 1b. Windows programs: relative, by name, PowerShell; and what is refused ----------
    for prog, argv, why in [
        ("powershell", ["-NoProfile", "-Command", "{file}"], "powershell -Command"),
        ("powershell", ["-NoProfile", "hook.ps1", "{file}"], "powershell with a script but no -File"),
        ("cmd", ["/c", "echo", "{file}"], "cmd /c"),
        ("cmd", ["/c", "x", "%KELPIE_FILE%"], "cmd /c with %KELPIE_FILE%"),
        ("ssh", ["host", "./x", "{file}"], "ssh"),
        ("bash", ["-c", "{file}"], "bash -c"),
    ]:
        try:
            save("bad", trigger={"event": "file", "glob": "*.md", "daily_cap": 5}, **{"action": "shell", "shell": {"program": prog, "argv": argv}})
            refused = None
        except RuntimeError as e:
            refused = str(e)
        run.check(f"{why} with the file name is refused when saved, and says what to do", bool(refused) and "script file" in refused, str(refused))
    run.check("…none of them was stored", auto("bad") is None)
    ps = ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "hook.ps1", "{file}"]
    for name, program, argv in [("rel", ".\\hook.cmd", ["{file}"]), ("bare", "evhook", ["{file}"]), ("psfile", "powershell", ps)]:
        save(name, trigger={"event": "file", "glob": "*.md", "daily_cap": 5}, action="shell", shell={"program": program, "argv": argv})
    resolved = auto("bare")
    run.check("a bare program name is found through PATH and PATHEXT (evhook is evhook.cmd) and the approval is of that file", resolved["command_hash"] != "" and not resolved["command_approved"])
    for name in ("rel", "bare", "psfile"):
        approve(name)
    time.sleep(GAP + 0.5)
    # Legal on NTFS, hostile to cmd (& ^ %), PowerShell ($( ) `) and POSIX shells (; ' ).
    hostile2 = "h $(New-Item pwned_ps.txt) `New-Item pwned_tick.txt`; it's & echo pwned_amp ^caret %PATH%.md"
    write(os.path.join(REPO, hostile2), "x\n")
    def hooked():
        out = []
        for kind in ("relative", "bare", "ps1"):
            try:
                out += open(f"{HOOKS}.{kind}", encoding="utf-8", errors="replace").read().splitlines()
            except OSError:
                pass
        return out
    got = until(lambda: len(hooked()) >= 3, 90)
    run.check("a batch file given by a relative path, one by name, and a PowerShell -File script all run", bool(got), str(hooked()))
    run.check("…each is given the hostile file name as one argument, unchanged", all(any(line.startswith(p) and hostile2 in line for line in hooked()) for p in ("relative:", "bare:", "ps1:")), str(hooked()))
    time.sleep(2)
    strays = [n for base in (REPO, WORK, os.getcwd()) for n in os.listdir(base) if n.startswith("pwned") or n in ("amp", "caret")]
    run.check("nothing in the name was executed by cmd or PowerShell", not strays, str(strays))
    drop("rel", "bare", "psfile")

    # ---------- 2. an automation's own edits do not set it off again ----------
    prompt = 'EDIT:{"span": 9, "files": {"spec/tasks.md": "tasks"}}'
    save("spec", kind="amp", prompt=prompt, trigger={"event": "file", "glob": "spec/**/*.md", "daily_cap": 20}, dry_run=False)
    time.sleep(1)
    write(os.path.join(REPO, "spec", "api.md"), "api v2\n")
    started = until(lambda: len(edits()) == 1, 40)
    run.check("saving a spec file starts the agent with the edit instruction", bool(started), str(lines(LAUNCHES)))
    # The agent edits spec/tasks.md at the start and near the end of 9 s: longer than the 3 s gap, so only
    # "the session it started is working" can explain no second start.
    time.sleep(14)
    run.check("the agent's own edits (matching its own trigger) did not start it again", len(edits()) == 1, str(len(edits())))
    tasks = open(os.path.join(REPO, "spec", "tasks.md"), encoding="utf-8").read() if os.path.exists(os.path.join(REPO, "spec", "tasks.md")) else ""
    run.check("…although it did edit the matching file twice", tasks.startswith("tasks") and tasks.endswith("2"), repr(tasks))
    sessions = [s for s in daemon(d, "session.list") if s["name"] == "spec"]
    run.check("one session was started for it, named after it", len(sessions) == 1, str([s["name"] for s in daemon(d, "session.list")]))
    # Once it is idle (the daemon counts working and waiting as busy) and a few quiet seconds have passed.
    idle_spec = until(lambda: all(s["status"] == "idle" for s in daemon(d, "session.list") if s["name"] == "spec" and s["alive"]), 90)
    time.sleep(GAP + 3)
    run.check("the session it started is idle again before the user saves", bool(idle_spec))
    write(os.path.join(REPO, "spec", "api.md"), "api v3\n")
    run.check("a later save by the user starts it again", bool(until(lambda: len(edits()) == 2, 40)), str(len(edits())))
    time.sleep(14)
    run.check("…and again its own edits do not", len(edits()) == 2, str(len(edits())))
    drop("spec")
    for s in daemon(d, "session.list"):
        if s["name"] == "spec":
            daemon(d, "session.close", {"handle": s["handle"]})

    # A quick-command action types its saved text into a session it started: the first time it starts one, then types
    # into that one; the user's own idle session of the same agent in the same folder is never touched.
    idle = start_agent(d, pid, "idle-amp")
    time.sleep(6)  # idle
    turns = lambda: [x for x in lines(LAUNCHES) if "turn" in x and "quick-ran" in json.dumps(x["turn"])]  # noqa: E731
    started = lambda: len([x for x in lines(LAUNCHES) if x.get("started")])  # noqa: E731
    before = started()
    typed = json.dumps({"work": 1, "files": {"quick-ran.txt": "typed"}})
    save("quick", kind="amp", action="quick", prompt=typed, quick_label="Write quick-ran", trigger={"event": "file", "glob": "go-quick*.txt", "daily_cap": 5})
    time.sleep(0.5)
    write(os.path.join(REPO, "go-quick-1.txt"), "go\n")
    run.check("a quick-command action with no session of its own starts one, which runs the prompt", bool(until(lambda: len(turns()) == 1 and started() == before + 1, 60)), f"{len(turns())} turns, {started() - before} starts")
    own = next((s for s in daemon(d, "session.list") if s["name"] == "quick"), None)
    run.check("…a session named after the automation, not the user's", bool(own) and own["id"] != idle["id"])
    time.sleep(32)  # past the quiet seconds after the turn, and the 30 s nobody has typed into it
    write(os.path.join(REPO, "go-quick-2.txt"), "go\n")
    run.check("the next event types the prompt into that session instead of starting another", bool(until(lambda: len(turns()) == 2, 40)) and started() == before + 1, f"{len(turns())} turns, {started() - before} starts")
    run.check("the user's own idle session of the same agent was never typed into", dones(d, idle) == 0, str(dones(d, idle)))
    drop("quick")
    if own:
        daemon(d, "session.close", {"handle": own["handle"]})

    # ---------- 3. agent-turn trigger, commits ----------
    save("turns", trigger={"event": "turn", "glob": "", "daily_cap": 20}, **command("turn"))
    approve("turns")
    save("commits", trigger={"event": "commit", "glob": "", "daily_cap": 20}, **command("commit"))
    approve("commits")
    time.sleep(GAP + 1)
    turn(d, idle, files={"agent-note.txt": "1"})
    run.check("an agent finishing a turn in the project runs the turn automation", bool(until(lambda: records("turns"), 30)), str(records("turns")))
    r = records("turns")[0] if records("turns") else {}
    run.check("…which is told it was a turn, in this folder", r.get("KELPIE_EVENT") == "turn" and r.get("cwd", "").lower() == root.lower())
    run.check("the turn left checkpoint refs under refs/kelpie", bool(until(lambda: git("for-each-ref", "refs/kelpie"), 20)), git("for-each-ref", "refs/kelpie")[:80])
    time.sleep(12)  # a poll or two of HEAD
    run.check("checkpoint refs are not commits: the commit automation did not run", not records("commits"), str(records("commits")))
    git("checkout", "-q", "-b", "side")
    time.sleep(12)
    run.check("switching branches is not a commit either", not records("commits"), str(records("commits")))
    write(os.path.join(REPO, "by-hand.txt"), "x\n")
    git("add", "-A")
    git("commit", "-qm", "by hand")
    sha = git("rev-parse", "HEAD")
    run.check("a commit made by hand runs it (within a poll)", bool(until(lambda: records("commits"), 30)), str(records("commits")))
    r = records("commits")[0] if records("commits") else {}
    run.check("…with the new commit and its branch", r.get("KELPIE_COMMIT") == sha and r.get("KELPIE_BRANCH") == "side", f"{r.get('KELPIE_COMMIT')} {r.get('KELPIE_BRANCH')}")
    time.sleep(GAP + 6)  # past the gap and the few quiet seconds after a command ends (its own edits are not events)
    n = len(records("commits"))
    turn(d, idle, files={"by-agent.txt": "1"}, run=[["git", "-c", "user.name=t", "-c", "user.email=t@t", "add", "-A"], ["git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "by the agent"]])
    run.check("a commit made by an agent runs it too", bool(until(lambda: len(records("commits")) > n, 30)), str(len(records("commits"))))
    time.sleep(12)
    n = len(records("commits"))
    time.sleep(12)
    run.check("and nothing runs again without another commit", len(records("commits")) == n, f"{n} -> {len(records('commits'))}")
    drop("turns", "commits")
    daemon(d, "session.close", {"handle": idle["handle"]})

    # ---------- 4. caps, the global switch, events from the window ----------
    save("capped", trigger={"event": "file", "glob": "cap-*.txt", "daily_cap": 2}, **command("cap", "{file}"))
    approve("capped")
    time.sleep(GAP + 0.5)
    for i in range(3):
        write(os.path.join(REPO, f"cap-{i}.txt"), "x\n")
        time.sleep(GAP + 1.5)
    run.check("a daily cap of 2 stops the third run", len(records("capped")) == 2, str(len(records("capped"))))
    run.check("…and the daemon says so once", auto("capped")["cap_count"] == 2, str(auto("capped")["cap_count"]))
    drop("capped")

    save("switch", trigger={"event": "file", "glob": "sw-*.txt", "daily_cap": 20}, **command("switch"))
    approve("switch")
    chip = lambda: d.js("const c = [...document.querySelectorAll('.statusbar .item')].find(i => (i.title || '').startsWith('Automations are')); return c ? {title: c.title, text: c.textContent.trim()} : null")  # noqa: E731
    until(chip, 10)
    run.check("the status bar offers the global switch while automations exist", bool(chip()) and chip()["title"].startswith("Automations are on"), str(chip()))
    d.js("[...document.querySelectorAll('.statusbar .item')].find(i => (i.title || '').startsWith('Automations are')).click()")
    run.check("clicking it pauses every automatic run", bool(until(lambda: daemon(d, "automation.state")["paused"] is True, 10)) and bool(until(lambda: (chip() or {}).get("text") == "paused", 10)), str(chip()))
    run.shot("03-paused")
    time.sleep(GAP + 0.5)
    write(os.path.join(REPO, "sw-1.txt"), "x\n")
    time.sleep(GAP + 1)
    run.check("while paused nothing runs", not records("switch"), str(records("switch")))
    d.js("[...document.querySelectorAll('.statusbar .item')].find(i => (i.title || '').startsWith('Automations are')).click()")
    run.check("clicking again resumes", bool(until(lambda: daemon(d, "automation.state")["paused"] is False, 10)))
    write(os.path.join(REPO, "sw-2.txt"), "x\n")
    run.check("…and the next change runs it", bool(until(lambda: records("switch"), 20)))
    drop("switch")

    save("pr", trigger={"event": "pr_checks_failed", "glob": "", "daily_cap": 5}, **command("pr"))
    approve("pr")
    save("done", trigger={"event": "finished", "glob": "", "daily_cap": 5}, **command("done"))
    approve("done")
    time.sleep(GAP + 0.5)
    d.js(f"window.__kelpie.automations.report({{kind: 'pr_checks_failed', root: arguments[0], number: 41}})", root)
    d.js(f"window.__kelpie.automations.report({{kind: 'finished', root: arguments[0], branch: 'feature-x', commit: 'c0ffee'}})", root)
    run.check("a pull request with failing checks, reported by the window, runs its automation", bool(until(lambda: records("pr"), 20)) and records("pr")[0]["KELPIE_PR_NUMBER"] == "41", str(records("pr")))
    run.check("a finished worktree, reported by the window, runs its automation", bool(until(lambda: records("done"), 20)) and records("done")[0]["KELPIE_BRANCH"] == "feature-x", str(records("done")))
    drop("pr", "done")

    # ---------- 5. who may do what ----------
    script = kc.Conn()  # the user's token, but not the app
    def refused(conn, method, params):
        try:
            conn.call(method, params)
        except SystemExit as e:
            return str(e)
        return None
    cmd = {"id": "x", "name": "x", "cwd": root, "action": "shell", "shell": {"program": sys.executable, "argv": []}, "trigger": {"event": "turn", "glob": "", "daily_cap": 1}}
    err = refused(script, "automation.save", cmd)
    run.check("a script with the user's token cannot save a command", bool(err) and "Kelpie app" in err, str(err))
    run.check("…nor report an event", bool(refused(script, "automation.event", {"kind": "file", "root": root, "files": ["a.md"]})))
    run.check("…nor approve a command", bool(refused(script, "automation.approve", {"id": "md", "hash": "x"})))
    run.check("…but can list them", refused(script, "automation.list", {}) is None)
    shell_session = daemon(d, "session.create", {"kind": "shell", "cwd": root, "name": "who"})

    class Raw(kc.Conn):
        def __init__(self, auth):
            token = kc.user_token
            kc.user_token = lambda: None
            try:
                super().__init__()
            finally:
                kc.user_token = token
            self.call("auth", auth)

    as_claimed = Raw({"session_id": shell_session["id"]})
    shapes = [None, {}, [1], "x", 7, True, cmd, {"id": "md"}, {"kind": "file", "root": root}]
    methods = ["automation.list", "automation.state", "automation.save", "automation.delete", "automation.run", "automation.approve", "automation.set_paused", "automation.resolve", "automation.event"]
    bad = [(m, s) for m in methods for s in shapes if not refused(as_claimed, m, s)]
    run.check("a claimed session (its token missing) is refused every automation method, whatever it sends", not bad, str(bad[:3]))
    path = os.path.join(WORK, "token.txt")
    # A cold PowerShell takes a while: type only once its prompt is idle.
    if until(lambda: "PS " in daemon(d, "session.read", {"handle": shell_session["handle"], "lines": 40})["text"] and next((x["status"] for x in daemon(d, "session.list") if x["id"] == shell_session["id"]), None) == "idle", 150):
        daemon(d, "session.input", {"handle": shell_session["handle"], "data": f"$env:KELPIE_SESSION_TOKEN | Out-File -Encoding ascii '{path}'\r"})
    token = until(lambda: os.path.exists(path) and open(path, encoding="ascii").read().strip(), 120)
    run.check("a session's shell has its token", bool(token) and len(token) == 64)
    if token:
        as_session = Raw({"session_id": shell_session["id"], "token": token})
        for scripting in (True, False):
            daemon(d, "daemon.set_policy", {"agents_control_others": scripting})
            bad = [(m, s) for m in methods for s in shapes if not refused(as_session, m, s)]
            run.check(f"a session's programs are refused every automation method (scripting {'on' if scripting else 'off'})", not bad, str(bad[:3]))
        daemon(d, "daemon.set_policy", {"agents_control_others": True})
    run.check("none of that created or changed an automation", daemon(d, "automation.list") == [], str(daemon(d, "automation.list")))
    daemon(d, "session.close", {"handle": shell_session["handle"]})

    # ---------- 6. the settings editor ----------
    d.js(f"{S}.openSettings('automations')")
    d.wait("return [...document.querySelectorAll('.btn')].some(b => b.textContent.includes('New automation'))", 10)
    d.js("[...document.querySelectorAll('.btn')].find(b => b.textContent.includes('New automation')).click()")
    d.wait("return !!document.querySelector('.automation-editor')", 5)

    def fill(label, value, tag="input"):
        d.js("const f = [...document.querySelectorAll('.automation-editor .field')].find(f => f.querySelector('span')?.textContent === arguments[0]); const el = f.querySelector(arguments[2]); el.value = arguments[1]; el.dispatchEvent(new Event(arguments[2] === 'select' ? 'change' : 'input', {bubbles: true}));", label, value, tag)

    def text():
        return d.js("return document.querySelector('.automation-editor').innerText")

    fill("Name", "ui-made")
    fill("Starts", "event", "select")
    opts = d.js("return [...document.querySelectorAll('.automation-editor .field')].find(f => f.querySelector('span')?.textContent === 'When').querySelectorAll('option').length")
    run.check("the editor offers the five events", opts == 5, str(opts))
    run.check("a file trigger says it needs the window open", "only while the window is open" in text(), text()[:300])
    fill("When", "commit", "select")
    run.check("a commit trigger says the daemon sees it, window or not", "also fires while the window is closed" in text(), text()[:300])
    fill("When", "file", "select")
    fill("Files", "docs/**/*.md")
    fill("Does", "shell", "select")
    fill("Program", sys.executable)
    fill("Arguments (one per line)", "recorder.py\n{file}", "textarea")
    run.check("the command form says no shell is used", "No shell is used" in text())
    run.shot("04-editor")
    d.js("[...document.querySelectorAll('.automation-editor .btn.primary')].find(b => b.textContent.trim() === 'Save').click()")
    saved = until(lambda: auto(next((x["id"] for x in daemon(d, "automation.list") if x["name"] == "ui-made"), "")), 10)
    run.check("saving from the editor stores the trigger, the command and no approval", bool(saved) and saved["trigger"] == {"event": "file", "glob": "docs/**/*.md", "daily_cap": 10} and saved["shell"]["argv"] == ["recorder.py", "{file}"] and saved["shell"]["approved_hash"] is None, str(saved))
    d.wait("return !!document.querySelector('.automation-item')", 10)
    item = d.js("return document.querySelector('.automation-item').innerText")
    run.check("the list says what starts it and shows the command", "docs/**/*.md" in item and "recorder.py" in item, item)
    run.shot("05-list")
    clicked = d.js("const b = [...document.querySelectorAll('.automation-item button')].find(b => b.textContent.trim() === 'Dry run'); if (b) b.click(); return !!b")
    run.check("each automation has a Dry run button", clicked)
    drop(saved["id"])

    # ---------- 7. storage ----------
    storage_checks(run)


def with_daemon(endpoint, data, fn):
    """Runs `fn(conn)` against a second, throw-away daemon (its own endpoint and data folder)."""
    saved = {k: os.environ.get(k) for k in ("KELPIE_ENDPOINT", "KELPIE_DATA_DIR")}
    os.environ.update({"KELPIE_ENDPOINT": endpoint, "KELPIE_DATA_DIR": data})
    proc = subprocess.Popen([kelpied, "serve"], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        c = None
        for _ in range(150):
            try:
                c = kc.Conn()
                break
            except OSError:
                time.sleep(0.1)
        if c is None:
            raise RuntimeError("the second daemon did not start")
        return fn(c)
    finally:
        try:
            kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
        except Exception:
            pass
        try:
            proc.wait(timeout=8)
        except Exception:
            proc.kill()
        try:
            os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", endpoint + ".token"))
        except OSError:
            pass
        for k, v in saved.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT


class AppConn(kc.Conn):
    """A connection that says it is the Kelpie app (what the window's connection does)."""

    def __init__(self):
        token = kc.user_token
        kc.user_token = lambda: None
        try:
            super().__init__()
        finally:
            kc.user_token = token
        self.call("auth", {"token": token(), "client": "app"})


def refused(conn, method, params=None):
    try:
        conn.call(method, params)
    except SystemExit as e:
        return str(e)
    return None


def dismiss_by_script_then_app(script):
    no = bool(refused(script, "automation.resolve", {"action": "dismiss"}))
    app = AppConn()
    return no, refused(app, "automation.resolve", {"action": "dismiss"}) is None


def storage_checks(run):
    # A damaged file.
    bad_dir = tempfile.mkdtemp(prefix="kelpie-evauto-bad-")
    garbage = '{"version": 2, "automations": [{"id": "a", "name": '
    write(os.path.join(bad_dir, "automations.json"), garbage)

    def damaged(c):
        state = c.call("automation.state", {})
        try:
            c.call("automation.save", {"id": "n", "name": "n", "kind": "claude", "prompt": "p", "cwd": bad_dir, "every": "daily"})
            refused = None
        except SystemExit as e:
            refused = str(e)
        return state, refused, c.call("automation.list", {})

    state, refused, listed = with_daemon(f"kelpie-evauto-bad-{os.getpid()}", bad_dir, damaged)
    names = os.listdir(bad_dir)
    kept = [n for n in names if n.startswith("automations.json.corrupt-")]
    run.check("a damaged automations file is reported by the daemon", "damaged" in (state["error"] or ""), str(state))
    run.check("…saving is refused with the reason", bool(refused) and "damaged" in refused, str(refused))
    run.check("…the list is empty, not silently replaced", listed == [] and "automations.json" not in names, str(names))
    run.check("…and the damaged bytes are kept under a .corrupt-<time> name, untouched", len(kept) == 1 and open(os.path.join(bad_dir, kept[0]), encoding="utf-8").read() == garbage, str(names))
    # A restart does not make it go away: the set-aside file is still unacknowledged, nothing is written, until dismissed.
    def again(c):
        before = c.call("automation.state", {})
        try:
            c.call("automation.save", {"id": "n", "name": "n", "kind": "claude", "prompt": "p", "cwd": bad_dir, "every": "daily"})
            refused = None
        except SystemExit as e:
            refused = str(e)
        return before, refused

    state2, refused2 = with_daemon(f"kelpie-evauto-bad-{os.getpid()}", bad_dir, again)
    run.check("after a restart the damaged file is still reported (no silent empty list), and saving still refused", state2["fix"] == "dismiss" and bool(refused2), str(state2))
    run.check("a script cannot dismiss it, the app can", with_daemon(f"kelpie-evauto-bad-{os.getpid()}", bad_dir, dismiss_by_script_then_app) == (True, True))
    state3, refused3 = with_daemon(f"kelpie-evauto-bad-{os.getpid()}", bad_dir, again)
    run.check("once dismissed it stays dismissed after a restart and the list can be written", state3["error"] is None and refused3 is None and any(n.endswith(".seen") for n in os.listdir(bad_dir)), str(state3))
    shutil.rmtree(bad_dir, ignore_errors=True)

    # The old format.
    old_dir = tempfile.mkdtemp(prefix="kelpie-evauto-old-")
    old = json.dumps([
        {"id": "d", "name": "daily", "kind": "claude", "prompt": "p1", "cwd": old_dir, "every": "daily", "time": "09:30", "deliver": "review", "last_run": 5},
        {"id": "w", "name": "weekly", "enabled": False, "kind": "codex", "prompt": "p2", "cwd": old_dir, "every": "weekly", "time": "08:15", "weekday": 3},
        {"id": "h", "name": "hourly", "kind": "gemini", "prompt": "p3", "cwd": old_dir, "every": "hourly", "time": "00:45"},
    ])
    write(os.path.join(old_dir, "automations.json"), old)
    listed = with_daemon(f"kelpie-evauto-old-{os.getpid()}", old_dir, lambda c: c.call("automation.list", {}))
    by = {a["id"]: a for a in listed}
    run.check("an old automations file keeps every schedule (and deliver) after the upgrade", len(by) == 3 and (by["d"]["every"], by["d"]["time"], by["d"]["deliver"], by["d"]["last_run"]) == ("daily", "09:30", "review", 5) and (by["w"]["every"], by["w"]["weekday"], by["w"]["enabled"]) == ("weekly", 3, False) and (by["h"]["every"], by["h"]["time"]) == ("hourly", "00:45"), str(listed)[:300])
    run.check("…keeps the old file as automations.json.bak", open(os.path.join(old_dir, "automations.json.bak"), encoding="utf-8").read() == old)
    new = json.load(open(os.path.join(old_dir, "automations.json"), encoding="utf-8"))
    run.check("…and writes the new format with a version", new.get("version") == 2 and len(new["automations"]) == 3, str(new)[:100])
    shutil.rmtree(old_dir, ignore_errors=True)


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def main():
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
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE])
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
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then the
    test daemon with its sessions (through this process's own, isolated endpoint), then the driver tree."""
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
    assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
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
    if run is not None and any(not r[1] for r in run.results):
        print(f"a check failed: data folder kept at {DATA}, fixtures at {WORK}", flush=True)
        return
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
