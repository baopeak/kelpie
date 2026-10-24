"""GUI e2e for off-machine notifications and the usage-limit actions.

    python -X utf8 scripts/e2e/notify.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder, debug logging to a file) and tauri-driver
with the same environment. Nothing leaves this computer: a local HTTP server on 127.0.0.1 stands in for ntfy,
the webhook and (through the test-only address override) the Telegram API. The agent is a stand-in `claude`
(a batch shim on PATH); its hooks are reported through `hook.report`. The channels' secrets go to the
credential store under this run's endpoint name only (the real app's entries are never read or written),
and are removed at the end.

1. With no channel on, an event reaches the inbox and nothing is sent.
2. Settings -> Notifications -> Off this computer, driven in the page: ntfy (random 128-bit topic, token,
   rotate), a webhook and Telegram with their test buttons; the screen-text warning; the secrets are never
   read back and appear in no file or log of the data folder.
3. Desktop and phone agree on suppression: a focused window that shows the session gets neither an inbox
   entry nor a message; the same event for a session it does not show gives exactly one of each.
4. The limit banner: a Claude session stopped by the usage limit shows it; a time the user confirms sets the
   daemon's timer, Cancel clears it; a made-up source, a past time and a session that is not limited are refused.
5. "Continue at reset" types `continue` only into a session that is still limited, in the same run and
   conversation, with nothing typed since: six sessions share one reset time and five are changed in
   different ways before it; only the untouched one gets the text, once, and each of the others leaves an
   inbox note.
6. Switch account: refused with the reason (not signed in; a different conversation already there) with the
   session left running; then done: the conversation is copied, the session restarts on the other account's
   home.
7. With the window closed: needs-you sends exactly one message to each channel; the same session again within
   30 s sends nothing; failed, done (with the turn-length filter), the event switches, the away filter, screen
   text (redacted, off by default) and a secret in the session's name; a server error is reported without
   its address.
8. Cleanup leaves no credential-store entry of this run.
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
import threading
import time
import traceback
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

# An isolated daemon: its own endpoint, data folder and credential-store service. The endpoint name is what
# keeps this run's secrets apart from the real app's ("Kelpie (<endpoint>)" against plain "Kelpie").
ENDPOINT = f"kelpie-notify-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-notify-data-")
WORK = os.path.join(scratch, f"notify-{os.getpid()}")
CLAUDE_HOME = os.path.join(WORK, "claude-home")
CODEX_HOME = os.path.join(WORK, "codex-home")
BIN = os.path.join(WORK, "bin")
PROJ_NAME = f"notify-proj-{os.getpid()}"
PROJ = os.path.join(WORK, PROJ_NAME)
OTHER_NAME = f"notify-other-{os.getpid()}"
OTHER = os.path.join(WORK, OTHER_NAME)
for d in (CLAUDE_HOME, CODEX_HOME, BIN, PROJ, OTHER):
    os.makedirs(d, exist_ok=True)
assert ENDPOINT.startswith("kelpie-notify-"), ENDPOINT


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT, SINK_PORT = free_port(), free_port()
SINK = f"http://127.0.0.1:{SINK_PORT}"
os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "KELPIE_NOTIFY_TELEGRAM_BASE": SINK, "KELPIE_NOTIFY_RESET_MARGIN_MS": "500",
    "CLAUDE_CONFIG_DIR": CLAUDE_HOME, "CODEX_HOME": CODEX_HOME,
    "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
    "KELPIE_E2E_DRIVER": f"http://127.0.0.1:{DRIVER_PORT}",
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)

NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver")
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver on PATH and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

# A stand-in `claude`: says which config home it runs on, and echoes every line it is typed.
FAKE = os.path.join(WORK, "fake_claude.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(
        "import msvcrt, os\n"
        "print('READY cfg=' + os.environ.get('CLAUDE_CONFIG_DIR', ''), flush=True)\n"
        "print('TOKEN=' + os.environ.get('KELPIE_SESSION_TOKEN', ''), flush=True)\n"
        "buf = ''\n"
        "esc = False\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch == '\\x1b':\n"
        "        esc = True\n"
        "        continue\n"
        "    if esc:\n"
        "        esc = not (ch.isalpha() or ch == '~')\n"
        "        continue\n"
        "    if ch in '\\r\\n':\n"
        "        if buf:\n"
        "            print('GOT:' + buf, flush=True)\n"
        "        buf = ''\n"
        "    else:\n"
        "        buf += ch\n"
    )
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" %*\r\n')

procs = {}
S = "window.__kelpie.store"
SECRET_KEY = "ghp_0123456789abcdefghijABCDEFGHIJ012345"
TOKEN_NTFY, TOKEN_BOT, CHAT = "tk_e2e_ntfy_token_9f3a", "123456:E2Ebottoken_zz91", "-1001234567"
HOOK_PATH = "/hook/e2e-secret-path-4412"


# ---------- the local server standing in for ntfy, the webhook and Telegram ----------

class Sink:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = []
        sink = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                n = int(self.headers.get("Content-Length") or 0)
                raw = self.rfile.read(n) if n else b""
                try:
                    body = json.loads(raw)
                except ValueError:
                    body = None
                with sink.lock:
                    sink.requests.append({"path": self.path, "auth": self.headers.get("Authorization"), "body": body, "at": time.time()})
                code = 500 if self.path.startswith("/boom") else 200
                self.send_response(code)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"{}")

            def log_message(self, *a):  # addresses and paths are not logged
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", SINK_PORT), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def all(self):
        with self.lock:
            return list(self.requests)

    def by(self, kind):
        """ntfy posts to /, the webhook to HOOK_PATH, Telegram to /bot…/sendMessage."""
        def match(r):
            p = r["path"]
            return {"ntfy": p == "/", "webhook": p == HOOK_PATH, "telegram": p.startswith("/bot") and p.endswith("/sendMessage"), "boom": p.startswith("/boom")}[kind]
        return [r for r in self.all() if match(r)]

    def clear(self):
        with self.lock:
            self.requests.clear()

    def count(self):
        return len(self.all())

    def wait_for(self, n, seconds=15):
        end = time.time() + seconds
        while time.time() < end and self.count() < n:
            time.sleep(0.1)
        return self.count() >= n


sink = Sink()


def until(fn, timeout=30, step=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def stays(fn, seconds):
    end = time.time() + seconds
    while time.time() < end:
        if not fn():
            return False
        time.sleep(0.25)
    return True


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


class Rpc:
    """The test's own connection to the daemon, with the user's token."""

    def __init__(self):
        self.conn = kc.Conn()

    def __call__(self, method, params=None):
        try:
            return self.conn.call(method, params or {})
        except SystemExit as e:  # kelpie-call exits on an error reply
            raise RuntimeError(str(e)) from None

    def close(self):
        """Hangs up: the daemon forgets what this client said about its window."""
        try:
            self.conn.f.close()
        except (AttributeError, OSError):
            pass


def refused(fn):
    """The error text of a call that is refused, else None."""
    try:
        fn()
    except RuntimeError as e:
        return str(e)
    return None


def hook(rpc, sid, event, **payload):
    rpc("hook.report", {"session_id": sid, "agent": "claude", "payload": {"hook_event_name": event, **payload}})


def info(rpc, sid):
    return next((s for s in rpc("session.list") if s["id"] == sid), None)


def wait_status(rpc, sid, status, timeout=10):
    return until(lambda: (i := info(rpc, sid)) and i["status"] == status and i, timeout, 0.1)


def screen(rpc, handle, lines=60):
    return rpc("session.read", {"handle": handle, "lines": lines})["text"]


def make_limited(rpc, sid):
    hook(rpc, sid, "UserPromptSubmit")
    wait_status(rpc, sid, "working")
    hook(rpc, sid, "StopFailure", error="rate_limit")
    return until(lambda: (i := info(rpc, sid)) and i.get("limited") and i, 10, 0.1)


def start_working(rpc, sid):
    hook(rpc, sid, "UserPromptSubmit")
    return wait_status(rpc, sid, "working")


def new_session(rpc, name, cwd=PROJ, project_id=None):
    s = rpc("session.create", {"kind": "claude", "cwd": cwd, "name": name, "project_id": project_id})
    # From the first moment a hook, not the start-up output, says what the agent is doing: output alone
    # would make a short "turn" of the banner, and its end an event of its own.
    hook(rpc, s["id"], "UserPromptSubmit")
    until(lambda: "READY" in screen(rpc, s["handle"]), 20)
    return s


def said(message, name, summary):
    """The message is "<name>: <summary>", with the turn's length after it once it took a second."""
    return re.fullmatch(re.escape(f"{name}: {summary}") + r"( \(\d+[hms]( \d+[ms])?\))?", message or "") is not None


