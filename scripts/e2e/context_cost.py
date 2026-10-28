"""GUI e2e for context, tokens and cost: the ring, the numbers, the exact mapping, the cap and the page.

    python -X utf8 scripts/e2e/context_cost.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint, data folder and Claude config home) and tauri-driver
with the same environment, so the app reads only the fixture transcripts it writes, and stops both on exit.
The agent is a stand-in `claude` (a batch shim on PATH that sleeps): no real Claude session is started.

1. Two Claude sessions in one folder, each with its own conversation id and fixture transcript (cache
   tokens, the same API message repeated per content block): each ring shows its own context share
   (71 % amber, 86 % red), the numbers equal the fixture truth (tokens, cost at Opus 5.5 prices, deduped).
2. A hook's transcript_path that names another conversation is not used; the daemon event carries the
   ids and the path but nothing else of the hook payload. Sub-agent transcripts (the real layout,
   `<id>/subagents/agent-*.jsonl`, one message repeated with growing output) count towards the session.
3. Lines appended live move the ring and the totals; a repeated message adds nothing.
4. A session without a conversation id (`--continue`) shows no ring, though a newer transcript sits in
   its folder; an id with no transcript gives no numbers.
5. A status turn end (output, then quiet) broadcasts `session.turn` without a hook.
6. The cost cap puts a red chip on the session over it and one inbox entry (not on the other session);
   it counts a session's conversations together (a /clear keeps the sum); the optional Esc is pressed
   once, the first time the session is over the cap and working.
7. The Usage & cost page lists the fixture project with the same totals.
"""
import importlib.util
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import traceback
import uuid
from datetime import datetime, timezone

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

ENDPOINT = f"kelpie-context-cost-{os.getpid()}"
DATA = tempfile.mkdtemp(prefix="kelpie-context-cost-data-")
WORK = os.path.join(scratch, f"context-cost-{os.getpid()}")
CLAUDE_HOME = os.path.join(WORK, "claude-home")
CODEX_HOME = os.path.join(WORK, "codex-home")
BIN = os.path.join(WORK, "bin")
PROJ_NAME = f"ctxcost-{os.getpid()}"
PROJ = os.path.join(WORK, PROJ_NAME)
for d in (CLAUDE_HOME, CODEX_HOME, BIN, PROJ):
    os.makedirs(d, exist_ok=True)

# A stand-in agent: `claude` resolves to this shim; it prints a banner, and on "go" writes output for a
# few seconds (the daemon sees a working turn that then ends).
FAKE = os.path.join(WORK, "fake_claude.py")
with open(FAKE, "w", encoding="utf-8") as f:
    f.write(
        "import msvcrt, time\n"
        "print('fake claude ready', flush=True)\n"
        "buf = ''\n"
        "while True:\n"
        "    ch = msvcrt.getwch()\n"
        "    if ch == '\\x1b':\n"
        "        print('GOT-ESC', flush=True)\n"
        "    elif ch in '\\r\\n':\n"
        "        if buf.strip() == 'go':\n"
        "            end = time.time() + 3\n"
        "            while time.time() < end:\n"
        "                print('working ' + str(time.time()), flush=True)\n"
        "                time.sleep(0.2)\n"
        "            print('done', flush=True)\n"
        "        buf = ''\n"
        "    else:\n"
        "        buf += ch\n"
    )
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{FAKE}" %*\r\n')

os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "CLAUDE_CONFIG_DIR": CLAUDE_HOME, "CODEX_HOME": CODEX_HOME,
    "PATH": BIN + os.pathsep + os.environ.get("PATH", ""),
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
    os.environ.pop(k, None)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
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

# ---------- fixture truth (Opus 5.5: 4 / 20 USD per MTok, cache read 0.20, 5 m write 5.00, 1 h write 8.00) ----------


