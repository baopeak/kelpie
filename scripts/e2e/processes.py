"""GUI e2e for processes, ports and preview: the chip, the preview, the Processes panel, guarded kills.

    python -X utf8 scripts/e2e/processes.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, and stops both on exit. KELPIE_ENDPOINT and KELPIE_DATA_DIR are set in this process before
anything talks to a daemon, so the user's own Kelpie is never reached. Every process it stops is one the
script started itself (servers it typed into its own sessions, a stranger it spawned to prove it survives).

1. Idle: with a visible shell that has nothing under it and prints nothing, the daemon makes no scans.
2. `python -m http.server <port>` typed in the shell: a `:<port>` chip on the tab and on the sidebar row
   within 6 s; clicking it opens a browser pane on that port; "Open preview beside" splits.
3. The Processes panel lists the tree with name, PID, RAM, CPU and the port; four more servers started from a
   second shell, and the scan of both sessions with five servers running is measured (daemon `took_us`; held to
   5 ms for a release build, only reported for a `debug` one).
4. Kill (confirmed, with a preview naming the program and the port) frees the port; the shell stays.
5. "Kill children" on an agent session stops the server its tool shell started and keeps the agent process
   and what it runs directly (a helper, as an MCP server is). After the session is killed and restarted (same
   session id, a new process in a new job), the new process is the session's own: listed as the root, never a
   child, refused by Kill, and kept by Kill children, which still frees the new server's port. A custom session
   whose own process is the server (`python -m http.server`, nothing under it) gets its chip too.
6. Guards: a stale identity (wrong start time, wrong program, a PID that is not in the tree, the session's
   own process) never stops anything; programs running in a session (token, no token, any shape of
   parameters) cannot list or stop processes.
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
from smoke_support import Run  # noqa: E402

if len(sys.argv) < 4:
    raise SystemExit(__doc__ + "\n(the app path is required here: the default one asks cargo for a target dir)")
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")
DEBUG_DAEMON = os.path.basename(os.path.dirname(kelpied)).lower() == "debug"

ENDPOINT = f"kelpie-processes-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-processes-data-", dir=scratch)
WORK = os.path.join(scratch, f"processes-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
PROJ = os.path.join(WORK, "proj")
for d_ in (BIN, PROJ):
    os.makedirs(d_, exist_ok=True)

PY = sys.executable
# A stand-in agent: a helper of its own (as an MCP server is), and a tool shell that runs a dev server; it stays.
# It writes its helper's PID where the test reads it.
AGENT = os.path.join(WORK, "fake_agent.py")
with open(AGENT, "w", encoding="utf-8") as f:
    f.write(
        "import os, subprocess, sys, time\n"
        "here = os.path.dirname(__file__)\n"
        "port = open(os.path.join(here, 'agent-port.txt')).read().strip()\n"
        "quiet = dict(stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)\n"
        "helper = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(3600)'], **quiet)\n"
        "tool = 'import subprocess, sys; subprocess.run([sys.executable, \"-m\", \"http.server\", sys.argv[1], \"--bind\", \"127.0.0.1\"])'\n"
        "shell = subprocess.Popen([sys.executable, '-c', tool, port], **quiet)\n"
        "open(os.path.join(here, 'agent-helper.txt'), 'w').write(str(helper.pid))\n"
        "print('fake agent ready, pid', os.getpid(), flush=True)\n"
        "time.sleep(3600)\n"
    )
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{PY}" "{AGENT}" %*\r\n')

# What a program inside a session tries: every shape of parameters, with the session's token and without it.
PROBE = os.path.join(WORK, "probe.py")
with open(PROBE, "w", encoding="utf-8") as f:
    f.write(
        "import json, os, subprocess, sys\n"
        "kelpied, out, sid = sys.argv[1:4]\n"
        "methods = ['process.list', 'process.ports', 'process.port_owner', 'process.kill', 'process.kill_children']\n"
        "shapes = ['', '{}', '[]', '[\"' + sid + '\", 1, 1]', '\"x\"', '7', json.dumps({'session': sid, 'pid': 1, 'start_time': 1}), json.dumps({'sessions': [sid]}), json.dumps({'port': 80})]\n"
        "res = {}\n"
        "for label, env in (('token', dict(os.environ)), ('claimed', {k: v for k, v in os.environ.items() if k != 'KELPIE_SESSION_TOKEN'})):\n"
        "    for m in methods:\n"
        "        for s in shapes:\n"
        "            r = subprocess.run([kelpied, 'call', m] + ([s] if s else []), env=env, capture_output=True, text=True)\n"
        "            res[label + '|' + m + '|' + s] = [r.returncode, (r.stdout + r.stderr).strip()[:200]]\n"
        "json.dump(res, open(out, 'w'))\n"
    )

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "PATH": BIN + os.pathsep + os.environ.get("PATH", "")})
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
stranger = None


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def listening(port):
    with socket.socket() as s:
        s.settimeout(1)
        return s.connect_ex(("127.0.0.1", port)) == 0


def alive(pid):
    out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH", "/FO", "CSV"], capture_output=True, text=True).stdout if os.name == "nt" else ""
    return f'"{pid}"' in out if os.name == "nt" else os.path.exists(f"/proc/{pid}")


def info(c, sid):
    return next((s for s in c.call("session.list") if s["id"] == sid), {})


def ready_shell(d, c, pid, name):
    s = js_await(d, f"{S}.newSession(arguments[0], 'shell', {{name: arguments[1]}})", pid, name)
    sid = s["id"]
    until(lambda: "PS " in c.call("session.read", {"handle": s["handle"]})["text"], 150)
    return s


def type_in(c, s, text):
    c.call("session.input", {"handle": s["handle"], "data": text + "\r"})


def server_cmd(port):
    return f"& '{PY}' -m http.server {port} --bind 127.0.0.1"


def refused(c, method, params):
    try:
        c.call(method, params)
        return False
    except SystemExit:
        return True


def processes(c, sid):
    return c.call("process.list", {"session": sid})


def helper_pid():
    """The PID the stand-in agent wrote for its helper, once it has."""
    try:
        return open(os.path.join(WORK, "agent-helper.txt")).read().strip() or None
    except OSError:
        return None


def scenario(run, c):
    global stranger
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 16, 30)
    info_ = c.call("daemon.info")
    run.check("the daemon is at API level 16 and counts its scans", info_["api"] == 16 and "process_scans" in info_, info_.get("api"))
    run.check("the app knows the process API", d.js(f"return {S}.daemonApi()") >= 16)
    d.js("window.__confirms = []; window.confirm = (m) => { window.__confirms.push(m); return true; };")

    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    shell = ready_shell(d, c, pid, "dev")
    sid = shell["id"]
    root = processes(c, sid)["root"]["pid"]

    # 1. idle: nothing under a visible session, nothing printed -> no scans
    time.sleep(6)
    before = c.call("daemon.info")["process_scans"]
    time.sleep(10)
    idle = c.call("daemon.info")["process_scans"] - before
    run.check("a visible idle session with nothing under it makes no scans (10 s)", idle == 0, f"scans in 10 s: {idle}")
    run.check("…and the panel being closed adds none", not d.js("return !!document.querySelector('.proc-panel')"))

    # 2. a server: the chip within 6 s
    port = free_port()
    t0 = time.time()
    type_in(c, shell, server_cmd(port))
    seen = until(lambda: d.js("return !!document.querySelector('.tab.active .port-chip[data-port=\"%d\"]')" % port), 12, 0.1)
    took = time.time() - t0
    run.check("the :port chip is on the tab within 6 s", bool(seen) and took <= 6, f"{took:.2f} s")
    run.check("…and on the sidebar row", bool(d.js("return !!document.querySelector('.session-row .port-chip[data-port=\"%d\"]')" % port)))
    run.check("…and the port really listens", listening(port))
    if not seen:
        print("hidden:", d.js("return document.hidden"), "visible:", d.js(f"return {S}.isSessionVisible(arguments[0])", sid),
              "daemon:", json.dumps(c.call("process.ports", {"sessions": [sid]}))[:300], "scans:", c.call("daemon.info")["process_scans"], flush=True)
    run.shot("01-chip")

    # preview: click opens a browser tab; the menu opens it beside
    panes_before = d.js(f"return {S}.workspace({S}.activeProject().id).tabs.length")
    d.js("document.querySelector('.tab.active .port-chip[data-port=\"%d\"]').click()" % port)
    opened = until(lambda: d.js("return " + S + ".workspace(" + S + ".activeProject().id).tabs.some(t => JSON.stringify(t.root).includes('\"kind\":\"browser\"'))"), 10)
    run.check("clicking the chip opens a browser pane", bool(opened))
    url = d.js("return JSON.stringify(" + S + ".workspace(" + S + ".activeProject().id).tabs)")
    run.check("…on that port", f"http://localhost:{port}/" in url, url[:200] if f"http://localhost:{port}/" not in url else "")
    d.js(f"{S}.selectTab({S}.activeProject().id, {S}.workspace({S}.activeProject().id).tabs[0].id)")
    until(lambda: d.js("return !!document.querySelector('.tab.active .port-chip[data-port=\"%d\"]')" % port), 8)
    # Right-click and pick in one go, as a user does: the menu closes when the window loses focus (the browser
    # pane just opened may take it), so it is opened again if it went away before the pick.
    beside = (
        "const chip = document.querySelector('.tab.active .port-chip[data-port=\"%d\"]'); if (!chip) return false;"
        "chip.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 200, clientY: 200}));"
        "const item = [...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Open preview beside'));"
        "if (!item) return false; item.click(); return true;"
    ) % port
    run.check("the chip's menu offers \"Open preview beside\"", bool(until(lambda: d.js(beside), 8)))
    split = until(lambda: d.js("return " + S + ".activeTab(" + S + ".activeProject().id).root.type === 'split'"), 8)
    run.check("\"Open preview beside\" splits next to the terminal", bool(split))
    run.shot("02-preview")
    d.js(f"{S}.selectTab({S}.activeProject().id, {S}.workspace({S}.activeProject().id).tabs[0].id)")

    # 3. the panel, and four more servers for the scan time, from a second shell (the first one's foreground
    # runs the server, so what is typed there now waits for it to end)
    extra = [free_port() for _ in range(4)]
    shell2 = ready_shell(d, c, pid, "dev2")
    sid2 = shell2["id"]
    for p_ in extra:
        type_in(c, shell2, f"Start-Process -WindowStyle Hidden -FilePath '{PY}' -ArgumentList '-m','http.server','{p_}','--bind','127.0.0.1'")
    run.check("four more servers listen", bool(until(lambda: all(listening(p_) for p_ in extra), 30)))
    d.js(f"{S}.showRightPanel('processes')")
    rows = until(lambda: d.js("return document.querySelectorAll('.proc-row').length") >= 6, 15)
    run.check("the panel lists the shell and its programs", bool(rows), d.js("return document.querySelectorAll('.proc-row').length"))
    text = d.js("return document.querySelector('.proc-panel').innerText")
    run.check("…with a PID, memory and the ports", str(port) in text and ("MB" in text or "KB" in text), text[:200])
    run.check("every server's port is shown", all(f":{p_}" in text for p_ in [port] + extra))
    run.shot("03-panel")
    both = c.call("process.ports", {"sessions": [sid, sid2]})["sessions"]
    ports_seen = {p["port"] for s_ in (sid, sid2) for p in both[s_]["ports"]}
    run.check("process.ports names all five servers", ports_seen == set([port] + extra), ports_seen)
    # Scan cost with five servers running: one look at both sessions, as the chips ask
    ts = [c.call("process.ports", {"sessions": [sid, sid2]})["took_us"] for _ in range(40)]
    ls = [processes(c, sid2)["took_us"] for _ in range(20)]
    med, p95 = statistics.median(ts), sorted(ts)[int(len(ts) * 0.95) - 1]
    print(f"scan with 5 servers: process.ports median {med / 1000:.2f} ms, p95 {p95 / 1000:.2f} ms, max {max(ts) / 1000:.2f} ms; process.list median {statistics.median(ls) / 1000:.2f} ms", flush=True)
    # The budget is the release build's (scripts/perf/README.md); an unoptimized daemon only reports its time.
    if DEBUG_DAEMON:
        print(f"debug daemon: the 5 ms budget applies to the release build; measured median {med / 1000:.2f} ms", flush=True)
    else:
        run.check("a scan with five servers takes 5 ms or less (median)", med <= 5000, f"median {med / 1000:.2f} ms")

    # 4. Kill from the panel: preview, confirmation, the port is freed, the shell stays
    target_row = "[...document.querySelectorAll('.proc-row')].find(r => r.querySelector('.port-chip[data-port=\"%d\"]'))" % port
    d.js("window.__confirms.length = 0; (%s).querySelector('.link-btn').click()" % target_row)
    asked = until(lambda: d.js("return window.__confirms.length") == 1, 10)
    msg = d.js("return window.__confirms[0] || ''")
    run.check("Kill asks first and names the program and the port it frees", bool(asked) and f":{port}" in msg and "PID" in msg, msg.replace("\n", " | ")[:200])
    freed = until(lambda: not listening(port), 15)
    run.check("Kill frees the port", bool(freed))
    run.check("the other servers are untouched", all(listening(p_) for p_ in extra))
    gone = until(lambda: not d.js("return !!document.querySelector('.proc-row .port-chip[data-port=\"%d\"]')" % port), 10)
    run.check("the row leaves the panel and the chip leaves the tab", bool(gone))
    run.check("the shell session is still running", info(c, sid).get("alive") is True and alive(root))
    run.shot("04-killed")

    # 5. Kill children keeps the agent
    agent_port = free_port()
    open(os.path.join(WORK, "agent-port.txt"), "w").write(str(agent_port))
    ag = js_await(d, f"{S}.newSession(arguments[0], 'claude', {{name: 'agent'}})", pid)
    until(lambda: listening(agent_port), 30)
    agent_root = until(lambda: (processes(c, ag["id"])["root"] or {}).get("pid"), 15)
    run.check("the agent session runs a server of its own", listening(agent_port) and bool(agent_root))
    d.js(f"{S}.showRightPanel('processes')")
    btn = "document.querySelector('[data-session=\"%s\"] [data-act=\"kill-children\"]')" % ag["id"]
    until(lambda: d.js("return !!" + btn), 15)
    # The open panel looks every 2 s: wait until it has seen the server listen
    until(lambda: d.js("return !!document.querySelector('[data-session=\"%s\"] .port-chip[data-port=\"%d\"]')" % (ag["id"], agent_port)), 15)
    helper = int(until(lambda: helper_pid(), 15) or 0)
    d.js("window.__confirms.length = 0; " + btn + ".click()")
    until(lambda: d.js("return window.__confirms.length") == 1, 10)
    msg = d.js("return window.__confirms[0] || ''")
    run.check("Kill children asks first and says the agent and its own programs keep running", "keep running" in msg and f":{agent_port}" in msg, msg.replace("\n", " | ")[:200])
    run.check("…and says how Windows stops a console program", "console program" in msg if os.name == "nt" else True, msg.replace("\n", " | ")[-160:])
    run.check("Kill children frees the agent's port (a server its tool shell started)", bool(until(lambda: not listening(agent_port), 15)))
    time.sleep(1)
    run.check("…the agent process and its session are still running", alive(agent_root) and info(c, ag["id"]).get("alive") is True)
    run.check("…the agent's own helper (what it runs directly) too", bool(helper) and alive(helper), helper)
    run.check("…and the other session's servers too", all(listening(p_) for p_ in extra))

    # 5b. a restart: the same session runs a new process (in a new job); it is the session's own, and Kill
    # children stops what it started but never it
    c.call("session.kill", {"handle": ag["handle"]})
    run.check("the agent session ended", bool(until(lambda: info(c, ag["id"]).get("alive") is False, 15)))
    agent_port2 = free_port()
    open(os.path.join(WORK, "agent-port.txt"), "w").write(str(agent_port2))
    os.remove(os.path.join(WORK, "agent-helper.txt"))
    c.call("session.restart", {"handle": ag["handle"]})
    up = until(lambda: listening(agent_port2), 30)
    new_root = until(lambda: (lambda r: r if r and r != agent_root else None)((processes(c, ag["id"])["root"] or {}).get("pid")), 15)
    run.check("after a restart the session runs a new process with its server", bool(up) and bool(new_root), f"{agent_root} -> {new_root}")
    after = processes(c, ag["id"])
    run.check("…the new process is the session's own, not one of its children",
              after["root"] and after["root"]["pid"] == new_root and all(p["pid"] != new_root for p in after["children"] + after["orphans"]) and new_root in after["keep"],
              json.dumps({k: after[k] for k in ("root", "keep")})[:300])
    run.check("…and the session's own process cannot be stopped from the list",
              refused(c, "process.kill", {"session": ag["id"], "pid": new_root, "start_time": after["root"]["start_time"], "exe": after["root"]["exe"]}))
    helper2 = int(until(lambda: helper_pid(), 15) or 0)
    until(lambda: d.js("return !!document.querySelector('[data-session=\"%s\"] .port-chip[data-port=\"%d\"]')" % (ag["id"], agent_port2)), 15)
    d.js("window.__confirms.length = 0; " + btn + ".click()")
    until(lambda: d.js("return window.__confirms.length") == 1, 10)
    msg = d.js("return window.__confirms[0] || ''")
    run.check("…Kill children after the restart names the server, not the agent's process", f":{agent_port2}" in msg and f"PID {new_root})" not in msg, msg.replace("\n", " | ")[:200])
    run.check("…frees the port", bool(until(lambda: not listening(agent_port2), 15)))
    time.sleep(1)
    run.check("…and keeps the new process, its helper and the session", alive(new_root) and bool(helper2) and alive(helper2) and info(c, ag["id"]).get("alive") is True, f"root {new_root}, helper {helper2}")

    # 5c. a session that is itself the server (a custom command, nothing under it): its port has a chip too
    srv_port = free_port()
    srv = js_await(d, f"{S}.newSession(arguments[0], 'custom', {{args: arguments[1], name: 'server'}})", pid, [PY, "-m", "http.server", str(srv_port), "--bind", "127.0.0.1"])
    up = until(lambda: listening(srv_port), 30)
    srv_list = processes(c, srv["id"])
    srv_root = srv_list["root"] or {}
    run.check("a custom session runs the server as its own process", bool(up) and srv_root.get("name", "").lower().startswith("python") and srv_root.get("ports"), json.dumps(srv_root)[:200])
    t0 = time.time()
    chip = until(lambda: d.js("return !!document.querySelector('.tab.active .port-chip[data-port=\"%d\"]')" % srv_port), 12, 0.1)
    run.check("…and its :port chip is on its tab", bool(chip), f"{time.time() - t0:.2f} s after it listened")

    # 6. guards on the daemon
    list_ = processes(c, sid2)
    root2 = list_["root"]["pid"]
    kids = [p for p in list_["children"] if p["name"].lower().startswith("python")]
    run.check("the second shell lists its servers", len(kids) >= 4, len(kids))
    victim = kids[0]
    stranger = subprocess.Popen([PY, "-c", "import time; time.sleep(600)"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    sp = c.call("process.list", {"session": sid2})  # a fresh scan: the stranger is not in it
    run.check("a process the test started outside the session is not in its tree", all(x["pid"] != stranger.pid for x in sp["children"] + sp["orphans"]))
    base = {"session": sid2, "pid": victim["pid"], "start_time": victim["start_time"], "exe": victim["exe"]}
    bad = {
        "wrong start time": dict(base, start_time=victim["start_time"] + 1),
        "start time of another era": dict(base, start_time=1),
        "wrong program": dict(base, exe="C:\\elsewhere\\other.exe"),
        "a pid that is not in the tree": {"session": sid2, "pid": stranger.pid, "start_time": 1, "exe": None},
        "the session's own process": {"session": sid2, "pid": root2, "start_time": list_["root"]["start_time"]},
        "the daemon": {"session": sid2, "pid": c.call("daemon.info")["pid"], "start_time": 1},
        "an unknown session": dict(base, session="nope"),
    }
    for name, params in bad.items():
        run.check(f"refused: {name}", refused(c, "process.kill", params))
    run.check("nothing was stopped by any of them", all(listening(p_) for p_ in extra) and alive(stranger.pid) and alive(root2) and alive(victim["pid"]))
    # A PID reuse: the server ends on its own, its number is claimed with the identity it had.
    short = ready_shell(d, c, pid, "short")
    type_in(c, short, f"& '{PY}' -c \"import time; time.sleep(3)\"")
    seen_short = until(lambda: [p for p in processes(c, short["id"])["children"] if p["name"].lower().startswith("python")], 10)
    old = seen_short[0] if seen_short else None
    run.check("a short-lived program was listed", bool(old))
    if old:
        until(lambda: not alive(old["pid"]), 15)
        reuse = {"session": short["id"], "pid": old["pid"], "start_time": old["start_time"], "exe": old["exe"]}
        run.check("a listed program that has ended is not stopped (stale identity)", refused(c, "process.kill", reuse))
        # Its number now claimed for the unrelated process, with the old identity
        forged = {"session": short["id"], "pid": stranger.pid, "start_time": old["start_time"], "exe": old["exe"]}
        run.check("the old identity on an unrelated live PID is refused", refused(c, "process.kill", forged))
    run.check("the unrelated process is alive after all of it", alive(stranger.pid))
    # Programs in a session
    out = os.path.join(WORK, "probe-out.json")
    type_in(c, shell, f"& '{PY}' '{PROBE}' '{kelpied}' '{out}' '{sid}'")
    done = until(lambda: os.path.exists(out) and os.path.getsize(out) > 2, 180)
    res = json.load(open(out)) if done else {}
    run.check("a session's programs ran every probe", len(res) == 2 * 5 * 9, len(res))
    allowed = {k: v for k, v in res.items() if v[0] == 0}
    run.check("…none of the 90 calls (token or claimed, any shape) was answered", not allowed, list(allowed)[:3])
    run.check("…each was refused for what it is", all("not available" in v[1] or "must send `auth`" in v[1] or "params must be an object" in v[1] or "invalid" in v[1].lower() or "usage" in v[1].lower() or "missing" in v[1].lower() for v in res.values()),
              [v for v in res.values() if v[0] != 0][:2])
    run.check("…and everything is still where it was", listening(extra[0]) and alive(stranger.pid) and alive(root))


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
        scenario(run, c)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        teardown(run)
    run.finish()


def teardown(run):
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then the test
    daemon with its sessions (and so every server it started), then the driver tree and the stranger."""
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
        # A server a shell started with Start-Process is not in the session's job: stop each session's programs first.
        for s in c.call("session.list"):
            if s.get("alive"):
                try:
                    c.call("process.kill_children", {"session": s["id"]})
                except SystemExit:
                    pass
        c.call("daemon.shutdown", {"kill_sessions": True})
    except Exception:
        pass
    for p in procs.values():
        try:
            p.wait(timeout=5)
        except Exception:
            p.kill()
    if stranger is not None:
        stranger.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
