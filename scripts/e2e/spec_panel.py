"""GUI e2e for the Plan tab (spec workspace): specs read from the repository, live checkbox progress,
ticking a box in the file, Run with… through the Runs engine, and New spec.

    python -X utf8 scripts/e2e/spec_panel.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie is never reached. Agents are stand-ins (`claude` and
`omp` shims on PATH running fake_agent below): each waits for an instruction file, ticks one box in the spec
file of its own working folder like an agent would, and reports with `kelpied run report`. Every repository is
a fixture under the scratch folder; this repository's own plans are never opened.

1. Closed tab, no work: the index counts nothing while another tab is shown, before the first open and after
   closing; the first open reads the specs.
2. Detection: plans/*/plan.md + phase-*.md, specs/**, .kiro/specs/**, docs/plans/** are listed with their tasks
   and progress; plans/reports, README, docs/guide.md, a junction out of the project and a name with control
   characters are not.
3. Live: a file edited by someone else ticks its box in the panel (debounced), with the group's progress.
4. Manual tick: a CRLF file changes by one byte and nothing else; ticking back restores the original bytes.
5. Concurrent edit: a file changed between reading and ticking is refused, left as the other party wrote it,
   and read again; a folder swapped for a junction after reading is refused and the target untouched.
6. Run with… here (claude, @path) and (omp, content inlined): the row shows the run's status through to Done,
   the agent's own edit ticks the box live; a hostile heading and task reach the prompt scrubbed.
7. Run with… in a worktree: without approval nothing is brought, after approval the copy and link are;
   the worktree has its own copy of the file; uncommitted spec files are flagged.
8. New spec: written plan-first into specs/<name>/, never over an existing file, name sanitised.
9. Parsing speed in the app, and nothing left running after the tab is closed.
"""
import importlib.util
import json
import os
import re
import shutil
import socket
import stat
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