def now_iso():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def assistant(msg_id, inp, read, out, w5=0, w1=0, flat_write=None, sidechain=False):
    """One transcript line of an API message; real transcripts repeat it once per content block."""
    usage = {"input_tokens": inp, "cache_read_input_tokens": read, "output_tokens": out}
    if flat_write is not None:
        usage["cache_creation_input_tokens"] = flat_write
    else:
        usage["cache_creation_input_tokens"] = w5 + w1
        usage["cache_creation"] = {"ephemeral_5m_input_tokens": w5, "ephemeral_1h_input_tokens": w1}
    line = {"type": "assistant", "timestamp": now_iso(), "cwd": PROJ, "isSidechain": sidechain,
            "message": {"id": msg_id, "model": "claude-opus-5-5", "usage": usage, "content": [{"type": "text", "text": "fixture"}]}}
    return json.dumps(line, separators=(",", ":"))


def price(inp, read, w5, w1, out):
    return (inp * 4 + read * 0.2 + w5 * 5 + w1 * 8 + out * 20) / 1e6


A1 = dict(inp=1000, read=600_000, w5=60_000, w1=40_000, out=9000)      # context 710 000 = 71 %
B1 = dict(inp=500, read=800_000, flat=50_000, out=9500)                  # context 860 000 = 86 %
A2 = dict(inp=2000, read=870_000, out=8000)                              # context 880 000 = 88 %
A_COST1 = price(A1["inp"], A1["read"], A1["w5"], A1["w1"], A1["out"])
B_COST = price(B1["inp"], B1["read"], B1["flat"], 0, B1["out"])
A_COST2 = A_COST1 + price(A2["inp"], A2["read"], 0, 0, A2["out"])
A_IN1 = A1["inp"] + A1["read"] + A1["w5"] + A1["w1"]
A_IN2 = A_IN1 + A2["inp"] + A2["read"]
B_IN = B1["inp"] + B1["read"] + B1["flat"]
# B also has a sub-agent transcript (real layout): one message repeated with growing output 1 -> 40 -> 300.
SUB = dict(inp=100, read=1000, out=300)
B_TOTAL_IN = B_IN + SUB["inp"] + SUB["read"]
B_TOTAL_OUT = B1["out"] + SUB["out"]
B_TOTAL_COST = B_COST + price(SUB["inp"], SUB["read"], 0, 0, SUB["out"])
# After a /clear cc2 continues in a new conversation costing C_COST more; the cap counts both.
C1 = dict(inp=0, read=500_000, out=5000)
C_COST = price(0, C1["read"], 0, 0, C1["out"])
CAP = 0.7  # between B (0.602) and A (0.924)


def fmt_tokens(n):
    def unit(v, s):
        return f"{round(v) if v >= 10 else round(v * 10) / 10:g}{s}"
    return str(n) if n < 1000 else unit(n / 1e3, "k") if n < 1e6 else unit(n / 1e6, "M")


def fmt_cost(c):
    return f"≈ ${c:.2f}"


def until(fn, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.25)
    return None


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def invoke(d, cmd, payload):
    r = d.invoke(cmd, payload)
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def transcript_path(conv):
    slug = re.sub(r"[^A-Za-z0-9]", "-", PROJ)
    folder = os.path.join(CLAUDE_HOME, "projects", slug)
    os.makedirs(folder, exist_ok=True)
    return os.path.join(folder, conv + ".jsonl")


def append(path, *lines):
    with open(path, "a", encoding="utf-8", newline="\n") as f:
        for line in lines:
            f.write(line + "\n")


def hook(d, session, conv, path, event="Stop", **extra):
    daemon(d, "hook.report", {"session_id": session["id"], "agent": "claude", "payload": {"hook_event_name": event, "session_id": conv, "transcript_path": path, **extra}})


def session_named(d, name):
    return d.js(f"return Object.values({S}.sessions).find(s => s.name === arguments[0]) || null", name)


def ring(d, name):
    """The ring on a session's sidebar row: {pct, level} or None."""
    return d.js("""
      const row = [...document.querySelectorAll('.session-row')].find(r => r.querySelector('.name')?.textContent === arguments[0]);
      const b = row && row.querySelector('.ctx-ring-btn');
      if (!b) return null;
      const svg = b.querySelector('svg.ctx-ring');
      return {pct: b.getAttribute('data-ctx-ring'), level: svg ? [...svg.classList].find(c => ['ok', 'warn', 'danger'].includes(c)) : null,
              chip: (b.querySelector('[data-cost-chip]') || {}).textContent || null, over: !!b.querySelector('.cost-chip.over'), title: b.title};
    """, name)