def os_idle_seconds():
    """Seconds since the last keyboard or mouse input (Windows), else None."""
    if os.name != "nt":
        return None
    import ctypes

    class LastInput(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_uint), ("dwTime", ctypes.c_uint)]

    info = LastInput(ctypes.sizeof(LastInput), 0)
    if not ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info)):
        return None
    return ((ctypes.windll.kernel32.GetTickCount() - info.dwTime) & 0xFFFFFFFF) / 1000


def timers():
    return rpc("session.scheduled")["timers"]


def inbox(d):
    return d.js(f"return {S}.state.inbox.map(i => ({{kind: i.kind, title: i.title, body: i.body, sessionId: i.sessionId}}))")


def credential_targets():
    """Targets in the Windows credential store (names only)."""
    if os.name != "nt":
        return set()
    out = subprocess.run(["cmdkey", "/list"], capture_output=True, text=True, errors="replace").stdout
    return {line.split(":", 1)[1].strip() for line in out.splitlines() if line.strip().lower().startswith(("target:", "ziel:"))}


def spawn(name, args, **kw):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), **kw)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


# ---------- the page ----------

def set_value(d, css, value):
    d.js("const el = document.querySelector(arguments[0]); el.value = arguments[1]; el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true}));", css, value)


def click_text(d, css, text):
    return d.js("const b = [...document.querySelectorAll(arguments[0])].find(x => x.textContent.trim() === arguments[1] && !x.disabled); if (!b) return false; b.click(); return true;", css, text)


def row(name):
    return f'[data-setting="{name}"]'


def row_text(d, name):
    """The text of a settings row ('' while the page redraws it)."""
    return d.js("const el = document.querySelector(arguments[0]); return el ? el.innerText : ''", row(name))


def toggle_on(d, name):
    return d.js("const b = document.querySelector(arguments[0] + ' button.switch'); return !!b && b.classList.contains('on')", row(name))


