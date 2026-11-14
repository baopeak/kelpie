"""Agent browser control on the real app: an agent session drives browser panes through Kelpie's MCP tools (kelpie_browser_*).

    python -X utf8 scripts/e2e/agent_browser.py <scratch dir> <target/debug/kelpie.exe>

Debug app + the debug kelpied next to it, both on this script's own endpoint and data folder (set in this process's environment before anything
starts; the user's daemon and app are never reached), a local web site, and fake agent sessions (a Python program registered as a custom agent
that writes its session id and token to a file). The agent side is `kelpied mcp` run with a session's id and token, spoken to over stdio, as an
agent's MCP client would. The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts
from the repository unless the port is already taken, then it stops). Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching
WebView2 (PATH or MSEDGEDRIVER).

Checks: with browser control off, no browser tool is listed and a call is refused before the app; with it on (read and act), the tools and
the guide are listed, an agent opens the fixture with kelpie_open_page and tabs lists it as its own, in the background, with synthetic input
and a private profile; the snapshot is marked as a page's content, compact (at most 2 KB) and shows the fields without the password's value;
the agent types (the text is not echoed) and clicks through a sign-in and sees "Welcome" in what changed (the site receives the values; never
printed), reads a console error, waits for text (met, and not met with a clear failure), reloads, gets a confirm() dismissed and reported,
and an unknown ref is refused; eval is refused until turned on, then runs in the agent's pane; another project's session sees and uses
nothing, and a same-project session cannot use a pane opened for another session; timing of snapshot and click with what changed (printed,
p95 checked against 500 ms and 1 s); with the tab shown, an act right after the person's input is refused while reading still works; a pane
the person shares is listed only to that project's sessions, is readable, refuses navigation to another site and eval; a second session
cannot act while the first holds the pane; Stop on the banner ends access. Private session: the person's shared pane is not private, the
agent's sign-in on the fixture (a cookie) is not in the person's pane, the person's sign-in is not in an agent page opened after it, which is
listed as private and may go to https://example.com/ (without network only the absence of a refusal is checked). Audit: a line per act call
with session, tool and target and no typed text, and no file of the data folder holds the password. With the app gone, a call fails within
1 s and says the window is not open.
On Windows also: in the tab shown, tabs says trusted input and screenshots; a click is trusted (the page sees event.isTrusted) and the main
window keeps its keyboard focus; typing is trusted and lands; a key on an option that takes no focus and typing into a field that refuses focus on mousedown still land;
an agent's Control+V is synthetic and pastes nothing; a screenshot is a PNG at most 1280 px wide, saved to a file, and one element can be
pictured; the network list has POST 200 /api/login; with another program's window in front of the pane holding the foreground and the
keyboard (foreground_window.py), the agent's click and typing are trusted and land in the page, Kelpie does not take the foreground and that
window gets no click or key; with the tab in the background, tabs says so, a screenshot is refused and a click is synthetic. A phone given
control (paired on a fake Tailscale origin, its passkey set up) takes the agent's pane over: every browser tool of the agent is refused saying
a phone controls it and nothing it sent reaches the page; once the phone lets go the agent reads and acts again. Not covered here: macOS/Linux
behaviour.
"""
import base64
import json
import os
import queue
import re
import secrets
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback
from http.cookies import CookieError, SimpleCookie
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
    """Whether something listens on port `p` of this computer, over IPv4 or IPv6 (a dev server on "localhost" may take either)."""
    for host in ("127.0.0.1", "::1"):
        try:
            socket.create_connection((host, p), 0.5).close()
            return True
        except OSError:
            pass
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
<button id=trust onclick="this.textContent = 'Trusted ' + event.isTrusted">Check trust</button>
<div role=listbox aria-label="Fruits"><div role=option onkeydown="this.textContent = 'Picked ' + event.key">Apple</div></div>
<input id=sticky aria-label="Sticky" onmousedown="event.preventDefault()">
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

    def _send(self, code, body, ctype="text/html; charset=utf-8", headers=()):
        data = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.startswith("/login"):
            return self._send(200, LOGIN)
        if self.path.startswith("/whoami"):
            # Who the browser's cookie says signed in: the sign-in form (`f-`, the agent's sign-in) or the link (`l-`, the person's).
            jar = SimpleCookie()
            try:
                jar.load(self.headers.get("Cookie") or "")
            except CookieError:
                pass
            sid = jar["sid"].value if "sid" in jar else ""
            state = "Signed out" if not sid else "Signed in by the sign-in form" if sid.startswith("f-") else "Signed in by the link"
            return self._send(200, f"<!doctype html><title>Who am I</title><h1>{state}</h1>")
        if self.path.startswith("/signin"):
            return self._send(302, "", headers=(("Set-Cookie", f"sid=l-{secrets.token_hex(8)}; Path=/"), ("Location", "/whoami")))
        return self._send(200, "<!doctype html><title>Other page</title><h1>Other page</h1>")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        if self.path == "/api/login":
            try:
                LOGINS.append(json.loads(body))
            except ValueError:
                pass
            return self._send(200, '{"ok":true}', "application/json", (("Set-Cookie", f"sid=f-{secrets.token_hex(8)}; Path=/"),))
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
    if not ok:
        # Where the call got lost: does the window hear agents' requests, and does the app still reach the daemon?
        w.js_async("const done = arguments[arguments.length-1]; window.__seen = []; const I = window.__TAURI_INTERNALS__, orig = I.invoke; window.__inv = []; I.invoke = function (cmd, args, o) { const p = orig.call(this, cmd, args, o); if (cmd === 'provider_reply' || cmd === 'browser_agent_run') { const rec = [cmd, args && args.preq, 'pending', args && args.error]; window.__inv.push(rec); p.then(() => { rec[2] = 'ok'; }, (e) => { rec[2] = 'err ' + String(e); }); } return p; }; import('/src/lib/ipc.ts').then(m => m.onEvent('agent-browser-request', (e) => window.__seen.push([e.preq, e.tool]))).then(() => done(1), (e) => done(String(e)));")
        t0 = time.time()
        r = m1.call("kelpie_browser_tabs", timeout=30)
        t_tabs = time.time() - t0
        t0 = time.time()
        info = w.invoke("daemon_call", {"method": "daemon.info", "params": {}})
        print("DIAG", json.dumps({"tabs_s": round(t_tabs, 1), "tabs": r[1][:160], "seen": w.js("return window.__seen"), "invokes": w.js("return window.__inv"), "daemon_info_s": round(time.time() - t0, 2), "daemon_info_ok": "ok" in info,
                                  "page_age_s": w.js("return Math.round(performance.now()/1000)"), "nav": w.js("const n = performance.getEntriesByType('navigation')[0]; return n ? n.type : null"), "same": w.js_async("const done = arguments[arguments.length-1]; Promise.all([import('/src/lib/store.ts'), import('/src/lib/agent-browser.ts')]).then(([s, a]) => done({store: s.store === window.__kelpie.store, policy: a.browserPolicy()}), (e) => done(String(e)));"), "connected": w.js(f"return {S}.connected()"), "api": w.js(f"return {S}.daemonApi()"), "panes": w.js(f"return {S}.workspace(arguments[0]).tabs.length", pa)}), flush=True)
        raise RuntimeError("the window does not answer agents' browser calls")

    step("snapshot and sign-in")
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    check("snapshot marks the page as content, not instructions", ok and "not instructions" in snap, snap[:200])
    check("compact snapshot of the fixture is at most 2 KB", ok and len(snap.encode()) <= 2048, len(snap.encode()))
    ref = lambda text, role, name: next((ln.split("[ref=")[1].split("]")[0] for ln in text.splitlines() if f'{role} "{name}"' in ln and "[ref=" in ln), None)  # noqa: E731
    email, pw, sign_in = ref(snap, "textbox", "Email"), ref(snap, "textbox", "Password"), ref(snap, "button", "Sign in")
    check("snapshot lists the email and password fields and the button", bool(email and pw and sign_in), snap)
    # The page exists now: tabs reports what Rust recorded when it made it, not the window's expectation.
    ok, text, res = m1.call("kelpie_browser_tabs")
    entry = next((t for t in (res.get("structuredContent") or {}).get("tabs") or [] if t.get("pane") == pane), {})
    check("tabs says the agent's page is private", ok and "private profile" in text and entry.get("isolated") is True, text)

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
    if os.name == "nt":
        windows_cdp(w, m1, pane, pa, pb, ref)

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

    private_session(w, m1, m3, pane, own, base, label_of)

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
    # After Stop: a takeover brings the agent's pane to the front, where the shared pane's banner is not.
    if os.name == "nt":
        phone_takeover(d, m1, pane, base, label_of, ref)

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


