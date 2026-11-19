"""GUI e2e: the first message typed into a new Claude chat, sent while the chat is still starting, is never lost.

    python -X utf8 scripts/e2e/chat_first_message.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug app and daemon, `pnpm dev` (the debug app loads the dev server), `tauri-driver` on PATH and `msedgedriver.exe`
matching the installed WebView2 (on PATH, or in MSEDGEDRIVER). The script starts its own tauri-driver and daemon (isolated endpoint
and data folder, set in this process before anything talks to a daemon) and stops both on exit. Runs the real, signed-in Claude
Code on its small model ("haiku") with one-word prompts; `KELPIE_LOAD_FORCE=L3:memory` in the environment runs it with the daemon
reporting memory pressure, as on a busy machine.

As a person would: open a new chat, type a message at once and press Enter, then look.
1. Right after Enter the message is on screen (a bubble, or a chip that says it waits), and the composer is empty. Meanwhile the
   window loads the chat again, as a busy machine makes it do (`session.resync`): the message must stay on screen, and the chat
   must show it is working, although Claude, still starting, has not read it yet.
2. Claude answers it, and the bubble stays.
3. A message sent once the chat is ready, and one sent while Claude works, are answered too.
"""
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

WORK = os.path.join(scratch, f"chat-first-{os.getpid()}")
PROJ = os.path.join(WORK, "proj")
os.makedirs(PROJ)
with open(os.path.join(PROJ, "README.md"), "w", encoding="utf-8") as f:
    f.write("fixture\n")
for args in (["init", "-q"], ["add", "."], ["-c", "user.name=e2e", "-c", "user.email=e2e@example.com", "commit", "-qm", "init"]):
    subprocess.run(["git", *args], cwd=PROJ, check=True)

DATA = tempfile.mkdtemp(prefix="kelpie-chat-first-data-")
# Never the user's daemon: the endpoint and data folder are this run's own, set here before anything connects.
os.environ.update({
    "KELPIE_ENDPOINT": f"kelpie-chat-first-{os.getpid()}", "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1", "KELPIE_DAEMON_PATH": kelpied,
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT"):
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

S = "window.__kelpie.store"
CTRL = f"const c = window.__kelpie.chat.getController({S}.sessions[arguments[0]]);"
# What a person sees in the chat pane: the user bubbles, the replies, the chips of messages that wait, the notes, the composer.
SEEN = """
const pane = document.querySelector('.chat');
if (!pane) return null;
const all = (sel) => [...pane.querySelectorAll(sel)].map(e => e.textContent.trim());
return { users: all('.msg-user'), replies: all('.msg-reply .msg-assistant'), chips: all('.queue-chip .queue-text'),
         notes: all('.result-line'), composer: pane.querySelector('textarea')?.value ?? null };
"""


def until(fn, timeout=30, interval=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


def seen():
    return d.js(SEEN) or {}


def shows(text):
    v = seen()
    return any(text in u for u in v.get("users", [])) or any(text in c for c in v.get("chips", []))


def answered(word):
    return any(word in r.lower() for r in seen().get("replies", []))


def type_and_send(text):
    """Clicks into the message field, types the text and presses Enter, as a person does."""
    d.click(".chat .chat-composer textarea")
    d.keys(text, "enter")


driver = subprocess.Popen([TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", NATIVE], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
until(lambda: socket.socket().connect_ex(("127.0.0.1", DRIVER_PORT)) == 0, 30)
run = Run(shots, app)
d = run.d
sid = None
try:
    run.boot()
    d.set_window_size(1200, 800)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    # The small model, the way a person sets it: the agent's profile.
    d.js(f"{S}.setProfile('claude', {{ args: ['--model', 'haiku'] }})")
    load = os.environ.get("KELPIE_LOAD_FORCE", "")

    # 1. A new chat, and a message typed and sent at once, while Claude is still starting.
    d.js("window.__kelpie.chat.newChat(arguments[0], 'claude').then(i => window.__firstChat = i && i.id)", pid)
    run.check("the new chat's message field shows", bool(until(lambda: d.js("return !!document.querySelector('.chat .chat-composer textarea')"), 30, 0.05)))
    sid = until(lambda: d.js("return window.__firstChat"), 10, 0.05)
    starting = d.js(CTRL + "return { ready: c.state.ready, settings: c.state.settings.length }", sid)
    type_and_send("Reply with only the word apple.")
    sent_at = time.time()
    run.check("…sent while the chat was still starting", not starting["settings"], f"{starting}; load {load or 'as measured'}")
    # What a busy machine does to a window while Claude starts: it falls behind and the daemon makes it load the chat again (the
    # `session.resync` event), the connection to the daemon comes back, the pane is drawn again. Each loads the transcript anew
    # from the daemon's log, before Claude has read the message.
    time.sleep(0.3)
    d.js(f"window.dispatchEvent(new CustomEvent('kelpie:chat-resync', {{ detail: arguments[0] }}))", sid)
    time.sleep(0.5)
    v = seen()
    run.check("right after Enter the composer is empty", v.get("composer") == "", repr(v.get("composer")))
    run.check("…and the message is on screen (a bubble, or a chip that says it waits)", shows("only the word apple"), str(v))
    run.shot("01-just-sent")
    gaps, idle = [], []
    end = time.time() + 240
    while time.time() < end and not answered("apple"):
        if not shows("only the word apple"):
            gaps.append(round(time.time() - sent_at, 1))
        if not d.js("return !!document.querySelector('.chat .typing')") and not answered("apple"):
            idle.append(round(time.time() - sent_at, 1))
        time.sleep(0.5)
    run.check("the message never leaves the screen while Claude starts", not gaps, f"missing at {gaps[:5]}{'…' if len(gaps) > 5 else ''} s")
    run.check("…and the chat shows it is working on it", not idle, f"no 'Working…' at {idle[:5]}{'…' if len(idle) > 5 else ''} s")
    run.check("Claude answers the first message", answered("apple"), f"after {round(time.time() - sent_at, 1)} s; {seen()}")
    v = seen()
    run.check("…and its bubble is there once, not as a chip", sum("only the word apple" in u for u in v.get("users", [])) == 1 and not v.get("chips"), str(v))
    run.shot("02-first-answered")
    until(lambda: d.js(CTRL + "return !c.state.busy", sid), 60)

    # 2. Once it is ready: a message, then another one while Claude works on it.
    type_and_send("Count from 1 to 5, one number per line, then say the word banana.")
    run.check("a message sent when ready shows at once", bool(until(lambda: shows("say the word banana"), 3, 0.1)))
    until(lambda: d.js(CTRL + "return c.state.busy", sid), 15, 0.1)
    type_and_send("After that, also say the word cherry.")
    run.check("a message sent while Claude works shows at once", bool(until(lambda: shows("also say the word cherry"), 3, 0.1)), str(seen()))
    run.check("the first of the two is answered", bool(until(lambda: answered("banana"), 180)), str(seen()))
    run.check("the one sent during the turn is answered too", bool(until(lambda: answered("cherry"), 180)), str(seen()))
    run.shot("03-ready-and-busy")
    run.check("no message was reported as not sent", not any("Not sent" in n for n in seen().get("notes", [])), str(seen().get("notes")))
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    try:
        # Claude is ended the way chat view ends it: stdin closed, then the session closed.
        if sid:
            d.js_async(f"const done = arguments[arguments.length-1]; const s = {S}.sessions[arguments[0]]; window.__kelpie.ipc.daemon('chat.close_stdin', {{ handle: s.handle }}).then(done, done);", sid)
            time.sleep(3)
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
