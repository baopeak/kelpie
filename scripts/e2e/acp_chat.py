"""GUI e2e for universal ACP chat: any ACP agent gets the chat view, and its turns, questions,
sign-in and status behave like the agent sent them.

    python -X utf8 scripts/e2e/acp_chat.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver with the same
environment, so the app talks only to that daemon, and stops both on exit.

A plugin "acp-lab" (installed and approved in Settings → Plugins, like a user would) offers two ACP agents
that run scripts/e2e/fake_acp_agent.py: "Fake ACP" and "Fake ACP sign-in" (`--auth`).

1. The agent is offered in the new-session menu with a chat view; choosing it opens the chat view.
2. A prompt runs a full turn: a thought, the tool card "Read notes.txt", the permission card (Allow), the
   answer, the session title "Fake task: …" and usage in the composer. Status goes working → waiting →
   working → idle and stays idle after the trailing usage update.
3. The model and mode pickers come from configOptions; changing the model sends session/set_config_option.
4. "ask me": a question card "Colour" (text field) is answered; the status is waiting while it is open;
   the agent replies "You said: <answer>".
5. The agent process is killed; Resume starts it again with session/load, which replays the conversation;
   the status is idle afterwards and a new prompt works.
6. The sign-in agent shows "Sign in to continue"; "Fake login" runs authenticate, the session opens and a
   prompt works.
Regression runs (attention.py, smoke.py, typecheck, unit tests) are separate commands.
"""
import importlib.util
import json
import os
import shutil
import socket
import stat
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
FAKE = os.path.join(HERE, "fake_acp_agent.py")

# Isolated daemon: its own endpoint and data folder; the app and tauri-driver inherit these.
ENDPOINT = f"kelpie-acp-chat-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-acp-chat-data-")
WORK = os.path.join(scratch, f"acp-chat-{os.getpid()}")
os.makedirs(WORK, exist_ok=True)
os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1"})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)

# The fake agents' conversations and the messages the client sent them (for the request checks).
STATE, LOG = os.path.join(WORK, "main-state.json"), os.path.join(WORK, "main-client.log")
STATE_AUTH, LOG_AUTH = os.path.join(WORK, "auth-state.json"), os.path.join(WORK, "auth-client.log")
PROJ = os.path.join(WORK, "proj")
PLUGIN_SRC = os.path.join(WORK, "plugin-src", "acp-lab")
MANIFEST = {
    "id": "acp-lab",
    "name": "ACP Lab",
    "version": "1.0.0",
    "description": "Fake ACP agents for the chat-view e2e",
    "contributes": {"agents": [
        {"id": "fake", "label": "Fake ACP", "bin": "python", "acp": [FAKE, "--state", STATE, "--log", LOG]},
        {"id": "auth", "label": "Fake ACP sign-in", "bin": "python", "acp": [FAKE, "--auth", "--state", STATE_AUTH, "--log", LOG_AUTH]},
    ]},
}
KIND, KIND_AUTH = "acp-lab-fake", "acp-lab-auth"


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
P = "window.__kelpie.plugins"
OPEN_PERM = "[...document.querySelectorAll('.perm')].filter(p => p.querySelector('.actions .btn'))"
ITEMS = ("c.state.items.map(i => ({kind: i.kind, text: i.text || '', name: i.name || '', status: i.status || '', "
         "output: i.output || '', resolved: i.resolved || ''}))")
TL = []  # status changes of the session under watch, in order


