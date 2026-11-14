"""E2E: sub-sessions live in their parent's tab, in the real app.

    python -u -X utf8 scripts/e2e/sub_session_tabs.py <scratch-dir> <debug kelpie.exe>

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this
process only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2.

The agents are a fake agent (a Python program registered as a custom agent kind, so it counts as an installed agent) that writes
its session token to a file; the parent's calls to `sub.spawn` / `sub.close` are made as the session itself, as an agent's MCP
tools make them. Starts no real agent and calls no model.

Checks: spawning children opens no tab; the parent's pane shows the family as sub-tabs (the root first, a grandchild under its
parent); a chip and `revealSession` (the sidebar's click) show a child in the same pane, the tab named "parent › child"; a child
the parent closes, and a child whose pane is closed, give the pane back to the parent; a tab per child left from before is folded
into the family's tab when the window starts.
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

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
scratch = os.path.abspath(sys.argv[1])
app = os.path.abspath(sys.argv[2])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
os.makedirs(scratch, exist_ok=True)
DATA = ps.isolate("sub-tabs")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-sub-tabs-")
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

WORK = os.path.join(scratch, f"sub-tabs-{os.getpid()}")
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


def scenario(run, d):
    w = run.d
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)

    def tabs():
        return w.js(f"return {S}.workspace(arguments[0]).tabs.map(t => ({{id: t.id, root: t.root}}))", pid)

    def leaf_panes():
        return [t["root"]["pane"] for t in tabs() if t["root"]["type"] == "leaf"]

    def chips(pane_id):
        return w.js("""return [...document.querySelectorAll('.pane[data-pane="' + arguments[0] + '"] .sub-strip .sub-chip:not(.more)')]
          .map(c => ({name: c.querySelector('.sub-chip-name').textContent, active: c.classList.contains('active'), depth: (c.querySelector('.sub-chip-depth') || {}).textContent || ''}))""", pane_id)

    def tab_title():
        return w.js("const t = document.querySelector('.tabbar .tab.active .title'); return t ? t.textContent : null")

    d.call("agent.set_custom", {"agents": [FAKE]})
    d.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 6})
    parent = d.call("session.create", {"kind": "fake", "cwd": WORK, "name": "parent", "project_id": pid})
    pc = session_conn(parent["id"])
    w.js(f"{S}.revealSession(arguments[0])", parent["id"])
    check("the parent opens in one tab", until(lambda: [p.get("sessionId") for p in leaf_panes()] == [parent["id"]], 15), tabs())
    pane = leaf_panes()[0]["id"]

    one = pc.call("sub.spawn", {"agent": "fake", "task": "one", "name": "child-one"})
    two = pc.call("sub.spawn", {"agent": "fake", "task": "two", "name": "child-two"})
    c1 = session_conn(one["id"])
    grand = c1.call("sub.spawn", {"agent": "fake", "task": "grand", "name": "grand"})
    want = ["parent", "child-one", "grand", "child-two"]
    got = until(lambda: [c["name"] for c in chips(pane)] == want and chips(pane), 20)
    check("the parent's pane shows the family as sub-tabs, root first, a grandchild under its parent", got, chips(pane))
    check("the root's chip is the one on screen; the grandchild is marked one level deeper", got and got[0]["active"] and got[2]["depth"] == "›", got)
    check("spawning children opened no tab", len(tabs()) == 1, len(tabs()))
    run.shot("01-family-strip")

    # A chip shows that child in the same pane.
    w.js("""[...document.querySelectorAll('.pane[data-pane="' + arguments[0] + '"] .sub-strip .sub-chip')].find(c => c.textContent.includes('child-two')).click()""", pane)
    check("a chip shows the child in the same pane, no new tab", until(lambda: leaf_panes()[0]["sessionId"] == two["id"] and leaf_panes()[0]["id"] == pane and len(tabs()) == 1, 10), tabs())
    check("the child's chip is the active one", until(lambda: [c["name"] for c in chips(pane) if c["active"]] == ["child-two"], 10), chips(pane))
    check("the tab is named after the parent and the child", until(lambda: tab_title() == "parent › child-two", 10), tab_title())
    run.shot("02-child-in-parent-tab")

    # The sidebar's click (revealSession) does the same.
    w.js(f"{S}.revealSession(arguments[0])", one["id"])
    check("revealing a sub-session uses the family's pane", until(lambda: leaf_panes()[0]["sessionId"] == one["id"] and len(tabs()) == 1, 10), tabs())

    # The parent closes the child on screen: the pane shows the parent again.
    w.js(f"{S}.revealSession(arguments[0])", two["id"])
    until(lambda: leaf_panes()[0]["sessionId"] == two["id"], 10)
    pc.call("sub.close", {"session": two["id"]})
    check("a child closed by its parent gives the pane back to the parent", until(lambda: leaf_panes()[0]["sessionId"] == parent["id"] and len(tabs()) == 1, 15), tabs())

    # Closing the pane of a child on screen stops it and shows the parent.
    w.js(f"{S}.revealSession(arguments[0])", grand["id"])
    until(lambda: leaf_panes()[0]["sessionId"] == grand["id"], 10)
    tab_id = tabs()[0]["id"]
    # As the pane's close button does (`back`).
    w.js_async(f"const done = arguments[arguments.length-1]; {S}.closePane(arguments[0], arguments[1], arguments[2], true, {{back: true}}).then(() => done(true), e => done(String(e)))", pid, tab_id, pane)
    listed = lambda: [s["id"] for s in d.call("session.list")]  # noqa: E731
    check("closing a grandchild's pane stops it", until(lambda: grand["id"] not in listed(), 15), listed())
    check("... and the pane shows the child above it, not a closed tab", until(lambda: len(tabs()) == 1 and leaf_panes()[0]["sessionId"] == one["id"], 10), tabs())

    # A tab per child left from before this change: folded into the family's tab when the window starts.
    w.js(f"{S}.revealSession(arguments[0])", parent["id"])
    w.js(f"{S}.addTab(arguments[0], {{id: 'old-child-pane', kind: 'terminal', sessionId: arguments[1]}})", pid, one["id"])
    # Saved as an older version left it: without the marker that the fold was done.
    w.js_async(f"""const done = arguments[arguments.length-1]; const k = window.__kelpie;
      k.store.saveNow().then(() => k.ipc.loadState()).then(st => {{ delete st.subTabsFolded; return k.ipc.saveState(st); }}).then(() => done(true), e => done(String(e)))""")
    check("set-up: an old tab of its own for a child", len(tabs()) == 2, len(tabs()))
    w.js("location.reload()")
    time.sleep(1)
    w.wait("return !!(window.__kelpie && window.__kelpie.store.loaded() && window.__kelpie.store.connected())", 60)
    folded = until(lambda: len(tabs()) == 1 and tabs(), 15)
    check("on start the child's own tab is folded into the parent's tab", folded and folded[0]["root"]["pane"]["sessionId"] == parent["id"], tabs())
    check("... and the folded child keeps running", one["id"] in listed())
    check("... and is a chip of the family's strip", until(lambda: "child-one" in [c["name"] for c in chips(leaf_panes()[0]["id"])], 10), chips(leaf_panes()[0]["id"]))
    run.shot("03-after-fold")
    w.wait(f"return {S}.state.subTabsFolded === true", 10)

    # Closing the tab (each pane closed, as the tab's close does) closes it even while it shows a child.
    w.js(f"{S}.revealSession(arguments[0])", one["id"])
    until(lambda: leaf_panes()[0]["sessionId"] == one["id"], 10)
    t = tabs()[0]
    w.js_async(f"const done = arguments[arguments.length-1]; {S}.closePane(arguments[0], arguments[1], arguments[2]).then(() => done(true), e => done(String(e)))", pid, t["id"], t["root"]["pane"]["id"])
    check("closing the tab while it shows a child closes the tab", until(lambda: len(tabs()) == 0, 10), tabs())
    check("... and the parent keeps running", parent["id"] in listed())


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
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
