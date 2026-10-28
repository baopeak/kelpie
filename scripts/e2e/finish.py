"""GUI e2e for the review queue, Finish, overlap chips, auto-worktree and automation delivery.

    python -X utf8 scripts/e2e/finish.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie is never reached. Agents are stand-ins (`claude` shims
on PATH running fake_agent below): they edit files in their folder and can run a command (the way an agent
calls `kelpied worktree set`). Every repository is a fixture under the scratch folder; Finish, merge, branch
deletion and worktree removal only ever run there.

1. One worktree with an agent running: no overlap scan. A second one editing the same file: both sidebar
   rows show "Also changed in <the other>".
2. An agent sets its worktree to review through the CLI: the card appears in the Board's Review column with
   files, +/-, checks, and the Review tab of the right panel lists it. Request changes sends the review notes
   to the agent and moves the card back to In progress.
3. Finish (squash): uncommitted files are listed and must be confirmed; the base's checks need approval first;
   they run in a temporary worktree of the merged result; main moves by one commit, the main checkout follows,
   the branch and the worktree are gone, the card is not left behind.
4. A branch that edits .kelpie.yaml is flagged on its card, its checks are blocked (and a command it adds never
   runs); Finish needs the user to choose.
5. A branch that conflicts with the base is announced in the dialog with "Resolve with agent" and Finish stays off; nothing
   moves. Marking such a worktree Done offers Review. A worktree on a detached HEAD cannot be finished (its commits would be
   lost with it): the dialog says so, nothing moves. Approving the checks does not approve a changed setup: (the next
   auto-worktree starts without running it).
6. Auto-worktree: ask shows an unticked box, never hides it, always ticks it and starts the session in a
   worktree named from the prompt.
7. An automation that delivers to review moves the card when its run ends.
8. Perf: 20 worktrees refresh from the cache within 50 ms, and no overlap scan runs with fewer than two
   active worktrees.
"""
import importlib.util
import json
import os
import shutil
import socket
import statistics
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

