"""Sub-session orchestration on an isolated daemon, no GUI (crates/kelpied/src/daemon/sub_rpc.rs).

    python scripts/e2e/sub_sessions.py <kelpied.exe>

Starts its own daemon on endpoint `kelpie-sub-e2e` with a temporary data folder (both set in this
process's environment before the daemon starts) and stops only that PID. The agents are a generated
fake agent: a Python program registered as a custom agent kind, so it counts as an installed agent.
It writes its own session id and token to a file, logs every line typed into it, and runs
`RUN <kelpied sub args>` lines through $KELPIE_CLI, as a real agent would use the CLI. The parent
calls are made as the session itself (its token), the way the MCP server and the CLI make them.

Checks: policy allow (parent and depth), ask (request event, deny, once, allow for this session),
caps (sub_max 1 refuses and says why, policy off refuses), sub.message and sub.report delivery into
an agent's typed input under the "not from the user" header, a shell parent gets its reports in its
inbox and nothing typed, sub.close of own descendants only, session.create from a session becomes a
linked child (agents only), a grandchild's depth, and the lineage_from hand-over.
The shell part needs Windows (PowerShell); the rest runs wherever the daemon does.
"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time

exe = os.path.abspath(sys.argv[1])
ENDPOINT = "kelpie-sub-e2e"
for key in [k for k in os.environ if k.startswith("KELPIE_")]:
    os.environ.pop(key)  # a script run from inside a Kelpie session must not pass that session to the test daemon
os.environ["KELPIE_ENDPOINT"] = ENDPOINT
os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix="kelpie-sub-data-")
WORK = tempfile.mkdtemp(prefix="kelpie-sub-work-")
STATE = tempfile.mkdtemp(prefix="kelpie-sub-state-")



def psq(path):
    """A PowerShell single-quoted literal: a quote in the path (a profile named O'Brien) is doubled."""
    return "'" + path.replace("'", "''") + "'"

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

FAKE_AGENT = r'''
import json, os, shlex, subprocess, sys
state = sys.argv[1]
sid = os.environ.get("KELPIE_SESSION_ID", "")
def put(name, data):
    path = os.path.join(state, sid + name)
    with open(path + ".tmp", "w", encoding="utf-8") as f:
        f.write(data)
    os.replace(path + ".tmp", path)
put(".json", json.dumps({"id": sid, "token": os.environ.get("KELPIE_SESSION_TOKEN", "")}))
log = open(os.path.join(state, sid + ".log"), "a", encoding="utf-8", buffering=1)
print("fake agent ready", flush=True)
while True:
    line = sys.stdin.readline()
    if not line:
        break
    text = line.rstrip("\r\n")
    log.write(text + "\n")
    if text.startswith("RUN "):
        r = subprocess.run([os.environ.get("KELPIE_CLI", "kelpied")] + shlex.split(text[4:]), capture_output=True, text=True, encoding="utf-8", errors="replace")
        log.write("RUN-EXIT %d\n%s%s\n" % (r.returncode, r.stdout, r.stderr))
    print("ok", flush=True)
'''
FAKE_PY = os.path.join(STATE, "fake_agent.py")
with open(FAKE_PY, "w", encoding="utf-8") as f:
    f.write(FAKE_AGENT)
FAKE = {"id": "fake", "label": "Fake agent", "bin": sys.executable, "launch": [FAKE_PY, STATE], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []}

results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail[:300]})" if detail and not ok else ""), flush=True)


def wait_for(fn, timeout, step=0.5):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def in_thread(fn):
    """Runs a call that the daemon answers later (an ask), keeping its result or refusal."""
    box = {}

    def run():
        try:
            box["ok"] = fn()
        except (SystemExit, Exception) as e:  # Conn.call exits with the daemon's refusal text
            box["err"] = str(e)

    t = threading.Thread(target=run, daemon=True)
    t.start()
    return t, box


def refusal(fn):
    """The refusal text of a call, or "" when it went through."""
    try:
        fn()
        return ""
    except (SystemExit, Exception) as e:
        return str(e)


