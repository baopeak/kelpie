"""GUI e2e for cross-review and best-of-N: review notes in the daemon, "Review with…", "Try with N agents",
run worktrees that bring `.kelpie.yaml` links, and the compare view.

    python -X utf8 scripts/e2e/best_of_n.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie is never reached. Agents are stand-ins (`claude`,
`codex`, `gemini` and `omp` shims on PATH running fake_agent below): they edit files in their worktree and
report with `kelpied run report`; the reviewer reads what it is typed and leaves notes with
`kelpied review add`. Every repository is a fixture under the scratch folder.

1. Review notes the app kept in its own state (written before the app started) move to the daemon with their
   ids and times, after a backup of the app's state.
2. "Try with several agents" before the repository's `.kelpie.yaml` is approved: worktrees start clean and
   the dialog says why. Discard removes one candidate with its worktree and branch.
3. After approval the dialog shows what each worktree gets (links, copies, size). Five worktrees with links
   are created in parallel within 3 s.
4. Three agents (claude, codex, gemini), one prompt: a Run of three independent worktree tasks, each with
   `deps: []` and no group; every worktree has the approved link and copy.
5. Compare: status, time, cost, files, +/-, checks, turns; best marks; two candidates' versions of a file
   side by side.
6. Review with… on one candidate: another vendor starts in the same worktree with the editable prompt and
   the redacted diff; its `kelpied review add` notes show in the diff gutter and in the store; it cannot
   write notes in another worktree.
7. Use this one: confirmation listing what goes, Finish, the others removed (worktrees, branches, sessions).
8. No idle cost afterwards.
"""
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

