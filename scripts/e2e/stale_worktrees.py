"""GUI e2e: the sidebar drops worktrees removed outside Kelpie and marks the doubtful ones.

    python -X utf8 scripts/e2e/stale_worktrees.py <screenshot-dir> <scratch-dir> <app.exe>

Needs the debug app and daemon (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`; a
plain `cargo build -p kelpie` binary loads the Vite dev server on :1427 instead, so start `pnpm dev` first),
`tauri-driver` on PATH (or in ~/.cargo/bin) and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in
MSEDGEDRIVER). The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before anything
talks to a daemon, so the user's own Kelpie is never reached. No network: the repository is local.

1. A repository with six worktrees made by git: adding it lists all six under it.
2. A shell is opened for wt1 (from a folder outside it, so its folder can be deleted while it runs).
3. Outside the app: `git worktree remove` wt1..wt4; wt5 is removed and its folder made again (exists, not listed).
4. The window gains focus: wt2..wt4 go with one toast; wt1 stays marked missing (it has a session); wt5 stays
   marked "not a worktree any more"; wt6 is untouched. Nothing on disk changed (branches all kept).
5. Undo brings wt2..wt4 back (marked missing); "Remove from sidebar" on wt1 takes its row away and its shell
   keeps running.
"""
import importlib.util
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
from smoke_support import Run  # noqa: E402

if len(sys.argv) < 4:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

ENDPOINT = f"kelpie-stale-wt-{os.getpid()}"
os.makedirs(scratch, exist_ok=True)
DATA = tempfile.mkdtemp(prefix="kelpie-stale-wt-data-", dir=scratch)
WORK = os.path.join(scratch, f"stale-wt-{os.getpid()}")
REPO = os.path.join(WORK, "repo")
WTS = {f"wt{i}": os.path.join(WORK, "repo-worktrees", f"wt{i}") for i in range(1, 7)}

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1"})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver") or shutil.which(os.path.join(os.path.expanduser("~"), ".cargo", "bin", "tauri-driver"))
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)
assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT

procs = {}
S = "window.__kelpie.store"


def git(cwd, *args, check=True):
    p = subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True, encoding="utf-8")
    if check and p.returncode:
        raise RuntimeError(f"git {args}: {p.stderr}")
    return p.stdout.strip()


def _writable(func, path, _exc):
    os.chmod(path, 0o666)
    func(path)


def rm(path):
    """Git's object files are read-only, and a process that just exited may still hold the folder for a moment."""
    for _ in range(20):
        if not os.path.exists(path):
            return
        try:
            shutil.rmtree(path, onexc=_writable)
        except OSError:
            time.sleep(0.5)
    print(f"could not remove {path}", flush=True)


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def make_repo():
    os.makedirs(REPO)
    git(REPO, "init", "-q", "-b", "main")
    for k, v in (("user.name", "E2E Author"), ("user.email", "e2e@kelpie.test"), ("core.autocrlf", "false"), ("commit.gpgsign", "false")):
        git(REPO, "config", k, v)
    with open(os.path.join(REPO, "a.txt"), "w", encoding="utf-8", newline="\n") as f:
        f.write("base\n")
    git(REPO, "add", "-A")
    git(REPO, "commit", "-q", "-m", "init")
    for name, path in WTS.items():
        git(REPO, "worktree", "add", "-q", "-b", name, path)


def rows(d):
    """Worktree rows under the repository: name → class list."""
    return d.js("""const out = {};
      for (const r of document.querySelectorAll('.worktree-list .project.wt-row')) out[r.querySelector('.project-name').textContent] = r.className;
      return out;""")


