"""Agent browser control on the real app: an agent session drives browser panes through Kelpie's MCP tools (kelpie_browser_*).

    python -X utf8 scripts/e2e/agent_browser.py <scratch dir> <target/debug/kelpie.exe>

Debug app + the debug kelpied next to it, both on this script's own endpoint and data folder (set in this process's environment before anything
starts; the user's daemon and app are never reached), a local web site, and fake agent sessions (a Python program registered as a custom agent
that writes its session id and token to a file). The agent side is `kelpied mcp` run with a session's id and token, spoken to over stdio, as an
agent's MCP client would. The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts
from the repository unless the port is already taken, then it stops). Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching
WebView2 (PATH or MSEDGEDRIVER).

Checks: tools absent and calls refused while browser control is off; with it on, an agent opens the fixture with kelpie_open_page, lists it,
snapshots it (compact, no password value), types and clicks through a sign-in and sees "Welcome" in what changed (the site receives the values;
never printed), reads console errors, waits for text, reloads, gets a confirm() dismissed and reported; eval refused until turned on, then only
in a pane opened for the agent; refusals: another project's session sees nothing, a same-project session cannot use the agent's pane, a pane
the person shares is usable by the project's sessions but not for leaving its site, act within 10 s of the person's input, a second session
while the first holds the pane, Stop on the banner ends access; the audit has a line per act call with no typed text, and no file of the data
folder holds the password; with the app gone a call fails fast and says so; timing of snapshot and click (printed, checked against budgets).
"""
import json
import os
import queue
import secrets
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

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
DATA = ps.isolate("agent-browser")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-agent-browser-")
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

EMAIL = "ann-" + secrets.token_hex(3) + "@example.test"
PASSWORD = "pw-" + secrets.token_hex(10)
LOGINS = []
results = []
procs = {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail[:400]}]" if detail else ""), flush=True)


def step(what):
    print(f"-- {time.strftime('%H:%M:%S')} {what}", flush=True)


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
    try:
        socket.create_connection(("127.0.0.1", p), 0.5).close()
        return True
    except OSError:
        return False


LOGIN = """<!doctype html><html><head><title>Fixture sign-in</title></head><body>
<h1>Sign in</h1>
<form id=f><label for=email>Email</label><input id=email name=email type=email autocomplete=username>
<label for=pw>Password</label><input id=pw name=password type=password autocomplete=current-password>
<select id=plan aria-label="Plan"><option value=basic>Basic</option><option value=pro>Pro</option></select>
<button type=submit>Sign in</button></form>
<h2 id=msg></h2>
<button id=count onclick="window.__count=(window.__count||0)+1; document.getElementById('n').textContent='Clicked '+window.__count">Count</button> <span id=n></span>
<button id=del onclick="document.getElementById('cr').textContent = confirm('Delete everything?') ? 'confirmed' : 'kept'">Delete account</button> <span id=cr></span>
<button id=later onclick="setTimeout(() => { const p = document.createElement('p'); p.textContent = 'Ready now'; document.body.appendChild(p); }, 700)">Later</button>
<a href="https://example.com/">Elsewhere</a>
<script>
console.error("boom from the fixture");
document.getElementById('f').addEventListener('submit', (e) => {
  e.preventDefault();
  const x = new XMLHttpRequest(); x.open('POST', '/api/login'); x.setRequestHeader('Content-Type', 'application/json');
  x.onload = () => { document.getElementById('msg').textContent = x.status === 200 ? 'Welcome' : 'Refused'; };
  x.send(JSON.stringify({email: document.getElementById('email').value, password: document.getElementById('pw').value, plan: document.getElementById('plan').value}));
});
</script></body></html>"""


class Site(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="text/html; charset=utf-8"):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/login"):
            return self._send(200, LOGIN)
        return self._send(200, "<!doctype html><title>Other page</title><h1>Other page</h1>")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        if self.path == "/api/login":
            try:
                LOGINS.append(json.loads(body))
            except ValueError:
                pass
            return self._send(200, '{"ok":true}', "application/json")
        self._send(404, "no")


FAKE_AGENT = r'''
import json, os, sys, time
state = sys.argv[1]
sid = os.environ.get("KELPIE_SESSION_ID", "")
path = os.path.join(state, sid + ".json")
with open(path + ".tmp", "w", encoding="utf-8") as f:
    f.write(json.dumps({"id": sid, "token": os.environ.get("KELPIE_SESSION_TOKEN", "")}))
os.replace(path + ".tmp", path)
print("fake agent ready", flush=True)
for line in sys.stdin:
    print("ok", flush=True)
'''