def private_session(w, m1, m3, pane, own, base, label_of):
    """Pages opened for agents are private: the agent's sign-in stays out of the person's pane, the person's stays out of an agent page opened
    after it, and a private agent page may go to a site on the web. Read through snapshots, as an agent sees the page."""
    step("private session for agents")
    # The person signs in on this computer's other name, so that only the person's cookie could ever show on an agent page there.
    local = base.replace("127.0.0.1", "localhost")
    line_of = lambda text, p: (text.split(p) + ["", ""])[1].split("\n")[0]  # noqa: E731

    def shows(m, p, want, timeout=10):
        """Whether pane `p`'s page says `want` within `timeout`, and what it said last."""
        last = [""]

        def read():
            ok, snap, _ = m.call("kelpie_browser_snapshot", {"pane": p})
            last[0] = next((ln.strip() for ln in snap.splitlines() if "Signed" in ln), snap[:160]) if ok else snap[:200]
            return want in last[0]

        return bool(until(read, timeout, 0.5)), last[0]

    ok, text, _ = m3.call("kelpie_browser_tabs")
    check("the person's shared pane is not private", own in text and "private profile" not in line_of(text, own), line_of(text, own))

    # The agent signed in through the fixture's form in its own page: its private session has the cookie.
    ok, text, _ = m1.call("kelpie_browser_navigate", {"pane": pane, "url": base + "/whoami"})
    seen, said = shows(m1, pane, "Signed in by the sign-in form")
    check("the agent's page keeps the agent's sign-in", ok and seen, said or text[:200])
    ok, text, _ = m3.call("kelpie_browser_navigate", {"pane": own, "url": base + "/whoami"})
    seen, said = shows(m3, own, "Signed out")
    check("the person's own pane does not carry the agent's sign-in", ok and seen, said or text[:200])

    # The person signs in in their own pane (the window navigates it, as its address bar does).
    w.invoke("browser_navigate", {"label": label_of(own), "url": local + "/signin"})
    seen, said = shows(m3, own, "Signed in by the link")
    check("the person's pane is signed in once the person signs in", seen, said)
    ok, text, _ = m3.call("kelpie_browser_navigate", {"pane": own, "url": base + "/login"})
    check("the shared pane goes back to the sign-in page", ok and "/login" in text, text[:200])

    ok, text, _ = m1.call("kelpie_open_page", {"url": local + "/whoami", "title": "Who am I"})
    check("the agent opens another page after the person signed in", ok, text)

    def fresh_tab():
        _, _, res = m1.call("kelpie_browser_tabs")
        tabs = (res.get("structuredContent") or {}).get("tabs") or []
        return next((t for t in tabs if t.get("kind") == "opened" and t.get("url", "").startswith(local + "/whoami")), None)

    fresh = until(fresh_tab, 20)
    fresh_id = fresh["pane"] if fresh else ""
    seen, said = shows(m1, fresh_id, "Signed out", 15) if fresh_id else (False, "no such tab")
    check("an agent page opened after the person signed in is signed out", seen, said)
    # Asked again once the snapshot created the page, so that Rust's record answers.
    made = fresh_tab() if fresh_id else None
    check("the new agent page is listed as private", made and made.get("isolated") is True, made)

    # A private agent page may go to any site on the web: the policy must not refuse it. Whether the site loads depends on the network.
    ok, text, _ = m1.call("kelpie_browser_navigate", {"pane": fresh_id, "url": "https://example.com/", "timeout_s": 15}, timeout=60) if fresh_id else (False, "no such tab", {})
    # The call itself must work: it lands, or says the page did not load or is still loading (no network). A refusal or an error fails.
    went = ok and "Now at" in text
    check("a private agent page is not refused a site on the web", fresh_id and went and "may use only this computer's own sites" not in text, text[:200])
    if went and "example.com" in text and "(loaded)" in text:
        check("the private agent page went to the site on the web", True, text[:120])
    else:
        print("NOTE https://example.com/ did not load (no network?); only the absence of the policy's refusal was checked: " + text[:160].encode("ascii", "replace").decode(), flush=True)