def sink_settings(d, run):
    """Scenario 1."""
    d.js(f"{S}.setSettings({{ language: 'en' }}); {S}.setSettingsSection('notifications'); {S}.setOverlay('settings');")
    d.wait("return !!document.querySelector('[data-setting=\"push-ntfy\"]')", 20)
    run.check("the off-computer settings are there (daemon API 12)", d.js("return !!document.querySelector('[data-setting=\"push-webhook\"]')"))

    # ntfy: on, server, topic, token.
    d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-ntfy"))
    d.wait("return !!document.querySelector(arguments[0] + ' input[placeholder=\"https://ntfy.sh\"]')", 10, 0.25, row("push-ntfy"))
    set_value(d, row("push-ntfy") + ' input[placeholder="https://ntfy.sh"]', SINK)
    until(lambda: rpc("notify.get")["config"]["ntfy"]["server"] == SINK, 10)
    run.check("ntfy is switched on in the page and the server saved by the daemon", rpc("notify.get")["config"]["ntfy"] == {"enabled": True, "server": SINK})
    secrets = rpc("notify.get")["secrets"]
    run.check("switching ntfy on made a topic; the daemon reply names no secret value", secrets["ntfy_topic"] is True and "ntfy_topic" not in rpc("notify.get") and SINK in json.dumps(rpc("notify.get")))
    click_text(d, row("push-ntfy") + " button", "Show topic")
    topic = until(lambda: d.js("const e = document.querySelector('[data-ntfy-topic]'); return e && e.textContent"), 10)
    run.check("the topic shown is 32 hex digits (128 random bits)", bool(topic) and re.fullmatch(r"[0-9a-f]{32}", topic) is not None, str(topic))
    state["topic"] = topic
    set_value(d, row("push-ntfy") + ' input[type="password"]', TOKEN_NTFY)
    run.check("the access token is saved, and the page then shows only that it is set", bool(until(lambda: rpc("notify.get")["secrets"]["ntfy_token"], 10)) and bool(until(lambda: "Saved in the system credential store" in row_text(d, 'push-ntfy'), 10)))
    sink.clear()
    click_text(d, row("push-ntfy") + " button", "Send a test")
    run.check("ntfy test button: one POST to the server, topic in the body and not in the address, token as a bearer header",
              sink.wait_for(1) and len(sink.by("ntfy")) == 1 and sink.by("ntfy")[0]["body"]["topic"] == topic and sink.by("ntfy")[0]["auth"] == f"Bearer {TOKEN_NTFY}", json.dumps(sink.all())[:300])
    run.check("…and the page says it was sent", bool(until(lambda: "Sent" in row_text(d, 'push-ntfy'), 10)))

    # Rotating the topic replaces it.
    d.js("window.confirm = () => true")
    click_text(d, row("push-ntfy") + " button", "New topic")
    topic2 = until(lambda: (t := d.js("const e = document.querySelector('[data-ntfy-topic]'); return e && e.textContent")) and t != topic and t, 10)
    run.check("New topic gives another 128-bit topic", bool(topic2) and re.fullmatch(r"[0-9a-f]{32}", topic2) is not None, str(topic2))
    state["topic"] = topic2

    # Webhook.
    d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-webhook"))
    d.wait("return !!document.querySelector(arguments[0] + ' input[type=\"password\"]')", 10, 0.25, row("push-webhook"))
    set_value(d, row("push-webhook") + ' input[type="password"]', SINK + HOOK_PATH)
    until(lambda: rpc("notify.get")["secrets"]["webhook_url"], 10)
    sink.clear()
    d.wait("return [...document.querySelectorAll(arguments[0] + ' button')].some(b => b.textContent.trim() === 'Send a test' && !b.disabled)", 10, 0.25, row("push-webhook"))
    click_text(d, row("push-webhook") + " button", "Send a test")
    run.check("webhook test button: one JSON POST to the saved address", sink.wait_for(1) and len(sink.by("webhook")) == 1 and sink.by("webhook")[0]["body"]["source"] == "kelpie", json.dumps(sink.all())[:300])

    # Telegram.
    d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-telegram"))
    d.wait("return !!document.querySelector(arguments[0] + ' input[type=\"password\"]')", 10, 0.25, row("push-telegram"))
    set_value(d, row("push-telegram") + ' input[type="password"]', TOKEN_BOT)
    until(lambda: rpc("notify.get")["secrets"]["telegram_token"], 10)
    d.wait("return !!document.querySelector(arguments[0] + ' input[placeholder=\"Chat id\"]')", 10, 0.25, row("push-telegram"))
    set_value(d, row("push-telegram") + ' input[placeholder="Chat id"]', CHAT)
    until(lambda: rpc("notify.get")["config"]["telegram"]["chat_id"] == CHAT, 10)
    sink.clear()
    d.wait("return [...document.querySelectorAll(arguments[0] + ' button')].some(b => b.textContent.trim() === 'Send a test' && !b.disabled)", 10, 0.25, row("push-telegram"))
    click_text(d, row("push-telegram") + " button", "Send a test")
    tg = sink.wait_for(1) and sink.by("telegram")
    run.check("Telegram test button: one sendMessage to the (local) Bot API with the chat id", bool(tg) and len(tg) == 1 and tg[0]["path"] == f"/bot{TOKEN_BOT}/sendMessage" and tg[0]["body"]["chat_id"] == CHAT, json.dumps(sink.all())[:300])

    # Screen text: off, with a warning when switched on.
    run.check("screen text is off by default and the warning is hidden", toggle_on(d, "push-screen") is False and "leave this computer" not in row_text(d, 'push-screen'))
    d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-screen"))
    run.check("switching it on shows the warning", bool(until(lambda: "leave this computer" in row_text(d, 'push-screen'), 10)) and rpc("notify.get")["config"]["include_screen"] is True)
    d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-screen"))
    until(lambda: rpc("notify.get")["config"]["include_screen"] is False, 10)
    run.check("including the phone link is on by default and the page can turn it off and on", rpc("notify.get")["config"]["phone_link"] is True and (d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-link")) or True)
              and bool(until(lambda: rpc("notify.get")["config"]["phone_link"] is False, 10)) and (d.js("document.querySelector(arguments[0] + ' button.switch').click()", row("push-link")) or True) and bool(until(lambda: rpc("notify.get")["config"]["phone_link"] is True, 10)))
    # Short turns are not skipped in the rest of the run.
    set_value(d, row("push-filters") + ' input[type="number"]', "0")
    run.check("the turn-length filter is editable", bool(until(lambda: rpc("notify.get")["config"]["min_turn_s"] == 0, 10)))
    run.shot("01-settings")
    d.js(f"{S}.setSettings({{ theme: 'light' }})")
    time.sleep(0.4)
    run.shot("01-settings-light")
    d.js(f"{S}.setSettings({{ theme: 'dark', density: 'compact' }})")
    time.sleep(0.4)
    run.shot("01-settings-compact")
    d.js(f"{S}.setSettings({{ theme: 'dark', density: 'comfortable' }})")
    d.js(f"{S}.setOverlay(null)")


def configure(**changes):
    """Replaces parts of the daemon's configuration (the page does the same through notify.set)."""
    cfg = rpc("notify.get")["config"]
    for k, v in changes.items():
        if isinstance(v, dict):
            cfg[k] = {**cfg[k], **v}
        else:
            cfg[k] = v
    rpc("notify.set", {"config": cfg})


state = {}
rpc = None


def scenario(run):
    global rpc
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 12, 30)
    run.check("the app talks to a daemon with API 12", d.js(f"return {S}.daemonApi()") >= 12)

    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    state["pid"] = pid
    other_pid = d.js(f"return {S}.addProject(arguments[0])", OTHER)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    watched = js_await(d, f"{S}.newSession(arguments[0], 'claude', {{name: 'watched'}})", pid)
    hook(rpc, watched["id"], "UserPromptSubmit")
    until(lambda: "READY" in screen(rpc, watched["handle"]), 20)
    until(lambda: rpc("notify.get")["windows"]["count"] >= 1, 15)

    # ---------- 2 (first, so no secret exists yet): no channel, nothing is sent ----------
    credentials_before = credential_targets()
    state["credentials_before"] = credentials_before
    n0 = len(inbox(d))
    d.js(f"{S}.switchProject(arguments[0])", other_pid)
    until(lambda: watched["id"] not in rpc("notify.get")["windows"]["visible"], 10)
    start_working(rpc, watched["id"])
    hook(rpc, watched["id"], "PermissionRequest")
    got = until(lambda: len(inbox(d)) > n0, 10)
    run.check("with no channel on, the event reaches the inbox (exactly one entry)", bool(got) and len(inbox(d)) == n0 + 1 and inbox(d)[0]["kind"] == "waiting", str(inbox(d)[:2]))
    run.check("…and nothing is sent anywhere", sink.count() == 0 and rpc("notify.get")["status"] == {})
    hook(rpc, watched["id"], "Stop")
    wait_status(rpc, watched["id"], "idle")

    # ---------- 1 ----------
    d.js(f"{S}.switchProject(arguments[0])", pid)
    sink_settings(d, run)
    sink.clear()

    # ---------- 3: desktop and phone agree on suppression ----------
    win = Rpc()  # a second window, as far as the daemon can tell
    app_focused = d.js("return document.hasFocus()")
    d.js(f"{S}.switchProject(arguments[0])", other_pid)
    until(lambda: watched["id"] not in rpc("notify.get")["windows"]["visible"], 10)
    win("ui.focus", {"focused": True, "visible": [watched["id"]]})
    before_inbox, before_sink = len(inbox(d)), sink.count()
    start_working(rpc, watched["id"])
    hook(rpc, watched["id"], "PermissionRequest")
    wait_status(rpc, watched["id"], "waiting")
    idle_now = os_idle_seconds()
    away = idle_now is not None and idle_now >= 300  # the daemon counts an idle computer as not looking
    if away:
        print(f"note: the computer has been idle for {idle_now:.0f} s: a window in front does not count as looking", flush=True)
        ok = until(lambda: len(inbox(d)) == before_inbox + 1 and sink.count() >= 3, 10)
        run.check("a computer idle for 5 minutes is not looking, whatever the window shows: the event reaches the inbox and the phone", bool(ok), f"{len(inbox(d)) - before_inbox} inbox, {sink.count() - before_sink} messages")
        time.sleep(31)  # the session's allowance of one message in 30 s
        before_inbox, before_sink = len(inbox(d)), sink.count()
        sink.clear()
        before_sink = 0
    else:
        held_back = stays(lambda: len(inbox(d)) == before_inbox and sink.count() == before_sink, 3)
        if (os_idle_seconds() or 0) < 295:  # the person may walk away during the run: the daemon then rightly stops holding back
            run.check("a focused window showing the session: no inbox entry and no message", held_back, f"{len(inbox(d)) - before_inbox} inbox, {sink.count() - before_sink} messages")
        else:
            print("note: the computer went idle during the check; not counted", flush=True)
    hook(rpc, watched["id"], "Stop")
    wait_status(rpc, watched["id"], "idle")

    win("ui.focus", {"focused": False, "visible": [watched["id"]]})
    time.sleep(0.5)
    start_working(rpc, watched["id"])
    hook(rpc, watched["id"], "PermissionRequest")
    ok = until(lambda: len(inbox(d)) > before_inbox and sink.count() >= 3, 10)
    time.sleep(2)
    run.check("the same event with the window not focused: one inbox entry and one message per channel",
              bool(ok) and len(inbox(d)) == before_inbox + 1 and len(sink.by("ntfy")) == 1 and len(sink.by("webhook")) == 1 and len(sink.by("telegram")) == 1,
              f"inbox +{len(inbox(d)) - before_inbox}, ntfy {len(sink.by('ntfy'))}, webhook {len(sink.by('webhook'))}, telegram {len(sink.by('telegram'))}")
    ntfy = sink.by("ntfy")[0]["body"] if sink.by("ntfy") else {}
    run.check("the message names the project and the session and says what happened",
              ntfy.get("title") == f"Kelpie · {PROJ_NAME}" and ntfy.get("message") == "watched: Needs your input" and ntfy.get("priority") == 4, json.dumps(ntfy)[:300])
    run.check("the screen is not in the message by default", "READY" not in ntfy.get("message", ""))
    hook(rpc, watched["id"], "Stop")
    wait_status(rpc, watched["id"], "idle")

    # The app's own window: when it has focus and shows the session, it suppresses both too.
    win.close()
    in_front = lambda: watched["id"] in rpc("notify.get")["windows"]["visible"]  # noqa: E731
    d.js(f"{S}.switchProject(arguments[0])", pid)
    if app_focused and not away and until(in_front, 10):
        n_inbox, n_sink = len(inbox(d)), sink.count()
        time.sleep(31)  # the per-session rate limit would hide a missing suppression otherwise
        start_working(rpc, watched["id"])
        hook(rpc, watched["id"], "PermissionRequest")
        wait_status(rpc, watched["id"], "waiting")
        held = stays(lambda: len(inbox(d)) == n_inbox and sink.count() == n_sink, 3)
        if in_front() and (os_idle_seconds() or 0) < 295:  # the OS may have moved focus away, or the person gone, meanwhile
            run.check("the app's own focused window showing the session suppresses desktop and phone alike", held, f"{len(inbox(d)) - n_inbox} inbox, {sink.count() - n_sink} messages")
        else:
            print("note: the window lost OS focus during the check; not counted", flush=True)
        hook(rpc, watched["id"], "Stop")
    else:
        print("note: the app window has no OS focus here; its own suppression was not exercised (the daemon side was)", flush=True)

    # ---------- 4: the banner ----------
    d.js(f"{S}.switchProject(arguments[0])", pid)
    s1 = make_limited(rpc, watched["id"])
    run.check("the daemon marks a Claude session stopped by rate_limit as limited", bool(s1))
    d.wait("return !!document.querySelector('[data-limit-banner]')", 15)
    banner = d.js("return document.querySelector('[data-limit-banner]').innerText")
    run.check("the pane shows the limit banner with Switch account and Hand off", "rate limit" in banner and "Switch account" in banner and "Hand off" in banner, banner)
    run.check("the usage meter has no reset time here, so the banner asks the user for one", bool(until(lambda: d.js("return !!document.querySelector('[data-limit-banner] input[type=time]')"), 15)))
    run.shot("02-banner")
    for theme, density in (("light", "comfortable"), ("dark", "compact")):  # the strip in light and compact, too
        d.js(f"{S}.setSettings({{ theme: arguments[0], density: arguments[1] }})", theme, density)
        time.sleep(0.4)
        run.shot(f"02-banner-{theme}-{density}")
        run.check(f"the banner stays on one line and inside the pane ({theme}, {density})", d.js("const b = document.querySelector('[data-limit-banner]'); const p = b.closest('.pane'); return b.scrollWidth <= b.clientWidth + 1 && b.getBoundingClientRect().right <= p.getBoundingClientRect().right + 1"))
    d.js(f"{S}.setSettings({{ theme: 'dark', density: 'comfortable' }})")
    now = time.time()
    nxt = time.localtime(now + 150)
    set_value(d, "[data-limit-banner] input[type=time]", f"{nxt.tm_hour:02d}:{nxt.tm_min:02d}")
    click_text(d, "[data-limit-banner] button", "Continue at this time")
    sched = until(timers, 10)
    run.check("the confirmed time sets one timer in the daemon, a few minutes ahead", bool(sched) and len(sched) == 1 and sched[0]["session"] == watched["id"] and sched[0]["source"] == "confirmed" and now < sched[0]["fire_at"] / 1000 < now + 400, str(sched))
    d.wait("return !!document.querySelector('[data-cancel-continue]')", 10)
    run.check("the banner now says when “continue” will be typed", "Will type" in d.js("return document.querySelector('[data-limit-banner]').innerText"))
    d.js("document.querySelector('[data-cancel-continue]').click()")
    run.check("Cancel removes the timer", bool(until(lambda: timers() == [], 10)))
    inbox_before = len(inbox(d))
    w = watched["handle"]
    soon = int(time.time() * 1000) + 60_000
    run.check("a time that is not from the meter or the user is refused", "usage meter" in (refused(lambda: rpc("session.schedule_continue", {"handle": w, "at": soon, "source": "guess"})) or ""))
    run.check("a past time is refused", "passed" in (refused(lambda: rpc("session.schedule_continue", {"handle": w, "at": int(time.time() * 1000) - 5000, "source": "confirmed"})) or ""))
    idle = new_session(rpc, "not-limited")
    run.check("a session that is not stopped by the limit cannot be given a timer", "not stopped by a usage limit" in (refused(lambda: rpc("session.schedule_continue", {"handle": idle["handle"], "at": soon, "source": "confirmed"})) or ""))
    run.check("none of that touched the inbox", len(inbox(d)) == inbox_before)
    hook(rpc, watched["id"], "UserPromptSubmit")  # works again: the banner goes
    run.check("the banner goes when the agent works again", bool(until(lambda: not d.js("return !!document.querySelector('[data-limit-banner]')"), 10)))
    hook(rpc, watched["id"], "Stop")

    # ---------- 5: continue at reset, only when still limited ----------
    names = ["fire", "typed", "restarted", "worked", "moved", "closed", "draft"]
    sessions = {n: new_session(rpc, f"c-{n}") for n in names}
    # The user began typing the next message while the agent worked (no Enter): it sits in the prompt.
    rpc("session.input", {"handle": sessions["draft"]["handle"], "data": "half a draft"})
    for n in names:
        make_limited(rpc, sessions[n]["id"])
    time.sleep(1.5)  # the failures have reached the inbox
    at = int(time.time() * 1000) + 2500
    for n in names:
        rpc("session.schedule_continue", {"handle": sessions[n]["handle"], "at": at, "source": "confirmed"})
    run.check("seven timers are set", len(timers()) == 7)
    n_inbox = len(inbox(d))
    rpc("session.input", {"handle": sessions["typed"]["handle"], "data": "mine\r"})  # the user types before the reset
    # What the window's terminal sends by itself (mouse, focus) is not the user typing.
    rpc("session.input", {"handle": sessions["fire"]["handle"], "data": "\x1b[<0;5;5M\x1b[<65;5;5M\x1b[I\x1b[O"})
    rpc("session.kill", {"handle": sessions["restarted"]["handle"]})
    until(lambda: not info(rpc, sessions["restarted"]["id"])["alive"], 10)
    rpc("session.restart", {"handle": sessions["restarted"]["handle"]})
    hook(rpc, sessions["worked"]["id"], "UserPromptSubmit")  # the agent worked again meanwhile
    rpc("session.update", {"handle": sessions["moved"]["handle"], "agent_session_id": str(uuid.uuid4())})  # /clear moved it to a new conversation
    rpc("session.close", {"handle": sessions["closed"]["handle"]})
    until(lambda: timers() == [], 15)
    until(lambda: "GOT:continue" in screen(rpc, sessions["fire"]["handle"]), 15)  # the machine may be busy
    time.sleep(1.5)
    typed = {n: screen(rpc, sessions[n]["handle"]).count("GOT:continue") for n in names if n != "closed"}
    run.check("only the untouched session got “continue”, once", typed == {"fire": 1, "typed": 0, "restarted": 0, "worked": 0, "moved": 0, "draft": 0}, str(typed))
    run.check("the session the user typed in has the user's text and nothing of the timer's", "GOT:mine" in screen(rpc, sessions["typed"]["handle"]))
    run.check("the draft was not submitted by the timer", "GOT:half a draft" not in screen(rpc, sessions["draft"]["handle"]) and "GOT:half a draftcontinue" not in screen(rpc, sessions["draft"]["handle"]))
    # A program in the session reporting the limit again cannot bring a cancelled timer back.
    hook(rpc, sessions["typed"]["id"], "StopFailure", error="rate_limit")
    time.sleep(0.6)
    run.check("a forged limit report does not re-arm a cancelled timer", not info(rpc, sessions["typed"]["id"]).get("limited") and "not stopped by a usage limit" in (refused(lambda: rpc("session.schedule_continue", {"handle": sessions["typed"]["handle"], "at": int(time.time() * 1000) + 60_000, "source": "confirmed"})) or ""))
    notes = until(lambda: len([i for i in inbox(d) if "was not sent" in i["title"]]) >= 6 and [i for i in inbox(d) if "was not sent" in i["title"]], 10) or []
    reasons = sorted(i["body"] for i in notes)
    # (A restarted agent's start-up output is a short burst of "work", which is an ordinary finished entry.)
    run.check("each of the six others left an inbox note saying why, and the one that fired left none", len(notes) == 6 and not any("c-fire" in i["title"] for i in notes) and any("you typed" in b for b in reasons) and any("restarted" in b for b in reasons) and any("no longer stopped" in b for b in reasons) and any("another conversation" in b for b in reasons) and any("closed or stopped" in b for b in reasons), "; ".join(reasons) + f" | inbox +{len(inbox(d)) - n_inbox}: " + "; ".join(f"{i['kind']}:{i['title']}" for i in inbox(d)[: len(inbox(d)) - n_inbox]))
    run.shot("03-inbox")

    # ---------- 6: switch account ----------
    acct = js_await(d, "window.__kelpie.accounts.addAccount('claude', 'Work')")
    sw = js_await(d, f"{S}.newSession(arguments[0], 'claude', {{name: 'switcher'}})", pid)
    hook(rpc, sw["id"], "UserPromptSubmit")
    until(lambda: "READY" in screen(rpc, sw["handle"]), 20)
    slug = re.sub(r"[^A-Za-z0-9]", "-", PROJ)
    transcript_dir = os.path.join(CLAUDE_HOME, "projects", slug)
    os.makedirs(transcript_dir, exist_ok=True)
    transcript = os.path.join(transcript_dir, sw["agent_session_id"] + ".jsonl")
    body = '{"type":"user","message":"hello"}\n{"type":"assistant","message":"hi"}\n'
    with open(transcript, "w", encoding="utf-8", newline="\n") as f:
        f.write(body)
    make_limited(rpc, sw["id"])
    d.wait("return !!document.querySelector('[data-limit-banner=\"' + arguments[0] + '\"]')", 15, 0.25, sw["id"])
    d.js("window.confirm = () => true")

    def pick_account(label):
        click_text(d, f'[data-limit-banner="{sw["id"]}"] button', "Switch account…")
        d.wait("return !!document.querySelector('.ctx-menu')", 5)
        return click_text(d, ".ctx-menu .ctx-item", label)

    pick_account("Work")
    toast = until(lambda: (t := d.js(f"const t = {S}.toast(); return t && t.text")) and "not signed in" in t and t, 10)
    run.check("an account that is not signed in is refused with the reason", bool(toast) and "not signed in" in toast, str(toast))
    run.check("…and the session was left running on its own account", info(rpc, sw["id"])["alive"] and "CLAUDE_CONFIG_DIR" not in info(rpc, sw["id"])["env"] and not os.path.exists(os.path.join(acct["dir"], "projects")))
    with open(os.path.join(acct["dir"], ".credentials.json"), "w", encoding="utf-8") as f:
        json.dump({"claudeAiOauth": {"accessToken": "fake-e2e-token-not-used"}}, f)
    # A different conversation already under that id there: never overwritten.
    clash_dir = os.path.join(acct["dir"], "projects", slug)
    os.makedirs(clash_dir, exist_ok=True)
    with open(os.path.join(clash_dir, sw["agent_session_id"] + ".jsonl"), "w", encoding="utf-8") as f:
        f.write("someone else's conversation\n")
    d.js(f"{S}.notify('')")
    pick_account("Work")
    toast = until(lambda: (t := d.js(f"const t = {S}.toast(); return t && t.text")) and "different conversation" in t and t, 10)
    run.check("a different conversation under that id is never overwritten: refused with the reason", bool(toast) and "different conversation" in toast and open(os.path.join(clash_dir, sw["agent_session_id"] + ".jsonl"), encoding="utf-8").read() == "someone else's conversation\n", str(toast))
    os.remove(os.path.join(clash_dir, sw["agent_session_id"] + ".jsonl"))
    pick_account("Work")
    moved = os.path.join(clash_dir, sw["agent_session_id"] + ".jsonl")
    after = until(lambda: (i := info(rpc, sw["id"])) and i["alive"] and i["env"].get("CLAUDE_CONFIG_DIR") == acct["dir"] and not i.get("limited") and i, 30)
    run.check("the session restarted with the other account's home", bool(after), json.dumps(info(rpc, sw["id"]).get("env")))
    run.check("the conversation was copied there and the original stays", os.path.exists(moved) and open(moved, encoding="utf-8").read() == body and os.path.exists(transcript))
    run.check("the new process runs on that home", bool(until(lambda: f"READY cfg={acct['dir']}" in screen(rpc, sw["handle"]), 20)))
    run.check("the banner is gone (not limited any more)", not d.js("return !!document.querySelector('[data-limit-banner]')"))
    os.remove(os.path.join(acct["dir"], ".credentials.json"))  # nothing may use the fake token


