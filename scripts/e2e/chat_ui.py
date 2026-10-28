"""Chat view transcript and composer with real agents. Claude Code: tool rows, a message queued
while it works, approval cards (the edit's line diff; answered with key 1), the files a turn
changed, the context meter, dates on messages, and a question answered in its card. Codex: the
pinned plan and the context ring.

Claude runs on its small model ("haiku") in a throwaway git folder; a few short turns. (Claude
Code in `-p` mode has no task-list tool, so its plan panel is not exercised.)

Prereqs: as smoke.py (daemon on KELPIE_ENDPOINT, `pnpm dev`, tauri-driver, debug app).

    python scripts/e2e/chat_ui.py <screenshot-dir>
"""
import os
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run  # noqa: E402

shots = sys.argv[1]
run = Run(shots)
d = run.d

CTRL = "const s = window.__kelpie.store.sessions[arguments[0]]; const c = window.__kelpie.chat.getController(s);"


def state(sid, expr):
    return d.js(CTRL + f"return {expr};", sid)


def wait_state(sid, expr, timeout=120):
    end = time.time() + timeout
    while time.time() < end:
        if state(sid, expr):
            return True
        time.sleep(0.5)
    return False


def wait_dom(expr, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        v = d.js(f"return {expr};")
        if v:
            return v
        time.sleep(0.5)
    return None


def type_message(text):
    d.click(".composer-box textarea")
    d.js("const t = document.querySelector('.composer-box textarea'); t.value = arguments[0]; t.dispatchEvent(new Event('input', { bubbles: true }));", text)
    d.keys("enter")


# The long form: Codex's Windows sandbox cannot start commands in an 8.3 short-name folder.
folder = os.path.realpath(tempfile.mkdtemp(prefix="kelpie-chat-ui-"))
with open(os.path.join(folder, "notes.txt"), "w", newline="\n") as f:
    f.write("alpha\nbeta\ngamma\n")
for args in (["init", "-q"], ["add", "."], ["-c", "user.name=e2e", "-c", "user.email=e2e@example.com", "commit", "-qm", "init"]):
    subprocess.run(["git", *args], cwd=folder, check=True)

try:
    run.boot()
    d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    d.js("window.__kelpie.store.addProject(arguments[0]);", folder)
    pid = d.js("return window.__kelpie.store.state.activeProject")
    sid = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.chat.newChat(arguments[0], 'claude').then(i => done(i.id));", pid)
    run.check("chat starts", wait_state(sid, "c.state.ready && c.state.settings.length > 0", 120))
    state(sid, "c.setSetting('model', 'haiku')")
    state(sid, "c.setSetting('mode', 'default')")
    wait_state(sid, "c.state.settings.find(x => x.id === 'model')?.value === 'haiku'", 30)

    # A turn that reads and asks to edit; a second message queued while it works.
    type_message("Read notes.txt, then use the Edit tool to change the word beta to BETA. Keep your replies very short.")
    run.check("busy line with elapsed time", wait_dom("document.querySelector('.chat .typing .elapsed')?.textContent", 30))
    type_message("When you are done, reply with only the word finished.")
    run.check("a message sent while busy waits in the queue", wait_state(sid, "c.state.queue.length === 1", 10) and wait_dom("document.querySelector('.queue-chip')", 5))
    run.shot("chat-ui-queue")
    # Approve what comes before the edit (a read outside the trusted folders) with key 1.
    OPEN = "[...document.querySelectorAll('.perm')].filter(p => p.querySelector('.actions .btn'))"
    diff = []
    end = time.time() + 240
    while time.time() < end and not diff:
        if d.js(f"return {OPEN}.length"):
            diff = d.js(f"const p = {OPEN}.pop(); return [...p.querySelectorAll('.perm-diff .diff-line')].map(e => e.textContent)")
            if not diff:
                d.click(".composer-box textarea")
                d.keys("1")
                time.sleep(1)
        time.sleep(0.5)
    run.check("an approval card shows the edit as a real line diff", "-beta" in diff and "+BETA" in diff and "-alpha" not in diff, str(diff))
    run.shot("chat-ui-approval")
    d.click(".composer-box textarea")
    d.keys("1")
    run.check("key 1 approves", wait_dom(f"!{OPEN}.length", 10))
    end = time.time() + 120
    while time.time() < end and "BETA" not in open(os.path.join(folder, "notes.txt")).read():
        time.sleep(1)
    run.check("notes.txt now says BETA", "BETA" in open(os.path.join(folder, "notes.txt")).read())
    run.check("the queued message went after the turn", wait_state(sid, "c.state.queue.length === 0 && !c.state.busy && c.state.items.some(i => i.kind === 'assistant' && /finished/i.test(i.text))", 180),
              str(state(sid, "c.state.items.slice(-3).map(i => i.kind + ': ' + (i.text || i.name || '').slice(0, 80))")))
    heads = d.js("return [...document.querySelectorAll('.chat .trow .tool-head .tname')].map(e => e.textContent)")
    run.check("tool rows read as verbs", any(h in ("Read", "Edit") or h.startswith("Explored") for h in heads), str(heads))
    files = wait_dom("[...document.querySelectorAll('.turn-files .turn-file')].map(e => e.innerText).join('|')", 10)
    run.check("the turn lists the file it changed", bool(files) and "notes.txt" in files and "+1" in files and "−1" in files, str(files))
    run.check("the context meter shows how full the context is", (wait_dom("document.querySelector('.ctx-meter')?.textContent", 10) or "").endswith("%"), str(state(sid, "c.state.usage")))
    stamps = d.js("return [...document.querySelectorAll('.msg-user-wrap .msg-time')].length + ':' + [...document.querySelectorAll('.msg-reply .msg-time')].length")
    users, replies = (int(x) for x in stamps.split(":"))
    run.check("prompts and replies show their date and time", users >= 2 and replies >= 1, stamps)
    run.shot("chat-ui-turn")
    # Reattaching (a reload) replays the log: the dates come from it, not from now.
    before = state(sid, "c.state.items.filter(i => i.kind === 'user').map(i => i.at)")
    d.js_async("const done = arguments[arguments.length-1]; const s = window.__kelpie.store.sessions[arguments[0]]; window.__kelpie.chat.getController(s).attach(s.handle).then(() => done(true));", sid)
    after = state(sid, "c.state.items.filter(i => i.kind === 'user').map(i => i.at)")
    run.check("a replayed transcript keeps the original times", bool(after) and all(a and abs(a - b) < 2000 for a, b in zip(after, before)), f"{before} -> {after}")

    # A question answered in its card.
    type_message("Use the AskUserQuestion tool to ask me which color I prefer, with the options Red and Blue. Then reply with only my answer.")
    run.check("the question shows as a card with its options", wait_dom("[...document.querySelectorAll('.perm .ask-opt')].map(e => e.innerText).join('|')", 120))
    run.shot("chat-ui-question")
    d.js("[...document.querySelectorAll('.perm .ask-opt')].find(e => /Blue/.test(e.innerText)).click()")
    d.js("[...document.querySelectorAll('.perm .actions .btn.primary')].pop().click()")
    run.check("the agent got the answer", wait_state(sid, "!c.state.busy && c.state.items.some(i => i.kind === 'assistant' && /blue/i.test(i.text))", 120),
              str(state(sid, "c.state.items.slice(-2).map(i => i.kind + ': ' + (i.text || '').slice(0, 80))")))
    run.shot("chat-ui-answered")
    d.js_async("const done = arguments[arguments.length-1]; const s = window.__kelpie.store.sessions[arguments[0]]; window.__kelpie.ipc.daemon('session.close', { handle: s.handle }).then(done, done);", sid)

    # Codex: its plan updates pin the plan; its token usage fills the context ring.
    sid = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.chat.newChat(arguments[0], 'codex').then(i => done(i.id));", pid)
    run.check("codex: chat starts", wait_state(sid, "c.state.ready && c.state.settings.length > 0", 120))
    model = state(sid, "c.state.settings.find(x => x.id === 'model')?.options[0]?.value")
    state(sid, f"c.setSetting('model', {model!r})")
    type_message("Use your plan tool to set a two-step plan (1. look at notes.txt 2. answer), mark both steps done, then reply with only the word gamma. Do not edit anything.")
    run.check("codex: the plan is pinned above the message field", wait_dom("document.querySelector('.plan .plan-count')?.textContent", 180), str(state(sid, "c.state.plan")))
    run.check("codex: the turn ends", wait_state(sid, "!c.state.busy && c.state.items.some(i => i.kind === 'assistant' && /gamma/i.test(i.text))", 180))
    run.check("codex: the context ring shows a percentage", (wait_dom("document.querySelector('.ctx-meter')?.textContent", 10) or "").endswith("%"), str(state(sid, "c.state.usage")))
    run.shot("chat-ui-codex")
    d.js_async("const done = arguments[arguments.length-1]; const s = window.__kelpie.store.sessions[arguments[0]]; window.__kelpie.ipc.daemon('session.close', { handle: s.handle }).then(done, done);", sid)
except Exception as e:  # noqa: BLE001 — finish() exits; report the failure as a check
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    run.finish()