def windows_cdp(w, m1, pane, pa, pb, ref):
    """Windows: trusted input and screenshots in the tab shown, synthetic input and a clear refusal in a background tab, the network list."""
    step("windows: trusted input, screenshot, network")
    line = lambda: (m1.call("kelpie_browser_tabs")[1].split(pane) + ["", ""])[1].split("\n")[0]  # noqa: E731
    check("tabs says trusted input and screenshots for the tab shown", until(lambda: "input: trusted" in line() and "screenshot: yes" in line(), 10), line())
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    trust = ref(snap, "button", "Check trust")
    # The person is typing in the main window (a text field of Kelpie's own, e.g. the pane's address bar): an agent's trusted click must not
    # take the keyboard from them.
    focus_js = ("const f = [...document.querySelectorAll('input:not([type]),input[type=text],input[type=search],input[type=url]')].find(e => e.offsetParent);"
                " if (arguments[0] && f) f.focus(); const a = document.activeElement;"
                " return {has: document.hasFocus(), active: a ? a.tagName + (a.id ? '#' + a.id : '') + (a.getAttribute('placeholder') ? '[' + a.getAttribute('placeholder') + ']' : '') : null, field: !!f};")
    before = w.js(focus_js, True)
    # The person's input a moment ago holds agents back for 10 s.
    clicked = until(lambda: (lambda r: r if r[0] else None)(m1.call("kelpie_browser_click", {"ref": trust})), 20, 1)
    after = w.js(focus_js, False)
    print("FOCUS", json.dumps({"before": before, "after": after}), flush=True)
    ok, text = (clicked or (False, ""))[:2]
    check("a click in the tab shown is trusted", ok and "(trusted)" in text, text)
    check("the main window keeps its keyboard focus and focused field through a trusted click", before and before.get("field") and after == before, {"before": before, "after": after})
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    check("the page saw event.isTrusted true", ok and 'button "Trusted true"' in snap, [ln for ln in snap.splitlines() if "Trusted" in ln])
    email = ref(snap, "textbox", "Email")
    ok, text, _ = m1.call("kelpie_browser_type", {"ref": email, "text": "trusted@example.test"})
    ok2, snap, _ = m1.call("kelpie_browser_snapshot")
    check("typing in the tab shown is trusted and lands in the field", ok and "(trusted)" in text and 'value="trusted@example.test"' in snap, text)
    # Elements a trusted step cannot reach the usual way still work, through the synthetic path: an option that takes no focus, a field
    # whose mousedown keeps the focus out.
    ok, text, _ = m1.call("kelpie_browser_press", {"key": "Enter", "ref": ref(snap, "option", "Apple")})
    ok2, snap2, _ = m1.call("kelpie_browser_snapshot")
    check("a key on an option that takes no focus still lands", ok and 'option "Picked Enter"' in snap2, text)
    ok, text, _ = m1.call("kelpie_browser_type", {"ref": ref(snap, "textbox", "Sticky"), "text": "abc"})
    ok2, snap2, _ = m1.call("kelpie_browser_snapshot")
    check("typing into a field that refuses focus on mousedown still lands", ok and 'textbox "Sticky"' in snap2 and 'value="abc"' in snap2, text)
    # The person's clipboard is out of an agent's reach: Control+V goes to the page only, as a synthetic key that pastes nothing.
    ok, text, _ = m1.call("kelpie_browser_press", {"key": "v", "modifiers": ["Control"], "ref": email})
    ok2, snap, _ = m1.call("kelpie_browser_snapshot")
    check("an agent's Control+V is synthetic and pastes nothing", ok and "synthetic" in text and 'value="trusted@example.test"' in snap, text)

    ok, text, res = m1.call("kelpie_browser_screenshot")
    images = [c for c in res.get("content", []) if c.get("type") == "image"]
    data = images[0].get("data", "") if images else ""
    size = re.search(r"\((\d+)×(\d+)\)", text)
    path = re.search(r"Saved (.+?) \(\d+×\d+\)", text)
    head = base64.b64decode(data[:64] + "=" * (-len(data[:64]) % 4)) if data else b""
    width = int.from_bytes(head[16:20], "big") if head.startswith(b"\x89PNG") else 0
    check("a screenshot of the tab shown is a PNG", ok and data.startswith("iVBORw0KGgo") and images[0].get("mimeType") == "image/png", text[:200])
    check("the screenshot is at most 1280 px wide", ok and size and 0 < int(size.group(1)) <= 1280 and 0 < width <= 1280, (size and size.group(0), width))
    check("the screenshot is saved to a file", ok and path and os.path.isfile(path.group(1).strip()), path and path.group(1))
    ok, text, _ = m1.call("kelpie_browser_screenshot", {"ref": trust})
    check("a screenshot of one element", ok and "Screenshot of [" in text, text[:200])

    ok, text, _ = m1.call("kelpie_browser_network", {"filter": "/api/login"})
    check("network lists the sign-in request", ok and "POST 200 /api/login" in text, text)
    another_program_in_front(w, m1, ref)

    # Another project shown: the pane is a background tab.
    w.js(f"{S}.switchProject(arguments[0])", pb)
    check("tabs says the hidden pane is a background tab with synthetic input", until(lambda: "background tab" in line() and "input: synthetic" in line(), 10), line())
    ok, text, _ = m1.call("kelpie_browser_screenshot")
    check("a screenshot of a background tab is refused and says why", not ok and "background" in text, text)
    ok, snap, _ = m1.call("kelpie_browser_snapshot")
    ok, text, _ = m1.call("kelpie_browser_click", {"ref": ref(snap, "button", "Count")})
    check("a click in a background tab still works, synthetic", ok and "(synthetic)" in text, text)
    w.js(f"{S}.switchProject(arguments[0])", pa)


