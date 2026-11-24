"""GUI e2e: a session the subscription's usage limit stopped continues by itself once the limit lifts.

    python -X utf8 scripts/e2e/auto_continue.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`), `tauri-driver` on PATH
and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER). Its own daemon (isolated endpoint and data
folder, started by the app from KELPIE_DAEMON_PATH), its own tauri-driver. No real agent and no model call: the terminal agent is a
stand-in `claude` (a batch shim on PATH) that echoes what it is typed, its hooks reported through `hook.report`; the chat agent is a
stand-in that speaks Claude's stream-json and stops on the usage limit exactly as `claude -p` prints it (a `rate_limit_event`, an
`assistant` line with `error: rate_limit`, a failed `result`), then answers normally once it is sent “continue”.

1. Terminal, setting on (the default): a Claude session stops on the limit with "resets <next minute>". With no click, the daemon
   sets the timer from the message; the sidebar row shows "continues HH:MM" instead of "error" and the banner has Cancel. When the
   time comes, `continue` is typed into it (once) and it works again.
2. Setting off: the next stop sets no timer; the row says "error" and the banner offers the button instead.
3. Chat: a Claude chat stops on the limit; the daemon reads the reset from its lines, the row shows the time, the banner says it will
   send “continue”; at the time “continue” goes through the chat's queue, the agent answers, and the chat is no longer limited.
"""
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
APP = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
KELPIED = os.path.join(os.path.dirname(APP), "kelpied.exe" if os.name == "nt" else "kelpied")
os.makedirs(scratch, exist_ok=True)
WORK = tempfile.mkdtemp(prefix="auto-continue-", dir=scratch)
PROJ = os.path.join(WORK, "proj")
BIN = os.path.join(WORK, "bin")
for p in (PROJ, BIN):
    os.makedirs(p)
for k in list(os.environ):
    if k.startswith(("KELPIE_", "CLAUDE_CODE_")) or k == "CLAUDECODE":
        os.environ.pop(k)
