"""Sessions waiting on a prompt shown on screen are recognised and can be answered safely.

    python scripts/e2e/attention.py <kelpied.exe>

Isolated daemon (own endpoint and data folder). Agents without hooks say they need the user only
on screen, so:
1. PowerShell `Read-Host "Continue (y/n)"` and a stand-in agent printing a numbered permission
   menu both turn `waiting`, with `waiting_since` and the prompt's keys in `session.read`.
2. `session.answer` with a stale `waiting_since` is refused; with the current one it types the
   answer, and the session stops waiting.
3. Ordinary output (a file that mentions "(y/n)", a question and a numbered list) does not count
   as waiting; a prompt whose screen keeps repainting stays one wait (no flicker).
4. Programs in a session may not answer prompts (`session.answer` is the user's).
"""
import importlib.util
import os
import subprocess
import sys
import tempfile
import time

exe = os.path.abspath(sys.argv[1])
os.environ["KELPIE_ENDPOINT"] = f"kelpie-attention-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = data = tempfile.mkdtemp(prefix="kelpie-attention-")
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail and not ok else ""), flush=True)


def wait_for(fn, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.3)
    return None


def info(c, sid):
    return next((s for s in c.call("session.list") if s["id"] == sid), {})


def ready_shell(c, cwd, name):
    s = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": name})
    wait_for(lambda: info(c, s["id"]).get("status") == "idle" and "PS " in c.call("session.read", {"handle": s["handle"]})["text"], 150)
    return s


proc = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
c = None
for _ in range(150):
    try:
        c = kc.Conn()
        break
    except OSError:
        time.sleep(0.1)
