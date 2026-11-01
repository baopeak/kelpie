"""GUI e2e: coming back to a chat that has a long history (a project switch remounts its pane).

    python -X utf8 scripts/e2e/chat_return.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app, `tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH,
or in MSEDGEDRIVER). The script starts its own tauri-driver and daemon (isolated endpoint, data folder and Claude config
home), set in this process before anything talks to a daemon, and stops both on exit. The agent is a stand-in `claude`
(a batch shim on PATH that waits): no real Claude session is started and no model is called.

1. First open: says it is loading rather than showing nothing, then shows the conversation.
2. Coming back: frame by frame, the rows are there from the start and never go away, it lands at the end of the
   conversation, and the transcript is not rebuilt.
3. A command is one line until it is clicked; one that failed shows its output by itself.
"""
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
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

WORK = os.path.join(scratch, f"chat-return-{os.getpid()}")
HOME, BIN = os.path.join(WORK, "claude-home"), os.path.join(WORK, "bin")
PROJ_A, PROJ_B = os.path.join(WORK, "a"), os.path.join(WORK, "b")
for path in (HOME, BIN, PROJ_A, PROJ_B, os.path.join(HOME, "projects", "fixture")):
    os.makedirs(path)
FAKE = os.path.join(WORK, "fake_claude.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write("import sys, time\nfor _ in sys.stdin:\n    pass\ntime.sleep(3600)\n")
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" %*\r\n')

DATA = tempfile.mkdtemp(prefix="kelpie-chat-return-data-")
os.environ.update({
    "KELPIE_ENDPOINT": f"kelpie-chat-return-{os.getpid()}", "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "KELPIE_DAEMON_PATH": kelpied, "CLAUDE_CONFIG_DIR": HOME, "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT, NATIVE_PORT = free_port(), free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
NATIVE, TAURI_DRIVER = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver"), shutil.which("tauri-driver")
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver on PATH and msedgedriver (PATH or MSEDGEDRIVER)")

CONV = "11111111-2222-4333-8444-555555555555"
LINES = 2000
pad = "lorem ipsum dolor sit amet " * 60
with open(os.path.join(HOME, "projects", "fixture", CONV + ".jsonl"), "w", encoding="utf-8") as f:
    for i in range(LINES):
        message = {"role": "user", "content": f"question {i} {pad}"} if i % 2 == 0 else {"id": f"m{i}", "role": "assistant", "content": [{"type": "text", "text": f"answer {i} {pad}"}]}
        f.write(json.dumps({"type": message["role"], "timestamp": "2026-01-01T00:00:00Z", "message": message}) + "\n")
    for tid, command, description, failed in (("t-ok", "echo fine", "A command that works", False), ("t-bad", "exit 3", "A command that fails", True)):
        f.write(json.dumps({"type": "assistant", "timestamp": "2026-01-01T00:00:00Z", "message": {"id": f"m-{tid}", "role": "assistant", "content": [{"type": "tool_use", "id": tid, "name": "Bash", "input": {"command": command, "description": description}}]}}) + "\n")
        f.write(json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": f"output of {tid}", "is_error": failed}]}}) + "\n")

S = "window.__kelpie.store"
ROWS = "document.querySelectorAll('.chat-inner > *:not([data-chat-loading])').length"
GAP = "const s = document.querySelector('.chat-scroll'); return Math.round(s.scrollHeight - s.scrollTop - s.clientHeight);"
# Row counts of every frame for `ms` after switching to the project.
COME_BACK = f"""
const [project, ms, done] = arguments;
{S}.switchProject(project);
const t0 = performance.now(), frames = [];
const tick = () => {{
  frames.push({ROWS});
  performance.now() - t0 < ms ? requestAnimationFrame(tick) : done(frames);
}};
requestAnimationFrame(tick);
"""


def until(fn, timeout=30, interval=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


driver = subprocess.Popen([TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", NATIVE], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
until(lambda: socket.socket().connect_ex(("127.0.0.1", DRIVER_PORT)) == 0, 30)
run = Run(shots, app)
d = run.d
try:
    run.boot()
    d.set_window_size(1300, 800)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false }});")
    a = d.js(f"return {S}.addProject(arguments[0])", PROJ_A)
    b = d.js(f"return {S}.addProject(arguments[0])", PROJ_B)
    d.js(f"{S}.switchProject(arguments[0])", a)

    session = d.js_async("const [p, c, h, done] = arguments; window.__kelpie.chat.newChat(p, 'claude', 'tab', {resume: c, home: h}).then(i => done(i ? i.id : null), e => done(String(e)));", a, CONV, HOME)
    run.check("the chat starts", bool(session), session)
    run.check("first open: says it is loading", bool(until(lambda: d.js("return !!document.querySelector('[data-chat-loading]')"), 10, 0.02)))
    rows = until(lambda: d.js(f"return {ROWS}"), 60)
    run.check("first open: then shows the conversation", bool(rows) and not d.js("return !!document.querySelector('[data-chat-loading]')"), rows)
    time.sleep(1.5)
    rows = d.js(f"return {ROWS}")
    run.shot("01-open")
    d.js(f"window.__firstRow = window.__kelpie.chat.getController({S}.sessions[arguments[0]]).state.items[0]", session)

    for attempt in range(1, 5):
        d.js(f"{S}.switchProject(arguments[0])", b)
        time.sleep(0.5)
        frames = d.js_async(COME_BACK, a, 3000)
        run.check(f"coming back ({attempt}): the rows are there at once and never go away", frames and min(frames) == rows, f"{len(frames)} frames, fewest rows {min(frames)} of {rows}")
        gap = d.js(GAP)
        run.check(f"…and it is at the end of the conversation", gap <= 2, f"{gap} px above the end")
    run.check("…and the transcript was not rebuilt", d.js(f"return window.__kelpie.chat.getController({S}.sessions[arguments[0]]).state.items[0] === window.__firstRow", session))
    run.shot("02-back")

    row = "[...document.querySelectorAll('.chat .trow.tool')].find(r => r.querySelector('.tsum')?.textContent.includes(arguments[0]))"
    body = "return !!" + row + "?.querySelector('.tool-body')"
    # textContent: a row out of view is not laid out (`content-visibility`), which leaves innerText empty.
    output = "return " + row + "?.querySelector('.tool-body')?.textContent ?? ''"
    click = row + ".querySelector('.tool-head').click()"
    run.check("a command shows what it is doing, on one line", d.js("return !!" + row, "A command that works") and not d.js(body, "A command that works"))
    run.check("a command that failed shows its output by itself", "output of t-bad" in d.js(output, "A command that fails"))
    d.js(click, "A command that works")
    run.check("clicking a command shows its output", bool(until(lambda: "output of t-ok" in d.js(output, "A command that works"), 5)))
    run.shot("03-commands")
    d.js(click, "A command that works")
    run.check("clicking it again folds it", bool(until(lambda: not d.js(body, "A command that works"), 5)))
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
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(driver.pid), "/T", "/F"], capture_output=True)
        else:
            driver.kill()
        time.sleep(1)
        shutil.rmtree(DATA, ignore_errors=True)
        shutil.rmtree(WORK, ignore_errors=True)