ENDPOINT = f"kelpie-bon-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-bon-data-")
WORK = os.path.join(scratch, f"bon-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
CAPTURE = os.path.join(WORK, "capture")
REPO = os.path.join(WORK, "repo")
for d_ in (BIN, CAPTURE, REPO):
    os.makedirs(d_, exist_ok=True)

TOKEN = "ghp_" + "abcdefghijklmnopqrstuvwxyz0123456789"  # planted where a diff would carry it
SEED_PROJECT = "seed-project"

# Stand-in agent. claude / codex / gemini do a different piece of work and report; omp is the reviewer: it
# keeps what it is typed, then leaves notes (and tries a worktree that is not its own).
FAKE = os.path.join(WORK, "fake_agent.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(r'''
import ctypes, os, subprocess, sys, threading, time
KELPIED = os.environ["KELPIE_E2E_KELPIED"]
CAPTURE = os.environ["KELPIE_E2E_CAPTURE"]
kind = sys.argv[1]
print("fake agent ready", flush=True)


def sh(*argv):
    return subprocess.run(list(argv), capture_output=True, text=True, encoding="utf-8", errors="replace")


def write(path, text):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    open(path, "w", encoding="utf-8", newline="").write(text)


def git(*args):
    sh("git", "-c", "user.name=agent", "-c", "user.email=agent@test", *args)


def work(seconds):
    end = time.time() + seconds
    while time.time() < end:
        print("working", flush=True)
        time.sleep(0.25)


if kind in ("claude", "codex", "gemini"):
    work({"claude": 1, "codex": 3, "gemini": 2}[kind])
    if kind == "claude":
        write("app.txt", "one\nCLAUDE two\nthree\n")
        write("greeting.txt", "hello from claude\n")
    elif kind == "codex":
        write("app.txt", "one\nCODEX two\nthree\nfour\n")
        write("greeting.txt", "hello from codex\n")
        write("notes.txt", "codex notes\n")
        git("add", "-A")
        git("commit", "-qm", "codex: greeting")
        write("scratch.txt", "left uncommitted\n")
    else:
        write("app.txt", "one\nGEMINI two\nthree\n" + "".join(f"filler line {i} of the gemini rewrite\n" for i in range(260)))
        write("settings.cfg", "mode=fast\nGITHUB_TOKEN = " + os.environ["KELPIE_E2E_TOKEN"] + "\n")
        write("greeting.txt", "hello from gemini\n")
    sh(KELPIED, "run", "report", "done", f"{kind} finished")
    print("done", flush=True)
    time.sleep(3600)
elif kind == "omp":
    # Raw, virtual-terminal input: what the terminal sends arrives byte for byte (a bracketed paste included).
    k32 = ctypes.windll.kernel32
    handle, mode = k32.GetStdHandle(-10), ctypes.c_uint32()
    k32.GetConsoleMode(handle, ctypes.byref(mode))
    k32.SetConsoleMode(handle, (mode.value & ~0x7) | 0x200)
    buf, last = [], [0.0]

    def reader():
        while True:
            buf.append(os.read(0, 65536))
            last[0] = time.time()

    threading.Thread(target=reader, daemon=True).start()
    while not buf or time.time() - last[0] < 2.5:
        time.sleep(0.2)
    text = b"".join(buf).decode("utf-8", "replace")
    write(os.path.join(CAPTURE, "review-input.txt"), text)
    out = []
    for target, note in (("settings.cfg:2", "never commit a token"), ("app.txt:2", "why capitals here?")):
        r = sh(KELPIED, "review", "add", "--base", "main", target, note)
        out.append((r.returncode, r.stdout.strip(), r.stderr.strip()))
    other = open(os.path.join(CAPTURE, "other-worktree.txt"), encoding="utf-8").read().strip()
    r = sh(KELPIED, "call", "note.add", '{"path": %s, "file": "app.txt", "line": 2, "note": "sneaky"}' % __import__("json").dumps(other))
    out.append((r.returncode, r.stdout.strip(), r.stderr.strip()))
    write(os.path.join(CAPTURE, "review-results.json"), __import__("json").dumps(out))
    print("reviewed", flush=True)
    time.sleep(3600)
else:
    time.sleep(3600)
''')
for name in ("claude", "codex", "gemini", "omp"):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" {name} %*\r\n')
# `chk <file>…` passes when every file is in the checked-out tree (the check Finish runs on the merged result).
CHK = os.path.join(WORK, "chk.py")
with open(CHK, "w", encoding="utf-8") as f:
    f.write("import os, sys\nsys.exit(0 if all(os.path.exists(a) for a in sys.argv[1:]) else 1)\n")
with open(os.path.join(BIN, "chk.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{CHK}" %*\r\n')

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "KELPIE_E2E_KELPIED": kelpied, "KELPIE_E2E_CAPTURE": CAPTURE,
                   "KELPIE_E2E_TOKEN": TOKEN, "PATH": BIN + os.pathsep + os.environ.get("PATH", "")})
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
R = "window.__kelpie.review"


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
        time.sleep(0.25)
    return None


def read(path):
    try:
        with open(path, encoding="utf-8", newline="") as f:
            return f.read().replace("\r\n", "\n")
    except OSError:
        return None


def write(path, text):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def get_run(d, rid):
    return daemon(d, "run.get", {"id": rid})


def tasks_by_id(r):
    return {t["id"]: t for t in r["tasks"]}


def type_into(d, selector, text):
    d.js("const e = document.querySelector(arguments[0]); e.value = arguments[1]; e.dispatchEvent(new InputEvent('input', {bubbles: true}))", selector, text)


def click_text(d, scope, text):
    return d.js("const b = [...document.querySelectorAll(arguments[0])].find(b => b.textContent.trim() === arguments[1] || b.textContent.trim().startsWith(arguments[1])); if (b) b.click(); return !!b", scope, text)


def dialog_text(d, selector):
    return d.js("return document.querySelector(arguments[0])?.innerText.replace(/\\s+/g, ' ') || ''", selector)


def project_at(d, path):
    return d.js(f"const n = p => p.replace(/\\\\/g, '/').toLowerCase(); return {S}.state.projects.find(p => n(p.path) === n(arguments[0]))?.id ?? null", path)


def cell(d, cand, metric):
    return d.js("return document.querySelector(`.compare-cell[data-cand='${arguments[0]}'][data-metric='${arguments[1]}']`)?.textContent.trim() ?? null", cand, metric)


def open_try(d):
    d.js(f"{S}.switchProject(arguments[0])", SEED_PROJECT)
    d.js("window.__kelpie.commands.find(c => c.id === 'runs.try').run()")
    d.wait("return !!document.querySelector('.try-dialog')", 15)
    until(lambda: "Measuring" not in dialog_text(d, ".try-dialog [data-bringing]") and dialog_text(d, ".try-dialog [data-bringing]"), 20)


def set_slot(d, i, kind):
    d.js("const s = document.querySelectorAll('.try-slot select')[arguments[0]]; s.value = arguments[1]; s.dispatchEvent(new Event('change', {bubbles: true}))", i, kind)


def slot_kinds(d):
    return d.js("return [...document.querySelectorAll('.try-slot select')].map(s => s.value)")


def start_try(d, prompt, kinds):
    type_into(d, ".try-prompt", prompt)
    while len(slot_kinds(d)) > len(kinds):
        d.js("document.querySelector('.try-slot .icon-btn').click()")
    while len(slot_kinds(d)) < len(kinds):
        click_text(d, ".try-dialog .btn", "Add agent")
    for i, k in enumerate(kinds):
        set_slot(d, i, k)
    d.js("[...document.querySelectorAll('.try-dialog .footer .btn.primary')].find(b => b.textContent.startsWith('Start')).click()")


def current_run(d, name_part):
    return until(lambda: next((r for r in daemon(d, "run.list") if name_part in r["name"]), None), 20)


def checkbox(d, label):
    return d.js("const l = [...document.querySelectorAll('.finish-dialog label')].find(l => l.textContent.trim().startsWith(arguments[0])); const i = l && l.querySelector('input'); return i ? i.checked : null", label)


def set_checkbox(d, label, on):
    d.js("const l = [...document.querySelectorAll('.finish-dialog label')].find(l => l.textContent.trim().startsWith(arguments[0])); const i = l.querySelector('input'); if (i.checked !== arguments[1]) i.click()", label, on)


def finish_enabled(d):
    return d.js("const b = [...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish'); return b ? !b.disabled : null")


def daemon_cpu_ms(pid):
    out = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Process -Id {pid}).TotalProcessorTime.TotalMilliseconds"], capture_output=True, text=True).stdout.strip()
    return float(out)


def seed_fixture():
    git("init", "-q", "-b", "main")
    write(os.path.join(REPO, ".gitignore"), "deps/\n.env\n")
    write(os.path.join(REPO, ".kelpie.yaml"), "link:\n  - deps\ncopy:\n  - .env\ncheck:\n  - chk greeting.txt\n")
    write(os.path.join(REPO, "app.txt"), "one\ntwo\nthree\n")
    write(os.path.join(REPO, "settings.cfg"), "mode=slow\nlog=info\n")
    write(os.path.join(REPO, "README.md"), "fixture\n")
    git("add", "-A")
    git("commit", "-qm", "init")
    write(os.path.join(REPO, ".env"), "KEY=1\n")
    write(os.path.join(REPO, "deps", "pkg", "a.js"), "module.exports = 1\n")
    for i in range(200):
        write(os.path.join(REPO, "deps", "pkg", f"f{i}.js"), "x\n")
    # What an older app kept in its own state: notes of the repository's project.
    notes = [
        {"id": "old-1", "path": "app.txt", "line": 2, "side": "new", "text": "two", "note": "from the app state", "at": 1700000000000},
        {"id": "old-2", "path": "README.md", "line": 1, "side": "new", "text": "fixture", "note": "second old note", "at": 1700000001000},
    ]
    state = {
        "version": 3, "projects": [{"id": SEED_PROJECT, "name": "repo", "path": REPO.replace("\\", "/"), "color": "#4f8cff"}], "activeProject": SEED_PROJECT, "workspaces": {},
        "settings": {}, "recent": [], "collapsed": {}, "keymap": {}, "approvedSetups": {}, "hibernated": [], "inbox": [],
        "reviewNotes": {SEED_PROJECT: notes}, "floating": {"sessions": [], "active": None, "open": False},
    }
    write(os.path.join(DATA, "app-state.json"), json.dumps(state))
    return notes


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 13, 30)
    run.check("the app talks to a daemon with review notes (API 13)", d.js(f"return {S}.daemonApi()") >= 13)
    d.js("window.confirm = (m) => { (window.__confirms ||= []).push(m); return true; }")
    pid = SEED_PROJECT
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.branch", pid) == "main", 20)

    # ---------- 1. the app's own notes move to the daemon ----------
    old = [n for n in json.loads(read(os.path.join(DATA, "seed-notes.json")))]
    moved = until(lambda: len(daemon(d, "note.list", {"path": REPO})) == 2, 30)
    run.check("the notes kept in the app's state reached the daemon", bool(moved))
    got = {n["id"]: n for n in daemon(d, "note.list", {"path": REPO})}
    run.check("…with their ids, times, lines and text", all(got.get(o["id"], {}).get(k) == o[k] for o in old for k in ("at", "line", "side", "text", "note", "path")), str(got))
    run.check("…and none was lost or doubled", sorted(got) == ["old-1", "old-2"])
    run.check("a backup of the app's state was made first", any(re.match(r"app-state\.notes-\d+\.bak\.json$", f) for f in os.listdir(DATA)), str(os.listdir(DATA)))
    run.check("the app no longer keeps them in its own state", until(lambda: not d.js(f"return Object.keys({S}.state.reviewNotes || {{}}).length"), 10))
    run.check("the app reads them from the daemon", sorted(n["id"] for n in d.js(f"return {S}.notesFor(arguments[0])", pid)) == ["old-1", "old-2"])
    daemon(d, "note.clear", {"path": REPO})
    run.check("note.clear empties the worktree's notes, and the app follows the event", bool(until(lambda: d.js(f"return {S}.notesFor(arguments[0]).length", pid) == 0, 10)))

    # ---------- 2. Try with several agents, before .kelpie.yaml is approved ----------
    open_try(d)
    text = dialog_text(d, ".try-dialog [data-bringing]")
    run.check("the Try dialog says the repository's .kelpie.yaml has not been approved", "has not been approved" in text, text)
    run.check("…three agents are suggested, the first two being claude and codex", len(slot_kinds(d)) == 3 and slot_kinds(d)[:2] == ["claude", "codex"], str(slot_kinds(d)))
    run.check("…and Start is off without a prompt", d.js("return [...document.querySelectorAll('.try-dialog .footer .btn.primary')].find(b => b.textContent.startsWith('Start')).disabled"))
    run.shot("01-try-unapproved")
    start_try(d, "Draft a greeting (unapproved)", ["claude", "codex"])
    r0 = current_run(d, "draft a greeting")
    run.check("Start made a Run of two independent worktree tasks", bool(r0) and len(r0["tasks"]) == 2 and all(t["worktree"] and t["deps"] == [] and not t["approve"] for t in r0["tasks"]), str(r0 and [(t["id"], t["worktree"], t["deps"]) for t in r0["tasks"]]))
    run.check("…with no group field, and the kinds that were chosen", bool(r0) and "group" not in r0 and sorted(t["kind"] for t in r0["tasks"]) == ["claude", "codex"])
    run.check("…and it opens in the Runs page", bool(until(lambda: d.js("return !!document.querySelector('.runs-page .run-lane')"), 15)))
    r0 = until(lambda: (lambda x: x if all(t["status"] == "done" for t in x["tasks"]) else None)(get_run(d, r0["id"])), 90)
    run.check("both agents finished and reported", bool(r0))
    t0 = list(r0["tasks"]) if r0 else []
    run.check("an unapproved .kelpie.yaml brings nothing: no link, no copy in the worktrees", bool(t0) and all(not os.path.exists(os.path.join(t["cwd"], "deps")) and not os.path.exists(os.path.join(t["cwd"], ".env")) for t in t0))
    run.check("…nothing is sent for it, not even .worktreeinclude entries (the run has no materialize)", bool(r0) and r0.get("materialize") is None and all(not t.get("brought") for t in t0), str(r0 and r0.get("materialize")))
    run.check("each worktree records the branch it was created from", bool(t0) and all(t.get("base") == "main" for t in t0), str([t.get("base") for t in t0]))
    pids0 = [until(lambda t=t: project_at(d, t["cwd"]), 20) for t in t0]
    run.check("the worktrees are in the sidebar under the repository with that base", all(pids0) and all(d.js(f"const p = {S}.state.projects.find(p => p.id === arguments[0]); return p.parentId === arguments[1] && p.baseBranch === 'main'", p, pid) for p in pids0))
    # Compare, then discard one candidate.
    click_text(d, ".runs-page .run-detail-head .toggle", "Compare")
    d.wait("return document.querySelectorAll('.compare-head').length === 2", 20)
    until(lambda: d.js("return [...document.querySelectorAll('.compare-actions [data-use]')].every(b => !b.disabled)"), 30)
    run.check("Compare lists the two candidates and enables Use this one for both", d.js("return [...document.querySelectorAll('.compare-actions [data-use]')].every(b => !b.disabled)"))
    gone_id = "codex"
    gone = tasks_by_id(r0)[gone_id]
    d.js("window.__confirms = []")
    d.js("document.querySelector(`.compare-actions[data-cand='codex'] [data-discard]`).click()")
    run.check("Discard asks first, naming the worktree's branch", bool(until(lambda: d.js("return (window.__confirms || []).length") == 1, 10)) and gone["branch"] in d.js("return window.__confirms[0]"), str(d.js("return window.__confirms")))
    run.check("…and removes the worktree and the sidebar entry", bool(until(lambda: not os.path.exists(gone["cwd"]) and not project_at(d, gone["cwd"]), 60)), git("worktree", "list"))
    run.check("…but keeps the unmerged branch, with the agent's commit and what it had not committed saved in a WIP commit on top",
              git("branch", "--list", gone["branch"]) != "" and "codex: greeting" in git("log", "--format=%s", gone["branch"]) and "WIP from" in git("log", "-1", "--format=%s", gone["branch"]), git("log", "--oneline", "-3", gone["branch"]))
    run.check("…and tells what became of it", any("kept with its work" in (i or "") for i in d.js(f"return {S}.state.inbox.map(i => i.body)")))
    run.check("…while the other candidate stays", os.path.isdir(tasks_by_id(r0)["claude"]["cwd"]))
    run.shot("02-after-discard")
    d.js(f"{S}.setOverlay(null)")

    # ---------- 3. approve, then five worktrees with links at once ----------
    js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'HEAD'); {S}.approveSetup(r.path, cfg.setupKey); }})()", pid)
    open_try(d)
    text = dialog_text(d, ".try-dialog [data-bringing]")
    run.check("approved, the dialog shows what each worktree gets", "links deps" in text and "copies .env" in text, text)
    run.check("…with the size of the copies for the agents chosen", re.search(r"\d+ B in 1 entries", text) is not None and "worktrees:" in text, text)
    run.shot("03-try-approved")
    d.js(f"{S}.setOverlay(null)")
    five = {"name": "Five at once", "objective": "", "cwd": REPO, "idle_done_secs": 0, "materialize": {"copy": [".env"], "link": ["deps"]},
            "tasks": [{"id": f"w{i}", "title": f"Worker {i}", "prompt": "idle", "kind": "custom", "args": [sys.executable, "-c", "import time; time.sleep(600)"], "worktree": True} for i in range(5)]}
    t_start = time.time()
    r5 = daemon(d, "run.create", five)
    # Where the daemon puts them: <repo>-worktrees/<run name>-<run id>-<task>.
    folders = [os.path.join(REPO + "-worktrees", f"five-at-once-{r5['id'][:6]}-w{i}") for i in range(5)]
    has = lambda f: os.path.isfile(os.path.join(f, "deps", "pkg", "f199.js")) and os.path.isfile(os.path.join(f, ".env"))  # noqa: E731
    took = None
    while time.time() - t_start < 20:
        if all(has(f) for f in folders):
            took = time.time() - t_start
            break
        time.sleep(0.02)
    ready = until(lambda: (lambda r: r if all(t["cwd"] and t["status"] == "running" for t in r["tasks"]) else None)(get_run(d, r5["id"])), 20)
    started_all = time.time() - t_start
    run.check(f"five worktrees with a link and a copy each exist {took or 0:.2f} s after the run was created (<= 3 s; their agents were all running after {started_all:.2f} s)", took is not None and took <= 3.0, f"{took}")
    run.check("…each link resolves to the repository's own folder", bool(ready) and all(os.path.isfile(os.path.join(t["cwd"], "deps", "pkg", "a.js")) for t in ready["tasks"]))
    brought = [t.get("brought") for t in ready["tasks"]] if ready else []
    run.check("…and the tasks say what they were given", bool(brought) and all(b and b["linked"] == ["deps"] and b["copied"] == [".env"] for b in brought), str(brought[:1]))
    daemon(d, "run.cancel", {"id": r5["id"]})
    run.check("cancelling stops their agents", bool(until(lambda: all(not (next((s for s in daemon(d, "session.list") if s["id"] == t["session_id"]), {}) or {}).get("alive") for t in get_run(d, r5["id"])["tasks"]), 20)))

    # ---------- 4. three agents, one prompt ----------
    open_try(d)
    slots = slot_kinds(d)
    start_try(d, "Add a greeting file and say hello", ["claude", "codex", "gemini"])
    r1 = current_run(d, "add a greeting file")
    started = time.time()
    run.check("three agents of three vendors became a Run of three independent worktree tasks", bool(r1) and sorted(t["kind"] for t in r1["tasks"]) == ["claude", "codex", "gemini"] and all(t["worktree"] and t["deps"] == [] for t in r1["tasks"]), str(slots))
    run.check("…the approved copies and links go with the run", bool(r1) and r1.get("materialize") == {"copy": [".env"], "link": ["deps"]}, str(r1 and r1.get("materialize")))
    r1 = until(lambda: (lambda x: x if all(t["status"] == "done" for t in x["tasks"]) else None)(get_run(d, r1["id"])), 120)
    run.check("all three finished and reported", bool(r1))
    T = tasks_by_id(r1) if r1 else {}
    run.check("every worktree has the approved link and the copied .env", bool(T) and all(os.path.isfile(os.path.join(t["cwd"], "deps", "pkg", "a.js")) and read(os.path.join(t["cwd"], ".env")) == "KEY=1\n" for t in T.values()))
    run.check("…the daemon recorded it per task", bool(T) and all((t.get("brought") or {}).get("linked") == ["deps"] for t in T.values()))
    run.check("…and the folders are real worktrees of the repository on their own branches", bool(T) and all(git("rev-parse", "--abbrev-ref", "HEAD", cwd=t["cwd"]) == t["branch"] for t in T.values()))
    pids = {k: until(lambda t=t: project_at(d, t["cwd"]), 30) for k, t in T.items()}
    run.check("…and each is a project in the sidebar", all(pids.values()), str(pids))
    gem = T["gemini"]

    # ---------- 5. compare ----------
    d.js(f"{S}.setOverlay('runs')")
    d.js("document.querySelector(`.runs-item[data-run='${arguments[0]}']`).click()", r1["id"])
    d.wait("return !!document.querySelector('.runs-page .run-detail-head .toggle')", 15)
    click_text(d, ".runs-page .run-detail-head .toggle", "Compare")
    d.wait("return document.querySelectorAll('.compare-head').length === 3", 20)
    until(lambda: all(cell(d, k, "files") not in (None, "–") for k in T), 40)
    heads = d.js("return [...document.querySelectorAll('.compare-head')].map(h => h.dataset.cand)")
    run.check("Compare has a column per candidate", sorted(heads) == ["claude", "codex", "gemini"], str(heads))
    run.check("…each says it is done", all("Done" in d.js("return document.querySelector(`.compare-head[data-cand='${arguments[0]}'] .run-status`).textContent", k) for k in T))
    times = {k: cell(d, k, "duration") for k in T}
    run.check("…how long each took", all(re.match(r"\d+s|\d+m", v or "") for v in times.values()), str(times))
    run.check("…the files each changed (claude 2, codex 4, gemini 3)", (cell(d, "claude", "files"), cell(d, "codex", "files"), cell(d, "gemini", "files")) == ("2", "4", "3"), str([cell(d, k, "files") for k in T]))
    lines = {k: cell(d, k, "lines") for k in T}
    run.check("…and +/- lines", all(re.match(r"\+\d+ −\d+", v or "") for v in lines.values()), str(lines))
    checks = {k: cell(d, k, "checks") for k in T}
    run.check("…the base's checks as Finish would run them", all("check" in (v or "") for v in checks.values()), str(checks))
    run.check("…turns and cost (none to read from a stand-in agent)", all(cell(d, k, "turns") is not None and cell(d, k, "cost") is not None for k in T))
    stars = d.js("return [...document.querySelectorAll('.compare-cell.best')].map(c => c.dataset.metric + ':' + c.dataset.cand)")
    sizes = {k: sum(int(x) for x in re.findall(r"\d+", v)) for k, v in lines.items()}
    smallest = min(sizes, key=sizes.get)
    run.check("the fastest candidate is marked", "duration:claude" in stars, str(stars))
    run.check("…and the one with the fewest changed lines", f"lines:{smallest}" in stars and list(sizes.values()).count(sizes[smallest]) == 1, f"{stars} {sizes}")
    run.shot("04-compare")
    d.js("document.querySelector(`.compare-file[data-file='app.txt']`).click()")
    d.wait("return !!document.querySelector('[data-compare-diff] .cm-mergeView')", 20)
    both = until(lambda: (lambda t: t if "CLAUDE two" in t and "CODEX two" in t else None)(d.js("return document.querySelector('[data-compare-diff]').innerText")), 20)
    run.check("a file opens as two candidates' versions side by side", bool(both), str(d.js("return document.querySelector('[data-compare-diff]')?.innerText.slice(0, 200)")))
    d.js("const s = document.querySelectorAll('.compare-pick select'); s[1].value = 'gemini'; s[1].dispatchEvent(new Event('change', {bubbles: true}))")
    both = until(lambda: (lambda t: t if "CLAUDE two" in t and "GEMINI two" in t else None)(d.js("return document.querySelector('[data-compare-diff]').innerText")), 20)
    run.check("…and any two can be compared", bool(both))
    run.shot("05-compare-diff")
    d.js(f"{S}.setOverlay(null)")

    # ---------- 6. Review with… on gemini's worktree ----------
    gp = pids["gemini"]
    write(os.path.join(CAPTURE, "other-worktree.txt"), T["claude"]["cwd"])
    d.js(f"{S}.setProjectStatus(arguments[0], 'review')", gp)
    d.js(f"{S}.setOverlay('board')")
    d.wait("return !![...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Review with'))", 40, 0.25, gp)
    run.shot("06-board-review")
    d.js("[...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Review with')).click()", gp)
    d.wait("return !!document.querySelector('.review-with')", 15)
    until(lambda: "Reading changes" not in dialog_text(d, ".review-with"), 20)
    info = dialog_text(d, ".review-with")
    run.check("Review with… asks which agent and shows the editable prompt", d.js("return document.querySelectorAll('.review-with .launch-card').length") >= 3 and "Do not change any files" in d.js("return document.querySelector('.review-prompt').value"), info[:200])
    run.check("…it suggests an agent that did not write the changes", d.js("return document.querySelector('.review-with .launch-card[style*=\"var(--accent)\"]')?.dataset.kind") != "gemini")
    run.check("…and says the diff is bounded and masked", "credentials masked" in info and "at most 48 KB" in info, info)
    d.js("document.querySelector(`.review-with .launch-card[data-kind='omp']`).click()")
    type_into(d, ".review-prompt", d.js("return document.querySelector('.review-prompt').value") + " Look at secrets first.")
    run.shot("07-review-with")
    sessions_before = {s["id"] for s in daemon(d, "session.list")}
    d.js("[...document.querySelectorAll('.review-with .footer .btn.primary')].find(b => b.textContent.includes('Start review')).click()")
    rs = until(lambda: next((s for s in daemon(d, "session.list") if s["id"] not in sessions_before and s["kind"] == "omp"), None), 30)
    run.check("a session of another vendor starts in the same worktree", bool(rs) and os.path.normcase(os.path.normpath(rs["cwd"])) == os.path.normcase(os.path.normpath(gem["cwd"])) and rs["project_id"] == gp, str(rs and (rs["cwd"], rs["project_id"])))
    typed = until(lambda: read(os.path.join(CAPTURE, "review-input.txt")), 90)
    run.check("it was typed the review prompt, with the user's edit", bool(typed) and "Do not change any files" in typed and "Look at secrets first." in typed)
    run.check("…and how to leave notes: the kelpied command with the base", bool(typed) and re.search(r"kelpied(\.exe)?\" review add --base main <file>:<line> \"<comment>\"", typed) is not None)
    run.check("…and the diff against main (committed work first, the uncommitted changes after)", bool(typed) and "Changes in " in typed and "```diff" in typed and "GEMINI two" in typed and "Uncommitted changes in the worktree" in typed)
    run.check("…with the planted token masked", bool(typed) and TOKEN not in typed and "[redacted]" in typed)
    run.check("…and bounded", bool(typed) and len(typed) < 70_000, str(len(typed or "")))
    results = until(lambda: read(os.path.join(CAPTURE, "review-results.json")), 90)
    results = json.loads(results) if results else []
    run.check("the reviewer's `kelpied review add` calls succeeded", len(results) >= 2 and results[0][0] == 0 and results[1][0] == 0, str(results))
    run.check("…and it could not write a note in another worktree", len(results) == 3 and results[2][0] != 0 and "own worktree" in (results[2][1] + results[2][2]), str(results[2:] ))
    notes = daemon(d, "note.list", {"path": gem["cwd"]})
    run.check("the daemon holds its two notes for the worktree, named after the agent", len(notes) == 2 and all(n["by"] for n in notes) and {n["path"] for n in notes} == {"settings.cfg", "app.txt"}, str(notes))
    by_path = {n["path"]: n for n in notes}
    run.check("…it filled in side and text from the diff", by_path["settings.cfg"]["side"] == "new" and TOKEN in by_path["settings.cfg"]["text"] and by_path["app.txt"]["text"] == "GEMINI two", str(notes))
    run.check("…and nothing was written to claude's worktree", daemon(d, "note.list", {"path": T["claude"]["cwd"]}) == [])
    run.check("the app has them (one store)", bool(until(lambda: d.js(f"return {S}.notesFor(arguments[0]).length", gp) == 2, 10)))
    d.js(f"{S}.setOverlay(null)")
    d.js(f"{S}.switchProject(arguments[0])", gp)
    d.js(f"{S}.openFile(arguments[0], 'app.txt', {{ diff: true, base: 'main' }})", gp)
    gutter = until(lambda: d.js("return document.querySelectorAll('.viewer .cm-note-mark.has').length"), 30)
    run.check("the note shows in the diff gutter of the normal diff view", bool(gutter), str(gutter))
    bar = d.js("return document.querySelector('.viewer .notes-bar')?.innerText.replace(/\\s+/g, ' ') || ''")
    run.check("…with the count and who wrote it", "2 review notes" in bar and "why capitals here?" in d.js("return document.querySelector('.viewer .notes-list')?.innerText || ''") and "omp" in d.js("return document.querySelector('.viewer .notes-list')?.innerText || ''"), bar)
    run.shot("08-diff-gutter")
    # Request changes reads the same store, and clears it.
    d.js(f"{S}.setOverlay('board')")
    d.wait("return !![...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Request changes'))", 20, 0.25, gp)
    d.js("[...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Request changes')).click()", gp)
    until(lambda: d.js("return [...document.querySelectorAll('.ctx-item')].some(e => e.textContent.includes('Send to'))"), 10)
    d.js("[...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Send to')).click()")
    run.check("Request changes sends the daemon's notes to the agent and moves the card back to In progress", bool(until(lambda: d.js(f"return {S}.state.projects.find(p => p.id === arguments[0]).status", gp) == "progress", 20)))
    run.check("…and clears them from the daemon", bool(until(lambda: daemon(d, "note.list", {"path": gem["cwd"]}) == [], 10)) and d.js(f"return {S}.notesFor(arguments[0]).length", gp) == 0)
    d.js(f"{S}.setOverlay(null)")

    # ---------- 7. use this one ----------
    winner, others = "codex", ["claude", "gemini"]
    wp = pids[winner]
    main_before = git("rev-parse", "main")
    commits_before = int(git("rev-list", "--count", "main"))
    d.js(f"{S}.setOverlay('runs')")
    d.js("document.querySelector(`.runs-item[data-run='${arguments[0]}']`).click()", r1["id"])
    d.wait("return !!document.querySelector('.runs-page .run-detail-head .toggle')", 15)
    if not d.js("return !!document.querySelector('.run-compare')"):
        click_text(d, ".runs-page .run-detail-head .toggle", "Compare")
    d.wait("return document.querySelectorAll('.compare-head').length === 3", 20)
    until(lambda: d.js("return [...document.querySelectorAll('.compare-actions [data-use]')].every(b => !b.disabled)"), 40)
    d.js("window.__confirms = []")
    d.js("document.querySelector(`.compare-actions[data-cand='codex'] [data-use]`).click()")
    asked = until(lambda: d.js("return (window.__confirms || [])[0] || ''"), 10)
    run.check("Use this one asks first, listing the candidates that will be removed", bool(asked) and "Finish “Codex”" in asked and T["claude"]["branch"] in asked and gem["branch"] in asked and T["codex"]["branch"] not in asked.split("\n\n")[1], str(asked))
    run.check("…and says their agents are stopped now and nothing goes before Finish has merged", "agents are stopped now" in (asked or "") and "After Finish has merged" in (asked or ""))
    stopped = until(lambda: not any(s["alive"] for s in daemon(d, "session.list") if s["project_id"] in (pids["claude"], pids["gemini"])), 60)
    run.check("…the other candidates' agents (and the reviewer) stopped at the confirmation", bool(stopped))
    d.wait("return !!document.querySelector('.finish-dialog')", 20)
    until(lambda: d.js("return !!document.querySelector('.finish-dialog .finish-strategies') && !document.querySelector('.finish-dialog').textContent.includes('Reading the worktree')"), 30)
    run.check("the winner's Finish opens (the others still exist)", all(os.path.isdir(T[k]["cwd"]) for k in others))
    run.check("…Finish lists the uncommitted file to confirm and merges into the recorded base without asking", "scratch.txt" in dialog_text(d, ".finish-dialog") and checkbox(d, "Merge into main") is None, dialog_text(d, ".finish-dialog")[:300])
    set_checkbox(d, "Commit exactly these files first", True)
    click_text(d, ".finish-dialog .btn", "Approve these checks")
    until(lambda: not d.js("return document.querySelector('.finish-dialog').innerText.includes('not been approved')"), 15)
    run.check("…the base's check is approved and Finish is on", bool(until(lambda: finish_enabled(d) is True, 15)), dialog_text(d, ".finish-dialog")[:300])
    run.shot("09-finish-winner")
    # Someone commits in the claude candidate after the user confirmed: it must survive the cleanup.
    write(os.path.join(T["claude"]["cwd"], "late.txt"), "committed after the confirmation\n")
    git("add", "-A", cwd=T["claude"]["cwd"])
    git("commit", "-qm", "late work", cwd=T["claude"]["cwd"])
    claude_tip = git("rev-parse", "HEAD", cwd=T["claude"]["cwd"])
    d.js("[...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish').click()")
    done = until(lambda: d.js("return document.querySelector('.finish-dialog .finish-note')?.textContent.includes('Finished.')"), 180)
    run.check("Finish ran to the end", bool(done), dialog_text(d, ".finish-dialog")[:400])
    run.check("main gained exactly one commit with codex's work", int(git("rev-list", "--count", "main")) == commits_before + 1 and git("show", "main:greeting.txt") == "hello from codex" and "CODEX two" in git("show", "main:app.txt") and git("show", "main:scratch.txt") == "left uncommitted", git("log", "--oneline", "-3", "main"))
    run.check("…it is on the old main, and the other candidates' work is not in it", git("rev-parse", "main~1") == main_before and "CLAUDE" not in git("show", "main:app.txt") and "GEMINI" not in git("show", "main:app.txt"))
    gone = until(lambda: not os.path.exists(T["gemini"]["cwd"]), 90)
    run.check("the unchanged candidate's worktree folder is gone", bool(gone))
    run.check("…its branch stays with its work (it is not merged), saved in a WIP commit", git("branch", "--list", T["gemini"]["branch"]) != "" and "WIP from" in git("log", "-1", "--format=%s", T["gemini"]["branch"]) and TOKEN in git("show", f"{T['gemini']['branch']}:settings.cfg"), git("branch", "--list"))
    time.sleep(2)
    run.check("the candidate that committed after the confirmation is kept: worktree, branch and commit", os.path.isdir(T["claude"]["cwd"]) and git("rev-parse", T["claude"]["branch"]) == claude_tip and read(os.path.join(T["claude"]["cwd"], "late.txt")) is not None)
    run.check("…and the user is told why", any("changed after you confirmed" in (i or "") for i in d.js(f"return {S}.state.inbox.map(i => i.body)")))
    run.check("…the sidebar lists it still, and no longer the others", bool(project_at(d, T["claude"]["cwd"])) and not project_at(d, T["gemini"]["cwd"]) and not project_at(d, T[winner]["cwd"]))
    run.check("nothing was forced: the branch of the candidate that changed was not touched", "late work" in git("log", "--format=%s", T["claude"]["branch"]))
    after = get_run(d, r1["id"])
    run.check("the run's tasks are all finished, so the run is", bool(until(lambda: get_run(d, r1["id"])["status"] != "running", 20)), str([(t["id"], t["status"]) for t in after["tasks"]]))
    run.check("…the removed ones are not brought back into the sidebar by the run", not project_at(d, T["gemini"]["cwd"]) and not project_at(d, T[winner]["cwd"]))
    run.shot("10-after-pick")
    d.js(f"{S}.setOverlay(null)")

    # ---------- 7b. a worktree inside the repository has notes of its own ----------
    write(os.path.join(REPO, ".gitignore"), read(os.path.join(REPO, ".gitignore")) + ".claude/\n")
    git("commit", "-qam", "ignore .claude")
    nest = os.path.join(REPO, ".claude", "worktrees", "nest")
    git("worktree", "add", "-q", "-b", "nest", nest, "main")
    nid = d.js(f"return {S}.addProject(arguments[0], {{ parentId: arguments[1], branch: 'nest', name: 'nest', open: false }})", nest, pid)
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.is_repo", nid), 20)
    d.js(f"{S}.addNote(arguments[0], {{ path: 'app.txt', line: 1, side: 'new', text: 'one', note: 'repo note' }})", pid)
    d.js(f"{S}.addNote(arguments[0], {{ path: 'app.txt', line: 2, side: 'new', text: 'two', note: 'nest note' }})", nid)
    both = until(lambda: d.js(f"return [{S}.notesFor(arguments[0]).length, {S}.notesFor(arguments[1]).length]", pid, nid) == [1, 1], 15)
    run.check("a worktree inside the repository has notes of its own: each shows only its own", bool(both), str(d.js(f"return [{S}.notesFor(arguments[0]).map(n => n.note), {S}.notesFor(arguments[1]).map(n => n.note)]", pid, nid)))
    keys = daemon(d, "note.list", {})
    run.check("…the daemon files them under two keys", len(keys) == 2 and sorted(n["note"] for v in keys.values() for n in v) == ["nest note", "repo note"], str(keys))
    js_await(d, f"window.__kelpie.worktrees.removeWorktree({S}.state.projects.find(p => p.id === arguments[0]), {{ force: false }})", nid)
    left = daemon(d, "note.list", {})
    run.check("removing it clears its notes and only its notes: the repository's stay", [n["note"] for v in left.values() for n in v] == ["repo note"] and not os.path.exists(nest), str(left))
    run.check("…and the repository's own view still has its note", [n["note"] for n in d.js(f"return {S}.notesFor(arguments[0])", pid)] == ["repo note"])

    # ---------- 8. no idle cost ----------
    pid_d = daemon(d, "daemon.info")["pid"]
    time.sleep(2)
    a = daemon_cpu_ms(pid_d)
    time.sleep(10)
    b = daemon_cpu_ms(pid_d)
    run.check(f"the daemon idles after the run: {b - a:.0f} ms of CPU in 10 s (<= 300 ms)", b - a <= 300, f"{a} {b}")
    run.check("the compare view is closed: nothing of it is left in the page", not d.js("return !!document.querySelector('.run-compare')"))


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def main():
    notes = seed_fixture()
    write(os.path.join(DATA, "seed-notes.json"), json.dumps(notes))
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
    shutil.rmtree(WORK + "-worktrees", ignore_errors=True)


if __name__ == "__main__":
    main()
