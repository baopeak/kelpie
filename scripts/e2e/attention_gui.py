"""GUI e2e for the attention queue: who needs the user, in what order, and answering without switching panes.

    python -X utf8 scripts/e2e/attention_gui.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, so the app talks only to that daemon, and stops both on exit.

1. A PowerShell `Read-Host "Continue (y/n)"` shell, then a shell running a numbered menu: both listed under
   "Needs you", the longest wait first, each with "waiting …"; the status-bar bell shows its dot.
2. Ctrl+Shift+. focuses the longest waiting session, the next press the other, then wraps around.
3. Answer on the dashboard card opens the quick answer with the screen and the y / n keys; y reaches the
   prompt and the session leaves Needs you; the dot stays while the menu still waits.
4. The inbox's "Needs you" item opens the popover for the menu; free text 2 + Enter answers it; the dot goes.
5. Stale answer: the popover is open, the prompt is answered from outside: the popover locks ("The prompt
   changed since this was shown.", keys disabled) and clicking a key types nothing. With nothing waiting,
   Ctrl+Shift+. says so.
"""
import importlib.util
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

# Isolated daemon: its own endpoint and data folder; the app and tauri-driver inherit these.
ENDPOINT = f"kelpie-attention-gui-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-attention-gui-data-")
WORK = os.path.join(scratch, f"attention-gui-{os.getpid()}")
os.makedirs(WORK, exist_ok=True)
os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1"})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"  # read by wd.Driver
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver")
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver on PATH and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

procs = {}
S = "window.__kelpie.store"
L = "window.__kelpie.layout"
MENU = (
    'print("Bash(npm test)")\nprint(" Do you want to proceed?")\nprint(" \\u276f 1. Yes")\n'
    'print("   2. Yes, and don\'t ask again")\nprint("   3. No (esc)")\nanswer = input()\nprint("ANSWERED=" + answer)\ninput()\n'
)


def until(fn, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.25)
    return None


def js_await(d, expr, *args):
    """Awaits a promise-returning expression in the page; raises the page's error."""
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def store_session(d, name):
    return d.js(f"return Object.values({S}.sessions).find(s => s.name === arguments[0]) || null", name)


def status_of(d, name):
    s = store_session(d, name)
    return s and s["status"]


def waiting_names(d):
    return d.js(f"return Object.values({S}.sessions).filter(s => s.alive && s.status === 'waiting').map(s => s.name)")


def screen(d, handle, lines=12):
    return daemon(d, "session.read", {"handle": handle, "lines": lines})["text"]


def active_name(d):
    return d.js(f"const t = {S}.activeTab({S}.state.activeProject); const p = t && {L}.findPane(t.root, t.focus); const s = p && p.sessionId ? {S}.sessions[p.sessionId] : null; return s ? s.name : null;")


def stays(fn, seconds):
    """True when `fn` holds at every look for `seconds`."""
    end = time.time() + seconds
    while time.time() < end:
        if not fn():
            return False
        time.sleep(0.5)
    return True


def open_dashboard(d):
    if not d.js("return !!document.querySelector('.dashboard')"):
        d.keys("ctrl+shift+m")
    d.wait("return !!document.querySelector('.dashboard')", 10)


def needs_you(d):
    return d.js("""
      const h = [...document.querySelectorAll('.dash-col h3')].find(x => x.textContent.startsWith('Needs you'));
      if (!h) return null;
      const col = h.parentElement;
      return [...col.querySelectorAll('.card')].map(c => ({name: c.querySelector('.name').textContent, meta: c.querySelector('.meta').textContent}));
    """)


def dashboard_columns(d):
    return d.js("return [...document.querySelectorAll('.dash-col h3')].map(h => h.textContent.replace(/\\d+$/, ''))")


def click_text(d, selector, text):
    d.js("const b = [...document.querySelectorAll(arguments[0])].find(x => x.textContent.trim() === arguments[1]); if (!b) throw new Error('no ' + arguments[1]); b.click();", selector, text)


def popover(d):
    return d.js("const q = document.querySelector('.quick-answer'); return q ? {title: q.querySelector('h2')?.textContent, text: q.innerText, keys: [...q.querySelectorAll('.quick-answer-keys button')].map(b => b.textContent.trim()), error: q.querySelector('.update-error')?.textContent || ''} : null")


def bell(d):
    return d.js("const b = document.querySelector('.bell-needs-you'); return b ? b.getAttribute('title') : null")