def start_agent(d, pid, name, args=None):
    opts = {"name": name}
    if args:
        opts["args"] = args
    info = js_await(d, f"{S}.newSession(arguments[0], 'claude', arguments[1])", pid, opts)
    if not info:
        raise RuntimeError(f"{name} did not start")
    return info


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    until(lambda: d.js(f"return {S}.daemonApi()") >= 9, 30)
    run.check("the app talks to a daemon that sends session.turn (API 9)", d.js(f"return {S}.daemonApi()") >= 9)
    d.js("window.__turns = []; window.addEventListener('kelpie:session-turn', e => window.__turns.push(e.detail));")

    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    a = start_agent(d, pid, "cc1")
    b = start_agent(d, pid, "cc2")
    noid = start_agent(d, pid, "noid", ["--continue"])
    conv_a, conv_b = a["agent_session_id"], b["agent_session_id"]
    run.check("two Claude sessions in one folder got their own conversation ids", bool(conv_a) and bool(conv_b) and conv_a != conv_b, f"{conv_a} {conv_b}")
    run.check("a session started with --continue has no conversation id", noid["agent_session_id"] is None, str(noid["agent_session_id"]))
    path_a, path_b = transcript_path(conv_a), transcript_path(conv_b)

    # ---------- 1. each ring shows its own numbers ----------
    append(path_a, assistant("msg_A1", A1["inp"], A1["read"], A1["out"], A1["w5"], A1["w1"]), assistant("msg_A1", A1["inp"], A1["read"], A1["out"], A1["w5"], A1["w1"]))
    append(path_b, assistant("msg_B1", B1["inp"], B1["read"], B1["out"], flat_write=B1["flat"]))
    sub_dir = os.path.join(os.path.dirname(path_b), conv_b, "subagents")
    os.makedirs(sub_dir, exist_ok=True)
    append(os.path.join(sub_dir, "agent-a1.jsonl"), *[assistant("msg_SUB", SUB["inp"], SUB["read"], out, sidechain=True) for out in (1, 40, 300, 40)])
    # A newer transcript of an unrelated conversation sits in the folder: it must not be picked up.
    append(transcript_path(str(uuid.uuid4())), assistant("msg_X", 1, 1, 1))
    # The hook reports the session and the path; B's hook names A's transcript (a lie) and must still get B's.
    hook(d, a, conv_a, path_a, prompt="secret text")
    hook(d, b, conv_b, path_a)
    run.check("the hook's turn event carries the ids and the path and nothing else", bool(until(lambda: d.js(
        "return window.__turns.some(t => t.session === arguments[0] && t.event === 'Stop' && t.agent_session_id === arguments[1] && t.transcript_path === arguments[2] && !('prompt' in t) && Object.keys(t).length === 4)", a["id"], conv_a, path_a), 10)))
    ra = until(lambda: ring(d, "cc1"), 15)
    rb = until(lambda: ring(d, "cc2"), 15)
    run.check("cc1's ring is 71 % and amber", bool(ra) and ra["pct"] == "71" and ra["level"] == "warn", str(ra))
    run.check("cc2's ring is 86 % and red (its own transcript, not the one named by its hook)", bool(rb) and rb["pct"] == "86" and rb["level"] == "danger", str(rb))
    if not rb:
        print("rows:", d.js(r"return [...document.querySelectorAll('.session-row')].map(r => r.innerText.replace(/\s+/g, ' '))"))
        print("turns:", d.js("return window.__turns.map(t => t.session.slice(0, 8) + ':' + t.event)"))
    run.shot("01-rings")
    ua = invoke(d, "session_usage", {"kind": "claude", "agentSessionId": conv_a, "cwd": PROJ})
    ub = invoke(d, "session_usage", {"kind": "claude", "agentSessionId": conv_b, "transcriptPath": path_a, "cwd": PROJ})
    run.check("cc1 numbers equal the fixture (message counted once, cache writes at 5 m / 1 h prices)",
              bool(ua) and ua["tokens_in"] == A_IN1 and ua["tokens_out"] == A1["out"] and abs(ua["cost"] - A_COST1) < 1e-9 and not ua["cost_exact"] and ua["totals"], str(ua))
    run.check("cc2: context from its main conversation (sub-agent messages are not its context), model and window known",
              bool(ub) and ub["context_used"] == 860_000 and ub["model"] == "claude-opus-5-5" and ub["context_window"] == 1_000_000, str(ub))
    run.check("cc2's totals are its own and its sub-agent's (the repeated message at its highest output), not those of the transcript its hook named",
              bool(ub) and ub["tokens_in"] == B_TOTAL_IN and ub["tokens_out"] == B_TOTAL_OUT and abs(ub["cost"] - B_TOTAL_COST) < 1e-9, str(ub))

    # ---------- 3. appended lines move the ring; a repeat adds nothing ----------
    append(path_a, assistant("msg_A2", A2["inp"], A2["read"], A2["out"]))
    hook(d, a, conv_a, path_a)
    ra = until(lambda: (r := ring(d, "cc1")) and r["pct"] == "88" and r, 15)
    run.check("an appended message moves cc1's ring to 88 % and red", bool(ra) and ra["level"] == "danger", str(ring(d, "cc1")))
    append(path_a, assistant("msg_A2", A2["inp"], A2["read"], A2["out"]))
    hook(d, a, conv_a, path_a)
    time.sleep(2.5)
    ua = invoke(d, "session_usage", {"kind": "claude", "agentSessionId": conv_a, "cwd": PROJ})
    run.check("totals after the append and a repeated message match the fixture",
              bool(ua) and ua["tokens_in"] == A_IN2 and ua["tokens_out"] == A1["out"] + A2["out"] and abs(ua["cost"] - A_COST2) < 1e-9, str(ua))
    tip = (ring(d, "cc1") or {}).get("title", "")
    run.check("the tooltip says estimate at API prices and names the model", "estimate at API prices" in tip and "claude-opus-5-5" in tip and "≈ $" in tip, tip)

    # ---------- 4. no id, no ring; an unknown id, no numbers ----------
    run.check("the session without an id has no ring", ring(d, "noid") is None, str(ring(d, "noid")))
    run.check("a conversation id with no transcript gives no numbers (the folder's newest file is not used)",
              invoke(d, "session_usage", {"kind": "claude", "agentSessionId": str(uuid.uuid4()), "cwd": PROJ}) is None)
    run.check("no id at all gives no numbers", invoke(d, "session_usage", {"kind": "claude", "cwd": PROJ}) is None)

    # ---------- 5. a status turn end broadcasts session.turn without a hook ----------
    daemon(d, "session.input", {"handle": a["handle"], "data": "go\r"})
    got = until(lambda: d.js("return window.__turns.some(t => t.session === arguments[0] && t.event === 'turn_end' && t.agent_session_id === arguments[1])", a["id"], conv_a), 40)
    run.check("a session that worked and went quiet broadcasts a turn_end event with its conversation id", bool(got))

    # ---------- 6. cost cap: chip + one inbox entry, only for the session over it ----------
    d.js(f"{S}.setSettings({{ costCap: {CAP} }})")
    over = until(lambda: (r := ring(d, "cc1")) and r["over"] and r, 15)
    run.check("cc1 (≈ $%.2f) gets a red chip over the $%.2f cap" % (A_COST2, CAP), bool(over) and over["chip"] == fmt_cost(A_COST2), str(over))
    run.check("cc2 (≈ $%.2f) stays under the cap: no chip" % B_TOTAL_COST, (ring(d, "cc2") or {}).get("chip") is None, str(ring(d, "cc2")))
    entries = until(lambda: d.js(f"return {S}.state.inbox.filter(i => i.kind === 'cost').map(i => i.sessionId)"), 10) or []
    time.sleep(1.5)
    entries = d.js(f"return {S}.state.inbox.filter(i => i.kind === 'cost').map(i => i.sessionId)")
    run.check("one inbox entry, for cc1", entries == [a["id"]], str(entries))
    run.shot("02-cap")
    # cc2 continues in a new conversation (what /clear does): the session's cost is the sum of both.
    conv_c = str(uuid.uuid4())
    path_c = transcript_path(conv_c)
    append(path_c, assistant("msg_C1", C1["inp"], C1["read"], C1["out"]))
    hook(d, b, conv_c, path_c)
    over_b = until(lambda: (r := ring(d, "cc2")) and r["over"] and r, 20)
    run.check("cc2's new conversation (≈ $%.2f) takes the session (≈ $%.2f in all) over the cap" % (C_COST, B_TOTAL_COST + C_COST),
              bool(over_b) and over_b["chip"] == fmt_cost(B_TOTAL_COST + C_COST), str(over_b))
    got = sorted(d.js(f"return {S}.state.inbox.filter(i => i.kind === 'cost').map(i => i.sessionId)"))
    run.check("…with its own inbox entry", got == sorted([a["id"], b["id"]]), str(got))
    # Optional Esc: once, when the session is over the cap and working.
    d.js(f"{S}.setSettings({{ costCapEsc: true }})")
    daemon(d, "session.input", {"handle": a["handle"], "data": "go\r"})
    esc = until(lambda: "GOT-ESC" in daemon(d, "session.read", {"handle": a["handle"], "lines": 40})["text"], 30)
    run.check("the session over the cap is interrupted with Esc while it works", bool(esc))
    until(lambda: d.js(f"return {S}.sessions[arguments[0]].status !== 'working'", a["id"]), 30)
    time.sleep(1)
    daemon(d, "session.input", {"handle": a["handle"], "data": "go\r"})
    until(lambda: d.js(f"return {S}.sessions[arguments[0]].status === 'working'", a["id"]), 30)
    time.sleep(3)
    text = daemon(d, "session.read", {"handle": a["handle"], "lines": 80})["text"]
    run.check("…and only once", text.count("GOT-ESC") == 1, str(text.count("GOT-ESC")))
    d.js(f"{S}.setSettings({{ costCapEsc: false }})")
    d.js(f"{S}.setSettings({{ costCap: 0 }})")
    run.check("turning the cap off removes the chip", until(lambda: (ring(d, "cc1") or {}).get("chip") is None, 10) is not None)

    # ---------- 7. the Usage & cost page ----------
    d.js(f"{S}.setOverlay('cost')")
    d.wait("return !!document.querySelector('.cost-page')", 20)
    d.js("document.querySelector('.cost-page [data-by=project]').click(); document.querySelector('.cost-page [data-range=all]').click();")
    row = until(lambda: d.js("""
      const r = [...document.querySelectorAll('.cost-page .cost-row')].find(x => x.querySelector('.label')?.textContent === arguments[0]);
      return r ? [...r.querySelectorAll('.num')].map(n => n.textContent) : null;
    """, PROJ_NAME), 90)
    want = [fmt_tokens(A_IN2 + B_TOTAL_IN + C1["read"]), fmt_tokens(A1["out"] + A2["out"] + B_TOTAL_OUT + C1["out"]), fmt_cost(A_COST2 + B_TOTAL_COST + C_COST)]
    run.check("the page lists the fixture project with the fixture totals", row == want, f"{row} != {want}")
    run.shot("03-cost-page")
    d.keys("escape")
    run.check("Esc closes the page", until(lambda: not d.js("return !!document.querySelector('.cost-page')"), 5) is not None)


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
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE])
    if not until(port_open, 30):
        raise SystemExit("tauri-driver did not open its port")

    run = None
    try:
        run = Run(shots, app=app)
        scenario(run)
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
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
        c = kc.Conn()
        c.call("daemon.shutdown", {"kill_sessions": True})
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