def until(fn, timeout=30, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


def js_await(d, expr, *args):
    """Awaits a promise-returning expression in the page; raises the page's error."""
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def write_manifest(folder, manifest):
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "kelpie-plugin.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)


def read_lines(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return [json.loads(x) for x in f if x.strip()]


def scenario(run, c):
    d = run.d

    def info(sid):
        return next((s for s in c.call("session.list") if s["id"] == sid), {})

    def status(sid):
        return info(sid).get("status")

    def sessions_of(kind):
        return [s for s in c.call("session.list") if s["kind"] == kind]

    def track(sid, fn, timeout=30):
        """Waits for `fn` (returns its value), recording each status change of the session on the way."""
        end = time.time() + timeout
        while time.time() < end:
            st = status(sid)
            if st and (not TL or TL[-1] != st):
                TL.append(st)
            v = fn()
            if v:
                return v
            time.sleep(0.15)
        return None

    def stays(sid, fn_status, seconds):
        """True when the status is `fn_status` at every look for `seconds`."""
        end = time.time() + seconds
        while time.time() < end:
            if status(sid) != fn_status:
                return False
            time.sleep(0.3)
        return True

    def ctl(sid, expr):
        return d.js("const c = window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return " + expr + ";", sid)

    def items(sid):
        return ctl(sid, ITEMS)

    def open_perm():
        return d.js(f"const p = {OPEN_PERM}.pop(); if (!p) return null; return {{id: p.dataset.perm, title: p.querySelector('.q').innerText, buttons: [...p.querySelectorAll('.actions .btn')].map(b => b.innerText.replace(/\\s+/g, ' ').trim()), asks: [...p.querySelectorAll('.ask-q')].map(q => q.innerText.trim())}};")

    def click_perm(label):
        return d.js(f"const p = {OPEN_PERM}.pop(); const b = p && [...p.querySelectorAll('.actions .btn')].find(x => x.innerText.trim().endsWith(arguments[0])); if (!b) return false; b.click(); return true;", label)

    def send(text):
        d.js("const t = document.querySelector('.composer-box textarea'); t.focus(); t.value = arguments[0]; t.dispatchEvent(new Event('input', { bubbles: true }));", text)
        d.keys("enter")

    def replies():
        return d.js("return [...document.querySelectorAll('.msg-reply')].map(e => e.innerText.trim())")

    def start_chat(label, kind):
        """New-session menu → the agent's card → Chat view → Start. Returns the new session (not
        yet initialised: the caller waits for what it needs)."""
        before = {s["id"] for s in sessions_of(kind)}
        d.js(f"{S}.setOverlay('newsession')")
        d.wait("return !!document.querySelector('.launch-card')", 10)
        d.js("const c = [...document.querySelectorAll('.launch-card')].find(x => x.lastElementChild.textContent.trim() === arguments[0]); if (!c) throw new Error('no card ' + arguments[0]); c.click();", label)
        d.wait("return [...document.querySelectorAll('.modal .toggle')].some(b => b.innerText.trim() === 'Chat view')", 10)
        d.js("[...document.querySelectorAll('.modal .toggle')].find(b => b.innerText.trim() === 'Chat view').click();")
        d.js("[...document.querySelectorAll('.modal .footer .btn.primary')].find(b => b.innerText.trim() === 'Start').click();")
        return until(lambda: next((x for x in sessions_of(kind) if x["id"] not in before), None), 30)

    # ---------- setup: the fake agents are a plugin, approved in Settings ----------
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    run.check("the app talks to an ACP-capable daemon (API 8)", until(lambda: d.js(f"return {S}.daemonApi()") >= 8, 30) is not None, str(d.js(f"return {S}.daemonApi()")))
    os.makedirs(PROJ, exist_ok=True)
    with open(os.path.join(PROJ, "notes.txt"), "w", encoding="utf-8") as f:
        f.write("hello\n")
    write_manifest(PLUGIN_SRC, MANIFEST)
    d.js(f"{S}.addProject(arguments[0])", PROJ)
    r = js_await(d, f"{P}.pluginInstallFolder(arguments[0])", PLUGIN_SRC)
    run.check("plugin installs from its folder", r == "acp-lab", str(r))
    run.check("before approval the agents are not offered", "acp-lab-fake" not in [p["id"] for p in c.call("agent.presets")])
    d.js(f"{S}.openSettings('plugins')")
    d.wait("return !!document.querySelector('[data-plugin=\"acp-lab\"]')", 10)
    d.js("[...document.querySelectorAll('[data-plugin=\"acp-lab\"] button')].find(b => b.innerText.trim() === 'Review & enable').click();")
    d.wait("return !!document.querySelector('[data-review=\"acp-lab\"]')", 10)
    review = d.js("return document.querySelector('[data-review=\"acp-lab\"]').innerText")
    run.check("review names each agent and its program", "Agent “Fake ACP”" in review and "Agent “Fake ACP sign-in”" in review and "python" in review, review[:300])
    # Chat mode starts the agent with bin + acp arguments; the review lists bin + launch only (plugin-settings.tsx).
    run.check("review shows the acp arguments chat mode runs (not only bin and launch)", "fake_acp_agent.py" in review and "--state" in review and "--auth" in review, review[:300])
    d.js("[...document.querySelectorAll('[data-review=\"acp-lab\"] button')].find(b => b.innerText.trim() === 'Enable plugin').click();")
    run.check("plugin enabled", until(lambda: "Enabled" in (d.js("return document.querySelector('[data-plugin=\"acp-lab\"]')?.innerText") or ""), 10))
    d.js(f"{S}.setOverlay(null)")
    until(lambda: any(p["id"] == KIND_AUTH for p in c.call("agent.presets")), 15)
    presets = {p["id"]: p for p in c.call("agent.presets")}
    run.check("plugin agent is an ACP agent (acp args, installed)", presets.get(KIND, {}).get("installed") is True and bool(presets.get(KIND, {}).get("acp")), str(presets.get(KIND)))
    run.check("sign-in agent is an ACP agent too", presets.get(KIND_AUTH, {}).get("acp", [])[:1] == [FAKE], str(presets.get(KIND_AUTH)))

    # ---------- 1. offered in the new-session menu with a chat view; opens in chat view ----------
    d.js(f"{S}.setOverlay('newsession')")
    d.wait("return !!document.querySelector('.launch-card')", 10)
    labels = d.js("return [...document.querySelectorAll('.launch-card')].map(c => c.lastElementChild.textContent.trim());")
    run.check("the new-session menu lists the plugin agents", "Fake ACP" in labels and "Fake ACP sign-in" in labels, str(labels))
    d.js("[...document.querySelectorAll('.launch-card')].find(x => x.lastElementChild.textContent.trim() === 'Fake ACP').click();")
    d.wait("return [...document.querySelectorAll('.modal .toggle')].some(b => b.innerText.trim() === 'Chat view')", 5)
    chat_offered = d.js("return [...document.querySelectorAll('.modal .toggle')].map(b => b.innerText.trim())")
    run.check("chat view is offered for the agent (Interface row)", "Chat view" in chat_offered and "Terminal (TUI)" in chat_offered, str(chat_offered))
    run.shot("01-new-session-chat-view")
    d.js("[...document.querySelectorAll('.modal .toggle')].find(b => b.innerText.trim() === 'Chat view').click();")
    d.js("[...document.querySelectorAll('.modal .footer .btn.primary')].find(b => b.innerText.trim() === 'Start').click();")
    s = until(lambda: next((x for x in sessions_of(KIND)), None), 30)
    run.check("the session is a chat session", bool(s) and s.get("chat") is True, str(s and {k: s.get(k) for k in ("kind", "chat", "status")}))
    sid = s["id"]
    ready = until(lambda: ctl(sid, "c.state.ready && c.state.settings.length > 0") or None, 60)
    run.check("the chat view opens and the session initialises", bool(ready) and d.js("return !!document.querySelector('.chat .composer-box textarea')"), str(ctl(sid, "c.state.settings.length")))
    run.check("the session is idle when it opens", until(lambda: status(sid) == "idle", 15), str(status(sid)))
    run.shot("02-chat-open")

    # ---------- 2. a full tool-using turn ----------
    TL.clear()
    send("Read notes.txt and report back")
    run.check("the tool card 'Read notes.txt' appears", track(sid, lambda: d.js("return [...document.querySelectorAll('.trow.tool')].some(t => t.innerText.includes('notes.txt'))"), 30))
    run.check("the thought is kept as a thinking item", any(i["kind"] == "thinking" and "Thinking about it." in i["text"] for i in items(sid)), str([i["kind"] for i in items(sid)]))
    perm = track(sid, open_perm, 30)
    run.check("a permission card asks to allow the read, with Allow and Reject", bool(perm) and perm["title"] == "Allow: Read notes.txt?" and perm["buttons"] == ["1 Allow", "2 Reject"], str(perm))
    run.check("the session is waiting while the card is open", track(sid, lambda: status(sid) == "waiting", 10), str(TL))
    run.shot("03-permission-card")
    run.check("Allow is clicked", click_perm("Allow"))
    done = track(sid, lambda: any(i["kind"] == "assistant" and "Done: the file says hello." in i["text"] for i in items(sid)), 30)
    run.check("the tool completes and the answer is shown", done and any(i["kind"] == "tool" and i["status"] == "done" and "hello from notes" in i["output"] for i in items(sid)), str([(i["kind"], i["status"]) for i in items(sid)]))
    run.check("the answer is in the transcript", d.js("return [...document.querySelectorAll('.msg-reply')].some(e => e.innerText.includes('Done: the file says hello.'))"))
    run.check("the session goes idle after the stop reason", track(sid, lambda: status(sid) == "idle", 15), str(TL))
    run.check("it stays idle after the trailing usage update (5 s)", stays(sid, "idle", 5), str(status(sid)))
    i = TL.index("waiting") if "waiting" in TL else -1
    run.check("the status went working, waiting, working, idle", i > 0 and TL[i - 1] == "working" and "working" in TL[i + 1:] and TL[-1] == "idle", str(TL))
    title = info(sid).get("title") or ""
    run.check("the session title comes from session_info_update", title.startswith("Fake task: Read notes.txt"), title)
    run.check("the pane label shows the title", until(lambda: d.js("return document.body.innerText.includes('Fake task: Read notes.txt')"), 10))
    meter = until(lambda: d.js("const m = document.querySelector('.ctx-meter'); return m ? m.title : ''"), 10)
    run.check("usage shows in the composer: tokens used of the window", bool(meter) and "tokens" in meter, str(meter) + " " + str(ctl(sid, "c.state.usage")))
    run.check("the cost of the turn is still shown after the trailing usage update", bool(meter) and "Cost so far" in meter, str(meter))
    run.shot("04-turn-done")

    # ---------- 3. pickers from configOptions; changing the model is sent ----------
    model_btn = d.js("const b = document.querySelector('.setting-btn[title=\"Model\"]'); return !!b")
    run.check("model and mode pickers come from configOptions", model_btn and d.js("return !!document.querySelector('.setting-btn[title=\"Mode\"]')"), str(ctl(sid, "c.state.settings.map(s => s.id + '=' + s.value)")))
    d.js("document.querySelector('.setting-btn[title=\"Mode\"]').click();")
    modes = until(lambda: d.js("const o = [...document.querySelectorAll('.setting-opt')].map(x => x.innerText.trim()); return o.length ? o : null"), 5) or []
    run.check("the mode picker lists Ask and Auto", "Ask" in " ".join(modes) and "Auto" in " ".join(modes), str(modes))
    d.keys("escape")
    d.js("document.querySelector('.setting-btn[title=\"Model\"]').click();")
    until(lambda: d.js("return document.querySelectorAll('.setting-opt').length > 0"), 5)
    d.js("[...document.querySelectorAll('.setting-opt')].find(x => x.innerText.trim().startsWith('Deep')).click();")
    run.check("the picker shows the model the agent confirmed", until(lambda: ctl(sid, "c.state.settings.find(x => x.id === 'model')?.value") == "deep", 10), str(ctl(sid, "c.state.settings.find(x => x.id === 'model')?.value")))
    sent = [x for x in read_lines(LOG) if x.get("method") == "session/set_config_option"]
    run.check("session/set_config_option was sent with configId model and value deep", any(x["params"].get("configId") == "model" and x["params"].get("value") == "deep" for x in sent), str(sent[-1:] if sent else None))
    run.shot("05-pickers")

    # ---------- 4. a question (elicitation form) answered in its card ----------
    send("ask me my favourite colour")
    q = until(lambda: open_perm(), 30)
    run.check("the question card shows its title and the Colour field", bool(q) and q["title"] == "Which colour?" and q["asks"] == ["Colour"], str(q))
    run.check("the session is waiting while the question is open", track(sid, lambda: status(sid) == "waiting", 10))
    run.shot("06-question-card")
    d.js(f"const p = {OPEN_PERM}.pop(); const i = p.querySelector('.text-input'); i.focus(); i.value = 'teal'; i.dispatchEvent(new Event('input', {{ bubbles: true }}));")
    d.js(f"const p = {OPEN_PERM}.pop(); [...p.querySelectorAll('.actions .btn.primary')].find(b => b.innerText.trim() === 'Submit answers').click();")
    run.check("the agent replies with the answer", track(sid, lambda: any("You said: teal" in r for r in replies()), 30), str(replies()[-2:]))
    run.check("the answer reached the agent as the field's value", any(x.get("result", {}).get("content", {}).get("answer") == "teal" and x.get("result", {}).get("action") == "accept" for x in read_lines(LOG)))
    run.check("the session goes idle after the answer", track(sid, lambda: status(sid) == "idle", 15), str(TL[-3:]))

    # ---------- 5. the agent process is killed; Resume loads the conversation ----------
    before_user = [i["text"] for i in items(sid) if i["kind"] == "user"]
    c.call("session.kill", {"handle": info(sid)["handle"]})
    run.check("killing the process ends the session", until(lambda: not info(sid).get("alive"), 15))
    resume = until(lambda: d.js("return [...document.querySelectorAll('.chat-composer .btn')].some(b => b.innerText.trim() === 'Resume')"), 10)
    run.check("the composer offers Resume", bool(resume))
    d.js("[...document.querySelectorAll('.chat-composer .btn')].find(b => b.innerText.trim() === 'Resume').click();")
    # A chat resumes as a new session (new id) that takes the old one's pane; the old one is closed.
    new = until(lambda: next((x for x in sessions_of(KIND) if x["alive"] and x["id"] != sid), None), 30)
    run.check("the resumed chat is a live session of the agent", bool(new), str([(x["id"], x["alive"]) for x in sessions_of(KIND)]))
    if new:
        sid = new["id"]
    loaded = until(lambda: any(x.get("method") == "session/load" for x in read_lines(LOG)), 30)
    run.check("the agent is asked to load the conversation (session/load), not start a new one", bool(loaded), str([x.get("method") for x in read_lines(LOG)][-6:]))
    replayed = until(lambda: any(i["kind"] == "user" and "Read notes.txt and report back" in i["text"] for i in items(sid)) and any("You said: teal" in r for r in replies()), 30)
    run.check("the replay brings back the conversation", bool(replayed), str([(i["kind"], i["text"][:40]) for i in items(sid)][:8]))
    run.check("the replayed user prompts match the original ones", [i["text"] for i in items(sid) if i["kind"] == "user"] == before_user, str([i["text"] for i in items(sid) if i["kind"] == "user"]))
    TL.clear()
    run.check("after the replay the session is idle, not working", track(sid, lambda: status(sid) == "idle", 15), str(TL))
    run.check("it stays idle after the replay (5 s)", stays(sid, "idle", 5), str(status(sid)))
    run.shot("07-resumed")
    send("Read notes.txt once more")
    run.check("a new prompt after resume asks permission again", until(lambda: (open_perm() or {}).get("title") == "Allow: Read notes.txt?", 30))
    run.check("…allowed, and answered", click_perm("Allow") and until(lambda: d.js("return [...document.querySelectorAll('.msg-reply')].filter(e => e.innerText.includes('Done: the file says hello.')).length") >= 2, 30))
    run.check("the session is idle again after that turn", track(sid, lambda: status(sid) == "idle", 15))

    # ---------- 6. sign-in: the agent needs it before a session opens ----------
    s2 = start_chat("Fake ACP sign-in", KIND_AUTH)
    run.check("the sign-in agent opens in chat view", bool(s2))
    if s2:
        card = until(lambda: open_perm(), 30)
        run.check("the chat asks to sign in", bool(card) and card["title"] == "Sign in to continue", str(card))
        run.check("the sign-in card offers the agent's method", bool(card) and any("Fake login" in b for b in card["buttons"]), str(card and card["buttons"]))
        run.shot("08-sign-in")
        run.check("choosing Fake login runs authenticate", click_perm("Fake login"))
        opened = until(lambda: ctl(s2["id"], "c.state.settings.length > 0") or None, 30)
        run.check("the session opens after sign-in", bool(opened), str(ctl(s2["id"], "c.state.items.map(i => i.kind)")))
        auth_sent = [x for x in read_lines(LOG_AUTH) if x.get("method") == "authenticate"]
        run.check("authenticate was sent with the method id", any(x["params"].get("methodId") == "fake-login" for x in auth_sent), str(auth_sent))
        send("Read notes.txt")
        run.check("a prompt after sign-in gets its permission card", until(lambda: (open_perm() or {}).get("title") == "Allow: Read notes.txt?", 30))
        click_perm("Allow")
        run.check("…and the answer", until(lambda: any(i["kind"] == "assistant" and "Done: the file says hello." in i["text"] for i in items(s2["id"])), 30))
        run.check("the signed-in session goes idle", track(s2["id"], lambda: status(s2["id"]) == "idle", 15))
        run.shot("09-signed-in")


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
        scenario(run, c)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False, traceback.format_exc().strip().splitlines()[-1])
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
        kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
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
