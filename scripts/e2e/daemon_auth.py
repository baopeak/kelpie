"""Who may call the session daemon, and what they may call (crates/kelpied/src/auth.rs).

    python scripts/e2e/daemon_auth.py <kelpied.exe>

Daemon only, isolated endpoint and data folder. A PowerShell session writes its own
KELPIE_SESSION_TOKEN to a file; the test then speaks as that session and checks:
anonymous and forged callers are refused, the scripting surface works, admin methods do not,
hook reports land on the caller's own session, raw input frames are ignored, and the
"agents can control other sessions" policy and restricted sessions narrow it to the own session.
"""
import importlib.util
import os
import struct
import subprocess
import sys
import tempfile
import time

exe = sys.argv[1]
os.environ["KELPIE_ENDPOINT"] = f"kelpie-auth-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix="kelpie-auth-")
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN"):
    os.environ.pop(k, None)

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

results = []


def check(name, ok, detail=""):
    results.append((name, ok))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail and not ok else ""), flush=True)


class Raw(kc.Conn):
    """A connection that authenticates as told (or not at all) and returns errors instead of exiting."""

    def __init__(self, auth=None):
        token = kc.user_token
        kc.user_token = lambda: None  # skip Conn's own user-token auth
        try:
            super().__init__()
        finally:
            kc.user_token = token
        self.auth_error = None
        if auth is not None:
            self.auth_error = self.try_call("auth", auth)[1]

    def try_call(self, method, params=None):
        try:
            return self.call(method, params), None
        except SystemExit as e:
            return None, str(e)

    def input_frame(self, handle, data):
        body = struct.pack("<I", handle) + data.encode()
        self.write(struct.pack("<I", len(body) + 1) + bytes([3]) + body)


def denied(conn, method, params=None):
    result, err = conn.try_call(method, params)
    return err is not None, err or result


def screen(c, handle, lines=40):
    return c.call("session.read", {"handle": handle, "lines": lines})["text"]


def wait_for(fn, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.3)
    return None


def session_token(c, s, tmp):
    """Has the session's shell write its own token to a file, and reads it."""
    path = os.path.join(tmp, f"{s['id']}.token")
    # A cold PowerShell on a busy machine takes a while: type only once its prompt is idle.
    status = lambda: next((x["status"] for x in c.call("session.list") if x["id"] == s["id"]), None)
    wait_for(lambda: "PS " in screen(c, s["handle"]) and status() == "idle", 150)
    c.call("session.input", {"handle": s["handle"], "data": f"$env:KELPIE_SESSION_TOKEN | Out-File -Encoding ascii '{path}'\r"})
    return wait_for(lambda: os.path.exists(path) and open(path, encoding="ascii").read().strip(), 120)


