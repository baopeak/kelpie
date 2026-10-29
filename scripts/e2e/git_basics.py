"""GUI e2e for the Changes panel's git basics: branch switcher, compact log, stash list.

    python -X utf8 scripts/e2e/git_basics.py <screenshot-dir> <scratch-dir> <app.exe>

Needs the debug app and daemon (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH (or in ~/.cargo/bin) and `msedgedriver.exe` matching the installed WebView2 (on PATH, or
in MSEDGEDRIVER). The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver (free
`--port` and `--native-port`) with the same environment, and stops both on exit. KELPIE_ENDPOINT and
KELPIE_DATA_DIR are set in this process before anything talks to a daemon, so the user's own Kelpie is never
reached. No network: the repository is local and has no remote.

1. Switch from a clean working tree: the branch changes (and back).
2. With uncommitted changes the switch is refused and nothing moves; "Open in a new worktree…" opens the
   worktree dialog for that existing branch, which checks it out in a new worktree.
3. A branch checked out in another worktree is refused with that worktree's folder.
4. History shows the last 100 commits (of 121), newest first.
5. Stashes: the "shared across worktrees" warning; Apply keeps the stash; Drop asks first (Cancel keeps it).
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

ENDPOINT = f"kelpie-git-basics-{os.getpid()}"
os.makedirs(scratch, exist_ok=True)
DATA = tempfile.mkdtemp(prefix="kelpie-git-basics-data-", dir=scratch)
WORK = os.path.join(scratch, f"git-basics-{os.getpid()}")
REPO = os.path.join(WORK, "repo")
HELD = os.path.join(WORK, "held-wt")

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


def write(path, text):
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def make_repo():
    """main with 121 commits, branches feature / other / held (held checked out in a linked worktree)."""
    os.makedirs(REPO)
    git(REPO, "init", "-q", "-b", "main")
    for k, v in (("user.name", "E2E Author"), ("user.email", "e2e@kelpie.test"), ("core.autocrlf", "false"), ("commit.gpgsign", "false")):
        git(REPO, "config", k, v)
    write(os.path.join(REPO, "a.txt"), "base\n")
    git(REPO, "add", "-A")
    git(REPO, "commit", "-q", "-m", "init")
    # 120 more commits in one process.
    stream = []
    for i in range(120):
        msg = f"commit {i}"
        stream.append(f"commit refs/heads/main\ncommitter E2E Author <e2e@kelpie.test> {1_700_000_000 + i} +0000\ndata {len(msg)}\n{msg}\n")
        if i == 0:
            stream.append("from refs/heads/main^0\n")
        stream.append("\n")
    # Bytes: text mode would turn "\n" into "\r\n" on Windows, which fast-import rejects.
    subprocess.run(["git", "-C", REPO, "fast-import", "--quiet"], input="".join(stream).encode(), capture_output=True, check=True)
    for b in ("feature", "other", "held"):
        git(REPO, "branch", b)
    git(REPO, "worktree", "add", "-q", HELD, "held")


def head():
    return git(REPO, "rev-parse", "--abbrev-ref", "HEAD")


def shown_branch(d):
    return d.js("return document.querySelector('.git-head .branch-switch .branch-name')?.textContent || null")


def open_switcher(d):
    if not d.js("return !!document.querySelector('.branch-switcher')"):
        d.click(".git-head .branch-switch")
    d.wait("return document.querySelectorAll('.branch-switcher .branch-row').length >= 4 || null", 15)


def pick(d, branch):
    open_switcher(d)
    d.click(f'.branch-switcher .branch-row[data-branch="{branch}"]')


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js("window.__confirms = []; window.__confirmAnswer = true; window.confirm = (m) => { window.__confirms.push(m); return window.__confirmAnswer; };")

    pid = d.js(f"return {S}.addProject(arguments[0])", REPO)
    d.wait(f"return {S}.projectInfo[arguments[0]]?.is_repo || null", 20, 0.25, pid)
    d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('git')", pid)
    d.wait("return document.querySelector('.git-head .branch-switch') || null", 15)
    run.check("the panel shows the branch as a switcher", until(lambda: shown_branch(d) == "main", 15), shown_branch(d))

    # 1. clean switch, and back
    pick(d, "feature")
    run.check("clean: switches to feature", until(lambda: head() == "feature", 15), head())
    run.check("…and the panel shows it", until(lambda: shown_branch(d) == "feature", 15), shown_branch(d))
    run.check("…and the switcher closes", until(lambda: not d.js("return !!document.querySelector('.branch-switcher')"), 5))
    open_switcher(d)
    run.check("the switcher lists local branches, current first, the held one marked",
              d.js("const r = [...document.querySelectorAll('.branch-row')]; return r[0].dataset.branch === 'feature' && r[0].classList.contains('current') && !!document.querySelector('.branch-row[data-branch=\"held\"] .chip')"))
    run.shot("01-switcher")
    pick(d, "main")
    run.check("clean: switches back to main", until(lambda: head() == "main", 15), head())

    # 2. dirty: refused, nothing moves, worktree offered
    write(os.path.join(REPO, "a.txt"), "dirty\n")
    pick(d, "feature")
    refusal = d.wait("return document.querySelector('.refusal')?.dataset.refusal || null", 15)
    run.check("dirty: the switch is refused", refusal == "dirty", refusal)
    run.check("…HEAD and the change stay", head() == "main" and read(os.path.join(REPO, "a.txt")) == "dirty\n", head())
    run.check("…the note says why and names the branch", "uncommitted changes" in d.js("return document.querySelector('.refusal').textContent") and "feature" in d.js("return document.querySelector('.refusal').textContent"))
    has_offer = d.js("return !!document.querySelector('.refusal [data-act=\"open-worktree\"]')")
    run.check("…and offers a new worktree", has_offer)
    run.shot("02-dirty-refused")
    if has_offer:
        d.click('.refusal [data-act="open-worktree"]')
        title = d.wait("return document.querySelector('.worktree-dialog h2')?.textContent || null", 10)
        run.check("the worktree dialog opens for the existing branch", "Open feature in a new worktree" in (title or ""), title)
        run.check("…with the branch fixed and no start point", d.js("const i = document.querySelector('.worktree-dialog input'); return i.readOnly && i.value === 'feature' && !document.querySelector('.worktree-dialog [list=\"wt-refs\"]')"))
        run.shot("03-worktree-dialog")
        d.click(".worktree-dialog .btn.primary")
        wt = d.wait(f"return {S}.state.projects.find(p => p.parentId === arguments[0] && p.branch === 'feature')?.path || null", 30, 0.25, pid)
        run.check("…which checks the branch out in a new worktree", bool(wt) and git(wt, "rev-parse", "--abbrev-ref", "HEAD") == "feature", wt)
        run.check("…and the main checkout is untouched", head() == "main" and read(os.path.join(REPO, "a.txt")) == "dirty\n")
        d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('git')", pid)
        d.wait("return document.querySelector('.git-head .branch-switch') || null", 15)

    # 3. a branch checked out in another worktree
    git(REPO, "checkout", "--", "a.txt")
    pick(d, "held")
    refusal = d.wait("return document.querySelector('.refusal')?.dataset.refusal || null", 15)
    text = d.js("return document.querySelector('.refusal')?.textContent || ''")
    run.check("held elsewhere: refused with that worktree's folder", refusal == "in_worktree" and "held-wt" in text, f"{refusal}: {text}")
    run.check("…HEAD stays", head() == "main")
    d.keys("escape")
    run.check("Escape closes the switcher (after a click on a row)", until(lambda: not d.js("return !!document.querySelector('.branch-switcher')"), 5))

    # 4. history: the last 100
    d.click('.group-label[data-section="log"]')
    n = d.wait("const n = document.querySelectorAll('.commit-row').length; return n >= 100 ? n : null", 20)
    first = d.js("return document.querySelector('.commit-row .subject')?.textContent")
    row = d.js("const r = document.querySelector('.commit-row'); return [r.querySelector('.hash').textContent, r.querySelector('.meta').textContent]")
    run.check("history shows exactly 100 commits, newest first", n == 100 and first == "commit 119", f"{n} rows, first {first!r}")
    run.check("…with short hash, author and relative date", len(row[0]) >= 7 and git(REPO, "rev-parse", "HEAD").startswith(row[0]) and "E2E Author" in row[1] and "ago" in row[1], str(row))
    run.check("…and says so", "Last 100 commits" in (d.js("return document.querySelector('.git-log .git-section-tools')?.textContent") or ""))
    run.shot("04-history")

    # 5. stashes
    write(os.path.join(REPO, "a.txt"), "first\n")
    git(REPO, "stash", "push", "-q", "-m", "first")
    write(os.path.join(REPO, "a.txt"), "second\n")
    git(REPO, "stash", "push", "-q", "-m", "second")
    d.click('.group-label[data-section="stashes"]')
    rows = d.wait("const n = document.querySelectorAll('.stash-row').length; return n === 2 ? n : null", 20)
    run.check("stashes listed, newest first", rows == 2 and d.js("return document.querySelector('.stash-row .subject').textContent") == "second")
    run.check("…under the shared-across-worktrees warning", "shared by every worktree" in (d.js("return document.querySelector('.stash-shared')?.textContent") or ""))
    run.shot("05-stashes")
    d.click('.stash-row[data-stash="stash@{1}"] [data-act="apply"]')
    run.check("apply brings the stash's changes", until(lambda: read(os.path.join(REPO, "a.txt")) == "first\n", 15), read(os.path.join(REPO, "a.txt")))
    time.sleep(1)
    run.check("…and keeps the stash", len(git(REPO, "stash", "list").splitlines()) == 2 and d.js("return document.querySelectorAll('.stash-row').length") == 2)
    d.js("window.__confirms = []; window.__confirmAnswer = false;")
    d.click('.stash-row[data-stash="stash@{0}"] [data-act="drop"]')
    asked = until(lambda: d.js("return window.__confirms[0] || null"), 5)
    time.sleep(1)
    run.check("drop asks first, naming the stash", bool(asked) and "stash@{0}" in asked and "second" in asked, asked)
    run.check("…Cancel keeps it", len(git(REPO, "stash", "list").splitlines()) == 2)
    d.js("window.__confirms = []; window.__confirmAnswer = true;")
    d.click('.stash-row[data-stash="stash@{0}"] [data-act="drop"]')
    left = until(lambda: d.js("return document.querySelectorAll('.stash-row').length === 1 || null"), 15)
    stashes = git(REPO, "stash", "list")
    run.check("…OK drops that one only", bool(left) and len(stashes.splitlines()) == 1 and "first" in stashes, stashes)
    run.shot("06-stash-dropped")


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


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


if __name__ == "__main__":
    main()
