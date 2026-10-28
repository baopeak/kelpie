"""Agents must resume the same conversation after the session daemon stops (reboot, update, crash).

    python scripts/e2e/agent_resume.py <kelpied.exe> <project-dir> [agent ...]   # default: claude codex omp

Runs an isolated daemon (own endpoint and data folder), tells each agent a number,
kills the daemon hard, starts it again, resumes every interrupted session and asks for a value
only the original conversation can produce (secret x 3). Uses the agents' real sign-in.
"""
import importlib.util
import os
import random
import re
import subprocess
import sys
import tempfile
import time

exe, project = sys.argv[1], sys.argv[2]
agents = sys.argv[3:] or ["claude", "codex", "omp"]
os.environ["KELPIE_ENDPOINT"] = f"kelpie-resume-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix="kelpie-resume-")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kelpie_call = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kelpie_call)

results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail and not ok else ""), flush=True)


def start_daemon():
    proc = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        try:
            return proc, kelpie_call.Conn()
        except OSError:
            time.sleep(0.1)
    raise SystemExit("daemon did not start")


def find(c, sid):
    return next((s for s in c.call("session.list") if s["id"] == sid), None)


def wait_idle(c, sid, timeout=240, min_wait=4):
    """Idle (or waiting) after the agent worked, with a drawn screen that stopped changing. A
    blank screen is never "ready": a resumed agent can take many seconds to draw (8 s for Claude
    Code on an idle machine, longer on a busy one), and text typed before that is lost."""
    end, last, stable = time.time() + timeout, None, 0
    time.sleep(min_wait)
    while time.time() < end:
        s = find(c, sid)
        screen = c.call("session.read", {"handle": s["handle"]})["text"]
        stable = stable + 1 if screen == last else 0
        last = screen
        drawn = sum(1 for line in screen.splitlines() if line.strip()) >= 3
        if s["status"] in ("idle", "waiting") and drawn and stable >= 6:
            return s
        time.sleep(0.5)
    return find(c, sid)


def screen(c, sid, lines=60):
    return c.call("session.read", {"handle": find(c, sid)["handle"], "lines": lines})["text"]


def wait_for(fn, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.5)
    return None


def accept_trust(c, sid):
    """A folder the agent has not seen asks to be trusted first (answer: "Yes, I trust")."""
    if wait_for(lambda: re.search(r"trust (this|the contents)", screen(c, sid), re.I) or find(c, sid).get("agent_session_id"), 40) and not find(c, sid).get("agent_session_id"):
        handle = find(c, sid)["handle"]
        text = screen(c, sid)
        # Claude puts "No, exit" first; Codex already highlights "Yes".
        if "No, exit" in text:
            c.call("session.input", {"handle": handle, "data": "\x1b[B"})
            time.sleep(0.4)
        c.call("session.input", {"handle": handle, "data": "\r"})


def type_line(c, handle, text):
    c.call("session.input", {"handle": handle, "data": f"\x1b[200~{text}\x1b[201~"})
    time.sleep(0.6)
    c.call("session.input", {"handle": handle, "data": "\r"})


def submit(c, sid, text, answer, tries=3):
    """Types `text` and makes sure the agent took it: it starts working or answers. An agent
    still starting up can keep the text but drop the Enter (Enter again), or drop it all (type
    again). Returns how many attempts it took, 0 if none worked."""
    for attempt in range(1, tries + 1):
        if attempt == 1 or text[:30] not in "\n".join(screen(c, sid).splitlines()[-8:]):
            type_line(c, find(c, sid)["handle"], text)
        else:
            c.call("session.input", {"handle": find(c, sid)["handle"], "data": "\r"})
        took = wait_for(lambda: find(c, sid)["status"] == "working" or answer in screen(c, sid, 400).split(text[:30])[-1], 20)
        if took:
            return attempt
    return 0


daemon, c = start_daemon()
secrets, sids = {}, {}
try:
    for kind in agents:
        secret = random.randint(1000, 9999)
        secrets[kind] = secret
        info = c.call("session.create", {
            "kind": kind, "cwd": project, "cols": 120, "rows": 36,
            "prompt": f"Remember this number for later: {secret}. Do not use any tools. Reply with only the word OK.",
        })
        sids[kind] = info["id"]
    for kind in agents:
        accept_trust(c, sids[kind])
    for kind in agents:
        s = wait_idle(c, sids[kind])
        answered = wait_for(lambda: re.search(r"^\W*OK\W*$", screen(c, sids[kind]).split("Reply with only the word OK.")[-1], re.M), 120)
        check(f"{kind}: first turn answered", s["alive"] and answered, screen(c, sids[kind])[-800:])
    # Claude and Codex hooks report the conversation id; others resume the folder's latest one.
    for kind in agents:
        if kind in ("claude", "codex"):
            check(f"{kind}: hook reported the conversation id", wait_for(lambda: find(c, sids[kind]).get("agent_session_id"), 30))
    time.sleep(2)
    before = {k: find(c, sids[k]).get("agent_session_id") for k in agents}
    for kind in agents:
        print(f"  {kind} conversation id before: {before[kind]}")
finally:
    subprocess.run(["taskkill", "/PID", str(daemon.pid), "/F", "/T"], capture_output=True)
    daemon.wait()

time.sleep(2)
daemon, c = start_daemon()
try:
    for kind in agents:
        s = find(c, sids[kind])
        check(f"{kind}: restored as interrupted", s is not None and s["interrupted"] and not s["alive"])
        check(f"{kind}: kept its conversation id", s is not None and s.get("agent_session_id") == before[kind], f"{s and s.get('agent_session_id')} != {before[kind]}")
        c.call("session.restart", {"handle": s["handle"]})
    # Not a "secret": GPT models refuse to repeat one ("I can't provide that").
    question = "Multiply the number I gave you earlier by 3. Do not use any tools. Reply with only the result."
    for kind in agents:
        s = wait_idle(c, sids[kind], min_wait=8)
        check(f"{kind}: resumed and running", s["alive"], screen(c, sids[kind])[-800:])
        attempts = submit(c, sids[kind], question, str(secrets[kind] * 3))
        check(f"{kind}: took the question", attempts > 0, screen(c, sids[kind])[-800:])
        if attempts > 1:
            print(f"  {kind} needed {attempts} attempts to take the question")
    for kind in agents:
        want = str(secrets[kind] * 3)
        # The screen can sit still while the agent works (Codex runs the user's hooks silently).
        wait_for(lambda: want in screen(c, sids[kind], 400).split("Multiply the number")[-1], 180)
        s = wait_idle(c, sids[kind])
        text = c.call("session.read", {"handle": s["handle"], "lines": 400})["text"]
        check(f"{kind}: remembers the conversation ({secrets[kind]} x 3 = {want})", want in text.split("Multiply the number")[-1], text[-600:])
    for kind in agents:
        s = find(c, sids[kind])
        c.call("session.kill", {"handle": s["handle"]})
finally:
    try:
        c.call("daemon.shutdown")
    except BaseException:
        pass
    time.sleep(1)
    subprocess.run(["taskkill", "/PID", str(daemon.pid), "/F", "/T"], capture_output=True)

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