ENDPOINT = f"kelpie-spec-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-spec-data-")
WORK = os.path.join(scratch, f"spec-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
CAPTURE = os.path.join(WORK, "capture")
REPO = os.path.join(WORK, "repo")
OUTSIDE = os.path.join(WORK, "outside")
for d_ in (BIN, CAPTURE, REPO, OUTSIDE):
    os.makedirs(d_, exist_ok=True)
SEED_PROJECT = "seed-project"

# Stand-in agent: waits for CAPTURE/tick-<kind>.json ({"path", "line"}), ticks that line of the file in its
# own working folder (the project, or its worktree), reports done, and stays.
FAKE = os.path.join(WORK, "fake_agent.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(r'''
import json, os, subprocess, sys, time
KELPIED = os.environ["KELPIE_E2E_KELPIED"]
CAPTURE = os.environ["KELPIE_E2E_CAPTURE"]
kind = sys.argv[1]
print("fake agent ready", flush=True)
open(os.path.join(CAPTURE, kind + "-start.json"), "w").write(json.dumps({"cwd": os.getcwd(), "argc": len(sys.argv)}))
ins = os.path.join(CAPTURE, "tick-" + kind + ".json")
end = time.time() + 60
while not os.path.exists(ins) and time.time() < end:
    time.sleep(0.2)
time.sleep(0.3)
job = json.load(open(ins))
os.remove(ins)
target = os.path.join(os.getcwd(), job["path"])
raw = open(target, "rb").read()
lines = raw.split(b"\n")
lines[job["line"]] = lines[job["line"]].replace(b"[ ]", b"[x]", 1)
open(target, "wb").write(b"\n".join(lines))
print("ticked", flush=True)
# A program in this session tries to start a task on an ssh host: the daemon must refuse.
try:
    attempt = {"name": "sneaky", "cwd": os.getcwd(), "tasks": [{"id": "x", "title": "x", "prompt": "x", "host": {"dest": "evil", "path": "/"}}]}
    r = subprocess.run([KELPIED, "call", "run.create", json.dumps(attempt)], capture_output=True, text=True)
    open(os.path.join(CAPTURE, kind + "-hostattempt.json"), "w").write(json.dumps([r.returncode, r.stdout, r.stderr]))
except Exception as e:
    open(os.path.join(CAPTURE, kind + "-hostattempt.json"), "w").write(json.dumps([-1, "", str(e)]))
# The same with the task as an array (read field by field, its tenth element is the host), a string, and a list that is not one.
for n, tasks in enumerate([[[None, "x", "x", "shell", [], {}, [], True, False, {"dest": "evil", "path": "/"}]], ["a task"], {"0": {"title": "x", "prompt": "x"}}]):
    r = subprocess.run([KELPIED, "call", "run.create", json.dumps({"name": "sneaky-shape-%d" % n, "cwd": os.getcwd(), "tasks": tasks})], capture_output=True, text=True)
    open(os.path.join(CAPTURE, kind + "-shape%d.json" % n), "w").write(json.dumps([r.returncode, r.stdout, r.stderr]))
subprocess.run([KELPIED, "run", "report", "done", kind + " ticked it"], capture_output=True)
time.sleep(3600)
''')
for name in ("claude", "omp"):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" {name} %*\r\n')

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "KELPIE_E2E_KELPIED": kelpied, "KELPIE_E2E_CAPTURE": CAPTURE,
                   "PATH": BIN + os.pathsep + os.environ.get("PATH", "")})
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

# ---------- an SSH "host": this machine through Git Bash, with its own stand-in agent ----------
SSH_DIR = os.path.join(WORK, "ssh-client")
HOSTHOME = os.path.join(WORK, "host-home")
HOSTBIN = os.path.join(WORK, "host-bin")
HOSTREPO = os.path.join(WORK, "host-repo")
BASH = os.environ.get("KELPIE_TEST_BASH", r"C:\Program Files\Git\bin\bash.exe")
HOST_PORT = free_port()
for d_ in (SSH_DIR, HOSTHOME, HOSTBIN, HOSTREPO):
    os.makedirs(d_, exist_ok=True)
if os.name == "nt":
    # Windows' OpenSSH refuses a config folder others may write.
    subprocess.run(["icacls", SSH_DIR, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:(OI)(CI)F", "/grant:r", "SYSTEM:(OI)(CI)F", "/grant:r", "Administrators:(OI)(CI)F"], capture_output=True)
KEY = os.path.join(SSH_DIR, "id_test")
subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", KEY], check=True)
if os.name == "nt":
    subprocess.run(["icacls", KEY, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:R"], capture_output=True)
open(os.path.join(SSH_DIR, "config"), "w").write("")
# Kelpie's ssh folder and known_hosts are this run's own, never ~/.ssh.
os.environ["KELPIE_SSH_DIR"] = SSH_DIR
fwd = lambda p: p.replace(chr(92), "/")
with open(os.path.join(HOSTBIN, "claude"), "w", encoding="utf-8", newline="\n") as f:
    f.write(f'#!/bin/sh\nexec "{fwd(sys.executable)}" "{fwd(os.path.join(HERE, "fake_host_agent.py"))}" "$@"\n')
posix = lambda p: subprocess.run([BASH, "-c", 'cygpath -u "$1"', "_", p], capture_output=True, text=True).stdout.strip()
HOSTREPO_POSIX = posix(HOSTREPO)


def host_sh(script, *args):
    """A command in the host's shell, with the link emulation the test host uses; true when it succeeds."""
    return subprocess.run([BASH, "-c", script, "_", *args], capture_output=True, env=dict(os.environ, MSYS="winsymlinks:lnk")).returncode == 0


def cksum_of(data):
    """POSIX cksum, as the host prints it."""
    table = []
    for n in range(256):
        c = n << 24
        for _ in range(8):
            c = ((c << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if c & 0x80000000 else (c << 1) & 0xFFFFFFFF
        table.append(c)
    crc = 0
    for b in list(data) + [(len(data) >> (8 * i)) & 0xFF for i in range(8) if len(data) >> (8 * i)]:
        crc = ((crc << 8) & 0xFFFFFFFF) ^ table[((crc >> 24) ^ b) & 0xFF]
    return f"{(~crc) & 0xFFFFFFFF} {len(data)}"

ssh_server = subprocess.Popen(
    [sys.executable, os.path.join(HERE, "ssh_test_server.py"), str(HOST_PORT), KEY + ".pub", os.path.join(SSH_DIR, "host_key")],
    env=dict(os.environ, KELPIE_TEST_HOME=HOSTHOME, MSYS="winsymlinks:lnk", PATH=HOSTBIN + os.pathsep + os.environ["PATH"]), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
ssh_server.stdout.readline()
procs["ssh"] = ssh_server
S = "window.__kelpie.store"
SI = "window.__kelpie.specIndex"
ESC = "\x1b"
BIDI = "\u202e"


def git(*args, cwd=REPO):
    r = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args], cwd=cwd, capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0", "LC_ALL": "C"})
    if r.returncode:
        raise RuntimeError(f"git {args}: {r.stderr}")
    return r.stdout.strip()


def until(fn, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.2)
    return None


def put(rel, data, root=REPO):
    path = os.path.join(root, *rel.split("/"))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data if isinstance(data, bytes) else data.encode("utf-8"))
    return path


def get(rel, root=REPO):
    with open(os.path.join(root, *rel.split("/")), "rb") as f:
        return f.read()


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


# ---------- fixture ----------

PHASE1 = ("# Phase 1: Parser\r\n\r\n## Steps\r\n- [ ] write the parser\r\n  - [ ] handle nested lists\r\n- [x] read the file\r\n\r\n"
          "## Tests\r\n- [ ] add parser tests\r\n\r\n```md\r\n- [ ] not a task, it is in a code block\r\n```\r\n").encode()
HOSTILE_SECTION = f"Steps {ESC}[2J{ESC}]52;c;aGk=\x07 @diff @terminal:main `tick` {BIDI}evil"
HOSTILE_TASK = f"do the thing @selection {ESC}[31m red {BIDI} rtl \"quoted\" `code`"
PHASE2 = f"# Phase 2: Hostile\n\n## {HOSTILE_SECTION}\n- [ ] {HOSTILE_TASK}\n".encode("utf-8")
PHASE3 = b"# Phase 3: Worktree\n\n## Steps\n- [ ] task for worktree one\n- [ ] task for worktree two\n- [ ] task for the uncommitted file\n"
LOGIN_TASKS = b"# Tasks\n\n- [ ] build login form\n- [ ] add rate limit\n- [x] write requirements\n"


def seed_fixture():
    git("init", "-q", "-b", "main")
    put(".gitignore", "deps/\n.env\n")
    put(".kelpie.yaml", "link:\n  - deps\ncopy:\n  - .env\n")
    put("README.md", "# readme\n- [ ] not a spec file\n")
    put("plans/260101-alpha/plan.md", "# Alpha plan\n\n| Phase | Status |\n|---|---|\n| 1 | pending |\n")
    put("plans/260101-alpha/phase-01-parser.md", PHASE1)
    put("plans/260101-alpha/phase-02-hostile.md", PHASE2)
    put("plans/260101-alpha/phase-03-worktree.md", PHASE3)
    put("plans/reports/note.md", "- [ ] reports are not plans\n")
    put("specs/login/requirements.md", "# Requirements\n\n- users sign in\n- sessions expire\n")
    put("specs/login/design.md", "# Design\n\nOne form.\n")
    put("specs/login/tasks.md", LOGIN_TASKS)
    put(".kiro/specs/pay/tasks.md", "# Implementation Plan\n\n- [ ] 1. Set up payments\n  - [ ] 1.1 Add the provider\n")
    put("docs/plans/q1.md", "# Q1\n\n- ship the panel\n- write the docs\n")
    put("docs/guide.md", "- [ ] not in a plans folder\n")
    git("add", "-A")
    git("commit", "-qm", "init")
    # The project's folder on the SSH host: its own copies of some specs.
    put("plans/260101-hostplan/plan.md", "# Host plan\n", root=HOSTREPO)
    put("plans/260101-hostplan/phase-01-remote.md", "# Phase 1\r\n\r\n- [ ] remote task one\r\n- [ ] remote task two\r\n- [x] remote done\r\n", root=HOSTREPO)
    put("specs/hosted/tasks.md", "# Tasks\n\n- [ ] hosted task\n", root=HOSTREPO)
    put("specs/login/tasks.md", LOGIN_TASKS, root=HOSTREPO)
    put("outside-target.md", "outside the project\n", root=HOSTHOME)
    assert host_sh('cd "$1" && ln -s tasks.md specs/hosted/linked.md && ln -s "$2" docs-link.md', HOSTREPO_POSIX, posix(os.path.join(HOSTHOME, "outside-target.md")))
    put(".env", "KEY=1\n")
    put("deps/pkg/a.js", "module.exports = 1\n")
    # Outside the project, reachable by a junction (and a symlinked file where the platform lets a test make one).
    put("tasks.md", b"- [ ] outside the project\n", root=OUTSIDE)
    put("tasks.md", b"- [ ] outside the project, too\n", root=OUTSIDE + "-swap")
    if os.name == "nt":
        subprocess.run(["cmd", "/c", "mklink", "/J", os.path.join(REPO, "specs", "escape"), OUTSIDE], capture_output=True, check=True)
    else:
        os.symlink(OUTSIDE, os.path.join(REPO, "specs", "escape"))
    # A name with a control character is skipped (it could not be written into a prompt safely).
    try:
        put("specs/odd\u202ename.md", "- [ ] odd\n")
    except OSError:
        pass
    state = {
        "version": 3, "projects": [{"id": SEED_PROJECT, "name": "repo", "path": REPO.replace("\\", "/"), "color": "#4f8cff"}], "activeProject": SEED_PROJECT, "workspaces": {},
        "settings": {}, "recent": [], "collapsed": {}, "keymap": {}, "approvedSetups": {}, "hibernated": [], "inbox": [],
        "reviewNotes": {}, "floating": {"sessions": [], "active": None, "open": False},
    }
    put("app-state.json", json.dumps(state), root=DATA)


# ---------- page helpers ----------

ROWS = """return [...document.querySelectorAll('.plan-task')].map(r => ({
  id: r.dataset.task, text: r.querySelector('.plan-text').textContent, line: +r.dataset.line,
  checked: r.querySelector('.plan-box').getAttribute('aria-checked') === 'true',
  chip: r.querySelector('.plan-run')?.textContent ?? null,
  path: r.closest('.plan-doc')?.dataset.path ?? null, group: r.closest('.plan-group')?.dataset.group ?? null }))"""


def rows(d):
    return d.js(ROWS)


def row(d, text):
    return next((r for r in rows(d) if r["text"] == text), None)


def groups(d):
    return d.js("return [...document.querySelectorAll('.plan-group')].map(g => ({id: g.dataset.group, progress: g.querySelector('.plan-group-head .run-progress')?.textContent.trim() ?? null, open: g.querySelector('.plan-group-head').getAttribute('aria-expanded') === 'true'}))")


def group(d, gid):
    return next((g for g in groups(d) if g["id"] == gid), None)


def open_group(d, gid):
    d.js("const g = document.querySelector(`.plan-group[data-group='${arguments[0]}']`); const h = g.querySelector('.plan-group-head'); if (h.getAttribute('aria-expanded') !== 'true') h.click()", gid)


def click_box(d, text):
    return d.js("const r = [...document.querySelectorAll('.plan-task')].find(r => r.querySelector('.plan-text').textContent === arguments[0]); if (r) r.querySelector('.plan-box').click(); return !!r", text)


def toast(d):
    """Every toast shown so far, oldest first (the app has one slot, and an unrelated notice may replace ours)."""
    return " | ".join(d.js("return window.__toasts || []"))


def stats(d):
    return d.js(f"return {{...{SI}.stats}}")


def show_panel(d, name):
    d.js(f"{S}.showRightPanel(arguments[0])", name)


def get_run(d, rid):
    return daemon(d, "run.get", {"id": rid})


def runs_with(d, text):
    return [r for r in daemon(d, "run.list") if text in r["name"]]


def row_ids_in_runs(d):
    return {t["id"]: r for r in daemon(d, "run.list") for t in r["tasks"]}


def run_with(d, text, kind, mode):
    """Opens Run with… on the row, picks the agent and where, and starts it."""
    d.js("const r = [...document.querySelectorAll('.plan-task')].find(r => r.querySelector('.plan-text').textContent === arguments[0]); r.querySelector('.plan-run-with').click()", text)
    d.wait("return !!document.querySelector('.plan-runwith')", 10)
    d.js("const [a, m] = document.querySelectorAll('.plan-runwith select'); a.value = arguments[0]; a.dispatchEvent(new Event('change', {bubbles: true})); m.value = arguments[1]; m.dispatchEvent(new Event('change', {bubbles: true}))", kind, mode)


def start_run(d):
    d.js("[...document.querySelectorAll('.plan-runwith .btn.primary')].find(b => b.textContent.trim() === 'Start').click()")


def tell_agent(kind, path, line):
    put(f"tick-{kind}.json", json.dumps({"path": path, "line": line}), root=CAPTURE)


def task_run(d, task_id, timeout=30):
    """The newest run holding this spec task."""
    def find():
        hits = [r for r in daemon(d, "run.list") if any(t["id"] == task_id for t in r["tasks"])]
        return max(hits, key=lambda r: r["created_at"]) if hits else None
    return until(find, timeout)


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 13, 30)
    d.js("window.confirm = () => true")
    d.js("window.__toasts = []; new MutationObserver(() => { const t = document.querySelector('.toast')?.textContent; if (t && t !== window.__toasts[window.__toasts.length - 1]) window.__toasts.push(t); }).observe(document.body, { childList: true, subtree: true, characterData: true })")
    pid = SEED_PROJECT
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.is_repo", pid), 30)
    until(lambda: d.js(f"return !!window.__kelpie.specIndex"), 30)
    run.check("the app talks to a daemon (API 13) and exposes the index counters", d.js(f"return {S}.daemonApi()") >= 13)
    show_panel(d, "files")
    d.wait("return !!document.querySelector('.rightpanel .panel-tabs')", 20)

    # ---------- 1. closed tab, no work ----------
    zero = stats(d)
    put("specs/login/tasks.md", LOGIN_TASKS + b"- [ ] added while closed\n")
    time.sleep(1.6)
    run.check("before the first open: nothing counted, though files changed under a watched project", zero == {"scans": 0, "reads": 0, "parses": 0} and stats(d) == zero, str(stats(d)))
    run.check("the Plan tab sits in the right panel's tab bar", d.js("return [...document.querySelectorAll('.panel-tabs .ptab')].some(b => b.textContent.trim() === 'Plan')"))
    put("specs/login/tasks.md", LOGIN_TASKS)
    d.js("[...document.querySelectorAll('.panel-tabs .ptab')].find(b => b.textContent.trim() === 'Plan').click()")
    run.check("opening the tab reads the specs", bool(until(lambda: d.js("return !!document.querySelector('.plan-group')"), 20)) and stats(d)["scans"] > 0 and stats(d)["parses"] > 0, str(stats(d)))
    run.shot("01-plan-tab")

    # ---------- 2. detection ----------
    ids = sorted(g["id"] for g in groups(d))
    run.check("plans, specs, .kiro/specs and docs/plans are listed, and only those", ids == [".kiro/specs/pay", "docs/plans", "plans/260101-alpha", "specs/login"], str(ids))
    alpha, login = group(d, "plans/260101-alpha"), group(d, "specs/login")
    run.check("a plan's progress is its phases' boxes together (1 of 8), a spec's its tasks (1 of 3)", alpha["progress"] == "1/8" and login["progress"] == "1/3", f"{alpha} {login}")
    run.check("unfinished groups are open, ordered before the one without boxes", [g["open"] for g in groups(d)][:3] == [True, True, True] and groups(d)[-1]["id"] == "docs/plans", str(groups(d)))
    all_paths = {r["path"] for r in rows(d)}
    run.check("plans/reports, README, docs/guide.md, the junction out of the project and the odd name are not listed",
              not any(x in p for p in all_paths for x in ("reports", "README", "guide", "escape", "odd")) and not any("outside the project" in r["text"] for r in rows(d)), str(sorted(all_paths)))
    run.check("a box inside a code block is not a task, nested tasks are indented", row(d, "not a task, it is in a code block") is None and row(d, "handle nested lists") is not None
              and d.js("const r = [...document.querySelectorAll('.plan-task')]; const a = r.find(x => x.textContent.includes('handle nested')), b = r.find(x => x.textContent.includes('write the parser')); return parseInt(a.style.paddingLeft) > parseInt(b.style.paddingLeft)"))
    open_group(d, "docs/plans")
    run.check("a file without boxes shows its list items as they are, not hidden", d.js("return [...document.querySelectorAll('.plan-doc[data-path=\"docs/plans/q1.md\"] .plan-plain li')].map(l => l.textContent)") == ["ship the panel", "write the docs"])
    run.check("a spec's requirements and design are listed with their items", d.js("return [...document.querySelectorAll('.plan-doc')].filter(x => x.dataset.path.startsWith('specs/login/')).map(x => x.dataset.path)") == ["specs/login/requirements.md", "specs/login/design.md", "specs/login/tasks.md"])
    run.check("the files outside the project were not touched", get("tasks.md", OUTSIDE) == b"- [ ] outside the project\n")

    # ---------- 3. live progress ----------
    t0 = time.time()
    put("specs/login/tasks.md", LOGIN_TASKS.replace(b"- [ ] build login form", b"- [x] build login form"))
    ok = until(lambda: (row(d, "build login form") or {}).get("checked"), 6)
    took = time.time() - t0
    run.check(f"a file edited by someone else ticks its box in the panel ({took:.2f} s, debounce 300 ms)", bool(ok) and took < 3, f"{took}")
    run.check("…and the group's progress follows (2 of 3)", bool(until(lambda: group(d, "specs/login")["progress"] == "2/3", 5)), str(group(d, "specs/login")))
    put("specs/login/tasks.md", LOGIN_TASKS + b"- [ ] a new task\n")
    run.check("a task added to the file appears", bool(until(lambda: row(d, "a new task"), 6)))
    put("specs/login/tasks.md", LOGIN_TASKS)
    run.check("…and goes again", bool(until(lambda: not row(d, "a new task") and row(d, "build login form") and not row(d, "build login form")["checked"], 6)))
    put("specs/login/extra.md", "- [ ] in a new file\n")
    run.check("a new spec file in a known folder is found", bool(until(lambda: row(d, "in a new file"), 6)))
    os.remove(os.path.join(REPO, "specs", "login", "extra.md"))
    run.check("…and forgotten when deleted", bool(until(lambda: not row(d, "in a new file"), 6)))
    time.sleep(1.2)  # the deletion's own events have been handled
    n0 = stats(d)
    put("README.md", "# readme\n- [ ] still not a spec\n")
    put("src/app.txt", "x")
    time.sleep(1.2)
    run.check("changes outside the roots cost nothing (no scan, read or parse)", stats(d) == n0, f"{n0} {stats(d)}")

    # ---------- 4. manual tick: one byte ----------
    before = get("plans/260101-alpha/phase-01-parser.md")
    run.check("the fixture file has Windows line ends", before.count(b"\r\n") > 5 and before.count(b"\n") == before.count(b"\r\n"))
    run.check("the box is found by its text", click_box(d, "write the parser"))
    after = until(lambda: (lambda b: b if b != before else None)(get("plans/260101-alpha/phase-01-parser.md")), 8)
    expect = before.replace(b"- [ ] write the parser", b"- [x] write the parser", 1)
    run.check("ticking writes the file: exactly one byte changed, CRLF intact", after == expect, f"{after!r}")
    diff = git("diff", "--numstat", "--", "plans/260101-alpha/phase-01-parser.md")
    run.check("git sees one line changed", diff.split()[:2] == ["1", "1"], diff)
    run.check("the panel shows it ticked, and the plan's progress (2 of 8)", bool(until(lambda: (row(d, "write the parser") or {}).get("checked") and group(d, "plans/260101-alpha")["progress"] == "2/8", 6)))
    click_box(d, "write the parser")
    back = until(lambda: get("plans/260101-alpha/phase-01-parser.md") == before, 8)
    run.check("ticking back restores the original bytes", bool(back))
    run.shot("02-after-ticks")

    # ---------- 5. concurrent edit ----------
    open_group(d, "plans/260101-alpha")
    root = REPO.replace("\\", "/")
    theirs = before.replace(b"- [ ] add parser tests", b"- [ ] add parser tests\r\n- [ ] a task someone else wrote")
    js_await(d, "(async () => { const inv = window.__TAURI_INTERNALS__.invoke; await inv('fs_write_file', { root: arguments[0], path: arguments[1], content: arguments[2] }); [...document.querySelectorAll('.plan-task')].find(r => r.querySelector('.plan-text').textContent === 'write the parser').querySelector('.plan-box').click(); })()",
             root, "plans/260101-alpha/phase-01-parser.md", theirs.decode())
    time.sleep(0.8)
    run.check("a file changed after the panel read it is refused: left as the other party wrote it", get("plans/260101-alpha/phase-01-parser.md") == theirs)
    run.check("…and the user is told nothing was written", "changed on disk" in toast(d) and "nothing was written" in toast(d), toast(d))
    run.check("…the file is read again: the new task shows, the box is still open", bool(until(lambda: row(d, "a task someone else wrote") and not row(d, "write the parser")["checked"], 6)))
    run.check("…and ticking again now works", click_box(d, "write the parser") and bool(until(lambda: b"- [x] write the parser" in get("plans/260101-alpha/phase-01-parser.md"), 8)))
    put("plans/260101-alpha/phase-01-parser.md", before)
    until(lambda: not row(d, "a task someone else wrote") and not row(d, "write the parser")["checked"], 6)

    # A folder swapped for a junction after the panel read it: the tick is refused and the target untouched.
    swap = os.path.join(REPO, "specs", "login")
    parked = swap + ".parked"
    os.rename(swap, parked)
    if os.name == "nt":
        subprocess.run(["cmd", "/c", "mklink", "/J", swap, OUTSIDE + "-swap"], capture_output=True, check=True)
    else:
        os.symlink(OUTSIDE + "-swap", swap)
    clicked = click_box(d, "add rate limit")
    time.sleep(1.0)
    run.check("a folder replaced by a link after the panel read it: the tick is refused, the target file untouched", clicked and get("tasks.md", OUTSIDE + "-swap") == b"- [ ] outside the project, too\n", toast(d))
    run.check("…and the user is told", "not a plain text file" in toast(d), toast(d))
    if os.name == "nt":
        os.rmdir(swap)
    else:
        os.unlink(swap)
    os.rename(parked, swap)
    run.check("…and once the folder is back the panel is too (no stale group left)", bool(until(lambda: group(d, "specs/login") and not group(d, "specs/login.parked") and row(d, "add rate limit"), 12)), str(groups(d)))

    # ---------- 6. Run with… here ----------
    p1 = "plans/260101-alpha/phase-01-parser.md"
    line_tests = rows(d)
    tests = next(r for r in line_tests if r["text"] == "add parser tests")
    tell_agent("claude", p1, tests["line"] - 1)
    run_with(d, "add parser tests", "claude", "here")
    offered = d.js("const [a, m] = document.querySelectorAll('.plan-runwith select'); return [[...a.options].map(o => o.value), [...m.options].map(o => o.value)]")
    run.check("Run with… offers the agents and where to run it, here and in a worktree of its own", offered[0][:3] == ["claude", "codex", "omp"] and offered[1] == ["here", "worktree"], str(offered))
    run.shot("03-run-with")
    start_run(d)
    r1 = until(lambda: next(iter(runs_with(d, "add parser tests")), None), 20)
    run.check("Start made a run of one task for the project", bool(r1) and len(r1["tasks"]) == 1 and r1["project_id"] == pid and r1["tasks"][0]["kind"] == "claude" and not r1["tasks"][0]["worktree"] and r1["tasks"][0]["deps"] == [], str(r1))
    task_id = r1["tasks"][0]["id"]
    run.check("…whose id comes from the task (the row finds its run without a store of its own)", re.fullmatch(r"spec-[0-9a-f]{8}", task_id) is not None and task_id == row(d, "add parser tests")["id"])
    prompt = r1["tasks"][0]["prompt"]
    line_no = tests["line"]
    run.check("the prompt names the task and the section, and refers to the file as @path for claude",
              prompt.startswith("Do one task from a spec file in this repository.") and 'Section: "Tests"' in prompt and f'Task (line {line_no}): "add parser tests"' in prompt and "@plans/260101-alpha/phase-01-parser.md" in prompt and "```" not in prompt, prompt)
    chip = until(lambda: (row(d, "add parser tests") or {}).get("chip"), 15)
    run.check("the row shows the run's status", chip in ("Starting", "Running", "Done"), str(chip))
    ticked = until(lambda: (row(d, "add parser tests") or {}).get("checked"), 30)
    run.check("the agent's edit of the file ticks the box in the panel, live", bool(ticked), str(row(d, "add parser tests")))
    done = until(lambda: (row(d, "add parser tests") or {}).get("chip") == "Done", 30)
    run.check("when the agent reports, the row says Done", bool(done), str(row(d, "add parser tests")))
    run.check("…the stand-in ran in the project folder and only its line changed", json.load(open(os.path.join(CAPTURE, "claude-start.json")))["cwd"].replace("\\", "/").lower() == REPO.replace("\\", "/").lower()
              and get(p1) == before.replace(b"- [ ] add parser tests", b"- [x] add parser tests", 1))
    run.shot("04-run-done")
    d.js("document.querySelector(`.plan-task[data-task='${arguments[0]}'] .plan-run`).click()", task_id)
    run.check("the status chip opens the run in the Runs page", bool(until(lambda: d.js("return !!document.querySelector('.runs-page')"), 10)))
    d.js(f"{S}.setOverlay(null)")
    put(p1, before)
    until(lambda: not row(d, "add parser tests")["checked"], 8)

    # An agent that does not read @path gets the file's content, labelled.
    tell_agent("omp", "specs/login/tasks.md", 3)
    open_group(d, "specs/login")
    run_with(d, "add rate limit", "omp", "here")
    start_run(d)
    r2 = until(lambda: next(iter(runs_with(d, "add rate limit")), None), 20)
    p2 = (r2 or {"tasks": [{"prompt": ""}]})["tasks"][0]["prompt"]
    run.check("another agent (omp) gets the content of the spec file fenced and labelled, and no @path", "File `specs/login/tasks.md`:" in p2 and "- [ ] build login form" in p2 and "@specs/" not in p2 and "Task (line 4): \"add rate limit\"" in p2, p2)
    run.check("…and its row reaches Done too", bool(until(lambda: (row(d, "add rate limit") or {}).get("chip") == "Done", 40)))
    put("specs/login/tasks.md", LOGIN_TASKS)

    # The hostile heading and task text, scrubbed on their way into the prompt.
    hostile_text = None
    until(lambda: any("do the thing" in r["text"] for r in rows(d)), 8)
    hostile_text = next(r["text"] for r in rows(d) if "do the thing" in r["text"])
    run.check("the hostile task is listed, its text shown without control or bidirectional characters", "do the thing" in hostile_text and "rtl" in hostile_text and not re.search("[\x00-\x08\x0b-\x1f\u202e]", hostile_text), repr(hostile_text))
    tell_agent("claude", "plans/260101-alpha/phase-02-hostile.md", 3)
    run_with(d, hostile_text, "claude", "here")
    start_run(d)
    r3 = until(lambda: next(iter(runs_with(d, "do the thing")), None), 20)
    p3 = (r3 or {"tasks": [{"prompt": ""}]})["tasks"][0]["prompt"]
    # eslint-disable-next-line
    bad = [c for c in p3 if (ord(c) < 32 and c != "\n") or ord(c) == 127 or 0x80 <= ord(c) <= 0x9f or ord(c) in (0x2028, 0x2029) or 0x200b <= ord(c) <= 0x200f or 0x202a <= ord(c) <= 0x202e or 0x2066 <= ord(c) <= 0x2069]
    run.check("the prompt holds no control, bidirectional or invisible character", bool(p3) and not bad, repr(bad))
    run.check("…the only @ in it is the file's path (a heading's @diff and @terminal cannot be mentions)", p3.count("@") == 1 and "(at)diff" in p3 and "(at)terminal:main" in p3 and "(at)selection" in p3, p3)
    run.check("…and the heading's quotes and backticks cannot close its quotes", p3.count('Section: "') == 1 and "`" not in p3.split("Spec file:")[0], p3)
    run.check("the run's name is scrubbed too", bool(r3) and not re.search(r"[\x00-\x1f\u202e]", r3["name"]) and "@" not in r3["name"], repr(r3 and r3["name"]))
    until(lambda: (row(d, hostile_text) or {}).get("chip") == "Done", 40)
    put("plans/260101-alpha/phase-02-hostile.md", PHASE2)

    # ---------- 7. Run with… in a worktree ----------
    p3f = "plans/260101-alpha/phase-03-worktree.md"
    one = next(r for r in rows(d) if r["text"] == "task for worktree one")
    tell_agent("claude", p3f, one["line"] - 1)
    run_with(d, "task for worktree one", "claude", "worktree")
    start_run(d)
    w1 = until(lambda: next(iter(runs_with(d, "task for worktree one")), None), 20)
    run.check("in a worktree: the task has one of its own", bool(w1) and w1["tasks"][0]["worktree"] is True, str(w1))
    run.check("…nothing is brought while .kelpie.yaml is not approved (no materialize, no .env)", bool(w1) and w1.get("materialize") is None, str(w1 and w1.get("materialize")))
    w1d = until(lambda: (lambda r: r if r["tasks"][0]["status"] == "done" else None)(get_run(d, w1["id"])), 60)
    run.check("the agent finished in its worktree and reported", bool(w1d))
    wt1 = w1d["tasks"][0]["cwd"] if w1d else ""
    run.check("…which is a folder of its own with its own copy of the spec file, ticked there only",
              bool(wt1) and os.path.normcase(wt1) != os.path.normcase(REPO) and b"- [x] task for worktree one" in get(p3f, wt1) and b"- [ ] task for worktree one" in get(p3f), wt1)
    run.check("…and without approval it holds no .env and no deps", bool(wt1) and not os.path.exists(os.path.join(wt1, ".env")) and not os.path.exists(os.path.join(wt1, "deps")))
    run.check("the row shows Done", bool(until(lambda: (row(d, "task for worktree one") or {}).get("chip") == "Done", 20)))
    run.check("the project's own file keeps the box open (the work is in the worktree, to be finished from there)", not row(d, "task for worktree one")["checked"])

    js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'HEAD'); {S}.approveSetup(r.path, cfg.setupKey); }})()", pid)
    two = next(r for r in rows(d) if r["text"] == "task for worktree two")
    tell_agent("claude", p3f, two["line"] - 1)
    run_with(d, "task for worktree two", "claude", "worktree")
    start_run(d)
    w2 = until(lambda: next(iter(runs_with(d, "task for worktree two")), None), 20)
    run.check("once approved, the run brings the copy and link of .kelpie.yaml", bool(w2) and w2.get("materialize") == {"copy": [".env"], "link": ["deps"]}, str(w2 and w2.get("materialize")))
    w2d = until(lambda: (lambda r: r if r["tasks"][0]["status"] == "done" else None)(get_run(d, w2["id"])), 60)
    wt2 = w2d["tasks"][0]["cwd"] if w2d else ""
    run.check("…the worktree has .env copied and deps linked", bool(wt2) and os.path.isfile(os.path.join(wt2, ".env")) and os.path.isfile(os.path.join(wt2, "deps", "pkg", "a.js")))

    # Uncommitted spec files are flagged for a worktree.
    put(p3f, get(p3f) + b"- [ ] added after the commit\n")
    three = until(lambda: row(d, "added after the commit"), 8)
    run_with(d, "added after the commit", "claude", "worktree")
    run.check("a spec file with uncommitted changes is flagged when it is run in a worktree", bool(until(lambda: "not committed" in (d.js("return document.querySelector('.plan-runwith p')?.textContent ?? ''") or ""), 8)), d.js("return document.querySelector('.plan-runwith')?.innerText"))
    d.js("[...document.querySelectorAll('.plan-runwith .btn')].find(b => b.textContent.trim() === 'Cancel').click()")
    run.check("Cancel closes the form without starting anything", not d.js("return !!document.querySelector('.plan-runwith')") and not runs_with(d, "added after the commit"))
    git("checkout", "--", p3f)
    run.shot("05-worktree-runs")

    # ---------- 8. New spec ----------
    d.js("window.__kelpie.commands.find(c => c.id === 'spec.new').run()")
    run.check("the New Spec palette command opens the Plan tab at the name field, focused", bool(until(lambda: d.js("return document.activeElement?.closest('.plan-new') !== null && document.activeElement.tagName === 'INPUT'"), 8)))
    def type_name(text):
        d.js("const e = document.querySelector('.plan-new input'); e.value = arguments[0]; e.dispatchEvent(new InputEvent('input', {bubbles: true}))", text)
    type_name("???")
    run.check("a name with nothing usable keeps Create off", d.js("return document.querySelector('.plan-new button[type=submit]').disabled"))
    type_name("../../Add CSV export!")
    run.check("the folder it will write is shown, sanitised", bool(until(lambda: "specs/add-csv-export/" in d.js("return document.querySelector('.plan-new .mono')?.textContent ?? ''"), 5)), d.js("return document.querySelector('.plan-new')?.innerText"))
    run.shot("06-new-spec")
    d.js("document.querySelector('.plan-new button[type=submit]').click()")
    made = until(lambda: os.path.exists(os.path.join(REPO, "specs", "add-csv-export", "tasks.md")), 10)
    run.check("Create wrote requirements, design and tasks into specs/<name>/", bool(made) and all(os.path.exists(os.path.join(REPO, "specs", "add-csv-export", f)) for f in ("requirements.md", "design.md", "tasks.md")))
    until(lambda: b"- [ ] " in get("specs/add-csv-export/tasks.md") and get("specs/add-csv-export/requirements.md") and get("specs/add-csv-export/design.md"), 10)
    req = get("specs/add-csv-export/requirements.md").decode()
    run.check("…plan first: requirements, then design, then tasks with boxes", req.startswith("# Requirements: ") and get("specs/add-csv-export/design.md").startswith(b"# Design: ") and b"- [ ] " in get("specs/add-csv-export/tasks.md"), req[:80])
    run.check("the new spec appears in the panel with its starter tasks", bool(until(lambda: group(d, "specs/add-csv-export") and group(d, "specs/add-csv-export")["progress"] == "0/2", 8)), str(groups(d)))
    run.check("…and its requirements open in an editor", bool(until(lambda: d.js(f"const p = {S}.activeTab({S}.state.activeProject); return JSON.stringify(p).includes('specs/add-csv-export/requirements.md')"), 8)))
    h_before = {f: get(f"specs/add-csv-export/{f}") for f in ("requirements.md", "design.md", "tasks.md")}
    put("specs/add-csv-export/tasks.md", h_before["tasks.md"].replace(b"Describe the first task", b"My own first task"))
    h_before["tasks.md"] = get("specs/add-csv-export/tasks.md")
    d.js(f"{S}.setOverlay(null)")
    d.js("window.__kelpie.commands.find(c => c.id === 'spec.new').run()")
    until(lambda: d.js("return !!document.querySelector('.plan-new input')"), 8)
    type_name("add csv EXPORT")
    d.js("document.querySelector('.plan-new button[type=submit]').click()")
    until(lambda: "already exists" in toast(d), 6)
    run.check("the same name again is refused and says which file exists", "already exists" in toast(d) and "specs/add-csv-export/" in toast(d), toast(d))
    run.check("…and no byte of the existing files changed", all(get(f"specs/add-csv-export/{f}") == h_before[f] for f in h_before))
    type_name("login")
    d.js("document.querySelector('.plan-new button[type=submit]').click()")
    until(lambda: "specs/login/" in toast(d), 6)
    run.check("a name that is an existing spec's folder is refused, and the folder is as it was", "already exists" in toast(d) and sorted(os.listdir(os.path.join(REPO, "specs", "login"))) == ["design.md", "requirements.md", "tasks.md"] and get("specs/login/tasks.md") == LOGIN_TASKS, toast(d))
    run.check("the junction folder's target gained nothing", sorted(os.listdir(OUTSIDE)) == ["tasks.md"] and sorted(os.listdir(OUTSIDE + "-swap")) == ["tasks.md"])

    # ---------- 10. SSH hosts ----------
    run.check("the app talks to a daemon that knows hosts (API 14)", d.js(f"return {S}.daemonApi()") >= 14)
    saved = js_await(d, "(async () => { const h = window.__kelpie.sshHosts; const s = await h.saveHost(h.newHost({ label: 'Spec host', hostname: '127.0.0.1', port: arguments[0], user: 'tester', auth: 'key', identityFile: arguments[1], tmux: false })); const k = await h.hostKeys(s.alias); await h.trustHost(k.keys.map((x) => x.line)); return { id: s.id, alias: s.alias }; })()", HOST_PORT, KEY.replace("\\", "/"))
    alias, host_id = saved["alias"], saved["id"]
    HP = HOSTREPO_POSIX
    run.check("a saved host in the app's own ssh folder, its key trusted", bool(alias) and os.path.exists(os.path.join(SSH_DIR, "known_hosts")), str(saved))

    # A remote project: its folder is on the host.
    rpid = js_await(d, f"{S}.addProject(arguments[0], {{ name: 'host repo', remote: {{ dest: arguments[1], path: arguments[2], tmux: false, agents: ['claude', 'codex', 'omp'] }} }})", f"ssh://{alias}{HP}", alias, HP)
    show_panel(d, "plan")
    run.check("a remote project has the Plan tab, its specs read over ssh", bool(until(lambda: group(d, "plans/260101-hostplan") and group(d, "specs/hosted"), 90)), str(groups(d)))
    run.check("…with their progress", group(d, "plans/260101-hostplan")["progress"] == "1/3" and group(d, "specs/hosted")["progress"] == "0/1", str(groups(d)))
    run.check("…and it says which host it reads", d.js("return document.querySelector('.plan-head')?.textContent.includes(arguments[0])", alias))
    run.shot("08-remote-plan")
    hp1 = "plans/260101-hostplan/phase-01-remote.md"
    before_h = get(hp1, HOSTREPO)
    click_box(d, "remote task one")
    after_h = until(lambda: (lambda b: b if b != before_h else None)(get(hp1, HOSTREPO)), 40)
    run.check("ticking on the host changes the one byte, CRLF intact", after_h == before_h.replace(b"- [ ] remote task one", b"- [x] remote task one", 1), repr(after_h))
    run.check("…and the row shows it", bool(until(lambda: (row(d, "remote task one") or {}).get("checked"), 20)))
    run.check("…through a file beside it that replaced it: no temporary file is left on the host", not [f for f in os.listdir(os.path.dirname(os.path.join(HOSTREPO, hp1))) if "kelpie-tmp" in f])
    theirs_h = after_h.replace(b"- [ ] remote task two", b"- [ ] remote task two\r\n- [ ] written by someone else")
    js_await(d, "(async () => { const inv = window.__TAURI_INTERNALS__.invoke; await inv('fs_write_file', { root: arguments[0], path: arguments[1], content: arguments[2] }); [...document.querySelectorAll('.plan-task')].find(r => r.querySelector('.plan-text').textContent === 'remote task two').querySelector('.plan-box').click(); })()",
             f"ssh://{alias}{HP}", hp1, theirs_h.decode())
    time.sleep(3)
    run.check("a file changed on the host after it was read is refused there too", get(hp1, HOSTREPO) == theirs_h and "changed on disk" in toast(d), toast(d))
    run.check("…and the refused tick read the file again, so the other party's task shows", bool(until(lambda: row(d, "written by someone else"), 30)))
    put("specs/hosted/tasks.md", b"# Tasks\n\n- [ ] hosted task\n- [ ] unwatched task\n", root=HOSTREPO)
    time.sleep(3)
    run.check("nothing else is watched on a host: an edit shows only after Refresh", row(d, "unwatched task") is None)
    d.js("document.querySelector('.plan-head .icon-btn').click()")
    run.check("Refresh reads the host again", bool(until(lambda: row(d, "unwatched task"), 60)))

    # Links on the host (Git Bash makes them with its own emulation, which the test host's shell reads as symlinks).
    run.check("the plan does not list a spec file that is a link on the host", len([r for r in rows(d) if r["text"] == "hosted task"]) == 1 and not any((r["path"] or "").endswith("linked.md") for r in rows(d)), str([r["path"] for r in rows(d)]))
    root_h = f"ssh://{alias}{HP}"
    def save(path, content, **extra):
        return js_await(d, "window.__TAURI_INTERNALS__.invoke('fs_write_file', { root: arguments[0], path: arguments[1], content: arguments[2], ...arguments[3] })", root_h, path, content, extra)
    save("specs/hosted/linked.md", "- [ ] hosted task\n- [ ] saved through the link\n")
    run.check("a file saved over ssh that is a link inside the project: the link stays, its target is written", host_sh('[ -L "$1/specs/hosted/linked.md" ]', HP) and b"saved through the link" in get("specs/hosted/tasks.md", HOSTREPO))
    save("docs-link.md", "written through to the target\n")
    run.check("…also for a link that leads out of the project (the editor's save has never refused that), the link intact", host_sh('[ -L "$1/docs-link.md" ]', HP) and open(os.path.join(HOSTHOME, "outside-target.md")).read() == "written through to the target\n")
    try:
        save("docs-link.md", "no way\n", expectedCksum=cksum_of(b"written through to the target\n"), confine=True)
        refused_out = None
    except RuntimeError as e:
        refused_out = str(e)
    run.check("a conditional write from the plan is refused when the link leads out of the project, and nothing is written", "leads out of the project" in (refused_out or "") and open(os.path.join(HOSTHOME, "outside-target.md")).read() == "written through to the target\n", str(refused_out))
    try:
        save("specs/hosted/tasks.md", "stale\n", expectedCksum="1 1", confine=True)
        stale = "written"
    except RuntimeError as e:
        stale = str(e)
    run.check("a write whose checksum is not the file's is refused (no error: a conflict) and the file is as it was", b"saved through the link" in get("specs/hosted/tasks.md", HOSTREPO), stale)
    run.check("no temporary file is left in the folders written to", not [f for f in os.listdir(os.path.join(HOSTREPO, "specs", "hosted")) + os.listdir(HOSTREPO) if "kelpie-tmp" in f])
    put("specs/hosted/tasks.md", b"# Tasks\n\n- [ ] hosted task\n- [ ] unwatched task\n", root=HOSTREPO)

    # Run with… on a remote project: the agent runs on its host, in its folder.
    run_with(d, "hosted task", "claude", "here")
    offered = d.js("const [a, m] = document.querySelectorAll('.plan-runwith select'); return [[...a.options].map(o => o.value), [...m.options].map(o => o.textContent)]")
    run.check("a remote project offers its host and no worktree", offered[1] == [f"On {alias}"], str(offered))
    start_run(d)
    h1 = until(lambda: next(iter(runs_with(d, "hosted task")), None), 40)
    t1 = (h1 or {"tasks": [{}]})["tasks"][0]
    run.check("the run's task carries the host and the folder there, and is not in a worktree", bool(h1) and t1["host"]["dest"] == alias and t1["host"]["path"] == HP and t1["worktree"] is False and "tmux" not in t1["host"] and h1["project_id"] == rpid, str(t1))
    run.check("…it is given the saved hosts' ssh config", bool(h1) and str(t1["host"].get("config", "")).replace("\\", "/").endswith("/ssh/config"), str(t1.get("host")))
    run.check("…and its prompt points at the file as @path", "@specs/hosted/tasks.md" in t1.get("prompt", "") and 'Task (line 3): "hosted task"' in t1.get("prompt", ""), t1.get("prompt", ""))
    done_h = until(lambda: (lambda r: r if r["tasks"][0]["status"] in ("done", "failed") else None)(get_run(d, h1["id"])), 120)
    run.check("the agent on the host finished, and its session ending ended the task", bool(done_h) and done_h["tasks"][0]["status"] == "done", str(done_h and (done_h["tasks"][0]["status"], done_h["tasks"][0].get("error"), done_h["tasks"][0].get("summary"))))
    sess_h = next((s for s in daemon(d, "session.list") if s["id"] == (done_h or {"tasks": [{}]})["tasks"][0].get("session_id")), None)
    run.check("…it was a remote session on that host in that folder, not in tmux", bool(sess_h) and (sess_h.get("remote") or {}).get("dest") == alias and sess_h["cwd"] == HP and not (sess_h.get("remote") or {}).get("tmux"), str(sess_h and (sess_h.get("remote"), sess_h["cwd"])))
    started = json.load(open(os.path.join(CAPTURE, "host-start.json")))
    got = open(os.path.join(CAPTURE, "host-prompt.txt"), encoding="utf-8").read()
    run.check("the agent ran in the project's folder on the host and was given the task", os.path.samefile(started["cwd"], HOSTREPO) and "Do one task from a spec file" in got, str(started))
    run.check("…it was not told to run a report command of this computer", "run report" not in got and "kelpied" not in got, got[-300:])
    run.check("…and its edit is in the file on the host", b"- [x] hosted task" in get("specs/hosted/tasks.md", HOSTREPO))
    run.check("the row says Done", bool(until(lambda: (row(d, "hosted task") or {}).get("chip") == "Done", 20)), str(row(d, "hosted task")))
    d.js("document.querySelector('.plan-head .icon-btn').click()")
    until(lambda: group(d, "specs/hosted"), 30)
    open_group(d, "specs/hosted")  # all done now: it was closed
    run.check("…and after Refresh the box is ticked", bool(until(lambda: (row(d, "hosted task") or {}).get("checked"), 60)), str((row(d, "hosted task"), groups(d), d.js("return document.querySelector('.plan-list')?.innerText.slice(0, 600)"))))

    # New spec on a host.
    d.js("window.__kelpie.commands.find(c => c.id === 'spec.new').run()")
    until(lambda: d.js("return !!document.querySelector('.plan-new input')"), 8)
    d.js("const e = document.querySelector('.plan-new input'); e.value = 'Host spec'; e.dispatchEvent(new InputEvent('input', {bubbles: true}))")
    d.js("document.querySelector('.plan-new button[type=submit]').click()")
    made_h = until(lambda: all(os.path.exists(os.path.join(HOSTREPO, "specs", "host-spec", f)) for f in ("requirements.md", "design.md", "tasks.md")), 60)
    run.check("New spec writes its three files on the host", bool(made_h))
    run.check("…and the panel lists it", bool(until(lambda: group(d, "specs/host-spec"), 60)))

    # A local project with the host linked: Run with… on that host.
    d.js(f"{S}.switchProject(arguments[0])", pid)
    show_panel(d, "plan")
    until(lambda: row(d, "add rate limit"), 20)
    js_await(d, f"{S}.linkHost(arguments[0], {{ hostId: arguments[1], path: arguments[2] }})", pid, host_id, HP)
    tasks_before = get("specs/login/tasks.md", REPO)
    run_with(d, "add rate limit", "claude", "here")
    offered = d.js("const [a, m] = document.querySelectorAll('.plan-runwith select'); return [...m.options].map(o => [o.value, o.textContent])")
    run.check("a local project with a linked host offers here, a worktree and that host", [o[0] for o in offered] == ["here", "worktree", f"host:{host_id}"] and offered[2][1] == "On Spec host", str(offered))
    d.js("const [a, m] = document.querySelectorAll('.plan-runwith select'); m.value = arguments[0]; m.dispatchEvent(new Event('change', {bubbles: true}))", f"host:{host_id}")
    run.check("on a host the form says the agent reads its own copy of the file", bool(until(lambda: "its own copy" in (d.js("return [...document.querySelectorAll('.plan-runwith p')].map(p => p.textContent).join(' ')") or ""), 8)))
    notes = d.js("return [...document.querySelectorAll('.plan-runwith p')].map(p => p.textContent).join(' ')")
    run.check("…and that it runs in the ssh session, not in tmux, ending with the connection", "not in tmux" in notes and "connection drops" in notes, notes)
    run.shot("09-run-on-host")
    start_run(d)
    h2 = until(lambda: next((r for r in runs_with(d, "add rate limit") if r["tasks"][0].get("host")), None), 60)
    run.check("the task carries the linked host and its folder", bool(h2) and h2["tasks"][0]["host"]["dest"] == alias and h2["tasks"][0]["host"]["path"] == HP and h2["project_id"] == pid, str(h2 and h2["tasks"][0].get("host")))
    done2 = until(lambda: (lambda r: r if r["tasks"][0]["status"] in ("done", "failed") else None)(get_run(d, h2["id"])), 120)
    run.check("it ran there: the host's copy is ticked, this computer's file is as it was", bool(done2) and done2["tasks"][0]["status"] == "done" and b"- [x] add rate limit" in get("specs/login/tasks.md", HOSTREPO) and get("specs/login/tasks.md", REPO) == tasks_before, str(done2 and done2["tasks"][0].get("status")))
    until(lambda: (row(d, "add rate limit") or {}).get("chip") == "Done", 20)
    d.js("[...document.querySelectorAll('.plan-task')].find(r => r.querySelector('.plan-text').textContent === 'add rate limit').querySelector('.plan-run').click()")
    run.check("the Runs page says where it ran", bool(until(lambda: "on " + alias in (d.js("return document.querySelector('.runs-page')?.innerText ?? ''") or ""), 10)), d.js("return document.querySelector('.runs-page')?.innerText ?? ''"))
    d.js(f"{S}.setOverlay(null)")

    # What the daemon accepts: the user's app may name a host; a program in a session may not; bad hosts and a worktree on a host are refused.
    def refused(**task):
        try:
            daemon(d, "run.create", {"name": "probe", "cwd": REPO, "tasks": [{"id": "t1", "title": "t", "prompt": "p", "kind": "custom", "args": [sys.executable, "-c", "pass"], **task}]})
            return None
        except RuntimeError as e:
            return str(e)
    run.check("a worktree on a host is refused, saying why", "cannot have one of its own" in (refused(worktree=True, host={"dest": alias, "path": HP}) or ""))
    run.check("a destination that is an ssh option, or has spaces, is refused", all("invalid ssh destination" in (refused(host={"dest": x, "path": HP}) or "") for x in ("-oProxyCommand=calc", "a b", "x;y")))
    run.check("a relative folder and port 0 are refused", "absolute folder" in (refused(host={"dest": alias, "path": "rel"}) or "") and "port" in (refused(host={"dest": alias, "port": 0, "path": HP}) or ""))
    run.check("nothing was created by the refused requests", not runs_with(d, "probe"))
    attempt = until(lambda: os.path.exists(os.path.join(CAPTURE, "claude-hostattempt.json")) and json.load(open(os.path.join(CAPTURE, "claude-hostattempt.json"))), 5)
    run.check("a program in a session that asks for a task on a host is refused by the daemon", bool(attempt) and "cannot start a task on an SSH host" in (attempt[1] + attempt[2]), str(attempt))
    run.check("…and no run came of it", not runs_with(d, "sneaky"))
    shapes = [until(lambda n=n: os.path.exists(os.path.join(CAPTURE, f"claude-shape{n}.json")) and json.load(open(os.path.join(CAPTURE, f"claude-shape{n}.json"))), 5) for n in range(3)]
    run.check("…and so are tasks sent as an array, a string or a map: none of those shapes gets past the scoping", all(x and ("must be an object" in x[1] + x[2] or "must be a list" in x[1] + x[2]) for x in shapes), str(shapes))
    run.check("…and none of them created a run", not runs_with(d, "sneaky-shape"), str([r["name"] for r in daemon(d, "run.list")]))

    # A task on a host whose connection fails: failed, with the reason; a retry asks first (the agent may still be running there).
    d.js("window.__confirms = []; window.confirm = (m) => { window.__confirms.push(m); return true; }")
    dead = daemon(d, "run.create", {"name": "dead host", "cwd": REPO, "tasks": [{"id": "t1", "title": "dead host task", "prompt": "p", "kind": "custom", "args": [sys.executable, "-c", "pass"], "host": {"dest": "127.0.0.1", "port": 1, "path": HP}}]})
    failed = until(lambda: (lambda r: r if r["tasks"][0]["status"] == "failed" else None)(get_run(d, dead["id"])), 60)
    run.check("a task on a host whose connection fails is failed, saying the agent stopped with the connection", bool(failed) and "stopped with the connection" in (failed["tasks"][0].get("error") or ""), str(failed and failed["tasks"][0].get("error")))
    d.js(f"{S}.setOverlay('runs')")
    d.wait("return !!document.querySelector('.runs-page')", 10)
    d.js("document.querySelector(`.runs-item[data-run='${arguments[0]}']`).click()", dead["id"])
    until(lambda: d.js("return [...document.querySelectorAll('.run-task .btn')].some(b => b.textContent.includes('Retry'))"), 10)
    d.js("[...document.querySelectorAll('.run-task .btn')].find(b => b.textContent.includes('Retry')).click()")
    asked = until(lambda: d.js("return window.__confirms.filter(m => m.includes('may still be running')).length"), 10)
    run.check("Retry of a task on a host asks first: the previous agent may still be running there", bool(asked), str(d.js("return window.__confirms")))
    run.check("…and starts it again once confirmed", bool(until(lambda: get_run(d, dead["id"])["tasks"][0]["started_at"] > failed["tasks"][0]["started_at"], 30)))
    daemon(d, "run.cancel", {"id": dead["id"]})
    daemon(d, "run.delete", {"id": dead["id"]})
    d.js(f"{S}.setOverlay(null)")

    # ---------- 9a. big files are parsed in a worker, with the whole-file answer ----------
    same = js_await(d, """(async () => {
      const si = window.__kelpie.specIndex, sp = window.__kelpie.specParse;
      const pad = 'filler words '.repeat(900) + '\\n\\n';
      const cases = {
        comment: '# P\\n\\n<!--\\n- [ ] commented out\\n- [x] also\\n-->\\n\\n- [ ] real\\n',
        details: '# P\\n\\n<details>\\n<summary>More</summary>\\n\\n- [ ] inside details\\n\\n</details>\\n\\n<details>\\n- [ ] raw html line\\n</details>\\n\\n- [ ] after\\n',
        nestedFence: '# P\\n\\n````md\\n```\\n- [ ] in the example\\n```\\n- [ ] still the example\\n````\\n\\n- [ ] real\\n',
        refLinks: '# P\\n\\n- [ ] read [the guide][g] first\\n- [ ] see [other]\\n\\n[g]: http://example.test/guide\\n[other]: http://example.test/o\\n',
        crlf: '# P\\r\\n\\r\\n- [ ] one\\r\\n  - [x] two\\r\\n',
      };
      const out = {};
      for (const [name, text] of Object.entries(cases)) {
        const big = pad + text;
        const viaWorker = await sp.parseSpecAsync('specs/a/tasks.md', big, 5, big.length);
        out[name] = [JSON.stringify(viaWorker) === JSON.stringify(si.parseSpec('specs/a/tasks.md', big, 5, big.length)), viaWorker.tasks.map((t) => t.text)];
      }
      return out;
    })()""")
    run.check("big files parsed in the worker give exactly the main thread's whole-file answer: comments, <details>, nested fences, reference links, CRLF",
              all(v[0] for v in same.values()) and same["comment"][1] == ["real"] and same["details"][1] == ["inside details", "after"] and same["nestedFence"][1] == ["real"] and same["refLinks"][1] == ["read the guide first", "see other"], str(same))
    lag = js_await(d, """(async () => {
      let text = '# T\\n\\n## S\\n'; for (let i = 0; text.length < 256 * 1024; i++) text += '- [ ] task number ' + i + ' does some work on the thing\\n';
      let last = performance.now(), worst = 0; const iv = setInterval(() => { const n = performance.now(); worst = Math.max(worst, n - last); last = n; }, 4);
      const t0 = performance.now(); const doc = await window.__kelpie.specParse.parseSpecAsync('specs/a/tasks.md', text, 1);
      const took = performance.now() - t0; clearInterval(iv);
      return { total: doc.total, worst, took };
    })()""")
    run.check(f"a 256 KB file ({lag['total']} tasks) took {lag['took']:.0f} ms to parse and the page was never held up more than {lag['worst']:.0f} ms (<= 50)", lag["total"] > 4000 and lag["worst"] <= 50, str(lag))

    # ---------- 9. speed in the app, then nothing left running ----------
    timing = js_await(d, """(async () => {
      const text = arguments[0]; const si = window.__kelpie.specIndex;
      const files = Array.from({length: 24}, (_, i) => text.replace('Phase 1', 'Phase ' + i));
      const once = () => { const t0 = performance.now(); files.forEach((f, i) => si.parseSpec('plans/p/phase-' + i + '.md', f, 1)); return performance.now() - t0; };
      const cold = once(); const warm = Array.from({length: 7}, once).sort((a, b) => a - b);
      return {cold, median: warm[3], min: warm[0], bytes: files.reduce((n, f) => n + f.length, 0)};
    })()""", (PHASE1.decode() * 6) + PHASE3.decode() * 6)
    run.check(f"parsing 24 plan files ({timing['bytes'] // 1024} KB) in the app: best of 7 {timing['min']:.1f} ms (<= 20), median {timing['median']:.1f}, first run {timing['cold']:.1f}", timing["min"] <= 20, str(timing))
    show_panel(d, "files")
    time.sleep(0.5)
    closed = stats(d)
    put("specs/login/tasks.md", LOGIN_TASKS + b"- [ ] after closing\n")
    put("plans/260101-alpha/phase-03-worktree.md", PHASE3 + b"- [ ] after closing too\n")
    time.sleep(1.6)
    run.check("with the tab closed again, edits cost nothing", stats(d) == closed, f"{closed} {stats(d)}")
    run.check("the tab's DOM is gone", not d.js("return !!document.querySelector('.plan-panel')"))
    show_panel(d, "plan")
    run.check("reopening shows what changed meanwhile", bool(until(lambda: row(d, "after closing"), 10)))
    d.js(f"document.documentElement.dataset.theme = 'light'")
    time.sleep(0.4)
    run.shot("07-light")
    d.js(f"document.documentElement.dataset.theme = 'dark'")


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def main():
    seed_fixture()
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


def unlink_junctions():
    """Junctions and links first: removing a tree must never walk into the folders they lead to."""
    for p in (os.path.join(REPO, "specs", "escape"), os.path.join(REPO, "specs", "login")):
        if os.path.islink(p) or os.path.isjunction(p):
            os.rmdir(p) if os.path.isdir(p) and os.name == "nt" else os.unlink(p)


def remove_tree(path):
    """Git leaves read-only object files; the daemon and app may still be letting go of the folder."""
    def writable(func, p, _):
        os.chmod(p, stat.S_IWRITE)
        func(p)
    for _ in range(10):
        if not os.path.lexists(path):
            return
        try:
            shutil.rmtree(path, onexc=writable)
        except OSError:
            time.sleep(0.5)


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
    unlink_junctions()
    for base in (DATA, WORK, WORK + "-worktrees", OUTSIDE + "-swap"):
        remove_tree(base)


if __name__ == "__main__":
    main()