ENDPOINT = f"kelpie-finish-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-finish-data-")
WORK = os.path.join(scratch, f"finish-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
REPO = os.path.join(WORK, "repo")
WT = {n: os.path.join(WORK, f"wt-{n}") for n in "abcd"}
MARK = os.path.join(WORK, "check-ran.txt")
PWN = os.path.join(WORK, "pwned.txt")
for d_ in (BIN, REPO):
    os.makedirs(d_, exist_ok=True)

# Stand-in agent. Each typed line is a JSON turn: {"work": s, "files": {path: text}, "run": [[argv], ...]}.
FAKE = os.path.join(WORK, "fake_agent.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(
        "import json, msvcrt, os, subprocess, sys, time\n"
        "print('fake agent ready', flush=True)\n"
        "if any('AUTOTURN' in a for a in sys.argv[1:]):\n"
        "    end = time.time() + 1.5\n"
        "    while time.time() < end:\n"
        "        print('thinking', flush=True)\n"
        "        time.sleep(0.2)\n"
        "    open('auto.txt', 'w').write('x\\n')\n"
        "    print('done', flush=True)\n"
        "buf = ''\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch in '\\r\\n':\n"
        "        try:\n"
        "            turn = json.loads(buf)\n"
        "        except ValueError:\n"
        "            buf = ''\n"
        "            continue\n"
        "        end = time.time() + turn.get('delay', 1.0)\n"
        "        while time.time() < end:\n"
        "            print('thinking ' + str(time.time()), flush=True)\n"
        "            time.sleep(0.2)\n"
        "        for p, text in turn.get('files', {}).items():\n"
        "            os.makedirs(os.path.dirname(p) or '.', exist_ok=True)\n"
        "            open(p, 'w', encoding='utf-8', newline='').write(text)\n"
        "        for argv in turn.get('run', []):\n"
        "            subprocess.run(argv, cwd=os.getcwd(), capture_output=True)\n"
        "        end = time.time() + turn.get('work', 1)\n"
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
# The checks of the fixture repository: `chk <file>` passes when the file is in the checked-out tree and
# leaves a mark saying where it ran; `chkpwn` is what a hostile branch would add.
CHK = os.path.join(WORK, "chk.py")
with open(CHK, "w", encoding="utf-8") as f:
    f.write(
        "import os, sys\n"
        "name = os.path.basename(sys.argv[0]).lower()\n"
        "if 'pwn' in name:\n"
        "    open(os.environ['KELPIE_FINISH_PWN'], 'w').write('ran')\n"
        "    sys.exit(0)\n"
        "if not all(os.path.exists(a) for a in sys.argv[1:]):\n"
        "    print('missing: ' + ' '.join(sys.argv[1:]))\n"
        "    sys.exit(1)\n"
        "open(os.environ['KELPIE_FINISH_MARK'], 'w').write(os.getcwd())\n"
    )
for name in ("chk", "chkpwn"):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{CHK}" %*\r\n')

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "KELPIE_FINISH_MARK": MARK, "KELPIE_FINISH_PWN": PWN,
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
R = "window.__kelpie.review"


def git(*args, cwd=REPO):
    r = subprocess.run(["git", "-c", "user.name=t", "-c", "user.email=t@t", *args], cwd=cwd, capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0", "LC_ALL": "C"})
    if r.returncode:
        raise RuntimeError(f"git {args}: {r.stderr}")
    return r.stdout.rstrip("\n") if args[:1] == ("status",) else r.stdout.strip()


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


def project(d, pid):
    return d.js(f"const p = {S}.state.projects.find(x => x.id === arguments[0]); return p ? {{id: p.id, name: p.name, status: p.status || 'todo', parentId: p.parentId || null}} : null", pid)


def status(d, pid):
    p = project(d, pid)
    return p["status"] if p else None


def start_agent(d, pid, name):
    info = js_await(d, f"{S}.newSession(arguments[0], 'claude', arguments[1])", pid, {"name": name})
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
    line = json.dumps({"work": work, "files": files or {}, "run": run or []})
    daemon(d, "session.input", {"handle": sess["handle"], "data": line + "\r"})
    if not until(lambda: dones(d, sess) > n, 60):
        raise RuntimeError("the stand-in never finished its turn")


def add_worktree(d, pid, name, path):
    git("worktree", "add", "-q", "-b", name, path, "main")
    return d.js(f"return {S}.addProject(arguments[0], {{ parentId: arguments[1], branch: arguments[2], name: arguments[2], open: false }})", path, pid, name)


def sidebar_chip(d, pid):
    return d.js("return document.querySelector(`.project[data-project='${arguments[0]}'] .primary-tag.overlap`)?.textContent.trim() || ''", pid)


def buttons(d, scope, text):
    return d.js("return [...document.querySelectorAll(arguments[0] + ' button')].filter(b => b.textContent.trim() === arguments[1]).length", scope, text)


def click_button(d, scope, text):
    return d.js("const b = [...document.querySelectorAll(arguments[0] + ' button')].find(b => b.textContent.trim() === arguments[1]); if (b) b.click(); return !!b", scope, text)


def checkbox(d, label):
    """State of the finish dialog's checkbox whose label starts with `label`: None when absent."""
    return d.js("const l = [...document.querySelectorAll('.finish-dialog label')].find(l => l.textContent.trim().startsWith(arguments[0])); const i = l && l.querySelector('input'); return i ? i.checked : null", label)


def set_checkbox(d, label, on):
    d.js("const l = [...document.querySelectorAll('.finish-dialog label')].find(l => l.textContent.trim().startsWith(arguments[0])); const i = l.querySelector('input'); if (i.checked !== arguments[1]) i.click()", label, on)


def finish_enabled(d):
    return d.js("const b = [...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish'); return b ? !b.disabled : null")


def open_finish_from_card(d, pid):
    d.js(f"{S}.setOverlay('board')")
    # The button works once the card has read the worktree's changes.
    d.wait("const b = [...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Finish')); return !!b && !b.disabled", 30, 0.25, pid)
    d.js("[...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Finish')).click()", pid)
    d.wait("return !!document.querySelector('.finish-dialog')", 15)
    until(lambda: d.js("return !!document.querySelector('.finish-dialog .finish-strategies') && !document.querySelector('.finish-dialog')?.textContent.includes('Reading the worktree')"), 20)


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 11, 30)
    run.check("the app talks to a daemon with review delivery (API 11)", d.js(f"return {S}.daemonApi()") >= 11)

    # ---------- the fixture repository ----------
    git("init", "-q", "-b", "main")
    write(os.path.join(REPO, "shared.txt"), "one\ntwo\nthree\n")
    write(os.path.join(REPO, "a.txt"), "alpha\nbeta\ngamma\n")
    write(os.path.join(REPO, ".kelpie.yaml"), "check:\n  - chk feat.txt\n")
    git("add", "-A")
    git("commit", "-qm", "init")
    pid = d.js(f"return {S}.addProject(arguments[0])", REPO)
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.branch", pid) == "main", 20)
    a = add_worktree(d, pid, "wt-a", WT["a"])
    b = add_worktree(d, pid, "wt-b", WT["b"])
    c = add_worktree(d, pid, "wt-c", WT["c"])
    d.js(f"{S}.switchProject(arguments[0])", pid)
    until(lambda: all(d.js(f"return {S}.projectInfo[arguments[0]]?.is_repo", x) for x in (a, b, c)), 20)
    scans = lambda: d.js(f"return {R}.overlapScans")  # noqa: E731

    # ---------- 1. overlap ----------
    s_a = start_agent(d, a, "agent-a")
    turn(d, s_a, {"shared.txt": "one\nA was here\nthree\n"})
    time.sleep(3.5)  # past the debounce after a turn ends
    run.check("one active worktree: no overlap scan", scans() == 0, str(scans()))
    s_b = start_agent(d, b, "agent-b")
    turn(d, s_b, {"shared.txt": "one\nB was here\nthree\n"})
    got = until(lambda: sidebar_chip(d, a) and sidebar_chip(d, b), 40)
    run.check("two worktrees editing the same file: both rows say who else changed it", bool(got) and "wt-b" in sidebar_chip(d, a) and "wt-a" in sidebar_chip(d, b), f"{sidebar_chip(d, a)!r} / {sidebar_chip(d, b)!r}")
    run.check("an overlap scan ran (and only with two active)", scans() >= 1, str(scans()))
    run.shot("01-overlap-chips")

    # ---------- 2. review through the CLI ----------
    write(os.path.join(WT["a"], "a.txt"), "alpha\nBETA\ngamma\n")
    git("commit", "-qam", "agent-a: beta in capitals", cwd=WT["a"])
    cli = [kelpied, "worktree", "set", "--status", "review", "--comment", "Feature A"]
    turn(d, s_a, {"feat.txt": "feature\n"}, run=[cli])
    in_review = until(lambda: status(d, a) == "review", 30)
    run.check("the agent's `kelpied worktree set --status review` moves its card to Review", bool(in_review), str(project(d, a)))
    d.js(f"{S}.setOverlay('board')")
    # The summary line appears first, the checks a moment later (they are read from the base branch's file).
    card = until(lambda: d.js("const t = document.querySelector(`.board-col[data-col='review'] .board-card[data-card='${arguments[0]}'] .review-body`)?.innerText.replace(/\\s+/g, ' '); return t && t.includes('checks need approval') ? t : ''", a), 30)
    run.check("the card shows files, +/-, commits and uncommitted files", bool(card) and "files" in card and "+" in card and "commit" in card and "uncommitted" in card, str(card))
    run.check("…and the checks (not approved yet)", bool(card) and "checks need approval" in card, str(card))
    run.check("…and it offers Open diff, Request changes and Approve → Finish", buttons(d, f".board-card[data-card='{a}'] .review-actions", "Open diff") == 1 and buttons(d, f".board-card[data-card='{a}'] .review-actions", "Request changes") == 1 and buttons(d, f".board-card[data-card='{a}'] .review-actions", "Approve → Finish") == 1)
    run.shot("02-board-review")
    d.js(f"{S}.setOverlay(null)")
    d.js(f"{S}.switchProject(arguments[0])", pid)
    d.js(f"{S}.showRightPanel('review')")
    listed = until(lambda: d.js("return document.querySelectorAll('.rightpanel .review-panel .review-item').length") == 1, 20)
    tab = d.js("return [...document.querySelectorAll('.rightpanel .ptab')].find(b => b.textContent.startsWith('Review'))?.textContent.trim()")
    run.check("the right panel's Review tab lists the worktree (and counts it)", bool(listed) and tab == "Review 1", f"{listed} {tab}")
    run.shot("03-review-tab")
    d.js("[...document.querySelectorAll('.rightpanel .review-file')].find(b => b.textContent.includes('a.txt')).click()")
    diff = until(lambda: d.js("return document.querySelector('.viewer-bar')?.textContent.includes('main') && document.querySelectorAll('.cm-content').length > 0"), 20)
    run.check("a file of the review opens a diff against main (committed changes included)", bool(diff), str(d.js("return document.querySelector('.viewer-bar')?.textContent")))

    # ---------- 2b. request changes ----------
    d.js(f"{S}.addNote(arguments[0], {{ path: 'a.txt', line: 2, side: 'new', text: 'BETA', note: 'keep beta lower case' }})", a)
    d.js(f"{S}.setOverlay('board')")
    d.wait("return !!document.querySelector(`.board-card[data-card='${arguments[0]}'] .review-actions`)", 20, 0.25, a)
    d.js("[...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Request changes')).click()", a)
    until(lambda: d.js("return [...document.querySelectorAll('.ctx-item')].some(e => e.textContent.includes('Send to'))"), 10)
    d.js("[...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Send to')).click()")
    moved = until(lambda: status(d, a) == "progress", 20)
    run.check("Request changes sends the notes to the agent and moves the card to In progress", bool(moved), str(project(d, a)))
    run.check("…and clears the notes it sent", d.js(f"return {S}.notesFor(arguments[0]).length", a) == 0)
    d.js(f"{S}.setOverlay(null)")
    turn(d, s_a, {}, run=[cli])
    run.check("the agent finishes the changes and puts it back in review", bool(until(lambda: status(d, a) == "review", 30)))

    # ---------- 3. Finish (squash) ----------
    main_before = git("rev-parse", "main")
    commits_before = int(git("rev-list", "--count", "main"))
    open_finish_from_card(d, a)
    files = d.js("return [...document.querySelectorAll('.finish-dialog .finish-list')][0]?.innerText.replace(/\\s+/g, ' ')")
    run.check("Finish lists the uncommitted files", bool(files) and "feat.txt" in files, str(files))
    run.check("…the strategy defaults to squash with a message drafted from the card", d.js("return document.querySelector('.finish-dialog input[type=radio]').checked") is True and "Feature A" in d.js("return document.querySelector('.finish-dialog textarea').value"))
    run.check("…the base's checks are shown, unapproved, from the base", d.js("return document.querySelector('.finish-dialog').innerText.includes('chk feat.txt') && document.querySelector('.finish-dialog').innerText.includes('not been approved')"))
    run.check("Finish stays disabled until the files are confirmed and the checks decided", finish_enabled(d) is False)
    set_checkbox(d, "Commit exactly these files first", True)
    run.check("…confirming the files is not enough while checks are unapproved", finish_enabled(d) is False)
    click_button(d, ".finish-dialog", "Approve these checks")
    until(lambda: not d.js("return document.querySelector('.finish-dialog').innerText.includes('not been approved')"), 15)
    run.check("…nor while the branch to merge into is unconfirmed (this worktree has no recorded base)", finish_enabled(d) is False and checkbox(d, "Merge into main") is False)
    run.check("approving the checks approved only the checks, not the file's setup", d.js(f"return !{S}.state.approvedSetups || !Object.keys({S}.state.approvedSetups).length") and d.js(f"return Object.values({S}.state.approvedChecks || {{}}).some(l => l.join() === 'chk feat.txt')"))
    set_checkbox(d, "Merge into main", True)
    approved = until(lambda: finish_enabled(d) is True, 15)
    run.check("confirming the base enables Finish", bool(approved))
    run.shot("04-finish-dialog")
    main_tip_text = d.js("return document.querySelector('.finish-dialog textarea').value")
    d.js("[...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish').click()")
    done = until(lambda: d.js("return document.querySelector('.finish-dialog .finish-note')?.textContent.includes('Finished.')"), 120)
    run.check("Finish ran to the end", bool(done), str(d.js("return document.querySelector('.finish-dialog')?.innerText")))
    run.shot("05-finished")
    run.check("main gained exactly one commit (squash), with the drafted message", int(git("rev-list", "--count", "main")) == commits_before + 1 and git("log", "-1", "--format=%s", "main") == main_tip_text.strip(), git("log", "--oneline", "-3", "main"))
    run.check("main's history has no WIP commit and parents on the old main", "WIP" not in git("log", "--format=%s", "main") and git("rev-parse", "main~1") == main_before)
    run.check("the work, committed and uncommitted, is in main", git("show", "main:feat.txt") == "feature" and "BETA" in git("show", "main:a.txt") and "A was here" in git("show", "main:shared.txt"))
    run.check("the main checkout follows (clean, files on disk)", read(os.path.join(REPO, "feat.txt")) == "feature\n" and git("status", "--porcelain") == "", f"{read(os.path.join(REPO, 'feat.txt'))!r} {git('status', '--porcelain')!r}")
    mark = read(MARK)
    run.check("the approved check ran on the merged result in a temporary worktree", bool(mark) and "kelpie-finish-" in mark and os.path.normcase(mark) != os.path.normcase(WT["a"]), str(mark))
    run.check("the temporary worktree is gone", not os.path.exists(mark or "x") and git("worktree", "list", "--porcelain").count("worktree ") == 3)
    run.check("the branch is deleted and the worktree folder removed", git("branch", "--list", "wt-a") == "" and not os.path.exists(WT["a"]))
    run.check("the card left the sidebar and board", project(d, a) is None)
    run.check("its sessions were stopped", not d.js(f"return Object.values({S}.sessions).some(s => s.id === arguments[0] && s.alive)", s_a["id"]))

    # ---------- 4. a branch that edits .kelpie.yaml ----------
    git("checkout", "--", "shared.txt", cwd=WT["b"])
    write(os.path.join(WT["b"], ".kelpie.yaml"), "check:\n  - chk feat.txt\n  - chkpwn\n")
    write(os.path.join(WT["b"], "b.txt"), "bee\n")
    git("add", "-A", cwd=WT["b"])
    git("commit", "-qm", "agent-b: more checks", cwd=WT["b"])
    d.js(f"{S}.setProjectStatus(arguments[0], 'review')", b)
    d.js(f"{S}.setOverlay('board')")
    flagged = until(lambda: d.js("const t = document.querySelector(`.board-card[data-card='${arguments[0]}'] .review-body`)?.innerText.replace(/\\s+/g, ' '); return t && t.includes('checks off') ? t : ''", b), 30)
    run.check("the card flags the .kelpie.yaml change", bool(flagged) and ".kelpie.yaml changed" in flagged, str(flagged))
    run.check("…and shows its checks as off", bool(flagged) and "checks off" in flagged, str(flagged))
    run.shot("06-config-flag")
    d.js(f"{S}.setOverlay(null)")
    open_finish_from_card(d, b)
    text = d.js("return document.querySelector('.finish-dialog').innerText")
    run.check("Finish says the branch's checks are off", "its checks are off" in text, text[:400])
    run.check("…the base's list is offered, unticked", checkbox(d, "Run the checks from main anyway") is False)
    run.check("…Finish waits for a choice", finish_enabled(d) is False)
    run.shot("07-finish-blocked")
    set_checkbox(d, "Finish without running the checks", True)
    set_checkbox(d, "Merge into main", True)
    run.check("choosing to finish without checks (and confirming the base) enables it", finish_enabled(d) is True)
    d.js("[...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish').click()")
    done = until(lambda: d.js("return document.querySelector('.finish-dialog .finish-note')?.textContent.includes('Finished.')"), 120)
    run.check("it finishes when the user chose that", bool(done), str(d.js("return document.querySelector('.finish-dialog')?.innerText")))
    run.check("the command the branch added never ran", not os.path.exists(PWN))
    run.check("…its .kelpie.yaml is merged as content", "chkpwn" in git("show", "main:.kelpie.yaml"))
    d.js(f"{S}.setOverlay(null)")

    # ---------- 5. conflict, and Done with unmerged changes ----------
    write(os.path.join(WT["c"], "a.txt"), "ALPHA-C\nBETA\ngamma\n")
    git("commit", "-qam", "agent-c: alpha", cwd=WT["c"])
    write(os.path.join(REPO, "a.txt"), "ALPHA-MAIN\nBETA\ngamma\n")
    git("commit", "-qam", "main: alpha", cwd=REPO)
    d.js("window.confirm = () => false")
    d.js(f"{R}.moveToStatus({S}.state.projects.find(p => p.id === arguments[0]), 'done')", c)
    run.check("marking a worktree with unmerged changes Done sends it to Review unless the user says Done (Cancel)", bool(until(lambda: status(d, c) == "review", 15)), str(project(d, c)))
    d.js("window.confirm = () => true")
    d.js(f"{S}.setProjectStatus(arguments[0], 'progress')", c)
    d.js(f"{R}.moveToStatus({S}.state.projects.find(p => p.id === arguments[0]), 'done')", c)
    run.check("…and Done stays Done when the user says so", bool(until(lambda: status(d, c) == "done", 15)), str(project(d, c)))
    d.js(f"{S}.setProjectStatus(arguments[0], 'review')", c)
    main_tip, c_tip = git("rev-parse", "main"), git("rev-parse", "wt-c")
    # The user approved this repository's worktree setup once (New worktree); main then changes its setup:.
    js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'main'); {S}.approveSetup(r.path, cfg.setupKey); }})()", pid)
    write(os.path.join(REPO, ".kelpie.yaml"), "setup:\n  - echo changed-setup\ncheck:\n  - chk feat.txt\n  - chkpwn\n")
    git("commit", "-qam", "main: a new setup command", cwd=REPO)
    main_tip = git("rev-parse", "main")
    open_finish_from_card(d, c)
    text = d.js("return document.querySelector('.finish-dialog').innerText")
    run.check("the base's checks changed since they were approved: the dialog shows the difference and asks again", "changed since you approved them" in text and d.js("return [...document.querySelectorAll('.finish-dialog .finish-checks .added')].map(e => e.textContent)") == ["chkpwn"], text[:600])
    click_button(d, ".finish-dialog", "Approve these checks")
    until(lambda: not d.js("return document.querySelector('.finish-dialog').innerText.includes('changed since you approved')"), 15)
    ok_setup = js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'main'); return {{ setup: window.__kelpie.worktrees.configApproved(r, cfg), checks: window.__kelpie.worktrees.checksApproved(r, cfg) }}; }})()", pid)
    run.check("approving the new checks did not approve the changed setup: it", ok_setup == {"setup": False, "checks": True}, str(ok_setup))
    run.check("a branch that already conflicts with the base is announced, with Resolve with agent", "already conflicts with main" in d.js("return document.querySelector('.finish-dialog').innerText") and "a.txt" in d.js("return document.querySelector('.finish-dialog').innerText") and buttons(d, ".finish-dialog", "Resolve with agent") == 1)
    set_checkbox(d, "Merge into main", True)
    run.check("…and Finish stays off (sessions are not stopped for a merge that will conflict)", finish_enabled(d) is False)
    run.check("nothing moved: main, the branch and the folder are as they were", git("rev-parse", "main") == main_tip and git("rev-parse", "wt-c") == c_tip and read(os.path.join(WT["c"], "a.txt")) == "ALPHA-C\nBETA\ngamma\n" and git("status", "--porcelain") == "")
    run.shot("08-conflict")
    d.js(f"{S}.setOverlay(null)")

    # ---------- 5b. a detached worktree is never finished ----------
    dd = add_worktree(d, pid, "wt-d", WT["d"])
    write(os.path.join(WT["d"], "on-branch.txt"), "1\n")
    git("add", "-A", cwd=WT["d"])
    git("commit", "-qm", "on the branch", cwd=WT["d"])
    git("switch", "--detach", "-q", cwd=WT["d"])
    write(os.path.join(WT["d"], "only-detached.txt"), "2\n")
    git("add", "-A", cwd=WT["d"])
    git("commit", "-qm", "only on the detached HEAD", cwd=WT["d"])
    d_head, d_branch, main_now = git("rev-parse", "HEAD", cwd=WT["d"]), git("rev-parse", "wt-d"), git("rev-parse", "main")
    d.js(f"{S}.setProjectStatus(arguments[0], 'review')", dd)
    open_finish_from_card(d, dd)
    text = d.js("return document.querySelector('.finish-dialog').innerText")
    run.check("a worktree on a detached HEAD: the dialog says its commits would be lost", "detached HEAD" in text and "git switch -c" in text, text[:400])
    run.check("…and Finish stays off", finish_enabled(d) is False)
    run.check("…while the backend refuses it too, whatever the dialog stored", "detached" in str(js_await(d, f"(async () => {{ try {{ await window.__kelpie.ipc.finishIntegrate({{ root: arguments[0], worktree: arguments[1], branch: 'wt-d', base: 'main', strategy: 'squash', message: 'x', checks: [], copy: [], link: [], allow_config_change: false, delete_branch: true }}); return 'merged'; }} catch (e) {{ return String(e); }} }})()", REPO, WT["d"])))
    run.check("nothing moved: the detached commit, the stale branch, main and the folder", git("rev-parse", "HEAD", cwd=WT["d"]) == d_head and git("rev-parse", "wt-d") == d_branch and git("rev-parse", "main") == main_now and os.path.isdir(WT["d"]))
    run.shot("08b-detached")
    d.js(f"{S}.setOverlay(null)")

    # ---------- 5c. New worktree ticks only what was approved as it stands ----------
    def run_box():
        return d.js("const l = document.querySelector('.worktree-dialog .kelpie-config label'); return l ? l.querySelector('input').checked : null")

    def open_new_worktree():
        d.js(f"{S}.switchProject(arguments[0])", pid)
        d.js("window.__kelpie.commands.find(c => c.id === 'worktree.new').run()")
        d.wait("return !!document.querySelector('.worktree-dialog .kelpie-config')", 20)
        time.sleep(0.5)

    js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'HEAD'); {S}.approveSetup(r.path, cfg.setupKey); {S}.approveChecks(r.path, ['chk feat.txt']); }})()", pid)
    open_new_worktree()
    run.check("the setup is approved as it stands but the check list changed since: the box is not ticked", run_box() is False)
    run.check("…and the new check is marked", "new since you approved" in d.js("return document.querySelector('.worktree-dialog .kelpie-config').innerText"))
    run.check("…and nothing was approved by opening the dialog", d.js(f"return {S}.state.approvedChecks[Object.keys({S}.state.approvedChecks)[0]].join()") == "chk feat.txt")
    d.js(f"{S}.setOverlay(null)")
    js_await(d, f"(async () => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await window.__kelpie.worktrees.configAtRef(r, 'HEAD'); {S}.approveChecks(r.path, cfg.check); }})()", pid)
    open_new_worktree()
    run.check("both sections approved as they stand: ticked", run_box() is True)
    d.js(f"{S}.setOverlay(null)")
    # Back to "main's setup was never approved" for the auto-worktree step below.
    d.js(f"{S}.approveSetup({S}.state.projects.find(p => p.id === arguments[0]).path, '[]')", pid)

    # ---------- 6. auto-worktree ----------
    d.js(f"{S}.switchProject(arguments[0])", pid)
    box = lambda: d.js("const l = [...document.querySelectorAll('.modal label.check-label')].find(l => l.textContent.includes('Run in a new worktree')); return l ? l.querySelector('input').checked : null")  # noqa: E731
    for mode, expect in (("ask", False), ("never", None), ("always", True)):
        d.js(f"{S}.setSettings({{ autoWorktree: arguments[0] }})", mode)
        d.js(f"{S}.setOverlay('newsession')")
        d.wait("return !!document.querySelector('.modal textarea')", 10)
        run.check(f"auto-worktree '{mode}': the box is {'absent' if expect is None else 'ticked' if expect else 'offered unticked'}", box() == expect, str(box()))
        if mode != "always":
            d.js(f"{S}.setOverlay(null)")
    d.js("const t = document.querySelector('.modal textarea'); t.value = 'Fix the login bug, please'; t.dispatchEvent(new Event('input', { bubbles: true }))")
    run.check("…the branch is previewed from the prompt", "fix-the-login-bug-please" in d.js("return document.querySelector('.modal').innerText"))
    d.js("[...document.querySelectorAll('.modal .footer .btn.primary')].find(b => b.textContent.trim() === 'Start').click()")
    wid = until(lambda: d.js(f"const p = {S}.state.projects.find(p => p.name === 'fix-the-login-bug-please' && p.parentId === arguments[0]); return p ? p.id : null", pid), 60)
    run.check("Start made a worktree named from the first prompt", bool(wid) and "fix-the-login-bug-please" in git("worktree", "list"), git("worktree", "list"))
    sess = until(lambda: d.js(f"return Object.values({S}.sessions).find(s => s.project_id === arguments[0])?.id || null", wid), 30)
    run.check("…and the session runs in it", bool(sess))
    run.check("the worktree records the branch it was created from", d.js(f"return {S}.state.projects.find(p => p.id === arguments[0]).baseBranch", wid) == "main")
    run.check("main's changed setup: was not approved, so it did not run (no setup terminal)", not d.js(f"return Object.values({S}.sessions).some(s => s.project_id === arguments[0] && s.name === 'setup')", wid))
    run.shot("09-auto-worktree")

    # ---------- 7. an automation that delivers to review ----------
    wpath = d.js(f"return {S}.state.projects.find(p => p.id === arguments[0]).path", wid)
    d.js(f"{S}.setProjectStatus(arguments[0], 'progress')", wid)
    saved = daemon(d, "automation.save", {"id": "", "name": "nightly-fix", "enabled": True, "kind": "claude", "prompt": "AUTOTURN", "cwd": wpath, "project_id": wid,
                                          "args": [], "env": {}, "every": "daily", "time": "03:00", "weekday": 1, "last_run": 0, "last_session": None, "last_error": None, "next_run": 0, "deliver": "review"})
    run.check("the daemon keeps the automation's delivery", saved.get("deliver") == "review", str(saved))
    bad = None
    try:
        daemon(d, "automation.save", {**saved, "id": "", "deliver": "slack"})
    except RuntimeError as e:
        bad = str(e)
    run.check("…and refuses an unknown one", bad is not None and "deliver" in bad, str(bad))
    daemon(d, "automation.run", {"id": saved["id"]})
    delivered = until(lambda: status(d, wid) == "review", 60)
    run.check("the card moves to Review when the automation's run ends", bool(delivered), str(project(d, wid)))
    run.check("…only once the run had done its work (not at its start-up output)", read(os.path.join(wpath, "auto.txt")) == "x\n")
    meta = daemon(d, "meta.get", {"path": wpath})
    run.check("`kelpied worktree get` would read the same status", meta and meta["status"] == "review", str(meta))

    # ---------- 8. performance ----------
    for i in range(20):
        add_worktree(d, pid, f"perf-{i}", os.path.join(WORK, f"perf-{i}"))
    ids = d.js(f"return {S}.state.projects.filter(p => p.name.startsWith('perf-')).map(p => p.id)")
    run.check("20 more worktrees are in the sidebar", len(ids) == 20)
    cold = js_await(d, f"(async () => {{ const ps = {S}.state.projects.filter(p => p.name.startsWith('perf-')); const t = performance.now(); await {R}.refreshChanges(ps, true); return performance.now() - t; }})()")
    warm = []
    for _ in range(7):
        warm.append(js_await(d, f"(async () => {{ const ps = {S}.state.projects.filter(p => p.name.startsWith('perf-')); const t = performance.now(); await {R}.refreshChanges(ps, false); return performance.now() - t; }})()"))
    med = statistics.median(warm)
    got = d.js(f"return {S}.state.projects.filter(p => p.name.startsWith('perf-')).every(p => !!{R}.changesOf(p.id)?.ok)")
    run.check("every perf worktree has a summary", got is True)
    run.check(f"queue refresh for 20 worktrees from the cache: median {med:.1f} ms <= 50 ms (cold {cold:.0f} ms)", med <= 50, str([round(x, 1) for x in warm]))
    before = scans()
    time.sleep(33)  # a full overlap interval
    run.check("no overlap scan ran with fewer than two active worktrees (21 worktrees, one with an agent)", scans() == before, f"{before} -> {scans()}")


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