ENDPOINT = f"kelpie-autocont-{os.getpid()}"
# The endpoint and data folder are this process's own: nothing here reaches the person's daemon.
os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": os.path.join(WORK, "data"), "KELPIE_E2E": "1", "KELPIE_DAEMON_PATH": KELPIED,
    "KELPIE_NOTIFY_RESET_MARGIN_MS": "500", "CLAUDE_CONFIG_DIR": os.path.join(WORK, "claude-home"),
    "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
})
assert ENDPOINT.startswith("kelpie-autocont-")
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
if not NATIVE or not shutil.which("tauri-driver"):
    raise SystemExit("needs tauri-driver and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(KELPIED):
    raise SystemExit(f"no daemon next to the app: {KELPIED}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

# The terminal stand-in: echoes every line it is typed as GOT:<line>.
FAKE_TUI = os.path.join(WORK, "fake_claude.py")
with open(FAKE_TUI, "w", encoding="utf-8") as f:
    f.write(
        "import msvcrt\n"
        "print('READY', flush=True)\n"
        "buf = ''\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch in '\\r\\n':\n"
        "        if buf:\n"
        "            print('GOT:' + buf, flush=True)\n"
        "        buf = ''\n"
        "    elif ch != '\\x1b':\n"
        "        buf += ch\n"
    )
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE_TUI}" %*\r\n')

# The chat stand-in: Claude's stream-json. The first message runs into the usage limit, whose reset is RESET_FILE's time (s);
# "continue" is answered.
RESET_FILE = os.path.join(WORK, "chat-reset.txt")
FAKE_CHAT = os.path.join(WORK, "fake_claude_chat.py")
with open(FAKE_CHAT, "w", encoding="utf-8") as f:
    f.write(
        "import json, sys, time\n"
        f"RESET = {RESET_FILE!r}\n"
        "SID = 'chat-conv-1'\n"
        "def out(o):\n"
        "    sys.stdout.write(json.dumps(o) + '\\n'); sys.stdout.flush()\n"
        "for line in sys.stdin:\n"
        "    try:\n"
        "        m = json.loads(line)\n"
        "    except ValueError:\n"
        "        continue\n"
        "    if m.get('type') == 'control_request':\n"
        "        out({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': m.get('request_id'), 'response': {}}})\n"
        "        continue\n"
        "    if m.get('type') != 'user':\n"
        "        continue\n"
        "    content = m.get('message', {}).get('content')\n"
        "    text = content if isinstance(content, str) else ' '.join(b.get('text', '') for b in content or [] if isinstance(b, dict))\n"
        "    out({'type': 'system', 'subtype': 'init', 'session_id': SID, 'model': 'claude-test', 'tools': []})\n"
        "    if text.strip() == 'continue':\n"
        "        out({'type': 'assistant', 'session_id': SID, 'message': {'id': 'm2', 'model': 'claude-test', 'role': 'assistant', 'content': [{'type': 'text', 'text': 'Resumed after the limit.'}]}})\n"
        "        out({'type': 'result', 'subtype': 'success', 'is_error': False, 'result': 'Resumed after the limit.', 'session_id': SID})\n"
        "        continue\n"
        "    reset = int(open(RESET).read())\n"
        "    shown = time.strftime('%I:%M%p', time.localtime(reset)).lstrip('0').lower()\n"
        "    words = \"You've hit your 5-hour limit \\u00b7 resets \" + shown + ' (Asia/Ho_Chi_Minh)'\n"
        "    out({'type': 'rate_limit_event', 'rate_limit_info': {'status': 'rejected', 'resetsAt': reset, 'rateLimitType': 'five_hour'}, 'session_id': SID})\n"
        "    out({'type': 'assistant', 'error': 'rate_limit', 'session_id': SID, 'message': {'id': 'm1', 'model': '<synthetic>', 'role': 'assistant', 'content': [{'type': 'text', 'text': words}]}})\n"
        "    out({'type': 'result', 'subtype': 'success', 'is_error': True, 'result': words, 'session_id': SID})\n"
    )


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def until(fn, timeout, step=0.25):
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


drv, native = free_port(), free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{drv}"
driver = subprocess.Popen(["tauri-driver", "--port", str(drv), "--native-port", str(native), "--native-driver", NATIVE], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(2)
S = "window.__kelpie.store"
C = f"window.__kelpie.chat.getController({S}.sessions[arguments[0]])"
rpc_conn = None


def rpc(method, params=None):
    global rpc_conn
    if rpc_conn is None:
        rpc_conn = kc.Conn()
    return rpc_conn.call(method, params or {})


def info(sid):
    return next((s for s in rpc("session.list") if s["id"] == sid), None)


def hook(sid, event, **payload):
    rpc("hook.report", {"session_id": sid, "agent": "claude", "payload": {"hook_event_name": event, **payload}})


def timers():
    return rpc("session.scheduled")["timers"]


def clock_words(t):
    """A time as Claude Code prints it: 3:20am."""
    return time.strftime("%I:%M%p", time.localtime(t)).lstrip("0").lower()


def next_minute(min_ahead=8):
    t = (int(time.time()) // 60 + 1) * 60
    return t if t - time.time() >= min_ahead else t + 60


def row_label(d, sid):
    """The state word of the session's sidebar row (rows are found by the session's name)."""
    name = info(sid)["name"]
    return d.js("const r = [...document.querySelectorAll('.session-row')].find(e => e.querySelector('.name')?.textContent === arguments[0]); const l = r && r.querySelector('.state-label'); return l ? l.innerText : '';", name)


def banner(d, sid):
    return d.js("const b = document.querySelector(`[data-limit-banner='${arguments[0]}']`); return b ? b.innerText : null", sid)


def screen(handle):
    return rpc("session.read", {"handle": handle, "lines": 40})["text"]


run = Run(shots, APP)
d = run.d
try:
    run.boot()
    d.set_window_size(1300, 800)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    run.check("the setting is on by default and reaches the daemon", bool(until(lambda: daemon(d, "daemon.policy").get("auto_continue") is True, 10)) and d.js(f"return {S}.state.settings.autoContinue") is True)

    # ---------- 1: terminal, automatic ----------
    def limited_terminal(name, reset):
        s = d.js_async(f"const [p, n, done] = arguments; {S}.newSession(p, 'claude', {{name: n}}).then(i => done(i), e => done(String(e)));", pid, name)
        sid = s["id"]
        until(lambda: "READY" in screen(s["handle"]), 20)
        hook(sid, "UserPromptSubmit", session_id="conv-" + name)
        until(lambda: info(sid)["status"] == "working", 10)
        hook(sid, "StopFailure", session_id="conv-" + name, error="rate_limit", last_assistant_message=f"You've hit your 5-hour limit · resets {clock_words(reset)} (Asia/Ho_Chi_Minh)")
        until(lambda: info(sid).get("limited"), 10)
        return s

    reset = next_minute()
    term = limited_terminal("term-auto", reset)
    tid = term["id"]
    timer = until(lambda: [t for t in timers() if t["session"] == tid], 10)
    run.check("the daemon set the timer by itself, from the message's time", bool(timer) and timer[0]["source"] == "message" and abs(timer[0]["fire_at"] / 1000 - reset) <= 1.5, str(timer))
    hhmm = time.strftime("%H:%M", time.localtime(reset))
    label = until(lambda: (lambda l: l if "continues" in l else None)(row_label(d, tid)), 10) or row_label(d, tid)
    run.check("the sidebar row shows when it continues instead of an error", "continues" in label and "error" not in label, label)
    b = until(lambda: banner(d, tid), 10) or ""
    run.check("the banner says “continue” will be typed, and offers Cancel", "Will type" in b and bool(d.js("return !!document.querySelector('[data-cancel-continue]')")), b)
    run.shot("01-terminal-waiting")
    got = until(lambda: "GOT:continue" in screen(term["handle"]), reset - time.time() + 20, 0.5)
    run.check("at the reset “continue” was typed with no click", bool(got) and time.time() >= reset, f"{time.time() - reset:.1f}s after the reset")
    time.sleep(1.5)
    run.check("…once", screen(term["handle"]).count("GOT:continue") == 1)
    run.check("…and the timer is gone", not [t for t in timers() if t["session"] == tid])
    hook(tid, "UserPromptSubmit", session_id="conv-term-auto")  # the agent picks it up
    run.check("the session works again: no banner, no error", bool(until(lambda: not info(tid).get("limited") and banner(d, tid) is None and "error" not in row_label(d, tid), 10)))
    hook(tid, "Stop", session_id="conv-term-auto")
    run.shot("02-terminal-continued")

    # ---------- 2: setting off ----------
    d.js(f"{S}.setSettings({{ autoContinue: false }})")
    run.check("turning the setting off reaches the daemon", bool(until(lambda: daemon(d, "daemon.policy").get("auto_continue") is False, 10)))
    off = limited_terminal("term-off", next_minute())
    oid = off["id"]
    run.check("with the setting off no timer is set", stays(lambda: not [t for t in timers() if t["session"] == oid], 4))
    b = until(lambda: banner(d, oid), 10) or ""
    run.check("the banner offers the button instead", "hit a usage or rate limit" in b and ("Continue at" in b or bool(d.js("return !!document.querySelector(`[data-limit-banner='${arguments[0]}'] input[type=time]`)", oid))), b)
    run.check("the row shows the failure", "error" in row_label(d, oid), row_label(d, oid))
    run.shot("03-setting-off")
    d.js(f"{S}.setSettings({{ autoContinue: true }})")
    until(lambda: daemon(d, "daemon.policy").get("auto_continue") is True, 10)

    # ---------- 3: chat ----------
    chat_reset = int(time.time()) + 20
    with open(RESET_FILE, "w") as f:
        f.write(str(chat_reset))
    argv = [sys.executable, FAKE_CHAT]
    cid = d.js_async(f"const [p, argv, cwd, done] = arguments; window.__kelpie.ipc.createSession({{kind: 'claude', cwd, project_id: p, name: 'chat-auto', chat_argv: argv}}).then(i => {{ {S}.setSessions(i.id, i); {S}.placePane(p, {{id: 'pane-' + i.id, kind: 'chat', sessionId: i.id}}, 'tab'); done(i.id); }}, e => done(String(e)));", pid, argv, PROJ)
    run.check("a chat session started", bool(cid) and not str(cid).startswith("Error"), str(cid))
    until(lambda: d.js(f"return !!{C}", cid), 20)
    time.sleep(1)
    d.js(f"{C}.send('do the work')", cid)
    climit = until(lambda: (i := info(cid)) and i.get("limited") and i, 20)
    run.check("the chat is marked stopped by the usage limit", bool(climit) and climit["failed"])
    ctimer = until(lambda: [t for t in timers() if t["session"] == cid], 10)
    run.check("the daemon set the chat's timer from its rate-limit event", bool(ctimer) and ctimer[0]["source"] == "message" and abs(ctimer[0]["fire_at"] / 1000 - chat_reset) <= 1, str(ctimer))
    cl = until(lambda: (lambda l: l if "continues" in l else None)(row_label(d, cid)), 10) or row_label(d, cid)
    run.check("the chat's row shows when it continues", "continues" in cl and "error" not in cl, cl)
    cb = until(lambda: banner(d, cid), 10) or ""
    run.check("the chat shows the banner: it will send “continue”", "Will send" in cb, cb)
    run.shot("04-chat-waiting")
    answered = until(lambda: d.js(f"return {C}.state.items.some(i => JSON.stringify(i).includes('Resumed after the limit'))", cid), chat_reset - time.time() + 20, 0.5)
    run.check("at the reset “continue” went through the chat's queue and the agent answered", bool(answered) and time.time() >= chat_reset)
    run.check("…sent as a message the chat shows", d.js(f"return {C}.state.items.some(i => i.kind === 'user' && JSON.stringify(i).includes('continue'))", cid))
    run.check("…and the chat is no longer limited, with no banner", bool(until(lambda: not info(cid).get("limited") and banner(d, cid) is None, 10)))
    run.shot("05-chat-continued")
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    try:
        daemon(d, "daemon.shutdown", {"kill_sessions": True})
    except Exception:
        pass
    try:
        run.finish()
    finally:
        subprocess.run(["taskkill", "/PID", str(driver.pid), "/T", "/F"], capture_output=True)