def new_shell(d, pid, name):
    info = js_await(d, f"{S}.newSession(arguments[0], 'shell', {{ name: arguments[1] }})", pid, name)
    if not until(lambda: "PS " in screen(d, info["handle"], 5), 60):
        raise RuntimeError(f"shell {name} never showed a prompt")
    return info


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 7, 30)
    run.check("the app talks to the attention-capable daemon (API 7)", d.js(f"return {S}.daemonApi()") >= 7)

    proj = os.path.join(WORK, "proj")
    os.makedirs(proj, exist_ok=True)
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)
    menu_py = os.path.join(WORK, "fake_agent.py")
    with open(menu_py, "w", encoding="utf-8") as f:
        f.write(MENU)

    first = new_shell(d, pid, "first")
    menu = new_shell(d, pid, "menu")

    # ---------- 1. both listed, oldest wait first; the bell dot ----------
    daemon(d, "session.input", {"handle": first["handle"], "data": 'Read-Host "Continue (y/n)"\r'})
    run.check("Read-Host (y/n) shell turns waiting", until(lambda: status_of(d, "first") == "waiting") is not None)
    time.sleep(1.5)  # the menu starts later than the first prompt
    daemon(d, "session.input", {"handle": menu["handle"], "data": f'python "{menu_py}"\r'})
    run.check("numbered menu shell turns waiting", until(lambda: status_of(d, "menu") == "waiting") is not None)
    ws = {s["name"]: s.get("waiting_since") for s in (store_session(d, "first"), store_session(d, "menu"))}
    run.check("both carry waiting_since, first before menu", isinstance(ws["first"], int) and isinstance(ws["menu"], int) and ws["first"] < ws["menu"], str(ws))
    open_dashboard(d)
    # Opening an overlay moves focus off the terminal; the view reports that to the program as ESC [ O.
    run.check("opening the dashboard keeps both prompts waiting",
              until(lambda: sorted(waiting_names(d)) == ["first", "menu"], 5) is not None, str(waiting_names(d)))
    run.check("dashboard columns: Needs you, Failed, Working, Done · unread, Idle / exited",
              dashboard_columns(d) == ["Needs you", "Failed", "Working", "Done · unread", "Idle / exited"], str(dashboard_columns(d)))
    cards = until(lambda: needs_you(d) if len(needs_you(d) or []) == 2 else None, 10) or needs_you(d) or []
    run.check("Needs you lists first, then menu (oldest wait first)", [c["name"] for c in cards] == ["first", "menu"], str([c["name"] for c in cards]))
    run.check("each card says how long it waited", len(cards) == 2 and all(re.search(r"waiting \d+ (s|min|h)", c["meta"]) for c in cards), str(cards))
    run.shot("01-needs-you")
    d.keys("escape")
    d.wait("return !document.querySelector('.dashboard')", 5)
    run.check("the status-bar bell dot shows, tooltip counts 2", bell(d) == "2 need you", str(bell(d)))
    run.shot("01b-bell-dot")

    # ---------- 2. Ctrl+Shift+. cycles the waiting sessions, longest wait first ----------
    seen = []
    for _ in range(3):
        d.keys("ctrl+shift+.")
        until(lambda: active_name(d) not in (None, *(seen[-1:] or [None])), 5)
        seen.append(active_name(d))
    run.check("Ctrl+Shift+. focuses first, then menu, then wraps to first", seen == ["first", "menu", "first"], str(seen))

    # ---------- 3. dashboard Answer on the y/n prompt ----------
    open_dashboard(d)
    d.js("const c = [...document.querySelectorAll('.dash-col .card')].find(c => c.querySelector('.name')?.textContent === 'first'); c.querySelector('.dash-answer').click();")
    p = until(lambda: popover(d), 10)
    run.check("Answer opens the quick-answer popover for first", bool(p) and p["title"] == "first", str(p and p["title"]))
    run.check("…with the screen tail and the y / n keys",
              bool(p) and "Continue (y/n)" in p["text"] and p["keys"] == ["y", "n"] and "Waiting for" in p["text"], str(p and p["keys"]))
    run.shot("02-quick-answer-yn")
    click_text(d, ".quick-answer-keys button", "y")
    run.check("y leaves the popover", until(lambda: popover(d) is None, 10))
    run.check("the session leaves waiting", until(lambda: status_of(d, "first") != "waiting", 15) is not None, str(status_of(d, "first")))
    text = screen(d, first["handle"])
    run.check("the shell received exactly one y", "Continue (y/n): y" in text and "Continue (y/n): yy" not in text, text[-200:])
    run.check("the menu still waits: the bell dot stays", bell(d) == "1 need you", str(bell(d)))

    # ---------- 4. inbox item opens the menu's popover; free text answers ----------
    d.click(".inbox-anchor")
    d.wait("return !!document.querySelector('.inbox .inbox-item')", 10)
    labels = d.js("return [...document.querySelectorAll('.inbox .group-label')].map(g => g.textContent)")
    run.check("inbox has a Needs you group with the menu", "Needs you" in labels and d.js("return [...document.querySelectorAll('.inbox .inbox-item .inbox-title')].map(t => t.textContent).includes('menu')"), str(labels))
    d.js("[...document.querySelectorAll('.inbox .inbox-item')].find(i => i.querySelector('.inbox-title')?.textContent === 'menu').click();")
    # The screen and keys arrive with `session.read`, a moment after the popover shows.
    p = until(lambda: (q := popover(d)) and q["keys"] and q, 10) or popover(d)
    run.check("the inbox item opens the popover for menu", bool(p) and p["title"] == "menu", str(p and p["title"]))
    run.check("…with keys 1, 2, 3 and the question", bool(p) and p["keys"] == ["1", "2", "3"] and "Do you want to proceed?" in p["text"], str(p and p["keys"]))
    d.click(".quick-answer-text input")
    d.keys("2", "enter")
    got = until(lambda: "ANSWERED=2" in screen(d, menu["handle"], 12), 20)
    run.check("free text 2 + Enter reaches the menu program", bool(got))
    run.check("the popover closes", until(lambda: popover(d) is None, 10))
    run.check("nothing waits once the menu is answered (for 6 s)", stays(lambda: bell(d) is None, 6), str(bell(d)))
    run.shot("03-answered-none-waiting")

    # ---------- 5. stale answer: the prompt was answered from outside meanwhile ----------
    late = new_shell(d, pid, "late")
    daemon(d, "session.input", {"handle": late["handle"], "data": 'Read-Host "Continue (y/n)"\r'})
    run.check("a third prompt turns waiting", until(lambda: status_of(d, "late") == "waiting") is not None)
    open_dashboard(d)
    d.js("const c = [...document.querySelectorAll('.dash-col .card')].find(c => c.querySelector('.name')?.textContent === 'late'); c.querySelector('.dash-answer').click();")
    p = until(lambda: popover(d), 10)
    run.check("the popover opens for late with y / n", bool(p) and p["title"] == "late" and p["keys"] == ["y", "n"], str(p and p["keys"]))
    daemon(d, "session.input", {"handle": late["handle"], "data": "y\r"})  # answered from outside (not the popover)
    run.check("the outside answer ends the wait", until(lambda: status_of(d, "late") != "waiting", 15) is not None)
    # The popover re-reads the screen every second; a prompt that changed locks it.
    locked = until(lambda: "The prompt changed since this was shown." in ((popover(d) or {}).get("text") or ""), 10)
    run.check("the popover locks: the prompt changed since it was shown", bool(locked), str((popover(d) or {}).get("text", ""))[:80])
    run.check("…and offers Show the new prompt", bool(locked) and "Show the new prompt" in popover(d)["text"])
    run.check("…with every key button disabled", bool(locked) and d.js("return [...document.querySelectorAll('.quick-answer-keys button')].every(b => b.disabled)"))
    before = screen(d, late["handle"])
    d.js("[...document.querySelectorAll('.quick-answer-keys button')].forEach(b => b.click());")  # disabled: no handler runs
    time.sleep(1.5)
    after = screen(d, late["handle"])
    run.check("the locked popover types nothing into the session", after == before and "Continue (y/n): n" not in after, after[-160:])
    run.shot("04-stale-answer-locked")
    d.js("document.querySelector('.overlay').dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))")
    d.wait("return !document.querySelector('.quick-answer')", 5)

    # ---------- nothing waiting: the next-waiting shortcut says so ----------
    run.check("nothing is waiting any more", waiting_names(d) == [], str(waiting_names(d)))
    d.keys("ctrl+shift+.")
    run.check("Ctrl+Shift+. with nobody waiting shows a toast", until(lambda: d.js("return document.body.innerText.includes(arguments[0])", "No session is waiting for you"), 5))


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
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-driver", NATIVE])
    if not until(port_open, 30):
        raise SystemExit("tauri-driver did not open its port")

    run = None
    try:
        run = Run(shots, app=app)
        scenario(run)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        teardown(run)
    run.finish()


def teardown(run):
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then
    the test daemon with its sessions, then the driver tree. Removes this run's token and folders."""
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
        c.call("daemon.shutdown", {"kill_sessions": True})
    except Exception:
        pass
    for p in procs.values():
        try:
            p.wait(timeout=5)
        except Exception:
            p.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    shutil.rmtree(DATA, ignore_errors=True)
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