try:
    cwd = tempfile.mkdtemp(prefix="kelpie-attention-cwd-")
    menu = os.path.join(cwd, "fake_agent.py")
    open(menu, "w", encoding="utf-8").write(
        'print("Bash(npm test)")\nprint(" Do you want to proceed?")\nprint(" \\u276f 1. Yes")\n'
        'print("   2. Yes, and don\'t ask again")\nprint("   3. No (esc)")\nanswer = input()\nprint("ANSWERED=" + answer)\ninput()\n')
    notes = os.path.join(cwd, "notes.txt")
    open(notes, "w", encoding="utf-8").write("Ask the user (y/n) before deleting.\nWhich one do you want?\n1. Back up\n2. Delete\n")
    ticking = os.path.join(cwd, "ticking_agent.py")
    open(ticking, "w", encoding="utf-8").write(
        'import sys, time, threading\n'
        'print(" Do you want to proceed?")\nprint(" \\u276f 1. Yes")\nprint("   2. No")\nsys.stdout.flush()\n'
        'def tick():\n'
        '    i = 0\n'
        '    while True:\n'
        '        time.sleep(1); i += 1\n'
        '        sys.stdout.write("\\x1b7\\x1b[1;70H" + "|/-\\\\"[i % 4] + "\\x1b8"); sys.stdout.flush()\n'
        'threading.Thread(target=tick, daemon=True).start()\ninput()\n')

    # 1. a yes/no prompt
    s1 = ready_shell(c, cwd, "yes-no")
    c.call("session.input", {"handle": s1["handle"], "data": 'Read-Host "Continue (y/n)"\r'})
    waiting = wait_for(lambda: info(c, s1["id"]).get("status") == "waiting" and info(c, s1["id"]), 30)
    check("Read-Host (y/n) turns the session waiting", bool(waiting), info(c, s1["id"]).get("status"))
    since = (waiting or {}).get("waiting_since")
    check("…with waiting_since", isinstance(since, int) and since > 0, since)
    prompt = c.call("session.read", {"handle": s1["handle"]}).get("prompt") or {}
    check("session.read names the keys", prompt.get("keys") == ["y", "n"], prompt)

    # 2. stale vs current answer
    try:
        c.call("session.answer", {"handle": s1["handle"], "waiting_since": (since or 0) - 1, "data": "n", "enter": True})
        stale_refused = False
    except SystemExit:
        stale_refused = True
    check("an answer to an older prompt is refused", stale_refused)
    c.call("session.answer", {"handle": s1["handle"], "waiting_since": since, "data": "y", "enter": True})
    done = wait_for(lambda: info(c, s1["id"]).get("status") != "waiting", 20)
    check("the answer is typed and the session stops waiting", bool(done), info(c, s1["id"]).get("status"))
    check("…and the program got it", "y" in c.call("session.read", {"handle": s1["handle"], "lines": 6})["text"].split("Continue (y/n)")[-1])

    # 1b. a numbered permission menu
    s2 = ready_shell(c, cwd, "menu")
    c.call("session.input", {"handle": s2["handle"], "data": f'python "{menu}"\r'})
    waiting = wait_for(lambda: info(c, s2["id"]).get("status") == "waiting" and info(c, s2["id"]), 30)
    check("a numbered menu turns the session waiting", bool(waiting), info(c, s2["id"]).get("status"))
    prompt = c.call("session.read", {"handle": s2["handle"]}).get("prompt") or {}
    check("…keys 1-3 and the question", prompt.get("keys") == ["1", "2", "3"] and "proceed" in (prompt.get("question") or ""), prompt)
    c.call("session.answer", {"handle": s2["handle"], "waiting_since": waiting["waiting_since"], "data": "1", "enter": True})
    got = wait_for(lambda: "ANSWERED=1" in c.call("session.read", {"handle": s2["handle"], "lines": 10})["text"], 20)
    check("the menu answer reaches the program", bool(got))

    # 3. ordinary output is not a prompt
    s3 = ready_shell(c, cwd, "plain")
    c.call("session.input", {"handle": s3["handle"], "data": f'Get-Content "{notes}"\r'})
    time.sleep(4)
    check("a file mentioning (y/n) and a numbered list is not waiting", info(c, s3["id"]).get("status") != "waiting", info(c, s3["id"]).get("status"))

    # 4. a session's programs may not answer
    tok_file = os.path.join(cwd, "tok.txt")
    c.call("session.input", {"handle": s3["handle"], "data": f"$env:KELPIE_SESSION_TOKEN | Out-File -Encoding ascii '{tok_file}'\r"})
    tok = wait_for(lambda: os.path.exists(tok_file) and open(tok_file, encoding="ascii").read().strip(), 60)
    user_token = kc.user_token
    kc.user_token = lambda: None
    try:
        as3 = kc.Conn()
    finally:
        kc.user_token = user_token
    as3.call("auth", {"session_id": s3["id"], "token": tok})
    try:
        as3.call("session.answer", {"handle": s1["handle"], "waiting_since": 1, "data": "y"})
        refused = False
    except SystemExit as e:
        refused = "not available to programs" in str(e)
    check("programs in a session may not answer prompts", refused)

    # 3b. a waiting prompt whose screen keeps repainting stays one wait
    s4 = ready_shell(c, cwd, "repaint")
    c.call("session.input", {"handle": s4["handle"], "data": f'python "{ticking}"\r'})
    first = wait_for(lambda: info(c, s4["id"]).get("status") == "waiting" and info(c, s4["id"]), 30)
    samples = []
    for _ in range(8):
        time.sleep(1)
        samples.append((info(c, s4["id"]).get("status"), info(c, s4["id"]).get("waiting_since")))
    steady = bool(first) and all(st == "waiting" and ws == first["waiting_since"] for st, ws in samples)
    check("a repainting prompt stays one wait (no flicker)", steady, samples)

    # 3c. an agent that reports through hooks keeps saying `waiting` after a key at the keyboard:
    # that wait is answered, and nothing may answer it a second time
    s5 = ready_shell(c, cwd, "hook-wait")
    hook = lambda event: c.call("hook.report", {"session_id": s5["id"], "agent": "claude", "payload": {"hook_event_name": event}})  # noqa: E731
    hook("PermissionRequest")
    w5 = wait_for(lambda: info(c, s5["id"]).get("status") == "waiting" and info(c, s5["id"]), 10) or {}
    c.call("session.input", {"handle": s5["handle"], "data": "1"})
    check("a hook's wait still shows as waiting after a key at the keyboard", info(c, s5["id"]).get("status") == "waiting")
    try:
        c.call("session.answer", {"handle": s5["handle"], "waiting_since": w5.get("waiting_since"), "data": "1"})
        answered_twice = True
    except SystemExit:
        answered_twice = False
    check("…and that wait can no longer be answered", not answered_twice)
    # The agent works on and asks again at once: no tick sees it working in between.
    hook("PostToolUse")
    hook("PermissionRequest")
    w5b = wait_for(lambda: info(c, s5["id"]).get("waiting_since") not in (None, w5.get("waiting_since")) and info(c, s5["id"]), 10) or {}
    try:
        c.call("session.answer", {"handle": s5["handle"], "waiting_since": w5b.get("waiting_since"), "data": "1"})
        next_answered = True
    except SystemExit:
        next_answered = False
    check("the next wait can be answered", next_answered)
    c.call("session.close", {"handle": s5["handle"]})

    for s in (s1, s2, s3, s4):
        c.call("session.close", {"handle": s["handle"]})
finally:
    proc.kill()
    try:
        os.remove(os.path.join(os.environ["LOCALAPPDATA"], "Kelpie", "auth", os.environ["KELPIE_ENDPOINT"] + ".token"))
    except OSError:
        pass

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
