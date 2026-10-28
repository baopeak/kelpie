"""E2E for confirmation dialogs: nothing happens until the user answers, and Cancel keeps things.

    python -X utf8 scripts/e2e/confirm_dialogs.py <screenshot-dir> <scratch-dir>

tauri-plugin-dialog replaces `window.confirm` with an async one (a native message box, answered
through IPC), so a caller that does not await it goes ahead at once and ignores the answer.

Covers, first with the real native dialog (Windows only: read and answered through UI Automation),
closing a Codex pane from its close button: the dialog names the agent, the pane stays while it
is open, Cancel keeps the pane and the agent, OK closes both. Then, with `window.confirm` replaced
by an async stub the script answers (same contract as the plugin), each surface waits for the
answer and honours it: Ctrl+Shift+W, the tab close button, the sidebar "Stop and close", the git
panel Discard, Move to Trash in the explorer and deleting a saved layout.

Starts a real Codex TUI (no prompt is sent, so no model call). Needs the dev environment as the
other scripts do (KELPIE_ENDPOINT / KELPIE_DATA_DIR / KELPIE_DAEMON_PATH set for tauri-driver).
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon, default_app  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
proj = os.path.join(scratch, "confirm-proj")
os.makedirs(proj, exist_ok=True)

run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def wait(cond, timeout=15, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def git(*args):
    subprocess.run(["git", *args], cwd=proj, check=True, capture_output=True)


def alive(sid):
    return any(s["id"] == sid and s.get("alive") for s in daemon(d, "session.list"))


def pane_of(sid):
    return d.js(f"return {S}.paneLocation(arguments[0])", sid)


def tab_ids():
    return d.js(f"return {S}.workspace({S}.state.activeProject).tabs.map(t => t.id)")


def start(kind, name=None):
    info = js_await(f"{S}.newSession({S}.state.activeProject, arguments[0], {{ where: 'tab', name: arguments[1] || undefined }})", kind, name)
    sid = info["id"]
    wait(lambda: pane_of(sid), 10)
    return sid


# ---------- the real native dialog (UI Automation) ----------

UIA = r"""
param([int]$procId, [string]$action)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$P = [System.Windows.Automation.PropertyCondition]
$cond = New-Object System.Windows.Automation.AndCondition((New-Object $P($A::ProcessIdProperty, $procId)), (New-Object $P($A::ClassNameProperty, '#32770')))
$dlg = $A::RootElement.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
if (-not $dlg) { '{"open": false}'; exit }
$texts = $dlg.FindAll([System.Windows.Automation.TreeScope]::Descendants, (New-Object $P($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Text))) | ForEach-Object { $_.Current.Name }
$clicked = $false
if ($action -ne 'probe') {
  # A task dialog: its buttons are CCPushButton panes, not Button controls.
  $btn = $dlg.FindFirst([System.Windows.Automation.TreeScope]::Descendants, (New-Object $P($A::NameProperty, $action)))
  if ($btn) {
    $inv = $null
    if ($btn.TryGetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern, [ref]$inv)) { $inv.Invoke() }
    else { Add-Type -AssemblyName System.Windows.Forms; $btn.SetFocus(); [System.Windows.Forms.SendKeys]::SendWait(' ') }
    $clicked = $true
  }
}
@{ open = $true; title = $dlg.Current.Name; text = ($texts -join ' | '); clicked = $clicked } | ConvertTo-Json -Compress
"""
uia_script = os.path.join(scratch, "confirm-uia.ps1")
with open(uia_script, "w", encoding="utf-8-sig") as f:
    f.write(UIA)


def app_pid():
    """The app under test, never the user's installed Kelpie (same exe name)."""
    exe = os.path.normcase(os.path.abspath(os.environ.get("KELPIE_E2E_APP") or default_app()))
    out = subprocess.run(["powershell", "-NoProfile", "-Command", "Get-Process kelpie -ErrorAction SilentlyContinue | ForEach-Object { \"$($_.Id)`t$($_.Path)\" }"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        pid, _, path = line.partition("\t")
        if path and os.path.normcase(os.path.abspath(path)) == exe:
            return int(pid)
    return None


def native(pid, action="probe"):
    out = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", uia_script, str(pid), action], capture_output=True, text=True).stdout.strip()
    try:
        return json.loads(out.splitlines()[-1]) if out else {"open": False}
    except json.JSONDecodeError:
        return {"open": False, "raw": out}


def native_case():
    pid = app_pid()
    run.check("found the app under test", pid is not None)
    if not pid:
        return
    run.check("window.confirm is the dialog plugin's async override",
              d.js("return window.confirm.toString().includes('plugin:dialog')"))
    sid = start("codex")
    run.check("Codex session started", wait(lambda: alive(sid), 20))
    time.sleep(2)  # let the TUI draw
    loc = pane_of(sid)
    sel = f".pane[data-pane=\"{loc['paneId']}\"] button[title=\"Close pane\"]"

    d.click(sel)
    dlg = wait(lambda: (lambda r: r if r.get("open") else None)(native(pid)), 10) or {}
    run.check("clicking Close pane opens the native dialog", dlg.get("open"), json.dumps(dlg))
    run.check("the dialog asks to stop codex", "Stop codex and close this pane?" in dlg.get("text", ""), dlg.get("text", ""))
    run.check("the dialog carries the app name", dlg.get("title") == "Kelpie", str(dlg.get("title")))
    time.sleep(1)
    run.check("the pane stays while the dialog is open", pane_of(sid) is not None)
    run.check("the agent keeps running while the dialog is open", alive(sid))
    run.shot("cd-01-native-dialog-open")

    r = native(pid, "Cancel")
    run.check("Cancel can be pressed", r.get("clicked"), json.dumps(r))
    wait(lambda: not native(pid).get("open"), 5)
    time.sleep(1)
    run.check("Cancel keeps the pane", pane_of(sid) is not None)
    run.check("Cancel keeps codex running", alive(sid))

    d.click(sel)
    wait(lambda: native(pid).get("open"), 10)
    r = native(pid, "OK")
    run.check("OK can be pressed", r.get("clicked"), json.dumps(r))
    run.check("OK closes the pane", wait(lambda: pane_of(sid) is None, 10))
    run.check("OK stops codex", wait(lambda: not alive(sid), 15))


# ---------- every other surface, through an async stub ----------

STUB = """window.__asks = []; window.confirm = (message) => new Promise((resolve) => window.__asks.push({ message, resolve }));"""


def asked(timeout=5):
    """Waits for the pending question and returns its text."""
    return wait(lambda: d.js("return window.__asks.length ? window.__asks[0].message : null"), timeout)


def answer(yes):
    d.js("window.__asks.shift().resolve(arguments[0])", yes)


def case(name, trigger, done, timeout=10):
    """Triggers twice: Cancel must leave things as they were (also while waiting), OK must act."""
    trigger()
    q = asked()
    run.check(f"{name}: asks first", q, str(q))
    time.sleep(0.8)
    run.check(f"{name}: nothing happens before the answer", not done())
    answer(False)
    time.sleep(0.8)
    run.check(f"{name}: Cancel keeps it", not done())
    trigger()
    asked()
    answer(True)
    run.check(f"{name}: OK does it", wait(done, timeout))


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, layouts: [], hibernate: false, tips: false }});")
    with open(os.path.join(proj, "tracked.txt"), "w", encoding="utf-8") as f:
        f.write("original\n")
    with open(os.path.join(proj, "trash-me.txt"), "w", encoding="utf-8") as f:
        f.write("bye\n")
    if not os.path.isdir(os.path.join(proj, ".git")):
        git("init", "-q")
        git("-c", "user.name=e2e", "-c", "user.email=e2e@example.invalid", "add", "tracked.txt")
        git("-c", "user.name=e2e", "-c", "user.email=e2e@example.invalid", "commit", "-qm", "init")
    d.js(f"return {S}.addProject(arguments[0])", proj)
    start("shell")

    if os.name == "nt":
        native_case()

    d.js(STUB)

    # Ctrl+Shift+W on a focused agent pane.
    sid = start("codex")
    wait(lambda: alive(sid), 20)
    time.sleep(1)
    d.click(f".pane[data-pane=\"{pane_of(sid)['paneId']}\"] .xterm")
    case("Ctrl+Shift+W", lambda: d.keys("ctrl+shift+w"), lambda: pane_of(sid) is None)
    run.check("Ctrl+Shift+W: OK stops the agent", wait(lambda: not alive(sid), 15))

    # Tab close button with a live agent in it.
    sid = start("codex")
    wait(lambda: alive(sid), 20)
    tab = pane_of(sid)["tabId"]
    case("tab close", lambda: d.click(f"[data-tab-id=\"{tab}\"] .close"), lambda: tab not in tab_ids())

    # Sidebar session menu: Stop and close.
    sid = start("codex", "confirm-sidebar-codex")
    wait(lambda: alive(sid), 20)

    def stop_from_sidebar():
        d.js("const r = [...document.querySelectorAll('.session-row')].find(e => e.textContent.includes('confirm-sidebar-codex')); r.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 120, clientY: 160}));")
        d.wait("return !!document.querySelector('.ctx-menu')", 5)
        d.js("[...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Stop and close')).click()")
    case("sidebar Stop and close", stop_from_sidebar, lambda: not alive(sid), 15)

    # Git panel: Discard a modified file.
    tracked = os.path.join(proj, "tracked.txt")
    with open(tracked, "w", encoding="utf-8") as f:
        f.write("edited\n")
    d.js(f"const s = {S}; if (s.workspace(s.state.activeProject).rightPanel !== 'git') s.setRightPanel('git');")
    d.wait("return [...document.querySelectorAll('.list-row')].some(r => r.title === 'tracked.txt')", 15)
    discard = "const r = [...document.querySelectorAll('.list-row')].find(r => r.title === 'tracked.txt'); r.querySelector('button[title=\"Discard\"]').click();"
    case("git Discard", lambda: d.js(discard), lambda: open(tracked, encoding="utf-8").read() == "original\n")

    # Explorer: Move to Trash.
    trash_me = os.path.join(proj, "trash-me.txt")
    d.js(f"const s = {S}; s.setRightPanel('files');")
    d.wait("return [...document.querySelectorAll('.tree-row')].some(r => r.title === 'trash-me.txt')", 15)

    def trash():
        d.js("const r = [...document.querySelectorAll('.tree-row')].find(r => r.title === 'trash-me.txt'); r.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 1300, clientY: 200}));")
        d.wait("return !!document.querySelector('.ctx-menu')", 5)
        d.js("[...document.querySelectorAll('.ctx-item')].find(e => e.textContent.includes('Move to Trash')).click()")
    case("Move to Trash", trash, lambda: not os.path.exists(trash_me))

    # Settings: delete a saved layout.
    p = d.js(f"return {S}.state.activeProject")
    d.js(f"const s = {S}; s.saveLayout(arguments[0], s.activeTab(arguments[0]).id, 'confirm-e2e')", p)
    d.js(f"{S}.openSettings('layouts')")
    found = wait(lambda: d.js("return !![...document.querySelectorAll('button[title=\"Delete layout\"]')].length"), 5)
    run.check("saved layout listed in Settings", found)
    if found:
        case("delete layout", lambda: d.js("document.querySelector('button[title=\"Delete layout\"]').click()"),
             lambda: not d.js(f"return {S}.state.settings.layouts?.some(l => l.name === 'confirm-e2e')"))
    d.js(f"{S}.setOverlay(null)")
    run.shot("cd-02-end")
    run.check("no question left unanswered", d.js("return window.__asks.length") == 0)
except Exception as e:  # noqa: BLE001
    run.check("scenario ran to completion", False, repr(e))
    try:
        run.shot("cd-99-error")
    except Exception:
        pass
finally:
    run.finish()