daemon = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    c = None
    for _ in range(100):
        try:
            c = kc.Conn()
            break
        except OSError:
            time.sleep(0.1)
    check("daemon starts and writes a user token for its endpoint", c is not None and kc.user_token() is not None)
    tmp = tempfile.mkdtemp(prefix="kelpie-auth-tok-")
    cwd = tempfile.mkdtemp(prefix="kelpie-auth-cwd-")

    anon = Raw()
    ok, detail = denied(anon, "session.list")
    check("anonymous caller is refused", ok and "not authenticated" in str(detail), detail)
    check("anonymous caller may still ping daemon.info", anon.try_call("daemon.info")[1] is None)
    bad = Raw({"token": "0" * 64})
    check("a wrong token is refused", bad.auth_error is not None, bad.auth_error)

    s1 = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": "auth-one"})
    s2 = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": "auth-two"})
    check("user token: session.create works", bool(s1.get("id")) and bool(s2.get("id")))
    tok1 = session_token(c, s1, tmp)
    check("a session gets KELPIE_SESSION_TOKEN in its environment", bool(tok1) and len(tok1) == 64, tok1)

    forged = Raw({"session_id": s2["id"], "token": tok1})
    check("session 1's token cannot speak for session 2", forged.auth_error is not None, forged.auth_error)

    as1 = Raw({"session_id": s1["id"], "token": tok1})
    check("session token authenticates", as1.auth_error is None, as1.auth_error)
    listed, err = as1.try_call("session.list")
    check("scripting: session.list", err is None, err)
    others = [s for s in (listed or []) if s["id"] != s1["id"]]
    check("session.list leaves other sessions' environment out", others and all("env" not in s for s in others), others[:1])
    check("scripting: read another session", as1.try_call("session.read", {"handle": s2["handle"]})[1] is None)
    check("scripting: meta.get", as1.try_call("meta.get", {"path": cwd})[1] is None)
    for method, params in [("run.approve", {"id": "x", "task": "y"}), ("automation.list", {}), ("agent.set_custom", {"agents": []}), ("mobile.status", {}),
                           ("daemon.set_policy", {"agents_control_others": True}), ("session.close", {"handle": s2["handle"]}),
                           ("daemon.shutdown", {})]:
        ok, detail = denied(as1, method, params)
        check(f"admin method refused to a session: {method}", ok, detail)

    # The real CLI inside the session authenticates with the session's token on its own.
    cli_out = os.path.join(tmp, "cli.txt")
    c.call("session.input", {"handle": s1["handle"], "data": f"& '{os.path.abspath(exe)}' worktree set --comment 'from-cli' *> '{cli_out}'; & '{os.path.abspath(exe)}' call automation.list *>> '{cli_out}'\r"})
    out = wait_for(lambda: os.path.exists(cli_out) and "automation.list" in open(cli_out, encoding="utf-8", errors="replace").read() and open(cli_out, encoding="utf-8", errors="replace").read(), 60) or ""
    meta = c.call("meta.get", {"path": cwd}) or {}
    check("`kelpied worktree set` inside a session works", meta.get("comment") == "from-cli", meta)
    check("`kelpied call automation.list` inside a session is refused", "not available to programs running in a Kelpie session" in out, out[-300:])

    probe = f"probe-model-{os.getpid()}"
    as1.try_call("hook.report", {"session_id": s2["id"], "agent": "claude", "payload": {"hook_event_name": "SessionStart", "model": probe}})
    models = {s["id"]: s.get("model") for s in c.call("session.list")}
    check("hook.report lands on the caller's own session", models.get(s1["id"]) == probe and models.get(s2["id"]) != probe, models)

    marker = f"rawinput{os.getpid()}"
    as1.input_frame(s2["handle"], f"echo {marker}\r")
    time.sleep(1.5)
    check("raw input frames from a session are ignored", marker not in screen(c, s2["handle"], 80))

    c.call("daemon.set_policy", {"agents_control_others": False})
    check("policy off: own session still readable", as1.try_call("session.read", {"handle": s1["handle"]})[1] is None)
    ok, detail = denied(as1, "session.read", {"handle": s2["handle"]})
    check("policy off: other sessions refused", ok, detail)
    ok, detail = denied(as1, "session.create", {"kind": "shell", "cwd": cwd})
    check("policy off: session.create refused", ok, detail)
    ok, detail = denied(as1, "meta.set", {"path": os.path.dirname(cwd), "comment": "x"})
    check("policy off: meta.set outside the own folder refused", ok, detail)
    check("policy off: meta.set in the own folder works", as1.try_call("meta.set", {"path": cwd, "comment": "auth test"})[1] is None)
    check("policy is saved for the next daemon start", '"agents_control_others":false' in open(os.path.join(os.environ["KELPIE_DATA_DIR"], "policy.json")).read().replace(" ", ""))
    c.call("daemon.set_policy", {"agents_control_others": True})
    check("policy on again: other sessions readable", as1.try_call("session.read", {"handle": s2["handle"]})[1] is None)

    s3 = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": "auth-restricted", "restricted": True})
    tok3 = session_token(c, s3, tmp)
    as3 = Raw({"session_id": s3["id"], "token": tok3})
    ok, detail = denied(as3, "session.read", {"handle": s2["handle"]})
    check("restricted session: other sessions refused while the policy is on", ok, detail)
    check("restricted session: own session readable", as3.try_call("session.read", {"handle": s3["handle"]})[1] is None)

    for s in (s1, s2, s3):
        c.call("session.close", {"handle": s["handle"]})
finally:
    daemon.kill()

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