def read_json(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def read_text(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


def fake_state(sid):
    return wait_for(lambda: read_json(os.path.join(STATE, sid + ".json")), 60)


def log_text(sid):
    return read_text(os.path.join(STATE, sid + ".log"))


def session_conn(s):
    """A connection that speaks as session `s`, with the token the fake agent wrote from its own environment."""
    token = kc.user_token
    kc.user_token = lambda: None  # no user token on this connection
    try:
        conn = kc.Conn()
    finally:
        kc.user_token = token
    conn.call("auth", {"session_id": s["id"], "token": fake_state(s["id"])["token"]})
    return conn


class Events:
    """A user connection that collects the daemon's events (sub.request is sent to the user only)."""

    def __init__(self):
        self.conn = kc.Conn()
        self.items = []
        self.lock = threading.Lock()
        threading.Thread(target=self._run, daemon=True).start()

    def _run(self):
        try:
            while True:
                kind, body = self.conn.frame()
                if kind == kc.KIND_JSON:
                    msg = json.loads(body)
                    if "event" in msg:
                        with self.lock:
                            self.items.append(msg)
        except Exception:
            pass

    def named(self, name):
        with self.lock:
            return [m.get("params", {}) for m in self.items if m.get("event") == name]


def screen(c, s, lines=60):
    return c.call("session.read", {"handle": s["handle"], "lines": lines})["text"]


def find(c, sid):
    return next((s for s in c.call("session.list") if s["id"] == sid), None)


def request_for(events, task, seen=(), timeout=15):
    """The newest request for `task` that is not one of `seen` (ids already answered)."""
    return wait_for(lambda: next((r for r in events.named("sub.request") if r.get("task") == task and r.get("id") not in seen), None), timeout) or {}


try:
    kc.Conn()
except OSError:
    pass  # nothing answers: a fresh daemon may start on this endpoint
else:
    # Before the cleanup below, which would otherwise remove the token of the daemon that answers.
    raise SystemExit(f"endpoint {ENDPOINT} already answers: an earlier daemon is still up; stop it by its PID, then rerun")

c = None
daemon = None
try:
    daemon = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(200):
        try:
            c = kc.Conn()
            break
        except OSError:
            time.sleep(0.1)
    check("isolated daemon starts on its own endpoint", c is not None)
    events = Events()

    # ---- setup: a fake agent kind, a parent agent session, policy ----
    check("policy default is ask", c.call("daemon.policy")["sub_sessions"] == "ask")
    c.call("agent.set_custom", {"agents": [FAKE]})
    fake_row = next((p for p in c.call("agent.presets") if p["id"] == "fake"), {})
    check("the fake agent is an installed agent", fake_row.get("installed") is True, fake_row)
    parent = c.call("session.create", {"kind": "fake", "cwd": WORK, "name": "parent"})
    check("user creates an agent parent session", bool(parent.get("id")))
    check("parent agent is idle and wrote its token", bool(wait_for(lambda: (find(c, parent["id"]) or {}).get("status") == "idle" and fake_state(parent["id"]), 60)))
    ps = session_conn(parent)

    # ---- policy allow: spawn links parent and depth ----
    c.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 4})
    check("policy set to allow with cap 4", c.call("daemon.policy")["sub_max"] == 4)
    one = ps.call("sub.spawn", {"agent": "fake", "task": "say hello one", "name": "child-one"})
    row = find(c, one["id"]) or {}
    check("allow: spawn returns a handle", bool(one.get("handle")), one)
    check("allow: child lists its parent and depth 1", row.get("parent") == parent["id"] and row.get("depth") == 1, {k: row.get(k) for k in ("parent", "depth")})
    check("allow: child got its first prompt typed in", bool(wait_for(lambda: "say hello one" in log_text(one["id"]), 60)))

    # ---- ask: request event, deny, once, allow for this session ----
    c.call("daemon.set_policy", {"sub_sessions": "ask"})
    t, box = in_thread(lambda: ps.call("sub.spawn", {"agent": "fake", "task": "say hello two", "name": "child-two"}))
    req = denied_req = request_for(events, "say hello two")
    check("ask: sub.request reaches the user with parent and task", req.get("session") == parent["id"] and req.get("agent") == "fake", req)
    check("ask: the request is listed as pending", any(p.get("id") == req.get("id") for p in c.call("sub.pending")))
    c.call("sub.answer", {"id": req.get("id"), "decision": "deny"})
    t.join(20)
    check("deny: the spawn is refused with the user's words", "denied" in box.get("err", ""), box)
    check("deny: no child was started", not any(s.get("name") == "child-two" for s in c.call("session.list")))
    check("deny: sub.request_done reaches the user", any(d.get("id") == req.get("id") and d.get("decision") == "deny" for d in events.named("sub.request_done")))

    t, box = in_thread(lambda: ps.call("sub.spawn", {"agent": "fake", "task": "say hello two", "name": "child-two"}))
    req = request_for(events, "say hello two", seen={denied_req.get("id")}, timeout=20)
    c.call("sub.answer", {"id": req.get("id"), "decision": "once"})
    t.join(30)
    two = box.get("ok") or {}
    check("once: the spawn returns after the user allows it", bool(two.get("handle")), box)
    check("once: the child lists its parent", (find(c, two.get("id", "")) or {}).get("parent") == parent["id"])

    t, box = in_thread(lambda: ps.call("sub.spawn", {"agent": "fake", "task": "say hello three", "name": "child-three"}))
    req = request_for(events, "say hello three", seen=set())
    c.call("sub.answer", {"id": req.get("id"), "decision": "session"})
    t.join(30)
    three = box.get("ok") or {}
    check("allow for this session: the spawn goes through", bool(three.get("handle")), box)
    asked_before = len(events.named("sub.request"))
    four = ps.call("sub.spawn", {"agent": "fake", "task": "say hello four", "name": "child-four"})
    time.sleep(1)
    check("allow for this session: the next spawn is not asked again", bool(four.get("handle")) and len(events.named("sub.request")) == asked_before, four)

    # ---- caps: a refusal says what is running and asks nothing ----
    c.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 1})
    asked_before = len(events.named("sub.request"))
    capped = refusal(lambda: ps.call("sub.spawn", {"agent": "fake", "task": "over the cap", "name": "over"}))
    check("sub_max 1: a spawn beyond it is refused with the running count", "4 sub-sessions are running" in capped and "the limit on this computer" in capped, capped)
    check("sub_max 1: the refused spawn asked the user nothing", len(events.named("sub.request")) == asked_before)
    c.call("daemon.set_policy", {"sub_sessions": "off"})
    off = refusal(lambda: ps.call("sub.spawn", {"agent": "fake", "task": "policy off", "name": "off"}))
    check("policy off: a spawn is refused and says where to turn it on", "turned off" in off and "Settings" in off, off)
    c.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 4})

    # ---- close: own descendants only ----
    for child in (two, three, four):
        ps.call("sub.close", {"session": child["id"]})
    check("sub.close of its own children works", find(c, two["id"]) is None and find(c, four["id"]) is None)
    other = c.call("session.create", {"kind": "custom", "cwd": WORK, "name": "not-mine", "args": [sys.executable, "-c", "import time; time.sleep(900)"]})
    refused = refusal(lambda: ps.call("sub.close", {"session": other["id"]}))
    check("sub.close of a session it did not open is refused", "not one of your sub-sessions" in refused and find(c, other["id"]) is not None, refused)

    # ---- the boundaries on what a session may open or message ----
    to_custom = refusal(lambda: ps.call("sub.message", {"to": other["id"], "text": "hi"}))
    check("sub.message to a non-agent session is refused", "is not an agent" in to_custom, to_custom)
    for kind in ("shell", "custom"):
        made = refusal(lambda: ps.call("session.create", {"kind": kind, "cwd": WORK, "name": "from-parent", "args": [sys.executable, "-c", "pass"]}))
        check(f"session.create {kind} from a session is refused", "agent sessions only" in made, made)

    via = ps.call("session.create", {"kind": "fake", "cwd": WORK, "name": "via-create", "prompt": "say hello via create"})
    check("session.create from a session becomes a linked child (depth 1)", via.get("parent") == parent["id"] and via.get("depth") == 1, {k: via.get(k) for k in ("parent", "depth")})

    # ---- a child opens a grandchild; the parent may close it through the chain ----
    c1 = session_conn(one)
    grand = c1.call("sub.spawn", {"agent": "fake", "task": "say hello grand", "name": "grand"})
    grow = find(c, grand["id"]) or {}
    check("a child opens a grandchild at depth 2 under itself", grow.get("parent") == one["id"] and grow.get("depth") == 2, {k: grow.get(k) for k in ("parent", "depth")})
    ps.call("sub.close", {"session": grand["id"]})
    check("the parent closes a grandchild through its child", find(c, grand["id"]) is None)

    # ---- mail: the parent's message reaches the child; the child's report reaches the parent ----
    queued = ps.call("sub.message", {"to": one["id"], "text": "please add a note to the file"})
    check("sub.message from the parent is queued for the child", queued.get("queued") is True, queued)
    rep = c1.call("sub.report", {"status": "done", "summary": "said hello in the file"})
    check("sub.report names the parent as the one told", bool(rep.get("delivered_to")), rep)
    check("the child's explicit report is on its row", (find(c, one["id"]) or {}).get("report", {}).get("status") == "done")
    check("the parent gets the report typed in under the not-from-the-user header",
          bool(wait_for(lambda: "not from the user" in log_text(parent["id"]) and "said hello in the file" in log_text(parent["id"]), 150, 1)), log_text(parent["id"])[-300:])
    check("the child gets the parent's message typed in under the header",
          bool(wait_for(lambda: "not from the user" in log_text(one["id"]) and "please add a note" in log_text(one["id"]), 150, 1)), log_text(one["id"])[-300:])

    # The CLI, run by the parent agent with its own environment, lists what it opened.
    c.call("session.input", {"handle": find(c, parent["id"])["handle"], "data": "RUN sub list --json\r"})
    listed = wait_for(lambda: "RUN-EXIT" in log_text(parent["id"]) and log_text(parent["id"]), 60) or ""
    check("kelpied sub list works inside a session (KELPIE_CLI and its own token)", "RUN-EXIT 0" in listed and one["id"] in listed and via["id"] in listed, listed[-300:])

    # ---- lineage_from: a conversation handed to a new session keeps its place and its children ----
    cv = session_conn(via)
    under = cv.call("sub.spawn", {"agent": "fake", "task": "say hello under via", "name": "under-via"})
    handed = c.call("session.create", {"kind": "fake", "cwd": WORK, "name": "handed", "lineage_from": via["id"]})
    hrow = find(c, handed["id"]) or {}
    check("lineage_from: the new session takes the old one's place under the parent", hrow.get("parent") == parent["id"] and hrow.get("depth") == 1, {k: hrow.get(k) for k in ("parent", "depth")})
    check("lineage_from: the old session is nobody's sub-session now", (find(c, via["id"]) or {}).get("parent") is None)
    check("lineage_from: its children follow the new session", (find(c, under["id"]) or {}).get("parent") == handed["id"])

    # ---- a shell parent: CLI spawn, reports wait in its inbox, nothing is typed into it ----
    if os.name == "nt":
        c.call("daemon.set_policy", {"sub_sessions": "allow", "sub_max": 8})
        shell = c.call("session.create", {"kind": "shell", "cwd": WORK, "name": "shell-parent"})
        ready = wait_for(lambda: "PS " in screen(c, shell) and (find(c, shell["id"]) or {}).get("status") == "idle", 180)
        check("shell parent is ready for input", bool(ready))
        spawn_out = os.path.join(STATE, "shell-spawn.json")
        c.call("session.input", {"handle": shell["handle"], "data": f"& $env:KELPIE_CLI sub spawn fake 'say hello from shell' --name shell-child --json *> {psq(spawn_out)}\r"})
        shell_child = wait_for(lambda: next((s for s in c.call("session.list") if s.get("name") == "shell-child"), None), 120)
        check("shell parent: `kelpied sub spawn` opens a child linked to it", bool(shell_child) and shell_child.get("parent") == shell["id"], {k: (shell_child or {}).get(k) for k in ("parent", "depth")})
        if shell_child:
            cs = session_conn(shell_child)
            cs.call("sub.report", {"status": "done", "summary": "shell child done"})
            time.sleep(6)
            check("shell parent: the report is not typed into the shell", "not from the user" not in screen(c, shell, 200))
            inbox_out = os.path.join(STATE, "shell-inbox.json")
            c.call("session.input", {"handle": shell["handle"], "data": f"& $env:KELPIE_CLI sub inbox --json *> {psq(inbox_out)}\r"})
            got = wait_for(lambda: "shell child done" in read_text(inbox_out), 60)
            check("shell parent: the report waits in its inbox (`kelpied sub inbox`)", bool(got), read_text(inbox_out)[-300:])
        else:
            check("shell parent: the report waits in its inbox", False, "no shell child to report")
    else:
        print("SKIP shell parent checks: needs Windows PowerShell")

finally:
    if c is not None:
        try:
            c.call("daemon.shutdown", {"kill_sessions": True})
        except Exception:
            pass  # the reply may race the daemon exiting (the pipe closes first)
    if daemon is not None:
        try:
            daemon.wait(15)
        except subprocess.TimeoutExpired:
            daemon.kill()  # only the daemon this script started
    token = kc.token_path()  # the user token for this endpoint only
    if os.path.exists(token):
        os.remove(token)
    for d in (os.environ["KELPIE_DATA_DIR"], WORK, STATE):
        shutil.rmtree(d, ignore_errors=True)

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
