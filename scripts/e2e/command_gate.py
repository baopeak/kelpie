"""The command gate, end to end through the hook Claude Code runs (`kelpied hook claude-gate`), on an isolated daemon.

    python scripts/e2e/command_gate.py <kelpied.exe>

Two PowerShell sessions stand for two agents: B in a critical project, A in a background one. Each pipes the JSON Claude
writes for a `Bash` tool call into the hook, inside its own session (so the hook proves it is that session, as Claude's
does), and keeps what the hook prints. It checks:
- a light command gets no output (it runs as written, and nothing is ever approved);
- B's heavy build runs as written and is kept as running;
- A's build, on a computer shared with B's by priority, is rewritten to fewer jobs (the command keeps its first words,
  so a person's allow rule for it still matches), with a note for the agent and every other field kept;
- once B reports its build ended, A's next build runs as written.
Never touches the user's daemon.
"""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time

exe = os.path.abspath(sys.argv[1])
os.environ["KELPIE_ENDPOINT"] = f"kelpie-gate-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix="kelpie-gate-")
os.environ["KELPIE_E2E"] = "1"
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN"):
    os.environ.pop(k, None)

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)


def wait_for(fn, timeout=20, every=0.3):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(every)
    return None


daemon = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    c = None
    for _ in range(100):
        try:
            c = kc.Conn()
            break
        except OSError:
            time.sleep(0.1)
    check("daemon starts on its own endpoint", c is not None)
    c.call("policy.resources.set", {"projects": {"crit": {"tier": "critical"}, "bg": {"tier": "background"}}})
    tmp = tempfile.mkdtemp(prefix="kelpie-gate-cwd-")

    def session(project, name):
        s = c.call("session.create", {"kind": "shell", "cwd": tmp, "name": name, "project_id": project})
        screen = lambda: c.call("session.read", {"handle": s["handle"], "lines": 40})["text"]
        status = lambda: next((x["status"] for x in c.call("session.list") if x["id"] == s["id"]), None)
        wait_for(lambda: "PS " in screen() and status() == "idle", 150)
        return s

    def ask(s, call, command, tag):
        """What the hook prints for this Bash call, run inside session `s`."""
        payload = json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_use_id": call, "permission_mode": "default",
                              "tool_input": {"command": command, "description": "build it", "timeout": 600000}})
        pfile, out = os.path.join(tmp, f"{tag}.json"), os.path.join(tmp, f"{tag}.out")
        with open(pfile, "w", encoding="utf-8") as f:
            f.write(payload)
        c.call("session.input", {"handle": s["handle"], "data": f"Get-Content -Raw '{pfile}' | & '{exe}' hook claude-gate | Out-File -Encoding utf8 '{out}'; 'done-{tag}' | Out-File '{out}.done'\r"})
        wait_for(lambda: os.path.exists(out + ".done"), 130)
        text = open(out, encoding="utf-8-sig").read().strip() if os.path.exists(out) else ""
        return json.loads(text) if text else None

    b = session("crit", "agent-b")
    a = session("bg", "agent-a")
    check("a light command: nothing printed, it runs as written", ask(a, "t0", "git status", "light") is None)
    first = ask(b, "b1", "cargo build", "b1")
    check("the critical session's build runs as written", first is None or "updatedInput" not in first.get("hookSpecificOutput", {}), first)
    out = ask(a, "a1", "cargo build", "a1")
    hs = (out or {}).get("hookSpecificOutput", {})
    new = hs.get("updatedInput", {})
    print("  A was told:", json.dumps(hs)[:400], flush=True)
    check("the background session's build is given fewer jobs", new.get("command", "") == "cargo build -j 1", new.get("command"))
    check("…with every other field of the call kept", new.get("description") == "build it" and new.get("timeout") == 600000, new)
    check("…with a note for the agent", "this session's share of the computer" in hs.get("additionalContext", ""), hs.get("additionalContext"))
    check("…and never an approval", "permissionDecision" not in hs, hs)
    # B's build ends (Claude's PostToolUse), and A's turn begins again: A's next build has the computer to itself.
    c.call("hook.report", {"session_id": b["id"], "agent": "claude", "payload": {"hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_use_id": "b1"}})
    c.call("hook.report", {"session_id": a["id"], "agent": "claude", "payload": {"hook_event_name": "UserPromptSubmit"}})
    again = ask(a, "a2", "cargo build", "a2")
    check("with B's build over, A's next build runs as written", again is None or "updatedInput" not in again.get("hookSpecificOutput", {}), again)
finally:
    try:
        kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
    except BaseException:
        pass
    try:
        daemon.wait(15)
    except subprocess.TimeoutExpired:
        daemon.kill()

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