def phone_takeover(d, m1, pane, base, label_of, ref):
    """A phone given control (paired on the fake Tailscale origin, its passkey set up) takes the agent's own pane over: every browser tool of the
    agent is refused while it holds the pane, saying a phone controls it, and nothing the agent sent reaches the page; once the phone lets go the
    agent reads and acts in the pane again. Windows: takeover streams through the WebView2 DevTools protocol."""
    step("phone takeover")
    import asyncio
    import types
    import screen_support as ss
    setup = types.SimpleNamespace(origin=f"{ps.FAKE_TAILNET_NAME}:8443", port=PHONE_PORT, ts_state=TS_STATE, ts_log=TS_LOG)

    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    async def agent(tool, args=None):
        return await asyncio.to_thread(m1.call, tool, {"pane": pane, **(args or {})})

    async def go():
        serve_port = await ss.phones_on(setup, call)
        phone, device, hello, _, _ = await ss.control_phone(setup, d, call, serve_port, "Agent browser phone")
        try:
            check("a phone paired on the Tailscale origin comes back unlocked, with control", hello.get("locked") is False and hello["device"]["permission"] == "control", hello.get("device"))
            await agent("kelpie_browser_navigate", {"url": base + "/login"})
            ok, snap, _ = await agent("kelpie_browser_snapshot")
            count, email = ref(snap, "button", "Count"), ref(snap, "textbox", "Email")
            before = await asyncio.to_thread(page_count, m1, pane)
            check("before the takeover the agent reads its pane", ok and bool(count), snap[:120])
            started = await phone.ask(type="tk_start", pane=label_of(pane), timeout=15)
            tk = (started or {}).get("tk")
            check("the phone takes the agent's pane over", bool(tk), started)
            tools = [("kelpie_browser_snapshot", {}), ("kelpie_browser_click", {"ref": count}), ("kelpie_browser_type", {"ref": email, "text": "while-held@example.test"}),
                     ("kelpie_browser_press", {"key": "Enter", "ref": count}), ("kelpie_browser_navigate", {"url": base + "/other"}), ("kelpie_browser_eval", {"expression": "1"}),
                     ("kelpie_browser_console", {}), ("kelpie_browser_network", {}), ("kelpie_browser_wait", {"text": "Sign in", "timeout_s": 1}),
                     ("kelpie_browser_screenshot", {})]
            answers = {}
            for tool, args in tools:
                ok, text, _ = await agent(tool, args)
                answers[tool] = (ok, text[:100])
            print("TAKEOVER answers", json.dumps(answers), flush=True)
            check("while the phone holds the pane every browser tool of the agent is refused, saying a phone controls it",
                  all(not ok and "phone" in text.lower() for ok, text in answers.values()), {k: v for k, v in answers.items() if v[0] or "phone" not in v[1].lower()})
            ok, text, _ = await asyncio.to_thread(m1.call, "kelpie_browser_tabs")
            print("TAKEOVER tabs", text.split(pane)[1].split("\n")[0] if pane in text else text[:200], flush=True)
            stopped = await phone.ask(type="tk_stop", tk=tk)
            check("the phone lets go", (stopped or {}).get("type") == "ack", stopped)
            back = await asyncio.to_thread(until, lambda: (lambda r: r if r[0] else None)(m1.call("kelpie_browser_snapshot", {"pane": pane})), 10)
            ok, snap, _ = back or (False, "", None)
            check("once the phone let go, the agent reads its pane again", ok, snap[:200])
            held = await asyncio.to_thread(page_count, m1, pane)
            check("…and nothing the agent sent while the phone held it reached the page", ok and held == before and "while-held@example.test" not in snap and "/other" not in snap.splitlines()[0], (before, held))
            ok, text, _ = await agent("kelpie_browser_click", {"ref": ref(snap, "button", "Count")})
            again = await asyncio.to_thread(until, lambda: (lambda n: n if n > before else None)(page_count(m1, pane)), 3)
            check("…and acts in it again", ok and again == before + 1, (text, again))
        finally:
            await phone.close()
            await call("mobile.revoke", {"id": device.id})

    asyncio.run(go())


