"""GUI e2e for turn checkpoints: timeline, diff, undo, concurrent sessions, artifacts, the watcher.

    python -X utf8 scripts/e2e/checkpoints.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie is never reached. Agents are stand-ins (`claude` and
`gemini` shims on PATH running fake_agent below): they edit files in their folder on request. Every
repository is a fixture under the scratch folder.

1. A session in a git folder gets a baseline; three turns (each editing files) give three turns in the
   Turns panel with their file counts; a turn's file opens a before/after diff pane.
2. Undo of turn 2 shows a preview, restores turn 2's files (and deletes the one it added), keeps turns 1
   and 3, and leaves HEAD and the index byte-identical.
3. Two sessions working in one folder at the same time: undo of one lists the shared file with the other
   session named and refuses without confirmation (nothing changes); the owners view lists it as shared;
   confirmed, it goes through.
3a. A file the user edits between two turns and the agent then edits too is flagged "changed between turns"
   (a look at the start of each turn), not reverted; undo refuses without confirmation.
3b. Another session's edit made while the owner sat idle (before the owner's next turn) is flagged shared,
   not reverted; the undo dialog stays bound to the session and turn it was opened for even when another
   agent becomes the most recently active one.
4. A checkpoint ref write emits no git `fs-changed` event (a working-tree edit does).
5. An agent that reports no turns is labelled approximate; turning snapshots off records nothing.
6. A turn's produced files render (markdown with a comment that becomes a review note, image, CSV).
7. Removing a worktree drops its sessions' snapshots.
"""
import hashlib
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

