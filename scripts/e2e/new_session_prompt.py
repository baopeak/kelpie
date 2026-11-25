"""GUI e2e: the first prompt typed in the New session dialog reaches the agent when the session opens as a chat.

    python -X utf8 scripts/e2e/new_session_prompt.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug app and daemon, `pnpm dev` (the debug app loads the dev server), `tauri-driver` on PATH and `msedgedriver.exe`
matching the installed WebView2 (on PATH, or in MSEDGEDRIVER). The script starts its own tauri-driver, and the app starts its own
daemon (isolated endpoint and data folder, set in this process before anything talks to a daemon); both stop on exit. No account or
network: "claude" is a stand-in that speaks Claude's stream-json, and an ACP agent saved as if from the registry runs
scripts/e2e/fake_acp_agent.py. Each one notes every prompt it receives, so the counts below are what the agents got.

As a person would, in the New session dialog:
1. Claude, "Chat view", a prompt and a name, Start: the prompt box is there, and Claude gets the prompt once and shows it as the first
   message; the typed name is the session's name in the sidebar row and on its tab.
2. Claude, a prompt typed with "Terminal (TUI)" selected, then "Chat view", Start: Claude gets that prompt once.
3. Claude, "Chat view", "Run in a new worktree", a prompt, Start: Claude gets it once, in the new worktree's folder.
4. An agent from the ACP registry (it always opens as a chat), a prompt, Start: the agent gets it once.
5. The window reloads: no prompt is sent a second time.
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
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

WORK = os.path.join(scratch, f"new-session-prompt-{os.getpid()}")
PROJ, BIN = os.path.join(WORK, "proj"), os.path.join(WORK, "bin")
for path in (PROJ, BIN):
    os.makedirs(path)
with open(os.path.join(PROJ, "README.md"), "w", encoding="utf-8") as f:
    f.write("fixture\n")
for args in (["init", "-q", "-b", "main"], ["add", "."], ["-c", "user.name=e2e", "-c", "user.email=e2e@example.com", "commit", "-qm", "init"]):
    subprocess.run(["git", *args], cwd=PROJ, check=True)

# What the agents received: one JSON line per prompt.
CLAUDE_LOG, ACP_LOG = os.path.join(WORK, "claude-prompts.jsonl"), os.path.join(WORK, "acp-client.jsonl")
FAKE_CLAUDE = os.path.join(WORK, "fake_claude.py")
with open(FAKE_CLAUDE, "w", encoding="utf-8") as f:
    f.write(
        "import json, os, sys, time\n"
        f"LOG = {CLAUDE_LOG!r}\n"
        "if '--input-format' not in sys.argv:\n"
        "    print('READY', flush=True)\n"
        "    time.sleep(3600)\n"
        "    sys.exit(0)\n"
        "SID = 'fake-conv-' + str(os.getpid())\n"
        "def out(o):\n"
        "    sys.stdout.write(json.dumps(o) + '\\n'); sys.stdout.flush()\n"
        "n = 0\n"
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
        "    with open(LOG, 'a', encoding='utf-8') as f:\n"
        "        f.write(json.dumps({'text': text, 'cwd': os.getcwd()}) + '\\n')\n"
        "    n += 1\n"
        "    out({'type': 'system', 'subtype': 'init', 'session_id': SID, 'model': 'claude-test', 'tools': []})\n"
        "    out(m)  # --replay-user-messages: the prompt comes back as the agent took it\n"
        "    reply = 'Got: ' + text\n"
        "    out({'type': 'assistant', 'session_id': SID, 'message': {'id': f'm{n}', 'model': 'claude-test', 'role': 'assistant', 'content': [{'type': 'text', 'text': reply}]}})\n"
        "    out({'type': 'result', 'subtype': 'success', 'is_error': False, 'result': reply, 'session_id': SID})\n"
    )
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE_CLAUDE}" %*\r\n')

DATA = tempfile.mkdtemp(prefix="kelpie-new-session-prompt-data-")
# Never the user's daemon: the endpoint and data folder are this run's own, set here before anything connects.
os.environ.update({
    "KELPIE_ENDPOINT": f"kelpie-new-session-prompt-{os.getpid()}", "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "KELPIE_DAEMON_PATH": kelpied, "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CONFIG_DIR"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT, NATIVE_PORT = free_port(), free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver") or shutil.which(os.path.join(os.path.expanduser("~"), ".cargo", "bin", "tauri-driver"))
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver on PATH and msedgedriver (PATH or MSEDGEDRIVER)")

S = "window.__kelpie.store"
REG_KIND = "e2e-registry-fake"
REGISTRY = {"id": REG_KIND, "label": "Registry Fake", "bin": sys.executable, "acp": [os.path.join(HERE, "fake_acp_agent.py"), "--log", ACP_LOG], "version": "1.0.0", "registryId": "e2e-fake"}


def until(fn, timeout=30, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


def claude_prompts():
    if not os.path.exists(CLAUDE_LOG):
        return []
    with open(CLAUDE_LOG, encoding="utf-8") as f:
        return [json.loads(x) for x in f if x.strip()]


def acp_prompts():
    if not os.path.exists(ACP_LOG):
        return []
    out = []
    with open(ACP_LOG, encoding="utf-8") as f:
        for x in f:
            m = json.loads(x) if x.strip() else {}
            if m.get("method") == "session/prompt":
                out.append(" ".join(b.get("text", "") for b in m.get("params", {}).get("prompt", []) if b.get("type") == "text"))
    return out


def received(text):
    """How many times the agents got `text` as a prompt."""
    return sum(1 for p in claude_prompts() if p["text"] == text) + sum(1 for p in acp_prompts() if p == text)


driver = subprocess.Popen([TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", NATIVE], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
until(lambda: socket.socket().connect_ex(("127.0.0.1", DRIVER_PORT)) == 0, 30)
run = Run(shots, app)
d = run.d
counts = {}


def session_ids():
    return set(d.js(f"return Object.keys({S}.sessions)") or [])


def toggle(label):
    return d.js("const b = [...document.querySelectorAll('.modal .toggle')].find(b => b.innerText.trim() === arguments[0]); if (b) b.click(); return !!b;", label)


def open_dialog(card):
    """The New session dialog, with the agent whose card label contains `card` chosen."""
    d.js(f"{S}.setOverlay('newsession')")
    d.wait("return !!document.querySelector('.modal .launch-card')", 10)
    ok = d.js("const c = [...document.querySelectorAll('.modal .launch-card')].find(x => x.lastElementChild.textContent.includes(arguments[0])); if (c) c.click(); return !!c;", card)
    run.check(f"the dialog offers {card}", ok, str(d.js("return [...document.querySelectorAll('.modal .launch-card')].map(c => c.lastElementChild.textContent.trim())")))


def type_prompt(text):
    """Types into the prompt box as a person would; False when the dialog shows none."""
    if not d.js("return !!document.querySelector('.modal textarea')"):
        return False
    d.click(".modal textarea")
    d.keys(text)
    return d.js("return document.querySelector('.modal textarea').value") == text


def start():
    before = session_ids()
    d.js("[...document.querySelectorAll('.modal .footer .btn.primary')].find(b => b.innerText.trim() === 'Start').click();")
    new = until(lambda: list(session_ids() - before), 60)
    return new[0] if new else None


def type_name(text):
    """Types into the dialog's Name field as a person would."""
    found = d.js("const r = [...document.querySelectorAll('.modal .form-row')].find(x => x.querySelector('label')?.textContent.trim() === 'Name (optional)'); const i = r?.querySelector('input'); if (i) i.setAttribute('data-e2e-name', ''); return !!i;")
    if not found:
        return False
    d.click(".modal [data-e2e-name]")
    d.keys(text)
    return d.js("return document.querySelector('.modal [data-e2e-name]').value") == text