def another_program_in_front(w, m1, ref):
    """Windows: another program's window has the foreground and the keyboard, over the whole pane (the person works in another app while an agent
    uses the tab shown). The agent's trusted click and typing land in the page under it (counted by the page itself, read with eval); Kelpie
    never takes the foreground (each change of it is logged, another program of the person's may take it); the window in front gets no click
    and no key. Its window is foreground_window.py, which logs what it gets."""
    step("windows: another program in front")
    rect = w.js("const h = [...document.querySelectorAll('.browser-host')].find(e => e.offsetParent); if (!h) return null;"
                " const r = h.getBoundingClientRect(), k = devicePixelRatio; return [r.left, r.top, r.width, r.height].map(v => Math.round(v * k));")
    log = os.path.join(scratch, f"foreground-{os.getpid()}.log")
    open(log, "w").close()
    probe = subprocess.Popen([sys.executable, os.path.join(HERE, "foreground_window.py"), log, app, *map(str, rect or [0, 0, 400, 300])], stdin=subprocess.PIPE)
    procs["foreground"] = probe
    lines = lambda: [json.loads(x) for x in open(log, encoding="utf-8").read().splitlines() if x.strip()]  # noqa: E731
    ready = until(lambda: next((x for x in lines() if "ready" in x or "error" in x), None), 20)
    hwnd = (ready or {}).get("ready")
    front = lambda: ctypes_user32().GetForegroundWindow() == hwnd  # noqa: E731
    check("another program's window is put in front of the pane, with the keyboard in its field", bool(hwnd) and ready.get("front") and ready.get("focus") and front(), {"rect": rect, "ready": ready})
    try:
        seen = len(lines())
        ok, snap, _ = m1.call("kelpie_browser_snapshot")
        before = page_count(m1)
        ok, text, _ = m1.call("kelpie_browser_click", {"ref": ref(snap, "button", "Count")})
        after = until(lambda: (lambda n: n if n > before else None)(page_count(m1)), 3) or page_count(m1)
        print("FOREGROUND click", json.dumps({"text": text.splitlines()[1:], "before": before, "after": after, "front": front()}), flush=True)
        check("with another program in front, the agent's click is trusted and lands in the page", ok and "(trusted)" in text and after == before + 1, (text, before, after))
        ok, text, _ = m1.call("kelpie_browser_type", {"ref": ref(snap, "textbox", "Email"), "text": "front@example.test"})
        ok2, snap2, _ = m1.call("kelpie_browser_snapshot")
        check("…the agent's typing is trusted and lands in the page's field", ok and "(trusted)" in text and 'value="front@example.test"' in snap2, text)
        time.sleep(0.5)
        got = lines()[seen:]
        moved = [e for e in got if e.get("event") == "foreground"]
        # Another program of the person's may take the foreground meanwhile (the probe logs each change, looked at every 20 ms): only the app's own counts.
        print("FOREGROUND changes", json.dumps({"changes": moved, "now": window_owner(ctypes_user32().GetForegroundWindow())}), flush=True)
        check("…Kelpie never takes the foreground, through the click and the typing", not any(e.get("app") for e in moved) and not window_owner(ctypes_user32().GetForegroundWindow())["this app"], moved)
        check("…and the window in front gets no click and no key", not [e for e in got if e.get("event") in ("button", "key")], got)
    finally:
        probe.stdin.close()
        try:
            probe.wait(10)
        except subprocess.TimeoutExpired:
            probe.kill()