def kids(d, pid):
    return d.js(f"return {S}.childrenOf(arguments[0]).map(p => p.name).sort()", pid)


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1280, 860)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js("window.__confirms = []; window.confirm = (m) => { window.__confirms.push(m); return true; };")

    # 1. adding the repository lists its six worktrees
    # What the sidebar's + does after the folder picker: the project, then its other worktrees.
    pid = d.js(f"const id = {S}.addProject(arguments[0]); window.__kelpie.worktrees.addExistingWorktrees({S}.state.projects.find(p => p.id === id)); return id", REPO)
    listed = until(lambda: len(kids(d, pid)) == 6, 20)
    run.check("adding the repository adds its six worktrees", listed, kids(d, pid))
    d.js(f"{S}.setWorktreesExpanded(arguments[0], true)", pid)
    ids = d.js(f"return Object.fromEntries({S}.childrenOf(arguments[0]).map(p => [p.name, p.id]))", pid)

    # 2. a shell for wt1, started outside its folder
    sid = d.js_async(f"""const done = arguments[arguments.length - 1];
      {S}.newSession(arguments[0], 'shell', {{ cwd: arguments[1], name: 'wt1-shell' }}).then(s => done(s ? s.id : null), e => done(String(e)));""", ids["wt1"], WORK)
    alive = until(lambda: d.js(f"return !!{S}.sessions[arguments[0]]?.alive", sid), 20)
    run.check("wt1 has a running shell", alive, sid)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    time.sleep(1.5)
    run.shot("01-before-six-worktrees")

    # 3. outside the app
    for name in ("wt1", "wt2", "wt3", "wt4", "wt5"):
        git(REPO, "worktree", "remove", "--force", WTS[name])
    os.makedirs(WTS["wt5"])
    with open(os.path.join(WTS["wt5"], "kept.txt"), "w", encoding="utf-8") as f:
        f.write("still here\n")
    gone = [n for n in ("wt1", "wt2", "wt3", "wt4") if not os.path.exists(WTS[n])]
    run.check("git removed four worktree folders", len(gone) == 4, gone)

    # 4. the window gains focus (checks are at most every 15 s; the first ran when the window connected)
    time.sleep(16)
    d.js("window.dispatchEvent(new Event('focus'))")
    went = until(lambda: kids(d, pid) == ["wt1", "wt5", "wt6"], 20)
    run.check("wt2..wt4 leave the sidebar", went, kids(d, pid))
    toast = d.wait("return document.querySelector('.toast')?.textContent || null", 5)
    run.check("one toast says how many went, with Undo", toast and "Removed 3 worktrees that no longer exist" in toast and "Undo" in toast, toast)
    r = until(lambda: (lambda x: x if "wt-missing" in x.get("wt1", "") and "wt-unlisted" in x.get("wt5", "") else None)(rows(d)), 10) or rows(d)
    run.check("wt1 (with a session) stays, marked missing", "wt-missing" in r.get("wt1", ""), r)
    run.check("wt5 (folder exists, git no longer lists it) stays, marked", "wt-unlisted" in r.get("wt5", ""), r)
    run.check("wt6 is untouched", r.get("wt6") is not None and "wt-missing" not in r["wt6"] and "wt-unlisted" not in r["wt6"], r)
    tags = d.js("return [...document.querySelectorAll('.wt-health')].map(e => e.textContent)")
    run.check("the marks say why", sorted(tags) == ["folder missing", "not a worktree any more"], tags)
    run.check("the shell still runs", d.js(f"return !!{S}.sessions[arguments[0]]?.alive", sid))
    branches = git(REPO, "branch", "--format=%(refname:short)").split()
    run.check("every branch is kept", all(f"wt{i}" in branches for i in range(1, 7)), branches)
    run.check("wt5's folder and file are kept", os.path.exists(os.path.join(WTS["wt5"], "kept.txt")))
    run.shot("02-after-focus")

    # 5. Undo, then Remove from sidebar on wt1
    d.js("[...document.querySelectorAll('.toast button')].find(b => b.textContent === 'Undo')?.click()")
    back = until(lambda: len(kids(d, pid)) == 6, 10)
    run.check("Undo brings the three back", back, kids(d, pid))
    marked = until(lambda: (lambda x: x if all("wt-missing" in x.get(n, "") for n in ("wt2", "wt3", "wt4")) else None)(rows(d)), 10)
    run.check("…marked missing, not removed again", marked, rows(d))
    run.shot("03-after-undo")
    d.js("""const r = [...document.querySelectorAll('.project.wt-row')].find(e => e.querySelector('.project-name').textContent === 'wt1');
      r.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 140, clientY: 200}));""")
    d.wait("return !!document.querySelector('.ctx-menu')", 5)
    items = d.js("return [...document.querySelectorAll('.ctx-item')].map(e => e.textContent)")
    run.check("a missing worktree's menu has Remove from sidebar, not Remove worktree… or Remove from Kelpie",
              any("Remove from sidebar" in i for i in items) and not any("Remove worktree" in i or "Remove from Kelpie" in i for i in items), items)
    d.js("[...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Remove from sidebar')).click()")
    left = until(lambda: "wt1" not in kids(d, pid), 10)
    run.check("Remove from sidebar takes wt1's row away", left, kids(d, pid))
    run.check("…after asking", len(d.js("return window.__confirms")) == 1, d.js("return window.__confirms"))
    run.check("…and its shell keeps running", d.js(f"return !!{S}.sessions[arguments[0]]?.alive", sid))
    run.shot("04-after-remove-from-sidebar")


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _conn():
    try:
        return kc.Conn()
    except (OSError, SystemExit):
        return None


def teardown(run):
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then the driver
    tree this script started, then the test daemon; removes the test data and repositories."""
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
    except (Exception, SystemExit):
        pass
    for p in procs.values():
        try:
            p.wait(timeout=10)
        except Exception:
            p.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    rm(DATA)
    rm(WORK)


def main():
    try:
        make_repo()
    except Exception:
        teardown(None)
        raise
    spawn("daemon", [kelpied, "serve"])
    if not until(lambda: _conn(), 15, 0.1):
        teardown(None)
        raise SystemExit("the daemon did not start")
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE])
    if not until(port_open, 30):
        teardown(None)
        raise SystemExit("tauri-driver did not open its port")
    run = None
    try:
        run = Run(shots, app=app)
        scenario(run)
    except Exception:
        traceback.print_exc()
        if run is None:
            teardown(None)
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        if run is not None:
            teardown(run)
    run.finish()


if __name__ == "__main__":
    main()
