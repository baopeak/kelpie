"""E2E: the sidebar lists a project's sessions in the order they are on screen, in the real app.

    python -u -X utf8 scripts/e2e/sidebar_screen_order.py <scratch-dir> <debug kelpie.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this
process only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2. The
sessions are a fake agent (a Python program registered as a custom agent kind); no real agent starts and no model is called.

Rows are read from the sidebar's DOM, named by their session. Panes and tabs are arranged with the store's own layout actions (the
ones a drag on a pane or a tab calls), and rows are clicked with WebDriver. Checks: two sessions side by side, the newer one in the
left pane, list the left one first (the owner's report: the right pane's session came first); top before bottom; tab 1 before
tab 2; a dragged tab and a moved pane re-sort the rows as the screen does; a family (a parent and its sub-session) is one block that
stays in place when the child is shown in the parent's pane; activity, status changes and clicks on rows move no row; a session
whose pane closes goes below those on screen.

`--before`: the same steps and screenshots (prefixed `before-`) against whatever the dev server serves, for a comparison with an
older version; its failures are printed and the exit code is 0.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

args = [a for a in sys.argv[1:] if not a.startswith("--")]
BEFORE = "--before" in sys.argv
if len(args) < 2:
    raise SystemExit(__doc__)
scratch = os.path.abspath(args[0])
app = os.path.abspath(args[1])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
os.makedirs(scratch, exist_ok=True)
DATA = ps.isolate("screen-order")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-screen-order-")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)

from smoke_support import Run, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

WORK = os.path.join(scratch, f"screen-order-{os.getpid()}")
STATE = os.path.join(WORK, ".fake-state")
os.makedirs(STATE, exist_ok=True)
FAKE_AGENT = r'''
import json, os, sys
state = sys.argv[1]
sid = os.environ.get("KELPIE_SESSION_ID", "")
path = os.path.join(state, sid + ".json")
with open(path + ".tmp", "w", encoding="utf-8") as f:
    f.write(json.dumps({"id": sid, "token": os.environ.get("KELPIE_SESSION_TOKEN", "")}))
os.replace(path + ".tmp", path)
print("fake agent ready", flush=True)
while sys.stdin.readline():
    print("ok", flush=True)
'''
FAKE_PY = os.path.join(STATE, "fake_agent.py")
with open(FAKE_PY, "w", encoding="utf-8") as f:
    f.write(FAKE_AGENT)
FAKE = {"id": "fake", "label": "Fake agent", "bin": sys.executable, "launch": [FAKE_PY, STATE], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []}

S = "window.__kelpie.store"
PREFIX = "before" if BEFORE else "after"
results = []
procs = {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail[:400]}]" if detail else ""), flush=True)


def until(fn, timeout=30, step=0.25):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:  # noqa: BLE001
            pass
        time.sleep(step)
    return None


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def port_open(p):
    for host in ("127.0.0.1", "::1"):
        try:
            socket.create_connection((host, p), 0.5).close()
            return True
        except OSError:
            pass
    return False


def token_of(sid):
    def read():
        try:
            with open(os.path.join(STATE, sid + ".json"), encoding="utf-8") as f:
                return json.load(f).get("token")
        except (OSError, ValueError):
            return None
    return until(read, 60)


def session_conn(sid):
    """A connection that speaks as session `sid`, with the token the fake agent wrote from its own environment."""
    kc = ps.kc
    token = kc.user_token
    kc.user_token = lambda: None  # no user token on this connection
    try:
        conn = kc.Conn()
    finally:
        kc.user_token = token
    conn.call("auth", {"session_id": sid, "token": token_of(sid)})
    return conn


# The sidebar rows of the test project, in screen order: each row's name (sessions here have unique names).
ROWS_JS = """
const block = document.querySelector('.project[data-project="' + arguments[0] + '"]').closest('.project-block');
return [...block.querySelectorAll('.session-list .session-row')].map(r => r.querySelector('.name').textContent);
"""

# Each terminal on screen and its left and top edges, to check a set-up as a person sees it.
TERMS_JS = """
return [...document.querySelectorAll('.tab-content .pane .term-wrap')].map(t => {
  const r = t.getBoundingClientRect();
  return {session: t.dataset.session, left: r.left, top: r.top};
});
"""


def scenario(run, d):
    w = run.d
    w.set_window_size(1280, 800)
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)
    d.call("agent.set_custom", {"agents": [FAKE]})
    d.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 6})

    def order():
        return w.js(ROWS_JS, pid)

    def make(name):
        s = d.call("session.create", {"kind": "fake", "cwd": WORK, "name": name, "project_id": pid})
        time.sleep(0.05)  # a later created_at for the next one
        return s

    def loc(sid):
        return w.js(f"return {S}.paneLocation(arguments[0])", sid)

    def place(sid, where):
        """Opens `sid` in a new pane: a new tab, or beside the focused pane of the open tab ("right" / "down")."""
        w.js(f"{S}.placePane(arguments[0], {{id: 'pane-' + arguments[1], kind: 'terminal', sessionId: arguments[1]}}, arguments[2])", pid, sid, where)
        until(lambda: loc(sid), 10)

    def term(sid):
        return next((t for t in w.js(TERMS_JS) if t["session"] == sid), None)

    def expect(label, want, timeout=8):
        got = until(lambda: order() == want and order(), timeout)
        check(label, got == want, got or order())

    # Tab 1: "alpha" (older), then "bravo" (newer) opened on its right and dragged to its left: the owner's set-up.
    alpha, bravo = make("alpha"), make("bravo")
    place(alpha["id"], "tab")
    place(bravo["id"], "right")
    a, b = loc(alpha["id"]), loc(bravo["id"])
    w.js(f"{S}.dropPane(arguments[0], arguments[1], arguments[2], arguments[3], 'left')", pid, b["tabId"], b["paneId"], a["paneId"])
    left = until(lambda: term(bravo["id"]) and term(alpha["id"]) and term(bravo["id"])["left"] < term(alpha["id"])["left"], 10)
    check("set-up: bravo (newer) is in the left pane, alpha (older) in the right one", left, w.js(TERMS_JS))
    time.sleep(1.0)
    run.shot(f"{PREFIX}-01-left-newer-right-older")
    expect("the left pane's session is listed before the right pane's", ["bravo", "alpha"])

    # Tab 2: "charlie" (older) below "delta" (newer).
    charlie, delta = make("charlie"), make("delta")
    place(delta["id"], "tab")
    place(charlie["id"], "down")
    below = until(lambda: term(delta["id"]) and term(charlie["id"]) and term(delta["id"])["top"] < term(charlie["id"])["top"], 10)
    check("set-up: delta (newer) is in the top pane, charlie (older) in the bottom one", below, w.js(TERMS_JS))
    time.sleep(1.0)
    run.shot(f"{PREFIX}-02-top-newer-bottom-older")
    expect("the top pane's session is listed before the bottom one's, and tab 1 before tab 2", ["bravo", "alpha", "delta", "charlie"])

    # The second tab dragged before the first (what a drag on the tab bar calls).
    w.js(f"{S}.moveTab(arguments[0], arguments[1], 0)", pid, loc(delta["id"])["tabId"])
    time.sleep(1.0)
    run.shot(f"{PREFIX}-03-tab-dragged-first")
    expect("a tab dragged first brings its sessions first", ["delta", "charlie", "bravo", "alpha"])

    # A pane moved (what a drag on a pane calls): charlie dragged above delta.
    c, dl = loc(charlie["id"]), loc(delta["id"])
    w.js(f"{S}.dropPane(arguments[0], arguments[1], arguments[2], arguments[3], 'top')", pid, c["tabId"], c["paneId"], dl["paneId"])
    time.sleep(1.0)
    run.shot(f"{PREFIX}-04-pane-moved-up")
    expect("a pane moved above another moves its row above", ["charlie", "delta", "bravo", "alpha"])

    # A family in tab 3: "parent" and its sub-session "kid".
    parent = make("parent")
    place(parent["id"], "tab")
    pc = session_conn(parent["id"])
    kid = pc.call("sub.spawn", {"agent": "fake", "task": "one", "name": "kid"})
    with_family = ["charlie", "delta", "bravo", "alpha", "parent", "kid"]
    expect("a family is one block at its tab's place, the sub-session under its parent", with_family, 20)
    ploc = loc(parent["id"])

    def click(name):
        i = order().index(name)
        el = w.js("const b = document.querySelector('.project[data-project=\"' + arguments[0] + '\"]').closest('.project-block'); "
                  "return [...b.querySelectorAll('.session-list .session-row')][arguments[1]];", pid, i)
        w._req("POST", w.s(f"/element/{next(iter(el.values()))}/click"), {})

    click("kid")
    shown = until(lambda: (loc(kid["id"]) or {}).get("paneId") == ploc["paneId"], 10)
    check("set-up: a click on the sub-session's row shows it in its parent's pane", shown, loc(kid["id"]))
    time.sleep(1.0)
    run.shot(f"{PREFIX}-05-child-in-parent-pane")
    expect("showing the child in its parent's pane keeps the family in place", with_family)

    # The family's tab dragged first while the child is on screen: the block follows the child's place.
    w.js(f"{S}.moveTab(arguments[0], arguments[1], 0)", pid, loc(kid["id"])["tabId"])
    time.sleep(1.0)
    run.shot(f"{PREFIX}-06-family-tab-first")
    family_first = ["parent", "kid", "charlie", "delta", "bravo", "alpha"]
    expect("the family's tab dragged first brings the whole family first", family_first)

    # Activity, status changes and clicks on rows: nothing moves. Sampled all along.
    samples = []

    def hook(sid, event):
        d.call("hook.report", {"session_id": sid, "agent": "claude", "payload": {"hook_event_name": event}})

    statuses = set()
    for i in range(6):
        for s in (alpha, charlie):
            h = next(x["handle"] for x in d.call("session.list") if x["id"] == s["id"])
            d.call("session.input", {"handle": h, "data": f"activity {i}\n"})
        hook(alpha["id"] if i % 2 else charlie["id"], "UserPromptSubmit" if i % 4 < 2 else "Stop")
        statuses.update(x["status"] for x in d.call("session.list") if x["id"] in (alpha["id"], charlie["id"]))
        samples.append(order())
        time.sleep(0.25)
    check("set-up: the sessions' status really changed", "working" in statuses, sorted(s for s in statuses if s))
    for name in ("alpha", "delta", "parent", "kid", "bravo"):
        click(name)
        time.sleep(0.6)
        samples.append(order())
    run.shot(f"{PREFIX}-07-after-activity-and-clicks")
    moved = [s for s in samples if s != family_first]
    check("activity, status changes and clicks on rows moved no row", not moved and order() == family_first, moved[:3] or order())

    # A pane closed while its session keeps running: on screen nowhere, it goes below the sessions on screen.
    bl = loc(bravo["id"])
    w.js_async(f"const done = arguments[arguments.length-1]; {S}.closePane(arguments[0], arguments[1], arguments[2], false).then(() => done(true), e => done(String(e)))",
               pid, bl["tabId"], bl["paneId"])
    until(lambda: not loc(bravo["id"]), 10)
    time.sleep(1.0)
    run.shot(f"{PREFIX}-08-pane-closed")
    expect("a session whose pane closed goes after the sessions on screen", ["parent", "kid", "charlie", "delta", "alpha", "bravo"])


def main():
    if port_open(1427):
        raise SystemExit("port 1427 is taken (another Vite dev server?): this script starts the app's own; stop that one first")
    d = ps.Daemon(kelpied, env={"KELPIE_E2E": "1"}).start()
    procs["daemon"] = d.proc
    procs["vite"] = subprocess.Popen(["cmd", "/c", "pnpm", "exec", "vite", "--port", "1427", "--strictPort"] if os.name == "nt" else ["pnpm", "exec", "vite", "--port", "1427", "--strictPort"], cwd=os.path.join(REPO, "app"), stdout=open(os.path.join(scratch, "vite.log"), "w"), stderr=subprocess.STDOUT)
    until(lambda: port_open(1427), 60)
    driver_port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
    procs["driver"] = subprocess.Popen([TAURI_DRIVER, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", NATIVE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run = None
    try:
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(scratch, "shots"), app=app)
        run.boot()
        info = run.d.invoke("daemon_call", {"method": "daemon.info", "params": {}}).get("ok") or {}
        check("the app is connected to this script's daemon", info.get("pid") == d.proc.pid, info.get("pid"))
        if info.get("pid") != d.proc.pid:
            raise SystemExit("the app is not on this script's daemon: stopping before any scenario")
        scenario(run, d)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        if run is not None:
            run.d.quit()
        for name in ("driver", "vite"):
            p = procs.get(name)
            if p and p.poll() is None:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
                else:
                    p.terminate()
        assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
        d.stop()
        shutil.rmtree(WORK, ignore_errors=True)
        shutil.rmtree(DATA, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"mode": PREFIX, "passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(0 if BEFORE or not failed else 1)


if __name__ == "__main__":
    main()