def page_count(m, pane=None):
    """How many clicks the fixture's Count button has had, as the page itself counts them (eval in the agent's own pane; the snapshot does
    not show the count's text)."""
    ok, text, _ = m.call("kelpie_browser_eval", {"expression": "window.__count || 0", **({"pane": pane} if pane else {})})
    try:
        return int(text.splitlines()[-1]) if ok else -1
    except ValueError:
        return -1


def window_owner(hwnd):
    """Which program a window belongs to, and its title: for a failure's detail."""
    import ctypes
    from ctypes import wintypes
    u = ctypes_user32()
    pid = wintypes.DWORD()
    u.GetWindowThreadProcessId(wintypes.HWND(hwnd), ctypes.byref(pid))
    title = ctypes.create_unicode_buffer(256)
    u.GetWindowTextW(wintypes.HWND(hwnd), title, 256)
    out = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Process -Id {pid.value} -ErrorAction SilentlyContinue).Path"], capture_output=True, text=True).stdout.strip()
    return {"hwnd": hwnd, "pid": pid.value, "exe": out, "title": title.value, "this app": os.path.normcase(out) == os.path.normcase(app)}


def ctypes_user32():
    import ctypes
    from ctypes import wintypes
    u = ctypes.windll.user32
    u.GetForegroundWindow.restype = wintypes.HWND
    return u


