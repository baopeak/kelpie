"""Chat view parity with the agents' terminal UIs: their own command lists in the composer (`/`,
Codex `$` skills, `@` files), the model / effort / mode pickers they expose, commands chat view
runs itself, and the hand-off to the terminal UI for the ones it cannot run.

Only Claude's `/context` (answered locally, no model call) is sent to an agent.

Prereqs: as smoke.py (daemon on KELPIE_ENDPOINT, `pnpm dev`, tauri-driver, debug app).

    python scripts/e2e/chat_parity.py <screenshot-dir> [claude|codex|omp ...]

KELPIE_E2E_TRUSTED_DIR: a folder Claude Code already trusts (default: this repository).
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run  # noqa: E402

shots = sys.argv[1]
agents = sys.argv[2:] or ["claude", "codex", "omp"]
folder = os.environ.get("KELPIE_E2E_TRUSTED_DIR", REPO)
run = Run(shots)
d = run.d

CTRL = "const s = window.__kelpie.store.sessions[arguments[0]]; const c = window.__kelpie.chat.getController(s);"


def state(sid, expr):
    return d.js(CTRL + f"return {expr};", sid)


def wait_state(sid, expr, timeout=90):
    end = time.time() + timeout
    while time.time() < end:
        if state(sid, expr):
            return True
        time.sleep(0.5)
    return False


def setting(sid, sid_setting):
    return state(sid, f"c.state.settings.find(x => x.id === {sid_setting!r})?.value ?? null")


def clear_composer():
    d.js("const t = document.querySelector('.composer-box textarea'); t.value = ''; t.dispatchEvent(new Event('input', { bubbles: true }));")


def menu():
    return d.js("return [...document.querySelectorAll('.composer-menu .composer-item .name')].map(e => e.textContent)")


def type_and_menu(text):
    clear_composer()
    d.click(".composer-box textarea")
    d.keys(text)
    end = time.time() + 5
    while time.time() < end and not menu():
        time.sleep(0.2)
    return menu()


def system_items():
    return d.js("return [...document.querySelectorAll('.chat .result-line')].map(e => e.innerText)")


try:
    run.boot()
    d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    d.js("window.__kelpie.store.addProject(arguments[0]);", folder)
    pid = d.js("return window.__kelpie.store.state.activeProject")
    for kind in agents:
        info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.chat.newChat(arguments[0], arguments[1]).then(done);", pid, kind)
        sid = info["id"]
        ok = wait_state(sid, "c.state.ready && c.state.commands.length > 0 && c.state.settings.length > 0", 120)
        names = state(sid, "c.state.commands.map(x => x.name)") or []
        settings = state(sid, "c.state.settings.map(x => x.id + '=' + x.value + ' (' + x.options.length + ')')") or []
        run.check(f"{kind}: commands and settings arrive", ok, f"{len(names)} commands; {', '.join(settings)}")
        run.check(f"{kind}: /compact is offered", "compact" in names)
        if kind == "claude":
            run.check("claude: skills and custom commands are listed", len(names) > 20 and "context" in names, f"{len(names)}")
            run.check("claude: model, effort and mode pickers", {"model", "effort", "mode"} <= {s.split("=")[0] for s in settings})
        if kind == "codex":
            run.check("codex: review and new are offered", {"review", "new"} <= set(names))
            run.check("codex: model, effort and mode pickers", {"model", "effort", "mode"} <= {s.split("=")[0] for s in settings})
        if kind == "omp":
            run.check("omp: the agent's own config options", {"mode", "model"} <= {s.split("=")[0] for s in settings})

        # `/` completes at the start of a message; Tab accepts.
        items = type_and_menu("/comp")
        run.check(f"{kind}: typing /comp suggests /compact first", bool(items) and items[0] == "/compact", str(items[:3]))
        d.keys("tab")
        value = d.js("return document.querySelector('.composer-box textarea').value")
        run.check(f"{kind}: Tab inserts the command", value == "/compact ", repr(value))
        # `@` lists files of the folder.
        items = type_and_menu("hello @READ")
        run.check(f"{kind}: @ suggests files", any("README" in i for i in items), str(items[:3]))
        run.shot(f"parity-{kind}-files")
        d.keys("escape")
        if kind == "codex" and state(sid, "c.state.skills.length"):
            items = type_and_menu("$")
            run.check("codex: $ suggests skills", bool(items) and all(i.startswith("$") for i in items), str(items[:3]))
        clear_composer()

        # Pickers change the agent's setting.
        if kind == "claude":
            state(sid, "c.setSetting('mode', 'plan')")
            run.check("claude: mode picker -> plan", wait_state(sid, "c.state.settings.find(x => x.id === 'mode')?.value === 'plan'", 20), str(setting(sid, "mode")))
            state(sid, "c.setSetting('model', 'haiku')")
            run.check("claude: model picker -> haiku", wait_state(sid, "c.state.settings.find(x => x.id === 'model')?.value === 'haiku'", 20), str(setting(sid, "model")))
            effort = state(sid, "c.state.settings.find(x => x.id === 'effort')?.options[0]?.value")
            state(sid, f"c.setSetting('effort', {effort!r})")
            run.check("claude: effort picker", wait_state(sid, f"c.state.settings.find(x => x.id === 'effort')?.value === {effort!r}", 20), str(setting(sid, "effort")))
            state(sid, "c.setSetting('mode', 'default')")
            # Built-in command answered by the CLI itself.
            state(sid, "c.send('/context')")
            run.check("claude: /context runs in chat view", wait_state(sid, "c.state.items.some(i => i.kind === 'assistant' && /Context Usage/.test(i.text))", 60))
            # Terminal-only command (`/color` opens a picker; `/doctor` may be shadowed by a
            # skill of the same name that does real work): offered in the terminal UI instead.
            state(sid, "c.send('/color')")
            time.sleep(1)
            run.check("claude: /color offers the terminal UI", any("terminal UI" in t and "Run it in the terminal" in t for t in system_items()), str(system_items()[-2:]))
            # …and the button continues in the TUI, where the command runs.
            d.js("[...document.querySelectorAll('.chat .result-line button')].pop().click()")
            tui = None
            end = time.time() + 60
            while time.time() < end and not tui:
                tui = d.js("const s = window.__kelpie.store; const t = s.activeTab(s.state.activeProject); const f = (n) => n.type === 'leaf' ? [n.pane] : [...f(n.a), ...f(n.b)]; const p = f(t.root).find(x => x.id === t.focus); return p.kind === 'terminal' ? s.sessions[p.sessionId] : null;")
                time.sleep(0.5)
            screen = ""
            end = time.time() + 90
            # Claude Code 2.1 answers `/color` with "Session color set to: <colour>".
            ran = lambda text: "session color set to" in text.lower()
            while tui and time.time() < end and not ran(screen):
                screen = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.ipc.daemon('session.read', { handle: arguments[0] }).then(r => done(r.text), e => done(''));", tui["handle"])
                time.sleep(1)
            run.check("claude: Run it in the terminal switches to the TUI and runs /color", bool(tui) and ran(screen), screen.strip()[-400:])
            run.shot("parity-claude-color-tui")
            if tui:
                sid = tui["id"]
        if kind == "codex":
            state(sid, "c.setSetting('mode', 'read-only')")
            run.check("codex: mode picker -> read-only", setting(sid, "mode") == "read-only")
            # A real turn with the picked model rides on turn/start (one small model call).
            model = state(sid, "c.state.settings.find(x => x.id === 'model')?.options[0]?.value")
            state(sid, f"c.setSetting('model', {model!r})")
            state(sid, "c.send('Reply with exactly the single word: gamma')")
            run.check(f"codex: a turn on the picked model ({model})", wait_state(sid, "!c.state.busy && c.state.items.some(i => i.kind === 'assistant' && /gamma/i.test(i.text))", 180), str(state(sid, "c.state.items.slice(-2).map(i => i.kind + ': ' + (i.text || '').slice(0, 120))")))
            state(sid, "c.send('/status')")
            time.sleep(1)
            run.check("codex: unmapped command offers the terminal UI", any("terminal UI" in t for t in system_items()), str(system_items()[-2:]))
        if kind == "omp":
            opt = state(sid, "c.state.settings.find(x => x.id === 'mode')?.options.find(o => o.value !== c.state.settings.find(x => x.id === 'mode').value)?.value")
            state(sid, f"c.setSetting('mode', {opt!r})")
            run.check(f"omp: mode picker -> {opt}", wait_state(sid, f"c.state.settings.find(x => x.id === 'mode')?.value === {opt!r}", 30), str(setting(sid, "mode")))
        run.shot(f"parity-{kind}")
        d.js_async("const done = arguments[arguments.length-1]; const s = window.__kelpie.store.sessions[arguments[0]]; window.__kelpie.ipc.daemon('session.close', { handle: s.handle }).then(done, done);", sid)
except Exception as e:  # noqa: BLE001 — finish() exits; report the failure as a check
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    run.finish()