def push_closed(results):
    """Scenario 7: the window is closed; only the daemon is left."""
    check = results.check
    check("the window is closed: the daemon has no window left", rpc("notify.get")["windows"] == {"count": 0, "focused": False, "visible": []})
    sink.clear()
    configure(min_turn_s=0, events={"needs_you": True, "done": True, "failed": True, "automation": True, "approval": True, "cost_cap": True}, away_only=False, include_screen=False)
    a = new_session(rpc, "closed-a", project_id=state["pid"])
    start_working(rpc, a["id"])
    hook(rpc, a["id"], "PermissionRequest")
    sink.wait_for(3, 15)
    time.sleep(2)
    check("needs-you with the window closed: exactly one message per channel", len(sink.by("ntfy")) == 1 and len(sink.by("webhook")) == 1 and len(sink.by("telegram")) == 1 and sink.count() == 3, str([r["path"] for r in sink.all()]))
    n = sink.by("ntfy")[0]["body"] if sink.by("ntfy") else {}
    check("…carrying the project (known from before the window closed), the session and what happened, to the topic, with the token", n.get("title") == f"Kelpie · {PROJ_NAME}" and said(n.get("message"), "closed-a", "Needs your input") and n.get("topic") == rpc("notify.get", {"reveal_topic": True})["ntfy_topic"] and sink.by("ntfy")[0]["auth"] == f"Bearer {TOKEN_NTFY}", json.dumps(n)[:200])
    hook(rpc, a["id"], "Stop")
    wait_status(rpc, a["id"], "idle")
    start_working(rpc, a["id"])
    hook(rpc, a["id"], "PermissionRequest")
    check("the same session again within 30 s sends nothing (rate limit)", stays(lambda: sink.count() == 3, 3))
    hook(rpc, a["id"], "Stop")

    # Failed, done, and the switches.
    sink.clear()
    b = new_session(rpc, "closed-b")
    start_working(rpc, b["id"])
    hook(rpc, b["id"], "StopFailure", error="server_error")
    sink.wait_for(3, 15)
    time.sleep(1)
    msg = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("a failed turn is sent once per channel", len(sink.by("ntfy")) == 1 and said(msg, "closed-b", "Stopped with an error"), msg)
    sink.clear()
    configure(events={"done": False})
    c = new_session(rpc, "closed-c")
    start_working(rpc, c["id"])
    hook(rpc, c["id"], "Stop")
    check("done is not sent while its switch is off", stays(lambda: sink.count() == 0, 3))
    configure(events={"done": True}, min_turn_s=3600)
    d2 = new_session(rpc, "closed-d")
    start_working(rpc, d2["id"])
    hook(rpc, d2["id"], "Stop")
    check("a short turn is not sent when only long ones are wanted", stays(lambda: sink.count() == 0, 3))
    configure(min_turn_s=1)
    e = new_session(rpc, "closed-e")
    start_working(rpc, e["id"])
    time.sleep(1.5)
    hook(rpc, e["id"], "Stop")
    sink.wait_for(3, 15)
    msg = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("a turn longer than the filter is sent, with its length", len(sink.by("ntfy")) == 1 and re.fullmatch(r"closed-e: Finished \(\d+s\)", msg) is not None, msg)

    # The away filter, with a window that has focus and shows something else.
    sink.clear()
    configure(min_turn_s=0, away_only=True)
    win = Rpc()
    win("ui.focus", {"focused": True, "visible": ["no-such-session"]})
    f = new_session(rpc, "closed-f")
    start_working(rpc, f["id"])
    hook(rpc, f["id"], "PermissionRequest")
    idle = os_idle_seconds()
    away = idle is not None and idle >= 300  # the filter counts an idle computer as away
    if away:
        print(f"note: the computer has been idle for {idle:.0f} s, which the away filter counts as away; the held-back case is skipped", flush=True)
    else:
        check("only-when-away: a focused window holds the message back", stays(lambda: sink.count() == 0, 3))
    win("ui.focus", {"focused": False, "visible": []})
    g = new_session(rpc, "closed-g")
    start_working(rpc, g["id"])
    hook(rpc, g["id"], "PermissionRequest")
    check("…and an unfocused one lets it through", sink.wait_for(3 + (3 if away else 0), 15))
    win.close()
    configure(away_only=False)

    # Screen text and redaction.
    sink.clear()
    secret_name = f"deploy {SECRET_KEY}"
    h = new_session(rpc, secret_name)
    start_working(rpc, h["id"])  # the hook takes over from output activity, which would end a "turn" by itself
    rpc("session.input", {"handle": h["handle"], "data": f"export GITHUB_TOKEN={SECRET_KEY}\r"})
    until(lambda: "GOT:export" in screen(rpc, h["handle"]), 10)
    hook(rpc, h["id"], "PermissionRequest")
    sink.wait_for(3, 15)
    text = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("a secret in the session's name is masked", "ghp_" not in text and "[redacted]" in text, text)
    check("screen text is not sent unless switched on", "GOT:" not in text and "READY" not in text, text)
    sink.clear()
    configure(include_screen=True)
    i = new_session(rpc, "closed-i")
    start_working(rpc, i["id"])
    rpc("session.input", {"handle": i["handle"], "data": f"export GITHUB_TOKEN={SECRET_KEY}\r"})
    until(lambda: "GOT:export" in screen(rpc, i["handle"]), 10)
    hook(rpc, i["id"], "PermissionRequest")
    sink.wait_for(3, 15)
    text = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    web = json.dumps(sink.by("webhook")[0]["body"]) if sink.by("webhook") else ""
    own = re.search(r"TOKEN=([0-9a-f]{16,})", screen(rpc, i["handle"]))  # the session's own token is printed by the stand-in agent
    check("with screen text on, the end of the screen is sent, credentials masked", "READY cfg=" in text and "GOT:export GITHUB_TOKEN=[redacted]" in text and "ghp_" not in text and "ghp_" not in web and bool(own) and own.group(1) not in text and own.group(1) not in web, text)
    configure(include_screen=False)

    # The other events: an automation run, a run waiting for approval, a cost cap the app reported.
    sink.clear()
    rpc("automation.save", {"id": "auto-e2e", "name": "nightly-e2e", "kind": "claude", "prompt": "hi", "cwd": PROJ, "every": "daily", "time": "03:00"})
    rpc("automation.run", {"id": "auto-e2e"})
    sink.wait_for(3, 15)
    msg = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("an automation run is sent (also without a session in front of anyone)", msg == "nightly-e2e: Automation started" and len(sink.by("webhook")) == 1 and len(sink.by("telegram")) == 1, msg)
    sink.clear()
    rpc("run.create", {"name": "Release", "cwd": PROJ, "project_id": state["pid"], "tasks": [{"id": "t1", "title": "Deploy", "prompt": "go", "kind": "claude", "approve": True}]})
    sink.wait_for(3, 20)
    msg = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("a run task waiting for approval is sent once", msg == "Release · Deploy: Waiting for your approval to start" and sink.by("ntfy")[0]["body"]["priority"] == 4 and len(sink.by("webhook")) == 1, msg)
    sink.clear()
    k = new_session(rpc, "closed-k")
    rpc("notify.report", {"kind": "cost", "session": k["id"], "detail": "Passed the cost cap: $1.20 of $1.00"})
    sink.wait_for(3, 15)
    msg = sink.by("ntfy")[0]["body"]["message"] if sink.by("ntfy") else ""
    check("a cost cap the app reported goes to the channels", msg == "closed-k: Passed the cost cap: $1.20 of $1.00", msg)
    token = re.search(r"TOKEN=([0-9a-f]{16,})", screen(rpc, k["handle"]))
    prog = Rpc()
    prog("auth", {"session_id": k["id"], "token": token.group(1) if token else ""})  # what a program in the session can do
    calls = [("notify.report", {"kind": "cost", "session": k["id"]}), ("notify.get", {}), ("notify.set", {"config": {}}), ("ui.focus", {"focused": True, "visible": [k["id"]]}),
             ("session.schedule_continue", {"handle": k["handle"], "at": 1, "source": "confirmed"}), ("session.cancel_scheduled", {"handle": k["handle"]}), ("session.set_env_restart", {"handle": k["handle"], "env": {}})]
    answers = {m: refused(lambda m=m, p=p: prog(m, p)) or "" for m, p in calls}
    check("programs in a session cannot use any of the new calls (they are the user's)", bool(token) and all("not available to programs running in a Kelpie session" in a for a in answers.values()), str(answers)[:400])
    prog.close()

    # A server that answers 500: reported without its address.
    sink.clear()
    rpc("notify.set", {"secrets": {"webhook_url": SINK + "/boom/secret-segment"}})
    configure(telegram={"enabled": False}, ntfy={"enabled": False})
    j = new_session(rpc, "closed-j")
    start_working(rpc, j["id"])
    hook(rpc, j["id"], "PermissionRequest")
    status = until(lambda: (s := rpc("notify.get")["status"].get("webhook")) and s.get("error") and s, 20)
    check("a server error is reported as such, with no address in it", bool(status) and status["error"] == "HTTP 500" and "127.0.0.1" not in json.dumps(rpc("notify.get")["status"]) and "boom" not in json.dumps(rpc("notify.get")["status"]), json.dumps(status))
    check("it was tried a second time", until(lambda: len(sink.by("boom")) >= 2, 10) is not None, str(len(sink.by("boom"))))


