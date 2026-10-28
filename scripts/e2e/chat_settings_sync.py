"""Switching between an agent's terminal UI and chat view keeps its settings: the permission
mode, model and effort the conversation ran with in one front-end are the ones the other starts
with. Claude Code (bypass + haiku → chat; accept-edits + low effort → back to the TUI) and Codex
(full access → chat; auto → back to the TUI).

Prereqs: as smoke.py (daemon on KELPIE_ENDPOINT, `pnpm dev`, tauri-driver, debug app).

    python scripts/e2e/chat_settings_sync.py <screenshot-dir>

KELPIE_E2E_TRUSTED_DIR: a folder the agents already trust (default: this repository).
KELPIE_E2E_CODEX_MODEL: a Codex model this account can use, when the configured one is not.
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run  # noqa: E402

run = Run(sys.argv[1])
d = run.d
folder = os.environ.get("KELPIE_E2E_TRUSTED_DIR", REPO)
codex_model = os.environ.get("KELPIE_E2E_CODEX_MODEL")

FOCUSED = "const s = window.__kelpie.store; const t = s.activeTab(s.state.activeProject); const p = (function f(n){return n.type==='leaf'?[n.pane]:[...f(n.a),...f(n.b)]})(t.root).find(p => p.id === t.focus); const sess = s.sessions[p.sessionId];"


def wait(cond, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(0.5)
    return None


def focused():
    return d.js(FOCUSED + "return sess ?? null;")


def switch(fn):
    return d.js_async("const done = arguments[arguments.length-1]; const s = window.__kelpie.store; const p = s.activeProject(); const t = s.activeTab(p.id);"
                      f"window.__kelpie.chat.{fn}(p, t, (function f(n){{return n.type==='leaf'?[n.pane]:[...f(n.a),...f(n.b)]}})(t.root).find(x => x.id === t.focus)).then(() => done(true), e => done(String(e)));")


def setting(id_):
    return d.js(FOCUSED + "const c = window.__kelpie.chat.getController(sess); return c.state.settings.find(x => x.id === arguments[0])?.value ?? null;", id_)


def pick(id_, value):
    d.js(FOCUSED + "window.__kelpie.chat.getController(sess).setSetting(arguments[0], arguments[1]);", id_, value)


def screen():
    return d.js_async("const done = arguments[arguments.length-1];" + FOCUSED + "window.__kelpie.ipc.daemon('session.read', { handle: sess.handle }).then(r => done(r.text), () => done(''));")


def answered_ok():
    s = focused()
    return s and s.get("agent_session_id") and s["status"] == "idle" and "OK" in screen().split("Reply with only OK.")[-1]


try:
    run.boot()
    d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    d.js("window.__kelpie.store.addProject(arguments[0]);", folder)
    pid = d.js("return window.__kelpie.store.state.activeProject")

    # ---------- Claude Code ----------
    d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'claude', { args: ['--model', 'haiku', '--dangerously-skip-permissions'], prompt: 'Reply with only OK.' }).then(() => done(true), e => done(String(e)));", pid)
    run.check("claude: TUI turn in bypass mode on haiku", wait(answered_ok, 180), screen()[-300:])
    switch("switchToChat")
    run.check("claude: chat view starts", wait(lambda: (focused() or {}).get("chat") and setting("mode"), 120))
    run.check("claude: chat keeps the bypass mode", wait(lambda: setting("mode") == "bypassPermissions", 30), str(setting("mode")))
    run.check("claude: chat keeps the model", "haiku" in str(setting("model")).lower(), str(setting("model")))
    run.shot("settings-claude-chat")
    pick("mode", "acceptEdits")
    run.check("claude: mode picked in chat", wait(lambda: setting("mode") == "acceptEdits", 20), str(setting("mode")))
    efforts = d.js(FOCUSED + "return window.__kelpie.chat.getController(sess).state.settings.find(x => x.id === 'effort')?.options.map(o => o.value) ?? [];")
    effort = "low" if "low" in efforts else (efforts[0] if efforts else None)
    if effort:
        pick("effort", effort)
        wait(lambda: setting("effort") == effort, 20)
    switch("switchToTerminal")
    tui = wait(lambda: (lambda s: s if s and not s.get("chat") and s["alive"] else None)(focused()), 60)
    args = (tui or {}).get("args", [])
    run.check("claude: the TUI starts with chat view's mode, model and effort",
              "--permission-mode" in args and args[args.index("--permission-mode") + 1] == "acceptEdits" and "--model" in args and (not effort or ("--effort" in args and args[args.index("--effort") + 1] == effort)), str(args))
    footer = wait(lambda: (lambda t: t if "accept edits on" in t.lower() else None)(screen()), 90) or screen()
    run.check("claude: the TUI shows accept-edits mode", "accept edits on" in footer.lower(), footer.strip()[-300:])
    run.shot("settings-claude-tui")
    d.js_async("const done = arguments[arguments.length-1];" + FOCUSED + "window.__kelpie.ipc.daemon('session.close', { handle: sess.handle }).then(done, done);")

    # ---------- Codex ----------
    codex_args = ["--dangerously-bypass-approvals-and-sandbox"] + (["-m", codex_model] if codex_model else [])
    d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'codex', { args: arguments[1], prompt: 'Reply with only OK.' }).then(() => done(true), e => done(String(e)));", pid, codex_args)
    run.check("codex: TUI turn with full access", wait(answered_ok, 240), screen()[-300:])
    switch("switchToChat")
    run.check("codex: chat keeps full access", wait(lambda: (focused() or {}).get("chat") and setting("mode") == "full-access", 120), str(setting("mode")))
    if codex_model:
        run.check("codex: chat keeps the model", setting("model") == codex_model, str(setting("model")))
    pick("mode", "auto")
    switch("switchToTerminal")
    tui = wait(lambda: (lambda s: s if s and not s.get("chat") and s["alive"] else None)(focused()), 60)
    args = (tui or {}).get("args", [])
    run.check("codex: the TUI starts with chat view's preset", "-s" in args and args[args.index("-s") + 1] == "workspace-write" and "--dangerously-bypass-approvals-and-sandbox" not in args, str(args))
    if codex_model:
        run.check("codex: the TUI keeps the model", "-m" in args and args[args.index("-m") + 1] == codex_model, str(args))
    run.check("codex: the TUI runs", wait(lambda: "Codex" in screen(), 60), screen()[-300:])
    run.shot("settings-codex-tui")
    d.js_async("const done = arguments[arguments.length-1];" + FOCUSED + "window.__kelpie.ipc.daemon('session.close', { handle: sess.handle }).then(done, done);")
except Exception as e:  # noqa: BLE001 — finish() exits; report the failure as a check
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    run.finish()