ENDPOINT = f"kelpie-checkpoints-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-checkpoints-data-")
WORK = os.path.join(scratch, f"checkpoints-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
REPO = os.path.join(WORK, "repo")
WT = os.path.join(WORK, "repo-wt")
for d_ in (BIN, REPO):
    os.makedirs(d_, exist_ok=True)

# Stand-in agent. Each typed line is a JSON turn: {"work": seconds, "files": {path: text}, "hex": {path: hex}}.
FAKE = os.path.join(WORK, "fake_agent.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(
        "import json, msvcrt, os, time\n"
        "print('fake agent ready', flush=True)\n"
        "buf = ''\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch in '\\r\\n':\n"
        "        try:\n"
        "            turn = json.loads(buf)\n"
        "        except ValueError:\n"
        "            buf = ''\n"
        "            continue\n"
        "        # A real agent thinks before its first edit: Kelpie looks at the folder as the turn starts.\n"
        "        end = time.time() + turn.get('delay', 1.5)\n"
        "        while time.time() < end:\n"
        "            print('thinking ' + str(time.time()), flush=True)\n"
        "            time.sleep(0.2)\n"
        "        for p, text in turn.get('files', {}).items():\n"
        "            os.makedirs(os.path.dirname(p) or '.', exist_ok=True)\n"
        "            open(p, 'w', encoding='utf-8', newline='').write(text)\n"
        "        for p, h in turn.get('hex', {}).items():\n"
        "            os.makedirs(os.path.dirname(p) or '.', exist_ok=True)\n"
        "            open(p, 'wb').write(bytes.fromhex(h))\n"
        "        end = time.time() + turn.get('work', 2)\n"
        "        while time.time() < end:\n"
        "            print('working ' + str(time.time()), flush=True)\n"
        "            time.sleep(0.2)\n"
        "        print('done', flush=True)\n"
        "        buf = ''\n"
        "    else:\n"
        "        buf += ch\n"
    )
for name in ("claude", "gemini"):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" %*\r\n')

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
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
S = "window.__kelpie.store"


def git(*args, cwd=REPO):
    r = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.autocrlf=false", *args], cwd=cwd, capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
    if r.returncode:
        raise RuntimeError(f"git {args}: {r.stderr}")
    return r.stdout.strip()


def user_state():
    idx = git("rev-parse", "--path-format=absolute", "--git-path", "index")
    with open(idx, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest(), git("rev-parse", "HEAD"), git("symbolic-ref", "HEAD")


def until(fn, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.25)
    return None


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def read(rel, root=REPO):
    try:
        # A restore writes files the way the user's git config says (CRLF with autocrlf).
        with open(os.path.join(root, rel), encoding="utf-8", newline="") as f:
            return f.read().replace("\r\n", "\n")
    except OSError:
        return None


def start_agent(d, pid, name, kind="claude", cwd=None):
    opts = {"name": name}
    if cwd:
        opts["cwd"] = cwd
    info = js_await(d, f"{S}.newSession(arguments[0], arguments[1], arguments[2])", pid, kind, opts)
    if not info:
        raise RuntimeError(f"{name} did not start")
    # Typing before the stand-in is running (slow on a busy machine) can lose the line.
    if not until(lambda: "fake agent ready" in daemon(d, "session.read", {"handle": info["handle"]})["text"], 90):
        raise RuntimeError(f"{name}: the stand-in agent never showed its banner")
    return info


def turn(d, sess, files=None, work=2, hex_files=None):
    """One agent turn: the fake edits the files, works, and goes quiet."""
    line = json.dumps({"work": work, "files": files or {}, "hex": hex_files or {}})
    daemon(d, "session.input", {"handle": sess["handle"], "data": line + "\r"})


def status_note(d):
    """checkpoint.status as text, for a failing check's detail (a repository switched off shows here)."""
    try:
        return json.dumps(daemon(d, "checkpoint.status"))
    except Exception as e:  # noqa: BLE001
        return f"status unavailable: {e}"


def records(d, sess):
    return daemon(d, "checkpoint.list", {"session": sess["id"]})["records"]


def wait_records(d, sess, count, timeout=45):
    return until(lambda: (r := records(d, sess)) and len(r) >= count and r, timeout)


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


# A 1x1 PNG.
PNG = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000001e221bc330000000049454e44ae426082"
MD = "# Plan\n\nFirst paragraph\n\n- one\n- two\n\n```mermaid\ngraph TD\n  A-->B\n```\n"


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 10, 30)
    run.check("the app talks to a daemon with checkpoint.* (API 10)", d.js(f"return {S}.daemonApi()") >= 10)
    # Snapshots are off by default until measured otherwise: this run turns them on, the way a user would.
    d.js(f"{S}.setSettings({{ checkpoints: true }})")
    until(lambda: daemon(d, "checkpoint.status")["enabled"], 10)
    run.check("the Settings toggle reaches the daemon", daemon(d, "checkpoint.status")["enabled"] is True)

    git("init", "-q", "-b", "main")
    for name in ("a.txt", "b.txt", "d.txt"):
        with open(os.path.join(REPO, name), "w", encoding="utf-8", newline="") as f:
            f.write(f"{name} v0\n")
    git("add", "-A")
    git("commit", "-qm", "init")
    pid = d.js(f"return {S}.addProject(arguments[0])", REPO)
    d.js("window.__fs = []; window.__kelpie.ipc.onEvent('fs-changed', e => window.__fs.push({t: Date.now(), git: e.git, root: e.root}));")
    d.js(f"{S}.showRightPanel('turns')")

    # ---------- 1. a baseline, three turns, the panel, a diff ----------
    s1 = start_agent(d, pid, "cc1")
    r = wait_records(d, s1, 1)
    run.check("a new session in a git folder gets a baseline", bool(r) and r[0]["kind"] == "baseline" and r[0]["n"] == 0, str(r))
    until(lambda: d.js("return document.querySelector('.rightpanel .panel-tabs .ptab.active')?.textContent === 'Turns'"), 10)
    run.check("the Turns tab is in the right panel", d.js("return [...document.querySelectorAll('.rightpanel .ptab')].some(b => b.textContent.startsWith('Turns'))"))
    until(lambda: d.js("return !!document.querySelector('.rightpanel .turns')"), 15)
    time.sleep(2)  # the fake agent is still starting; its banner is not a turn
    before = user_state()
    turn(d, s1, {"a.txt": "turn one\n"})
    wait_records(d, s1, 2)
    turn(d, s1, {"b.txt": "turn two\n", "sub/c.txt": "added in two\n"})
    wait_records(d, s1, 3)
    turn(d, s1, {"d.txt": "turn three\n"})
    recs = wait_records(d, s1, 4)
    run.check("nothing was switched off by the first turns", daemon(d, "checkpoint.status")["disabled"] == [], status_note(d))
    run.check("three turns are recorded after the baseline", bool(recs) and [x["kind"] for x in recs] == ["baseline", "turn", "turn", "turn"], str(recs))
    run.check("their file counts are 1, 2, 1", bool(recs) and [x["files"] for x in recs[1:]] == [1, 2, 1], str([x["files"] for x in recs or []]))
    run.check("the user's HEAD, branch and index are untouched by the snapshots", user_state() == before, f"{user_state()} vs {before}")
    rows = until(lambda: (n := d.js("return document.querySelectorAll('.rightpanel .turn-row.turn').length")) == 3 and n, 15)
    run.check("the panel lists the three turns", rows == 3, str(d.js("return document.querySelectorAll('.rightpanel .turn-row').length")))
    run.shot("01-turns")
    d.js("document.querySelectorAll('.rightpanel .turn-row.turn')[1].querySelector('.turn-head').click()")
    files = until(lambda: d.js("return [...document.querySelectorAll('.rightpanel .turn-row.turn')[1].querySelectorAll('.turn-file .turn-path')].map(x => x.textContent.replace(/[\\u200e\\u200f]/g, ''))"), 10)
    run.check("turn 2 expands to its two files", sorted(files or []) == ["b.txt", "sub/c.txt"], str(files))
    d.js("document.querySelectorAll('.rightpanel .turn-row.turn')[1].querySelector('.turn-file').click()")
    shown = until(lambda: d.js("return document.querySelector('.viewer-bar')?.textContent.includes('turn #2') && document.querySelectorAll('.cm-content').length > 0"), 20)
    run.check("clicking a file opens a before/after diff of that turn", bool(shown), str(d.js("return document.querySelector('.viewer-bar')?.textContent")))
    run.shot("02-diff")

    # ---------- 2. undo turn 2 ----------
    before = user_state()
    d.js("document.querySelectorAll('.rightpanel .turn-row.turn')[1].querySelector('.row-actions button').click()")
    rows = until(lambda: d.js("return [...document.querySelectorAll('.turn-undo .turn-undo-row')].map(r => r.innerText.replace(/\\s+/g, ' ').trim())"), 15)
    run.check("undo shows a preview of every file it will restore or delete", bool(rows) and len(rows) == 2 and any("delete" in x.lower() and "c.txt" in x for x in rows) and any("restore" in x.lower() and "b.txt" in x for x in rows), str(rows))
    run.check("nothing is changed while the preview is open", read("b.txt") == "turn two\n" and read("sub/c.txt") == "added in two\n")
    run.shot("03-undo-preview")
    d.js("document.querySelector('.turn-undo .footer .btn.primary').click()")
    gone = until(lambda: read("b.txt") == "b.txt v0\n" and read("sub/c.txt") is None, 20)
    run.check("undo restores b.txt and deletes the file turn 2 added", bool(gone), f"{read('b.txt')!r} {read('sub/c.txt')!r}")
    run.check("turns 1 and 3 stay", read("a.txt") == "turn one\n" and read("d.txt") == "turn three\n")
    run.check("undo leaves HEAD and the index byte-identical", user_state() == before)
    last = until(lambda: (r := records(d, s1)) and r[-1]["kind"] == "undo" and r[-1], 20)
    run.check("the undo is recorded as its own step (undid #2)", bool(last) and last["undo_of"] == 2, str(last))
    again = daemon(d, "checkpoint.undo", {"session": s1["id"], "n": 2})
    run.check("undoing turn 2 again finds its files already put back", again.get("applied") is True and not again.get("files"), str(again))

    # ---------- 4. the watcher ----------
    # (before the shared-folder scenario so that nothing else writes while this one looks)
    time.sleep(2)
    d.js("window.__fs.length = 0")
    turn(d, s1, {"watch.txt": "x\n"}, work=1)
    n_before = len(records(d, s1))
    wait_records(d, s1, n_before + 1)
    time.sleep(2.5)
    ev = d.js("return window.__fs.filter(e => e.root.toLowerCase().replace(/\\\\/g, '/') === arguments[0]).map(e => ({t: e.t, git: e.git}))", REPO.lower().replace("\\", "/"))
    # The edit is made before the turn ends; the ref is written after the snapshot began reading.
    # Both times come from the daemon's record, so a busy machine does not shift the boundary.
    read_at = records(d, s1)[-1]["read_at"]
    during = [e for e in ev if e["t"] < read_at + 200]
    late = [e for e in ev if e["t"] >= read_at + 200 and e["git"]]
    run.check("the working-tree edit does raise git fs-changed events (the listener works)", any(e["git"] for e in during), str(ev))
    run.check("the checkpoint ref write raises none", not late, str(late))

    # ---------- 3. two sessions in one folder ----------
    s2 = start_agent(d, pid, "cc2")
    b2 = wait_records(d, s2, 1)
    s3 = start_agent(d, pid, "cc3")
    b3 = wait_records(d, s3, 1)
    run.check("both new sessions got a baseline", bool(b2) and bool(b3), status_note(d))
    time.sleep(2)
    turn(d, s2, {"shared.txt": "from cc2\n", "only2.txt": "cc2\n"}, work=6)
    time.sleep(1.5)
    turn(d, s3, {"shared.txt": "from cc2 and cc3\n", "only3.txt": "cc3\n"}, work=6)
    r2 = wait_records(d, s2, 2, 60)
    r3 = wait_records(d, s3, 2, 60)
    run.check("both sessions recorded their overlapping turns", bool(r2) and bool(r3), status_note(d))
    if not (r2 and r3):
        raise RuntimeError("the concurrent turns left no records: " + status_note(d))
    n2 = r2[-1]["n"] if r2 else 1
    state_before = (read("shared.txt"), read("only2.txt"), read("only3.txt"))
    res = daemon(d, "checkpoint.undo", {"session": s2["id"], "n": n2})
    need = res.get("needs_confirmation") or []
    shared = [i for i in need if i["path"] == "shared.txt"]
    run.check("undo of cc2's turn refuses without confirmation", res.get("applied") is False and bool(need), str(res))
    run.check("…it lists shared.txt as shared, naming cc3", bool(shared) and shared[0]["state"] == "shared" and shared[0]["with"][0]["name"] == "cc3", str(shared))
    run.check("…and nothing was changed", (read("shared.txt"), read("only2.txt"), read("only3.txt")) == state_before)
    owners = daemon(d, "checkpoint.owners", {"session": s2["id"]})["files"]
    row = next((o for o in owners if o["path"] == "shared.txt"), None)
    run.check("the owners view lists shared.txt as shared by two sessions", bool(row) and row["shared"] and len(row["sessions"]) == 2, str(row))
    # Through the panel: the shared file is flagged and unticked.
    choose = d.js("const sel = document.querySelector('.rightpanel .turns select'); if (!sel) return false; const o = [...sel.options].find(o => o.textContent === 'cc2'); if (!o) return false; sel.value = o.value; sel.dispatchEvent(new Event('change', {bubbles: true})); return true")
    run.check("the panel can show cc2's turns", bool(choose))
    until(lambda: d.js("return document.querySelectorAll('.rightpanel .turn-row.turn').length") >= 1, 15)
    d.js("const rows = document.querySelectorAll('.rightpanel .turn-row.turn'); rows[0].querySelector('.row-actions button').click()")
    flagged = until(lambda: d.js("return [...document.querySelectorAll('.turn-undo .turn-undo-row.risky')].map(r => ({text: r.innerText.replace(/\\s+/g, ' '), checked: r.querySelector('input').checked}))"), 15)
    run.check("the preview flags shared.txt, names cc3 and leaves it unticked", bool(flagged) and "cc3" in flagged[0]["text"] and flagged[0]["checked"] is False, str(flagged))
    run.shot("04-undo-shared")
    d.js("document.querySelector('.turn-undo .footer .btn:not(.primary)').click()")
    ok = daemon(d, "checkpoint.undo", {"session": s2["id"], "n": n2, "confirm": [i["path"] for i in need]})
    run.check("with every shared file confirmed the undo goes through", ok.get("applied") is True and read("only2.txt") is None, str(ok))

    # ---------- 3a. the user edits between turns, the agent edits the same file ----------
    s6 = start_agent(d, pid, "cc6")
    run.check("a session for the between-turns scenario got its baseline", bool(wait_records(d, s6, 1)), status_note(d))
    time.sleep(2)
    turn(d, s6, {"between.txt": "agent turn one\n"})
    wait_records(d, s6, 2)
    # The user fixes the file in an editor while the agent is idle...
    with open(os.path.join(REPO, "between.txt"), "w", encoding="utf-8", newline="") as f:
        f.write("agent turn one\nuser fix\n")
    time.sleep(1)
    # ...and the next turn rewrites the same file.
    turn(d, s6, {"between.txt": "agent turn one\nuser fix\nagent line\n"})
    r6 = wait_records(d, s6, 3)
    n6 = r6[-1]["n"] if r6 else 2
    res = daemon(d, "checkpoint.undo", {"session": s6["id"], "n": n6})
    row = next((i for i in res.get("needs_confirmation") or [] if i["path"] == "between.txt"), None)
    run.check("the file changed between turns is flagged, with the reason", res.get("applied") is False and bool(row) and row["state"] == "modified" and row["reason"].startswith("changed between"), str(res))
    run.check("…and the user's fix was not reverted", read("between.txt") == "agent turn one\nuser fix\nagent line\n", repr(read("between.txt")))
    run.check("the looks at the start of a turn make no records of their own", [x["kind"] for x in records(d, s6)] == ["baseline", "turn", "turn"], str([x["kind"] for x in records(d, s6)]))

    # ---------- 3b. the idle gap, and the dialog bound to its session ----------
    s4 = start_agent(d, pid, "cc4")
    s5 = start_agent(d, pid, "cc5")
    run.check("two more sessions got their baselines", bool(wait_records(d, s4, 1)) and bool(wait_records(d, s5, 1)), status_note(d))
    time.sleep(2)
    turn(d, s4, {"gap1.txt": "cc4 turn one\n"})
    run.check("cc4's first turn was recorded", bool(wait_records(d, s4, 2)), status_note(d) + " " + str(records(d, s4)[-1:]))
    # cc5 works while cc4 sits idle; cc4's next turn then edits only another file.
    turn(d, s5, {"gap2.txt": "cc5 while cc4 is idle\n"})
    run.check("cc5's turn was recorded", bool(wait_records(d, s5, 2)), status_note(d) + " " + str(records(d, s5)[-1:]))
    turn(d, s4, {"only4.txt": "cc4 turn two\n"})
    r4 = wait_records(d, s4, 3)
    n4 = r4[-1]["n"] if r4 else 2
    state_before = (read("gap2.txt"), read("only4.txt"))
    res = daemon(d, "checkpoint.undo", {"session": s4["id"], "n": n4})
    gap = next((i for i in res.get("needs_confirmation") or [] if i["path"] == "gap2.txt"), None)
    run.check("cc5's edit made while cc4 was idle is flagged shared, naming cc5", res.get("applied") is False and bool(gap) and gap["state"] == "shared" and gap["with"][0]["name"] == "cc5", str(res))
    run.check("…and was not reverted", (read("gap2.txt"), read("only4.txt")) == state_before)
    # cc5 snapshots once more (its snapshot holds cc4's file now, so that file is shared with it).
    turn(d, s5, {}, work=3)
    wait_records(d, s5, 3)
    time.sleep(2)
    d.js("const sel = document.querySelector('.rightpanel .turns select'); const o = [...sel.options].find(o => o.textContent === 'cc4'); sel.value = o.value; sel.dispatchEvent(new Event('change', {bubbles: true})); return true")
    until(lambda: d.js("return document.querySelectorAll('.rightpanel .turn-row.turn').length") >= 2, 15)
    d.js("document.querySelectorAll('.rightpanel .turn-row.turn')[0].querySelector('.row-actions button').click()")
    heading = until(lambda: d.js("return document.querySelector('.turn-undo h2')?.textContent"), 15)
    run.check("the dialog names the session and the turn it was opened for", bool(heading) and "cc4" in heading and f"#{n4}" in heading, str(heading))
    until(lambda: d.js("return document.querySelectorAll('.turn-undo .turn-undo-row').length") >= 1, 15)
    # The user confirms cc4's own file (shared only through cc5's snapshot), not cc5's gap2.txt.
    d.js("[...document.querySelectorAll('.turn-undo .turn-undo-row')].find(r => r.textContent.includes('only4.txt')).querySelector('input').click()")
    # Another agent becomes the most recently active one while the dialog is open.
    turn(d, s5, {}, work=3)
    until(lambda: d.js(f"return {S}.sessions[arguments[0]].status === 'working'", s5["id"]), 15)
    until(lambda: d.js(f"return {S}.sessions[arguments[0]].status !== 'working'", s5["id"]), 30)
    time.sleep(1)
    run.check("the panel stays on cc4", d.js("const sel = document.querySelector('.rightpanel .turns select'); return sel.options[sel.selectedIndex].textContent") == "cc4")
    d.js("document.querySelector('.turn-undo .footer .btn.primary').click()")
    done = until(lambda: any(x["kind"] == "undo" for x in records(d, s4)), 30)
    run.check("the undo went to cc4's turn", bool(done) and [x for x in records(d, s4) if x["kind"] == "undo"][-1]["undo_of"] == n4, str(records(d, s4)[-1:]))
    run.check("…and cc5 has no undo step", not any(x["kind"] == "undo" for x in records(d, s5)))
    run.check("cc4's own file came back, cc5's was left alone", read("only4.txt") is None and read("gap2.txt") == "cc5 while cc4 is idle\n", f"{read('only4.txt')!r} {read('gap2.txt')!r}")

    # ---------- 5. approximate boundaries, switching off ----------
    g1 = start_agent(d, pid, "gem", kind="gemini")
    wait_records(d, g1, 1)
    time.sleep(2)
    turn(d, g1, {"gem.txt": "gem\n"})
    rg = wait_records(d, g1, 2)
    run.check("an agent without turn hooks is labelled approximate", bool(rg) and rg[-1]["approximate"] is True and rg[-1]["source"] == "idle", str(rg))
    run.check("…and the row says so", bool(until(lambda: d.js("const sel = document.querySelector('.rightpanel .turns select'); const o = [...sel.options].find(o => o.textContent === 'gem'); if (o && sel.value !== o.value) { sel.value = o.value; sel.dispatchEvent(new Event('change', {bubbles: true})); } return !!document.querySelector('.rightpanel .turn-approx')"), 15)))
    d.js(f"{S}.setSettings({{ checkpoints: false }})")
    until(lambda: not daemon(d, "checkpoint.status")["enabled"], 10)
    count = len(records(d, g1))
    turn(d, g1, {"off.txt": "off\n"})
    time.sleep(7)
    run.check("with snapshots off a turn records nothing", len(records(d, g1)) == count, f"{count} -> {len(records(d, g1))}")
    d.js(f"{S}.setSettings({{ checkpoints: true }})")
    until(lambda: daemon(d, "checkpoint.status")["enabled"], 10)

    # ---------- 6. artifacts ----------
    a1 = start_agent(d, pid, "art")
    wait_records(d, a1, 1)
    time.sleep(2)
    turn(d, a1, {"out/notes.md": MD, "out/data.csv": "name,qty\nbolt,4\nnut,9\n"}, hex_files={"out/shot.png": PNG})
    ra = wait_records(d, a1, 2)
    n_art = ra[-1]["n"] if ra else 1
    d.js("const sel = document.querySelector('.rightpanel .turns select'); const o = [...sel.options].find(o => o.textContent === 'art'); sel.value = o.value; sel.dispatchEvent(new Event('change', {bubbles: true}));")
    until(lambda: d.js("return document.querySelectorAll('.rightpanel .turn-row.turn').length") >= 1, 15)
    d.js("document.querySelector('.rightpanel .turn-row.turn .turn-head').click()")
    chips = until(lambda: d.js("return [...document.querySelectorAll('.rightpanel .turn-artifacts .turn-chip')].map(c => c.textContent)"), 15)
    run.check("the turn lists what it produced (markdown, CSV, image)", sorted(chips or []) == ["data.csv", "notes.md", "shot.png"], str(chips))
    chip = lambda name: d.js("[...document.querySelectorAll('.rightpanel .turn-artifacts .turn-chip')].find(c => c.textContent === arguments[0]).click()", name)
    chip("notes.md")
    md = until(lambda: d.js("return document.querySelectorAll('.turn-file-view .md-block').length"), 20)
    run.check("markdown is rendered block by block", bool(md) and md >= 3, str(md))
    d.js("document.querySelectorAll('.turn-file-view .md-block .md-comment')[2].click()")
    until(lambda: d.js("return !!document.querySelector('.md-note-input textarea')"), 10)
    d.js("const t = document.querySelector('.md-note-input textarea'); t.value = 'make this a numbered list'; t.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('.md-note-input .btn.primary').click()")
    notes = until(lambda: d.js(f"return {S}.notesFor(arguments[0]).filter(n => n.path === 'out/notes.md')", pid), 10)
    run.check("a comment becomes a review note on the source line of that block", bool(notes) and notes[0]["line"] == 5 and notes[0]["note"] == "make this a numbered list" and notes[0]["text"].startswith("- one"), str(notes))
    run.shot("05-artifact-markdown")
    chip("shot.png")
    img = until(lambda: d.js("return !!document.querySelector('.turn-file-view .image-view img')"), 15)
    run.check("the image renders from the turn's snapshot", bool(img))
    chip("data.csv")
    csv = until(lambda: d.js("return document.querySelectorAll('.turn-file-view .data-table tbody tr').length"), 15)
    run.check("the CSV renders as a table", csv == 2, str(csv))

    # ---------- 7. a removed worktree drops its snapshots ----------
    git("worktree", "add", "-q", "-b", "wt-branch", WT, "HEAD")
    w1 = start_agent(d, pid, "wt1", cwd=WT)
    wait_records(d, w1, 1)
    refs = git("for-each-ref", "refs/kelpie/" + w1["id"], cwd=REPO)
    run.check("the worktree session has snapshot refs", bool(refs), refs)
    # The folder cannot be removed while the agent runs in it: stop it (the session stays).
    daemon(d, "session.kill", {"handle": w1["handle"]})
    until(lambda: not d.js(f"return {S}.sessions[arguments[0]].alive", w1["id"]), 20)
    time.sleep(1)
    removed = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.ipc.gitWorktreeRemove(arguments[0], arguments[1], true).then(() => done(true), e => done(String(e)));", REPO, WT)
    run.check("the worktree was removed", removed is True and not os.path.exists(WT), str(removed))
    gone = until(lambda: not git("for-each-ref", "refs/kelpie/" + w1["id"], cwd=REPO), 15)
    run.check("removing the worktree deletes its sessions' refs", bool(gone))
    run.check("…and their records", daemon(d, "checkpoint.list", {"session": w1["id"]})["records"] == [])


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
        # What the daemon logged and recorded is the evidence: keep it.
        print(f"a check failed: data folder kept at {DATA}, fixtures at {WORK}", flush=True)
        return
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
