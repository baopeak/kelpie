"""E2E: going between a parent and its sub-sessions from the sidebar, in the real app, as a person clicks.

    python -u -X utf8 scripts/e2e/sub_session_nav.py <scratch-dir> <debug kelpie.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this
process only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2.

The sessions are a fake agent (a Python program registered as a custom agent kind) that writes its session token to a file; the
parent opens its two sub-sessions with `sub.spawn` as the session itself, as an agent's MCP tools do. Starts no real agent and
calls no model. The set-up is the owner's report: a parent "Claude" with two sub-sessions, then another, unrelated "Claude" in a
tab of its own.

Checks, with WebDriver clicks on the sidebar rows (not store calls): the parent and its children are one group, children oldest
first, the unrelated session outside it; each click shows the clicked session (pane, tab title, active chip, breadcrumb for a
child), marks its row and only its row, pulses its row and pane, and leaves the keyboard in its terminal; showing a child never
reorders the sidebar; a chip and the breadcrumb's link do the same; a folded parent shows its count and stands in for its hidden
child on screen.

`--before`: the same clicks and screenshots (prefixed `before-`) against whatever the dev server serves, for a comparison with
an older version; its failures are printed and the exit code is 0.
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
DATA = ps.isolate("sub-nav")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-sub-nav-")
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

WORK = os.path.join(scratch, f"sub-nav-{os.getpid()}")
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


# The sidebar rows of the test project, in screen order, each with what a person sees of it. Read from the DOM so it works on an
# older version too (rows found by their name, the parent "Claude" told from the other by its fold arrow).
ROWS_JS = """
const block = document.querySelector('.project[data-project="' + arguments[0] + '"]').closest('.project-block');
return [...block.querySelectorAll('.session-list .session-row')].map(r => ({
  name: r.querySelector('.name').textContent,
  id: r.dataset.rowSession || null,
  kids: !!r.querySelector('.sub-chev'),
  selected: r.classList.contains('selected'),
  holds: r.classList.contains('holds-selected'),
  pulse: /reveal-pulse-[ab]/.test(r.className),
  family: !!r.closest('.session-family'),
  familyFirst: !!r.closest('.session-family') && r.closest('.session-family').querySelector('.session-row') === r,
  badge: (r.querySelector('.sub-badge') || {}).textContent || '',
  inView: (() => { const a = r.getBoundingClientRect(), b = document.querySelector('.project-list').getBoundingClientRect(); return a.height > 0 && a.top >= b.top - 1 && a.bottom <= b.bottom + 1; })(),
}));
"""

# What the active tab shows: the focused pane's session (from the DOM), the tab title, the strip's chips and breadcrumb, whether the
# pane is pulsing and whether the keyboard is in its terminal.
VIEW_JS = """
const tab = document.querySelector('.tabbar .tab.active .title');
const panes = [...document.querySelectorAll('.tab-content .pane')];
const st = window.__kelpie.store;
const pid = st.state.activeProject;
const t = st.activeTab(pid);
const pane = t && document.querySelector('.pane[data-pane="' + t.focus + '"]') || panes[0];
const term = pane && pane.querySelector('.term-wrap');
const active = document.activeElement;
return {
  session: term ? term.dataset.session : null,
  tab: tab ? tab.textContent : null,
  chips: pane ? [...pane.querySelectorAll('.sub-strip .sub-chip:not(.more)')].map(c => ({name: c.querySelector('.sub-chip-name').textContent, active: c.classList.contains('active')})) : [],
  crumbs: pane && pane.querySelector('.sub-crumbs') ? pane.querySelector('.sub-crumbs').textContent : '',
  pulse: !!(pane && pane.querySelector('.pane-pulse[class*=reveal-pulse-]')),
  keyboard: !!(active && term && term.contains(active)),
};
"""


def scenario(run, d):
    w = run.d
    w.set_window_size(1280, 800)
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)

    def rows():
        return w.js(ROWS_JS, pid)

    def view():
        return w.js(VIEW_JS)

    d.call("agent.set_custom", {"agents": [FAKE]})
    d.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 6})
    parent = d.call("session.create", {"kind": "fake", "cwd": WORK, "name": "Claude", "project_id": pid})
    pc = session_conn(parent["id"])
    w.js(f"{S}.revealSession(arguments[0])", parent["id"])
    one = pc.call("sub.spawn", {"agent": "fake", "task": "one", "name": "chat-first-message-lost"})
    time.sleep(0.05)  # a later created_at for the second child
    two = pc.call("sub.spawn", {"agent": "fake", "task": "two", "name": "sidebar-clarity"})
    other = d.call("session.create", {"kind": "fake", "cwd": WORK, "name": "Claude", "project_id": pid})
    w.js(f"{S}.revealSession(arguments[0])", other["id"])
    until(lambda: len(rows()) == 4, 20)
    w.js(f"{S}.revealSession(arguments[0])", parent["id"])
    time.sleep(1.2)  # the reveal's pulse is over
    names = {parent["id"]: "Claude (parent)", one["id"]: "chat-first-message-lost", two["id"]: "sidebar-clarity", other["id"]: "Claude (other)"}

    def which(r):
        """The session a row stands for, by name (and, for the two "Claude" rows, by the parent's fold arrow)."""
        if r["name"] == "Claude":
            return parent["id"] if r["kids"] else other["id"]
        return one["id"] if r["name"] == "chat-first-message-lost" else two["id"] if r["name"] == "sidebar-clarity" else None

    def order():
        return [names.get(which(r)) for r in rows()]

    start = rows()
    run.shot(f"{PREFIX}-01-start")
    want_order = ["Claude (parent)", "chat-first-message-lost", "sidebar-clarity", "Claude (other)"]
    check("the sidebar lists the parent, its children oldest first, then the other session", order() == want_order, order())
    check("the parent and its children are one group; the other session is outside it",
          [r["family"] for r in start] == [True, True, True, False] and start[0]["familyFirst"], start)
    if not BEFORE:
        check("each row names its session", all(r["id"] == which(r) for r in start), start)

    def row_element(sid):
        i = next(i for i, r in enumerate(rows()) if which(r) == sid)
        return w.js(f"const b = document.querySelector('.project[data-project=\"' + arguments[0] + '\"]').closest('.project-block'); return [...b.querySelectorAll('.session-list .session-row')][arguments[1]];", pid, i)

    def click_element(el):
        w._req("POST", w.s(f"/element/{next(iter(el.values()))}/click"), {})

    def landed(sid, label, tab_title, chip, crumbs):
        """After a click on `sid`: what a person sees."""
        pulse_seen = {"row": False, "pane": False}

        def seen():
            v, rs = view(), rows()
            row = next((r for r in rs if which(r) == sid), None)
            pulse_seen["row"] |= bool(row and row["pulse"])
            pulse_seen["pane"] |= v["pulse"]
            return v["session"] == sid and v
        v = until(seen, 10, 0.05)
        shot = f"{PREFIX}-{label}"
        run.shot(shot)
        v = v or view()
        rs = rows()
        check(f"{label}: the pane shows the clicked session", v["session"] == sid, names.get(v["session"], v["session"]))
        check(f"{label}: the tab reads {tab_title!r}", until(lambda: view()["tab"] == tab_title, 5), view()["tab"])
        if chip:
            check(f"{label}: the matching chip is the active one", [c["name"] for c in v["chips"] if c["active"]] == [chip], v["chips"])
        else:
            check(f"{label}: no sub-session strip for a session outside the family", v["chips"] == [], v["chips"])
        if crumbs:
            check(f"{label}: the breadcrumb says where it is: {crumbs!r}", v["crumbs"] == crumbs, v["crumbs"])
        else:
            check(f"{label}: no breadcrumb on a session at the top", v["crumbs"] == "", v["crumbs"])
        sel = [names.get(which(r)) for r in rs if r["selected"]]
        check(f"{label}: its row, and only its row, is marked selected", sel == [names[sid]], sel)
        check(f"{label}: its row is in view", any(which(r) == sid and r["inView"] for r in rs), rs)
        check(f"{label}: its row and its pane pulsed", pulse_seen["row"] and pulse_seen["pane"], pulse_seen)
        check(f"{label}: the keyboard is in its terminal", until(lambda: view()["keyboard"], 3, 0.1), view())
        check(f"{label}: the sidebar kept its order", order() == want_order, order())

    for label, sid, title, chip, crumbs in (
        ("02-click-child-one", one["id"], "Claude › chat-first-message-lost", "chat-first-message-lost", "Claude›chat-first-message-lostsub-session"),
        ("03-click-child-two", two["id"], "Claude › sidebar-clarity", "sidebar-clarity", "Claude›sidebar-claritysub-session"),
        ("04-click-other", other["id"], "Claude", None, None),
        ("05-click-parent", parent["id"], "Claude", "Claude", None),
    ):
        click_element(row_element(sid))
        landed(sid, label, title, chip, crumbs)
        time.sleep(1.0)

    # A chip, then the breadcrumb's link back to the parent.
    click_element(w.js("return [...document.querySelectorAll('.tab-content .sub-strip .sub-chip')].find(c => c.textContent.includes('sidebar-clarity'))"))
    landed(two["id"], "06-chip-child-two", "Claude › sidebar-clarity", "sidebar-clarity", "Claude›sidebar-claritysub-session")
    time.sleep(1.0)
    if not BEFORE:
        click_element({"element": w.find(".tab-content .sub-crumbs button.sub-crumb")})
        landed(parent["id"], "07-crumb-back-to-parent", "Claude", "Claude", None)
        time.sleep(1.0)

    # Folded, the parent shows how many are below it and stands in for its child on screen.
    click_element(row_element(one["id"]))
    until(lambda: view()["session"] == one["id"], 10)
    time.sleep(1.0)
    w.js("const b = document.querySelector('.project[data-project=\"' + arguments[0] + '\"]').closest('.project-block'); b.querySelector('.session-row .sub-chev').click()", pid)
    folded = until(lambda: len(rows()) == 2 and rows(), 5)
    run.shot(f"{PREFIX}-08-folded-with-child-on-screen")
    parent_row = folded and next((r for r in folded if which(r) == parent["id"]), None)
    check("folded: the children's rows are hidden", bool(folded), rows())
    check("folded: the parent shows the count of sessions below it", parent_row and parent_row["badge"] == "2", parent_row)
    check("folded: the parent stands in for its child on screen", parent_row and parent_row["holds"], parent_row)
    w.js("const b = document.querySelector('.project[data-project=\"' + arguments[0] + '\"]').closest('.project-block'); b.querySelector('.session-row .sub-chev').click()", pid)
    check("unfolded: the children come back, in order", until(lambda: order() == want_order, 5), order())


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