def shown_names(sid):
    """The session's name as the sidebar row and the tab that holds it show it."""
    return d.js(f"""const s = {S}; const row = document.querySelector(`.session-row[data-row-session='${{arguments[0]}}'] .name`);
      const loc = s.paneLocation(arguments[0]); const tab = loc && document.querySelector(`.tab[data-tab-id='${{loc.tabId}}']`);
      return {{ store: s.sessions[arguments[0]]?.name ?? null, row: row ? row.textContent.trim() : null, tab: tab ? tab.innerText.trim() : null }};""", sid)


def chat_users(sid):
    return d.js(f"const c = window.__kelpie.chat.getController({S}.sessions[arguments[0]]); return c.state.items.filter(i => i.kind === 'user').map(i => i.text);", sid) or []


def settle(text, seconds=10):
    """Waits for `text` to arrive, then a moment longer, so a second copy would show too."""
    until(lambda: received(text) >= 1, seconds)
    time.sleep(2)
    return received(text)


try:
    run.boot()
    d.set_window_size(1300, 860)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false, autoWorktree: 'ask', registryAgents: [arguments[0]] }});", REGISTRY)
    proj = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0])", proj)
    until(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.is_repo === true", proj), 20)
    d.js_async("const done = arguments[arguments.length - 1]; window.__kelpie.plugins.syncPluginAgents(true).then(() => done(true), () => done(false));")
    until(lambda: d.js(f"return !!{S}.registryAgent(arguments[0]) && window.__kelpie.agents.acpArgv(arguments[0]) !== null", REG_KIND), 15)

    # 1. Chat view chosen first, then the prompt.
    P1 = "first prompt in chat view"
    open_dialog("Claude")
    run.check("1: Chat view is offered for Claude", toggle("Chat view"))
    typed = type_prompt(P1)
    run.check("1: the prompt box is there in chat view", typed)
    NAME1 = "named chat"
    run.check("1: the name is typed in the Name field", type_name(NAME1))
    run.shot("01-dialog-chat-view")
    sid1 = start()
    run.check("1: the session starts", bool(sid1))
    counts["chat view"] = settle(P1)
    run.check("1: Claude got the prompt exactly once", counts["chat view"] == 1, str(claude_prompts()))
    shown = until(lambda: P1 in chat_users(sid1), 10) if sid1 else None
    run.check("1: the chat shows it as the first message", bool(shown) and chat_users(sid1)[:1] == [P1], str(sid1 and chat_users(sid1)))
    run.check("1: and the reply to it", bool(until(lambda: d.js("return [...document.querySelectorAll('.chat .msg-assistant')].some(e => e.textContent.includes(arguments[0]))", "Got: " + P1), 10)))
    names = until(lambda: (lambda n: n if n["row"] == NAME1 and NAME1 in (n["tab"] or "") else None)(shown_names(sid1)), 10) if sid1 else None
    names = names or (shown_names(sid1) if sid1 else {})
    run.check("1: the session has the typed name", names.get("store") == NAME1, str(names))
    run.check("1: the sidebar row shows the typed name", names.get("row") == NAME1, str(names))
    run.check("1: the chat's tab shows the typed name", NAME1 in (names.get("tab") or ""), str(names))
    run.shot("02-chat-first-prompt")

    # 2. Typed with Terminal selected, then switched to Chat view.
    P2 = "typed in terminal then chat"
    open_dialog("Claude")
    toggle("Terminal (TUI)")
    run.check("2: the prompt is typed with Terminal (TUI) selected", type_prompt(P2))
    toggle("Chat view")
    run.check("2: the typed prompt is still shown after choosing Chat view", d.js("return document.querySelector('.modal textarea')?.value ?? null") == P2, str(d.js("return document.querySelector('.modal textarea')?.value ?? null")))
    sid2 = start()
    run.check("2: the session starts as a chat", bool(sid2) and d.js(f"return !!{S}.sessions[arguments[0]]?.chat", sid2))
    counts["terminal then chat"] = settle(P2)
    run.check("2: Claude got the prompt exactly once", counts["terminal then chat"] == 1, str(claude_prompts()))

    # 3. In a new worktree.
    P3 = "work on the readme in a worktree"
    open_dialog("Claude")
    toggle("Chat view")
    typed3 = type_prompt(P3)
    run.check("3: the prompt box is there", typed3)
    box = d.js("const l = [...document.querySelectorAll('.modal .check-label')].find(x => x.textContent.includes('Run in a new worktree')); const i = l?.querySelector('input'); if (i && !i.checked) i.click(); return l ? l.textContent.trim() : null;")
    run.check("3: Run in a new worktree is offered and ticked", bool(box), str(box))
    run.shot("03-dialog-worktree")
    sid3 = start()
    run.check("3: the session starts", bool(sid3))
    cwd3 = d.js(f"return {S}.sessions[arguments[0]]?.cwd ?? null", sid3) if sid3 else None
    run.check("3: it runs in a new worktree folder", bool(cwd3) and os.path.normcase(os.path.abspath(cwd3)) != os.path.normcase(PROJ) and "-worktrees" in cwd3, str(cwd3))
    if sid3 and not d.js(f"return Object.values({S}.state.workspaces).some(w => w.tabs.some(t => JSON.stringify(t.root).includes(arguments[0])))", sid3):
        run.check("3: the chat pane is placed", False)
    counts["new worktree"] = settle(P3, 20)
    got3 = [p for p in claude_prompts() if p["text"] == P3]
    run.check("3: Claude got the prompt exactly once, in the worktree", counts["new worktree"] == 1 and all(os.path.normcase(p["cwd"]) == os.path.normcase(cwd3 or "") for p in got3), str(got3))
    run.shot("04-worktree-chat")

    # 4. An agent from the ACP registry: it always opens as a chat.
    P4 = "hello registry agent"
    open_dialog("Registry Fake")
    run.check("4: the prompt box is there", type_prompt(P4))
    sid4 = start()
    run.check("4: the session starts as a chat", bool(sid4) and d.js(f"return !!{S}.sessions[arguments[0]]?.chat", sid4))
    counts["registry agent"] = settle(P4, 20)
    run.check("4: the agent got the prompt exactly once", counts["registry agent"] == 1, str(acp_prompts()))
    run.shot("05-registry-chat")

    # 5. Reload: nothing is sent again.
    d.js("location.reload()")
    time.sleep(2)
    run.boot()
    for sid in (s for s in (sid1, sid2, sid3, sid4) if s):
        # Shows the chat, so that its view attaches again and replays the log.
        d.js(f"const s = {S}; const loc = s.paneLocation(arguments[0]); if (loc) {{ s.switchProject(loc.projectId); s.selectTab(loc.projectId, loc.tabId); }}", sid)
        time.sleep(2)
    time.sleep(4)
    after = {name: received(p) for name, p in (("chat view", P1), ("terminal then chat", P2), ("new worktree", P3), ("registry agent", P4))}
    run.check("5: after a reload every prompt was still received exactly once", all(v == 1 for v in after.values()), str(after))
    run.shot("06-after-reload")
finally:
    print("COUNTS " + json.dumps(counts), flush=True)
    print("CLAUDE GOT " + json.dumps([p["text"] for p in claude_prompts()]), flush=True)
    print("ACP GOT " + json.dumps(acp_prompts()), flush=True)
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