# The phone that takes a pane over reaches this script's daemon through a fake Tailscale CLI (never the real one), at this port.
PHONE_PORT = 48100 + os.getpid() % 400
TS_BIN = TS_STATE = TS_LOG = None


def main():
    global SITE_PORT, TS_BIN, TS_STATE, TS_LOG
    site = ThreadingHTTPServer(("127.0.0.1", 0), Site)
    SITE_PORT = site.server_address[1]
    threading.Thread(target=site.serve_forever, daemon=True).start()
    if port_open(1427):
        raise SystemExit("port 1427 is taken (another Vite dev server?): this script starts the app's own; stop that one first")
    tailscale = os.path.join(scratch, f"agent-browser-tailscale-{os.getpid()}")
    os.makedirs(tailscale, exist_ok=True)
    TS_BIN, TS_STATE, TS_LOG = ps.write_fake_tailscale(tailscale)
    # The load pinned calm: a busy computer refuses a phone's takeover, and these checks are about agents, not load.
    d = ps.Daemon(kelpied, env={"KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": TS_BIN, "KELPIE_LOAD_FORCE": "L0:calm"}).start()
    procs["daemon"] = d.proc
    procs["vite"] = subprocess.Popen(["cmd", "/c", "pnpm", "exec", "vite", "--port", "1427", "--strictPort"] if os.name == "nt" else ["pnpm", "exec", "vite", "--port", "1427", "--strictPort"], cwd=os.path.join(REPO, "app"), stdout=open(os.path.join(scratch, "vite.log"), "w"), stderr=subprocess.STDOUT)
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
        if info.get("pid") != d.proc.pid:
            raise SystemExit("the app is not on this script's daemon: stopping before any scenario")
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
        shutil.rmtree(tailscale, ignore_errors=True)
        shutil.rmtree(DATA, ignore_errors=True)
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