class Mcp:
    """`kelpied mcp` run as a session, spoken to over stdio."""

    def __init__(self, sid, token):
        env = {**os.environ, "KELPIE_SESSION_ID": sid, "KELPIE_SESSION_TOKEN": token, "KELPIE_SESSION_ENDPOINT": ENDPOINT}
        self.p = subprocess.Popen([kelpied, "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env)
        self.q = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.n = 0
        self.init = self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _read(self):
        for line in self.p.stdout:
            try:
                self.q.put(json.loads(line))
            except ValueError:
                pass

    def send(self, msg):
        self.p.stdin.write((json.dumps(msg) + "\n").encode())
        self.p.stdin.flush()

    def rpc(self, method, params, timeout=60):
        self.n += 1
        n = self.n
        self.send({"jsonrpc": "2.0", "id": n, "method": method, "params": params})
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.q.get(timeout=max(0.05, end - time.time()))
            except queue.Empty:
                break
            if m.get("id") == n:
                return m
        raise TimeoutError(method)

    def tools(self):
        return [t["name"] for t in self.rpc("tools/list", {})["result"]["tools"]]

    def call(self, name, args=None, timeout=60):
        """(ok, text, result)."""
        r = self.rpc("tools/call", {"name": name, "arguments": args or {}}, timeout)
        if "error" in r:
            return False, r["error"].get("message", ""), r
        res = r["result"]
        text = "\n".join(c.get("text", "") for c in res.get("content", []) if c.get("type") == "text")
        return not res.get("isError"), text, res

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(5)
        except Exception:  # noqa: BLE001
            self.p.kill()


def session_call(sid, token, method, params):
    """A daemon call made as session `sid` (its token), as the MCP server and the CLI make them; returns (ok, value or refusal)."""
    user = ps.kc.user_token
    ps.kc.user_token = lambda: None
    try:
        c = ps.kc.Conn()
    finally:
        ps.kc.user_token = user
    try:
        c.call("auth", {"session_id": sid, "token": token})
        return True, c.call(method, params)
    except (SystemExit, Exception) as e:  # noqa: BLE001
        return False, str(e)


def pct(v, q):
    v = sorted(v)
    return v[min(len(v) - 1, int(len(v) * q))] if v else 0


S = "window.__kelpie.store"


def scenario(run, d):
    w = run.d
    work = os.path.join(scratch, f"agent-browser-{os.getpid()}")
    state = os.path.join(work, "state")
    proj_a, proj_b = os.path.join(work, "proj-a"), os.path.join(work, "proj-b")
    for p in (state, proj_a, proj_b):
        os.makedirs(p, exist_ok=True)
    fake_py = os.path.join(state, "fake_agent.py")
    with open(fake_py, "w", encoding="utf-8") as f:
        f.write(FAKE_AGENT)
    d.call("agent.set_custom", {"agents": [{"id": "fake", "label": "Fake agent", "bin": sys.executable, "launch": [fake_py, state], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []}]})
    pa = w.js(f"return {S}.addProject(arguments[0])", proj_a)
    pb = w.js(f"return {S}.addProject(arguments[0])", proj_b)

    def agent(name, project, cwd):
        s = d.call("session.create", {"kind": "fake", "cwd": cwd, "name": name, "project_id": project})
        tok = until(lambda: json.load(open(os.path.join(state, s["id"] + ".json"), encoding="utf-8")), 60)
        if not tok:
            raise RuntimeError(f"fake agent {name} did not start")
        return s["id"], tok["token"]

    step("starting three fake agent sessions")
    s1, t1 = agent("opener", pa, proj_a)
    s2, t2 = agent("other-project", pb, proj_b)
    s3, t3 = agent("same-project", pa, proj_a)
    base = f"http://127.0.0.1:{SITE_PORT}"
    label_of = lambda pane: "browser-" + pane  # noqa: E731

    step("policy off")
    # Off by default.
    m = Mcp(s1, t1)
    names = m.tools()
    check("browser control off: no browser tools listed", not any(n.startswith("kelpie_browser_") for n in names), [n for n in names if "browser" in n])
    ok, v = session_call(s1, t1, "browser.call", {"tool": "tabs", "args": {}})
    check("browser control off: a direct call is refused before the app", not ok and "Browser control is off" in v, v)
    m.close()

    step("policy act")
    d.call("daemon.set_policy", {"browser_agents": "act"})
    w.js_async("const done = arguments[arguments.length-1]; import('/src/lib/agent-browser.ts').then(m => m.refreshBrowserPolicy()).then(() => done(1), () => done(0));")
    m1 = Mcp(s1, t1)
    names = m1.tools()
    check("browser control on: browser tools listed", "kelpie_browser_snapshot" in names and "kelpie_browser_click" in names, len(names))
    check("the guide is in the server's instructions", "snapshot" in json.dumps(m1.init).lower())

    step("open page")
    ok, text, _ = m1.call("kelpie_open_page", {"url": base + "/login", "title": "Fixture sign-in"})
    check("kelpie_open_page opens the fixture", ok, text)

    tabs = until(lambda: (lambda r: r if r[0] and "/login" in r[1] else None)(m1.call("kelpie_browser_tabs")), 20)
    ok, text, res = tabs or (False, "", {})
    tabs_data = (res.get("structuredContent") or {}).get("tabs") or []
    pane = tabs_data[0]["pane"] if tabs_data else ""
    check("tabs lists the page as the agent's, in the background, synthetic input", ok and "opened by you" in text and "background" in text and "synthetic" in text, text)

    step("snapshot and sign-in")
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    check("snapshot marks the page as content, not instructions", ok and "not instructions" in snap, snap[:200])
    check("compact snapshot of the fixture is at most 2 KB", ok and len(snap.encode()) <= 2048, len(snap.encode()))
    ref = lambda text, role, name: next((ln.split("[ref=")[1].split("]")[0] for ln in text.splitlines() if f'{role} "{name}"' in ln and "[ref=" in ln), None)  # noqa: E731
    email, pw, sign_in = ref(snap, "textbox", "Email"), ref(snap, "textbox", "Password"), ref(snap, "button", "Sign in")
    check("snapshot lists the email and password fields and the button", bool(email and pw and sign_in), snap)

    ok, text, _ = m1.call("kelpie_browser_type", {"ref": email, "text": EMAIL})
    check("type into the email field (text not echoed)", ok and EMAIL not in text, text)
    ok, text, _ = m1.call("kelpie_browser_type", {"ref": pw, "text": PASSWORD})
    check("type into the password field (text not echoed)", ok and PASSWORD not in text, text)
    ok, snap2, _ = m1.call("kelpie_browser_snapshot")
    check("the password's value never appears in a snapshot", ok and PASSWORD not in snap2 and EMAIL in snap2, "")
    ok, text, _ = m1.call("kelpie_browser_click", {"ref": sign_in})
    welcome = "Welcome" in text or until(lambda: "Welcome" in m1.call("kelpie_browser_snapshot")[1], 5)
    check("click Sign in: what changed shows Welcome", ok and welcome, text)
    check("the site received the typed values", until(lambda: LOGINS and LOGINS[-1].get("email") == EMAIL and LOGINS[-1].get("password") == PASSWORD, 5), len(LOGINS))

    step("console, wait, reload, dialog")
    ok, text, _ = m1.call("kelpie_browser_console", {"level": "error"})
    check("console returns the page's error", ok and "boom from the fixture" in text, text)
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    m1.call("kelpie_browser_click", {"ref": ref(snap, "button", "Later")})
    ok, text, _ = m1.call("kelpie_browser_wait", {"text": "Ready now", "timeout_s": 5})
    check("wait for text that appears later", ok, text)
    ok, text, _ = m1.call("kelpie_browser_wait", {"text": "never there", "timeout_s": 1})
    check("wait for text that never appears fails and says so", not ok and "Not met" in text, text)
    ok, text, _ = m1.call("kelpie_browser_navigate", {"action": "reload"})
    check("navigate reload", ok and "/login" in text, text)
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    ok, text, _ = m1.call("kelpie_browser_click", {"ref": ref(snap, "button", "Delete account")})
    check("a confirm() is dismissed and reported", ok and "confirm" in text.lower() and "dismissed" in text, text)
    ok, text, _ = m1.call("kelpie_browser_click", {"ref": "e99999"})
    check("an unknown ref is refused", not ok, text)

    step("eval")
    ok, text, _ = m1.call("kelpie_browser_eval", {"expression": "1+1"})
    check("eval refused while scripts are off", not ok, text)
    d.call("daemon.set_policy", {"browser_eval": True})
    ok, text, _ = m1.call("kelpie_browser_eval", {"expression": "document.title"})
    check("eval allowed in the agent's own pane once turned on", ok and "Fixture sign-in" in text, text)

    step("other sessions")
    # Other sessions.
    m2 = Mcp(s2, t2)
    ok, text, res = m2.call("kelpie_browser_tabs")
    check("another project's session sees no pane", pane and pane not in text, text)
    ok, text, _ = m2.call("kelpie_browser_snapshot", {"pane": pane})
    check("another project's session cannot use the pane", not ok, text)
    m3 = Mcp(s3, t3)
    ok, text, _ = m3.call("kelpie_browser_snapshot", {"pane": pane})
    check("a same-project session cannot use a pane opened for another session", not ok, text)

    step("timing")
    # Timing on the fixture (background tab).
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    count = ref(snap, "button", "Count")
    snaps, clicks = [], []
    for _ in range(20):
        t0 = time.perf_counter()
        m1.call("kelpie_browser_snapshot")
        snaps.append((time.perf_counter() - t0) * 1000)
        t0 = time.perf_counter()
        m1.call("kelpie_browser_click", {"ref": count})
        clicks.append((time.perf_counter() - t0) * 1000)
    check("snapshot p95 under 500 ms", pct(snaps, 0.95) < 500, f"p50 {pct(snaps, 0.5):.0f} ms, p95 {pct(snaps, 0.95):.0f} ms")
    check("click with what changed p95 under 1 s", pct(clicks, 0.95) < 1000, f"p50 {pct(clicks, 0.5):.0f} ms, p95 {pct(clicks, 0.95):.0f} ms")

    step("shown tab and person input")
    # Show the tab: the person sees it; their input wins for 10 s.
    tab = w.js(f"const p = arguments[0], id = arguments[1]; const t = {S}.workspace(p).tabs.find(t => JSON.stringify(t.root).includes(id)); if (t) {{ {S}.switchProject(p); {S}.focusPane(p, t.id, id); }} return t ? t.id : null;", pa, pane)
    shown = until(lambda: "shown" in m1.call("kelpie_browser_tabs")[1].split(pane)[1].split("\n")[0], 10)
    check("the tab shown by the person says shown", tab and shown, tab)
    w.invoke("browser_test_input", {"label": label_of(pane), "events": [
        {"method": "Input.dispatchMouseEvent", "params": {"type": "mousePressed", "x": 5, "y": 5, "button": "left", "clickCount": 1}},
        {"method": "Input.dispatchMouseEvent", "params": {"type": "mouseReleased", "x": 5, "y": 5, "button": "left", "clickCount": 1}}]})
    ok, text, _ = m1.call("kelpie_browser_click", {"ref": count})
    check("an act right after the person's input is refused", not ok and ("person" in text.lower() or "user" in text.lower()), text)
    ok, text, _ = m1.call("kelpie_browser_snapshot")
    check("reading is still allowed then", ok, text[:120])

    step("shared pane")
    # A pane the person opens and shares.
    own = w.js(f"const p = arguments[0]; const id = 'own' + Date.now(); {S}.addTab(p, {{id, kind: 'browser', url: arguments[1]}}); return id;", pa, base + "/login")
    until(lambda: "ok" in w.invoke("browser_info", {"label": label_of(own)}), 15)
    ok, text, _ = m3.call("kelpie_browser_tabs")
    check("a pane the person did not share is not listed", own not in text, text)
    until(lambda: w.js("return !!document.querySelector('[data-agent-share]:not([disabled])')"), 10)
    w.js("const b = [...document.querySelectorAll('[data-agent-share]')].find(b => b.offsetParent); b && b.click();")
    listed = until(lambda: (lambda r: r if own in r[1] and "shared" in r[1] else None)(m3.call("kelpie_browser_tabs")), 10)
    check("after Share, the project's other session lists it as shared", listed, listed and listed[1])
    ok, text, _ = m3.call("kelpie_browser_snapshot", {"pane": own})
    check("the project's session reads the shared pane", ok and "Sign in" in text, text[:120])
    ok, text, _ = m3.call("kelpie_browser_navigate", {"pane": own, "url": "https://example.com/"})
    check("a shared pane may not be taken to another site", not ok, text)
    ok, text, _ = m3.call("kelpie_browser_eval", {"pane": own, "expression": "1"})
    check("eval is refused in a shared pane", not ok, text)
    ok, text, _ = m2.call("kelpie_browser_tabs")
    check("another project's session does not see the shared pane", own not in text, text)

    step("lease")
    # One session acts in a pane at a time.
    ok, snap, _ = m3.call("kelpie_browser_snapshot", {"pane": own})
    m3.call("kelpie_browser_click", {"pane": own, "ref": ref(snap, "button", "Count")})
    ok, text, _ = m1.call("kelpie_browser_snapshot", {"pane": own})
    check("the opener's session also may use the shared pane of its project", ok, text[:120])
    ok, text, _ = m1.call("kelpie_browser_click", {"pane": own, "ref": ref(text, "button", "Count")})
    check("a second session cannot act while the first holds the pane", not ok and ("using" in text.lower() or "another" in text.lower()), text)

    step("stop")
    # Stop on the banner.
    banner = until(lambda: w.js("return !!document.querySelector('[data-agent-stop]')"), 5)
    w.js("const b = [...document.querySelectorAll('[data-agent-stop]')].find(b => b.offsetParent); b && b.click();")
    ok, text, _ = m3.call("kelpie_browser_snapshot", {"pane": own})
    check("Stop on the banner ends the agents' access to the pane", banner and not ok, text)

    step("audit")
    # Audit.
    entries = d.call("browser.audit", {"limit": 200}).get("entries", [])
    acts = [e for e in entries if e.get("tool") in ("click", "type", "press", "navigate", "eval")]
    check("the audit has lines for act calls with session, tool and target", len(acts) >= 5 and all(e.get("session") and e.get("tool") for e in acts), len(acts))
    check("no audit line holds the typed text", not any(PASSWORD in json.dumps(e) or EMAIL in json.dumps(e) for e in entries))
    leaks = []
    for root, _, files in os.walk(DATA):
        for f in files:
            try:
                if PASSWORD.encode() in open(os.path.join(root, f), "rb").read():
                    leaks.append(os.path.relpath(os.path.join(root, f), DATA))
            except OSError:
                pass
    check("no file of the data folder holds the password", not leaks, leaks)

    for x in (m2, m3):
        x.close()
    return m1


def main():
    global SITE_PORT
    site = ThreadingHTTPServer(("127.0.0.1", 0), Site)
    SITE_PORT = site.server_address[1]
    threading.Thread(target=site.serve_forever, daemon=True).start()
    d = ps.Daemon(kelpied, env={"KELPIE_E2E": "1"}).start()
    procs["daemon"] = d.proc
    if port_open(1427):
        d.stop()
        raise SystemExit("port 1427 is taken (another Vite dev server?): this script starts the app's own; stop that one first")
    procs["vite"] = subprocess.Popen(["cmd", "/c", "pnpm", "exec", "vite", "--port", "1427", "--strictPort"] if os.name == "nt" else ["pnpm", "exec", "vite", "--port", "1427", "--strictPort"], cwd=os.path.join(REPO, "app"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    until(lambda: port_open(1427), 60)
    driver_port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
    procs["driver"] = subprocess.Popen([TAURI_DRIVER, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", NATIVE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run = m1 = None
    try:
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(scratch, "shots"), app=app)
        run.boot()
        info = run.d.invoke("daemon_call", {"method": "daemon.info", "params": {}}).get("ok") or {}
        check("the app is connected to this script's daemon", info.get("pid") == d.proc.pid, info.get("pid"))
        m1 = scenario(run, d)
        # The app goes away (ending the WebDriver session closes it): a call fails fast and says why.
        run.d.quit()
        run = None
        gone = until(lambda: not app_running(), 20)
        time.sleep(1)  # the daemon notices the app's connection close
        t0 = time.perf_counter()
        ok, text, _ = m1.call("kelpie_browser_tabs")
        ms = (time.perf_counter() - t0) * 1000
        check("with the app closed a call fails within 1 s and says the window is not open", gone and not ok and "window" in text.lower() and ms < 1000, f"{ms:.0f} ms: {text}")
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        if m1:
            m1.close()
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
        site.shutdown()
        shutil.rmtree(os.path.join(scratch, f"agent-browser-{os.getpid()}"), ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


def app_running():
    """Whether the app this script's driver started still runs (tauri-driver ends it with the session)."""
    if os.name != "nt":
        return False
    out = subprocess.run(["powershell", "-NoProfile", "-Command", f"@(Get-CimInstance Win32_Process -Filter \"Name='kelpie.exe'\" | Where-Object {{ $_.ExecutablePath -eq '{app}' }}).Count"], capture_output=True, text=True).stdout.strip()
    return out not in ("", "0")


if __name__ == "__main__":
    main()
