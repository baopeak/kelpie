"""GUI e2e: a Claude chat keeps its permission mode when it comes back after the computer (and so the daemon) stopped.

    python -X utf8 scripts/e2e/chat_mode_restore.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app, `tauri-driver` on PATH, `msedgedriver.exe` matching the installed WebView2 (on PATH, or in
MSEDGEDRIVER), and a signed-in `claude`: it sends ONE message to Haiku (a one-word reply), so that Claude itself writes the
transcript a restart reads. Its own daemon, data folder and tauri-driver; the daemon it stops abruptly is that one, checked
by its path (Windows: it reads process command lines). The scratch project's Claude transcript folder is removed at the end.

1. With Permissions: Yolo, a new chat starts asking before edits and offers bypass; switching to it works.
2. The daemon stops abruptly (as when the computer restarts): the chat comes back as a new session, in bypass, started
   with `--permission-mode bypassPermissions`.
3. With Permissions: Manual, bypass picked in a chat whose process was started without the flag that allows it: it asks
   first, then the old process stops and Claude continues the same conversation in the same pane, started in bypass.
"""
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
APP = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
KELPIED = os.path.join(os.path.dirname(APP), "kelpied.exe" if os.name == "nt" else "kelpied")
os.makedirs(scratch, exist_ok=True)
WORK = tempfile.mkdtemp(prefix="mode-check-", dir=scratch)
PROJ = os.path.join(WORK, "proj")
os.makedirs(PROJ)
for k in list(os.environ):
    if k.startswith(("KELPIE_", "CLAUDE_CODE_")) or k == "CLAUDECODE":
        os.environ.pop(k)
os.environ.update({"KELPIE_ENDPOINT": f"kelpie-mode-{os.getpid()}", "KELPIE_DATA_DIR": os.path.join(WORK, "data"), "KELPIE_E2E": "1", "KELPIE_DAEMON_PATH": KELPIED})
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
if not NATIVE or not shutil.which("tauri-driver") or not shutil.which("claude"):
    raise SystemExit("needs tauri-driver, msedgedriver (PATH or MSEDGEDRIVER) and claude")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


drv, native = free_port(), free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{drv}"