def restart_daemon(check):
    """A pending timer does not survive the daemon: it is reported as lost, once."""
    global rpc
    t = new_session(rpc, "closed-timer")
    make_limited(rpc, t["id"])
    rpc("session.schedule_continue", {"handle": t["handle"], "at": int(time.time() * 1000) + 3_600_000, "source": "confirmed"})
    check("a pending timer is written to the data folder", len(timers()) == 1 and os.path.exists(os.path.join(DATA, "scheduled.json")))
    try:
        rpc("daemon.shutdown", {"kill_sessions": False})
    except RuntimeError:
        pass  # the reply may race the daemon exiting
    procs["daemon"].wait(timeout=30)
    procs["daemon"] = subprocess.Popen([kelpied, "serve"], env=dict(os.environ, KELPIE_LOG="trace"), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    rpc = None
    for _ in range(300):
        try:
            rpc = Rpc()
            break
        except OSError:
            time.sleep(0.1)
    r = rpc("session.scheduled")
    check("after the restart nothing is pending and the timer is reported as lost", r["timers"] == [] and [x["session"] for x in r["lost"]] == [t["id"]], str(r))
    check("…once", rpc("session.scheduled")["lost"] == [] and not os.path.exists(os.path.join(DATA, "scheduled.json")))


def secrets_and_files(results):
    """Where the secrets are and are not."""
    values = [state["topic"], TOKEN_NTFY, TOKEN_BOT, "e2e-secret-path-4412"]
    hits = []
    for base, _dirs, files in os.walk(DATA):
        for name in files:
            path = os.path.join(base, name)
            try:
                blob = open(path, "rb").read()
            except OSError:
                continue
            hits += [(os.path.relpath(path, DATA), v) for v in values if v.encode() in blob]
    logfile = os.path.join(DATA, "logs", "kelpied.log")
    log = open(logfile, "rb").read() if os.path.exists(logfile) else b""
    results.check("the daemon's own log (KELPIE_LOG=trace, <data>/logs/kelpied.log) was written at a level that would show the HTTP client", len(log) > 2000 and (b" TRACE " in log or b" DEBUG " in log), f"{len(log)} bytes")
    hits += [("kelpied.log", v) for v in values + [f"127.0.0.1:{SINK_PORT}", "sendMessage", f"/bot{TOKEN_BOT}"] if v.encode() in log]
    results.check("no secret, request address or request path is in any file of the data folder or in the daemon's trace log", not hits, str(hits))
    cfg = open(os.path.join(DATA, "notify.json"), encoding="utf-8").read()
    results.check("notify.json holds the settings only", "ntfy" in cfg and SINK in cfg and TOKEN_BOT not in cfg)
    new = credential_targets() - state["credentials_before"]
    results.check("the credential store gained entries only under this run's endpoint name", bool(new) and all(ENDPOINT in t for t in new), str(sorted(new))[:400])


def main():
    global rpc
    log = open(os.path.join(WORK, "daemon.log"), "wb")
    procs["daemon"] = subprocess.Popen([kelpied, "serve"], env=dict(os.environ, KELPIE_LOG="trace"), stdout=log, stderr=log)
    for _ in range(150):
        try:
            rpc = Rpc()
            break
        except OSError:
            time.sleep(0.1)
    if rpc is None:
        raise SystemExit("the daemon did not start")
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    if not until(port_open, 30):
        raise SystemExit("tauri-driver did not open its port")

    run = None
    try:
        run = Run(shots, app=app)
        try:
            scenario(run)
        except Exception:
            traceback.print_exc()
            run.check("the GUI scenarios ran to completion", False)
        run.d.quit()  # the window closes; the daemon and its sessions go on
        time.sleep(2)
        try:
            push_closed(run)
            restart_daemon(run.check)
            secrets_and_files(run)
        except Exception:
            traceback.print_exc()
            run.check("the window-closed scenarios ran to completion", False)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
    finally:
        teardown(run)
    left = credential_targets() - state.get("credentials_before", set())
    run.check("cleanup left no credential-store entry of this run", not left, str(sorted(left)))
    run.finish()


def teardown(run):
    """Removes the secrets first (through the daemon), then quits the app, the daemon and the driver."""
    try:
        Rpc()("notify.clear")
    except Exception:
        pass
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
        Rpc()("daemon.shutdown", {"kill_sessions": True})
    except Exception:
        pass
    for p in procs.values():
        try:
            p.wait(timeout=8)
        except Exception:
            p.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    sink.server.shutdown()
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