driver = subprocess.Popen(["tauri-driver", "--port", str(drv), "--native-port", str(native), "--native-driver", NATIVE], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
time.sleep(2)
S = "window.__kelpie.store"
C = f"window.__kelpie.chat.getController({S}.sessions[arguments[0]])"


def until(fn, timeout, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


run = Run(shots, APP)
d = run.d
try:
    run.boot()
    d.set_window_size(1300, 800)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false, agentPermissions: 'yolo' }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    CONV = str(uuid.uuid4())
    # A conversation id from the start (the daemon starts it as `--session-id`), so a restart resumes it by its transcript.
    sid = d.js_async("const [p, c, done] = arguments; window.__kelpie.chat.newChat(p, 'claude', 'tab', {resume: c}).then(i => done(i ? i.id : null), e => done(String(e)));", pid, CONV)
    mode = lambda s: d.js(f"const m = {C}.state.settings.find(x => x.id === 'mode'); return m ? [m.value, m.options.map(o => o.value)] : null", s)
    got = until(lambda: mode(sid), 60)
    run.check("a new chat starts asking before edits", got and got[0] == "default", got)
    run.check("…and, in Yolo, offers bypass", got and "bypassPermissions" in got[1], got)
    d.js(f"{C}.setSetting('mode', 'bypassPermissions')", sid)
    run.check("switching to bypass works", bool(until(lambda: (mode(sid) or [None])[0] == "bypassPermissions", 20)), mode(sid))
    # One real message (Haiku, a one-word reply), so that Claude itself writes the transcript the restart reads.
    d.js(f"{C}.setSetting('model', 'haiku')", sid)
    time.sleep(1)
    d.js(f"{C}.send('Reply with only the word OK.')", sid)
    run.check("a message is answered in bypass", bool(until(lambda: d.js(f"return !{C}.state.busy && {C}.state.items.some(i => i.kind === 'result')", sid), 90)), d.js(f"return {C}.state.items.map(i => i.kind).join(',')", sid))
    # As when the computer restarts: the daemon and the chat's process just stop. Only this test's daemon (its own pid, from
    # the debug build) is stopped; the app starts a new one itself (KELPIE_DAEMON_PATH).
    info = daemon(d, "daemon.info")
    exe = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-Process -Id {info['pid']}).Path"], capture_output=True, text=True).stdout.strip()
    assert os.path.normcase(exe) == os.path.normcase(KELPIED), exe
    subprocess.run(["taskkill", "/PID", str(info["pid"]), "/T", "/F"], capture_output=True)
    new = until(lambda: d.js(f"return Object.values({S}.sessions).find(s => s.alive && s.kind === 'claude' && s.id !== arguments[1] && s.agent_session_id === arguments[0])?.id ?? null", CONV, sid), 90)
    run.check("after a daemon restart the chat comes back, as a new session", bool(new), f"{new} (was {sid})")
    if new:
        back = until(lambda: (lambda m: m if m and m[0] else None)(mode(new)), 60)
        run.check("…in bypass, the mode it last ran in", back and back[0] == "bypassPermissions", back)
        argv = subprocess.run(["powershell", "-NoProfile", "-Command", "Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'claude*' -and $_.CommandLine -like '*" + CONV + "*' } | ForEach-Object { $_.CommandLine }"], capture_output=True, text=True).stdout
        run.check("…started with that mode", "--permission-mode bypassPermissions" in argv, " ".join(a for a in argv.split() if a.startswith("--") and "format" not in a))

    # 3. Bypass picked in a chat whose process cannot switch to it (started before chat processes got the flag), with
    # Permissions: Manual. It asks first, then Claude continues the same conversation, started in bypass.
    d.js(f"{S}.setSettings({{ agentPermissions: 'manual' }}); window.__confirms = []; window.confirm = (m) => (window.__confirms.push(m), true);")
    OLD = str(uuid.uuid4())
    old_argv = ["claude", "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--replay-user-messages", "--permission-prompt-tool", "stdio", "--resume", OLD]
    old = d.js_async(f"const [p, c, argv, cwd, done] = arguments; window.__kelpie.ipc.createSession({{kind: 'claude', cwd, project_id: p, agent_session_id: c, chat_argv: argv}}).then(i => {{ {S}.setSessions(i.id, i); {S}.placePane(p, {{id: 'pane-' + i.id, kind: 'chat', sessionId: i.id}}, 'tab'); done(i.id); }}, e => done(String(e)));", pid, OLD, old_argv, PROJ)
    run.check("a chat started without the flag", bool(until(lambda: (mode(old) or [None])[0] == "default", 60)), mode(old))
    picked = d.js("const btn = [...document.querySelectorAll('.setting-btn')].pop(); btn.click(); const opt = [...document.querySelectorAll('.setting-opt')].find(o => /Bypass permissions/.test(o.textContent)); if (!opt) return false; opt.click(); return true;")
    run.check("…offers bypass in its mode chip", bool(picked))
    run.check("…asks before bypassing (Permissions: Manual)", bool(until(lambda: d.js("return window.__confirms.length"), 5)), d.js("return window.__confirms"))
    again = until(lambda: d.js(f"return Object.values({S}.sessions).find(s => s.alive && s.id !== arguments[1] && s.agent_session_id === arguments[0])?.id ?? null", OLD, old), 60)
    run.check("…and starts Claude again for the same conversation", bool(again), f"{again} (was {old})")
    if again:
        run.check("…in bypass", bool(until(lambda: (mode(again) or [None])[0] == "bypassPermissions", 60)), mode(again))
        run.check("…the old process stopped: one writer", not d.js(f"return !!{S}.sessions[arguments[0]]?.alive", old))
        procs = subprocess.run(["powershell", "-NoProfile", "-Command", "Get-CimInstance Win32_Process | Where-Object { $_.Name -like 'claude*' -and $_.CommandLine -like '*" + OLD + "*' } | ForEach-Object { $_.CommandLine }"], capture_output=True, text=True).stdout.strip().splitlines()
        run.check("…one Claude process for it, started in bypass", len(procs) == 1 and "--permission-mode bypassPermissions" in procs[0], f"{len(procs)} process(es)")
        run.check("…in the same pane", d.js(f"return {S}.workspace(arguments[0]).tabs.some(t => JSON.stringify(t.root).includes(arguments[1]))", pid, again))
    run.shot("03-bypass-restart")
    for s in list(filter(None, [again, old])):
        d.js_async(f"const [id, done] = arguments; const s = {S}.sessions[id]; s ? window.__kelpie.ipc.daemon('chat.close_stdin', {{ handle: s.handle }}).then(() => done(1), () => done(0)) : done(0);", s)
    for s in list(filter(None, [new, sid])):
        d.js_async(f"const [id, done] = arguments; const s = {S}.sessions[id]; s ? window.__kelpie.ipc.daemon('chat.close_stdin', {{ handle: s.handle }}).then(() => done(1), () => done(0)) : done(0);", s)
    time.sleep(3)
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
        time.sleep(1)
        shutil.rmtree(WORK, ignore_errors=True)
        for p in os.listdir(os.path.expanduser(r"~\.claude\projects")):
            if "mode-check-" in p:
                shutil.rmtree(os.path.join(os.path.expanduser(r"~\.claude\projects"), p), ignore_errors=True)
