"""E2E: a bell from an agent without hooks is "needs you" only when its reply asks something, in the real app.

    python -u -X utf8 scripts/e2e/bell_reads_the_reply.py <scratch-dir> <debug kelpie.exe> <kelpied.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). The daemon given runs on an endpoint and data folder of this process only, and is stopped by
its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2 (MSEDGEDRIVER or PATH).

The sessions are a fake agent without hooks (a Python program registered as a custom agent kind), drawn like Devin: its reply, a
rule with the conversation's title, its input box `❭ …`, a rule and a status bar, then a lone BEL, as Devin's `notify` rings when
a turn ends and when it asks. A typed `caseN` makes it work for four seconds and end that turn; `tab` rings at once, as a shell
does for a Tab with no completion. Starts no real agent and calls no model. Sessions are never opened in a pane, so a finished
turn nobody looked at is unread.

Cases, each its own session; the status is read from the daemon and the row from the sidebar's DOM:
  1 summary       a summary that asks nothing                    done (unread), not "needs you"
  2 decisions     the owner's decisions in Vietnamese prose       needs you
  3 menu          a numbered permission menu                      needs you
  4 tab           a bell right after typed input                  nothing changes
  5 chrome        `?` only in the input box and the status bar    done (unread)

`--before`: the same cases against another daemon (the one before the change), with its results printed; the exit code is 0.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

args = [a for a in sys.argv[1:] if not a.startswith("--")]
BEFORE = "--before" in sys.argv
if len(args) < 3:
    raise SystemExit(__doc__)
scratch, app, kelpied = (os.path.abspath(a) for a in args[:3])
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
os.makedirs(scratch, exist_ok=True)
DATA = ps.isolate("bell")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-bell-")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)

from smoke_support import Run, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

WORK = os.path.join(scratch, f"bell-{os.getpid()}")
os.makedirs(WORK, exist_ok=True)
FAKE_AGENT = r'''
import sys, time
RULE = "─" * 60
REPLIES = {
    "case1": "Đã xong. Tôi đã cập nhật README và chạy lại test.\n\nKết quả:\n- 14 test đều qua.\n- Không còn cảnh báo nào.",
    "case2": "Còn hai việc cần bạn quyết:\n\n1. Cài Tesseract để chạy OCR: cần quyền cài phần mềm trên máy, nên tôi chờ bạn đồng ý.\n2. Ảnh mẫu: tôi đã lưu vào thư mục samples.\n3. Git: thư mục dự án chưa có git. Bạn có muốn tôi git init và commit không?",
    "case5": "All done. The tests pass and the README is updated.",
}
MENU = "Allow running `rm -rf build`?\n\n❯ 1. Yes\n  2. No\n\nEsc to cancel"

def write(s):
    sys.stdout.write(s)
    sys.stdout.flush()

def draw(body, box="Ask Fake anything", status="Fake · bypass"):
    text = body if box is None else body + "\n" + RULE + " Đọc thông tin dự án ─\n❭ " + box + "\n" + RULE + "\n" + status
    write("\x1b[2J\x1b[H" + text.replace("\n", "\r\n"))

write("\x1b]0;fake: Đọc thông tin dự án\x07")
draw("Fake agent ready.")
for line in sys.stdin:
    cmd = line.strip()
    if cmd == "tab":
        write("\a")
        continue
    if cmd not in ("case1", "case2", "case3", "case5"):
        continue
    for i in range(8):
        write("\r\n⠋ Working " + str(i))
        time.sleep(0.5)
    if cmd == "case3":
        draw(MENU, box=None)
    elif cmd == "case5":
        draw(REPLIES[cmd], box="Anything else?", status="? for shortcuts · Ready?")
    else:
        draw(REPLIES[cmd])
    write("\a")
'''
FAKE_PY = os.path.join(WORK, "fake_agent.py")
with open(FAKE_PY, "w", encoding="utf-8") as f:
    f.write(FAKE_AGENT)
FAKE = {"id": "fake", "label": "Fake agent", "bin": sys.executable, "launch": ["-X", "utf8", FAKE_PY], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []}

S = "window.__kelpie.store"
PREFIX = "before" if BEFORE else "after"
# (name, what is typed, the state wanted after the change)
CASES = [
    ("1-summary", "case1", "done"),
    ("2-decisions", "case2", "waiting"),
    ("3-menu", "case3", "waiting"),
    ("4-tab", "tab", "idle"),
    ("5-chrome", "case5", "done"),
]
results = []
procs = {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail[:400]}]" if detail else ""), flush=True)


def until(fn, timeout=30, step=0.25):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:  # noqa: BLE001
            pass
        time.sleep(step)
    return None


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def port_open(p):
    for host in ("127.0.0.1", "::1"):
        try:
            socket.create_connection((host, p), 0.5).close()
            return True
        except OSError:
            pass
    return False


# Each session's row as a person sees it: its state (the row's class), the word beside it, and the dot's tooltip.
ROWS_JS = """
return [...document.querySelectorAll('.session-row[data-row-session]')].map(r => ({
  id: r.dataset.rowSession,
  name: r.querySelector('.name').textContent,
  state: ['waiting', 'done', 'working', 'idle', 'error', 'exited'].find(c => r.classList.contains(c)) || '',
  label: (r.querySelector('.state-label') || {}).textContent || '',
  dot: ((r.querySelector('.dot') || {}).getAttribute && r.querySelector('.dot').getAttribute('title')) || '',
}));
"""


def scenario(run, d):
    w = run.d
    w.set_window_size(1280, 800)
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)
    d.call("agent.set_custom", {"agents": [FAKE]})

    def info(sid):
        return next((s for s in d.call("session.list") if s["id"] == sid), {})

    def row(sid):
        return next((r for r in w.js(ROWS_JS) if r["id"] == sid), None)

    sessions = {}
    for name, _, _ in CASES:
        s = d.call("session.create", {"kind": "fake", "cwd": WORK, "name": name, "project_id": pid})
        sessions[name] = s
    for name, s in sessions.items():
        check(f"{name}: the fake agent drew its input box", until(lambda: "Ask Fake anything" in json.dumps(d.call("session.read", {"handle": s["handle"]})), 30))
        check(f"{name}: its row is in the sidebar, not opened in a pane", until(lambda: row(s["id"]), 15) and not w.js(f"return {S}.paneLocation(arguments[0])", s["id"]))
    run.shot(f"{PREFIX}-01-start")

    seen = {}
    for name, typed, want in CASES:
        s = sessions[name]
        d.call("session.input", {"handle": s["handle"], "data": typed + "\r"})
        if typed != "tab":
            until(lambda: info(s["id"]).get("status") == "working", 10, 0.1)
        # The turn ends about 4 s after the prompt; then give the bell's read and the row time to settle.
        time.sleep(8 if typed != "tab" else 3)
        st = until(lambda: (lambda i: i.get("status") in ("idle", "waiting") and i)(info(s["id"])), 20) or info(s["id"])
        time.sleep(1.0)
        st = info(s["id"])
        r = until(lambda: (lambda x: x and x["state"] in ("idle", "waiting", "done") and x)(row(s["id"])), 10) or row(s["id"])
        prompt = d.call("session.read", {"handle": s["handle"]}).get("prompt")
        seen[name] = {"status": st.get("status"), "unread": st.get("unread"), "waiting_since": bool(st.get("waiting_since")), "keys": prompt and prompt.get("keys"), "row": r and r["state"], "label": r and r["label"], "dot": r and r["dot"]}
        print(f"  {name}: {json.dumps(seen[name], ensure_ascii=True)}", flush=True)
        run.shot(f"{PREFIX}-{name}")
        daemon_state = "waiting" if st.get("status") == "waiting" else ("done" if st.get("unread") else st.get("status"))
        check(f"{name}: the daemon says {want}", daemon_state == want, seen[name])
        check(f"{name}: the sidebar row shows {want}" + (" with the word 'needs you'" if want == "waiting" else ""),
              r and r["state"] == want and ((r["label"] == "needs you") == (want == "waiting")), r)
        if name == "2-decisions":
            # A question in prose: the wait a phone shows has no keys to offer.
            check(f"{name}: the wait carries no answer keys", seen[name]["keys"] is None, seen[name])
        if name == "3-menu":
            check(f"{name}: the menu's keys are offered", seen[name]["keys"] == ["1", "2"], seen[name])

    counts = {k: sum(1 for v in seen.values() if v["row"] == k) for k in ("waiting", "done", "idle")}
    print(json.dumps({"mode": PREFIX, "rows": counts}), flush=True)
    run.shot(f"{PREFIX}-06-all")
    # What the person sees on the case the owner reported, opened at the end (opening it marks it read).
    w.js(f"{S}.revealSession(arguments[0])", sessions["2-decisions"]["id"])
    time.sleep(1.5)
    run.shot(f"{PREFIX}-07-decisions-opened")
    w.js(f"{S}.revealSession(arguments[0])", sessions["1-summary"]["id"])
    time.sleep(1.5)
    run.shot(f"{PREFIX}-08-summary-opened")


def main():
    if port_open(1427):
        raise SystemExit("port 1427 is taken (another Vite dev server?): this script starts the app's own; stop that one first")
    d = ps.Daemon(kelpied, env={"KELPIE_E2E": "1"}).start()
    procs["daemon"] = d.proc
    print(f"daemon pid {d.proc.pid} endpoint {ENDPOINT}", flush=True)
    procs["vite"] = subprocess.Popen(["cmd", "/c", "pnpm", "exec", "vite", "--port", "1427", "--strictPort"] if os.name == "nt" else ["pnpm", "exec", "vite", "--port", "1427", "--strictPort"], cwd=os.path.join(REPO, "app"), stdout=open(os.path.join(scratch, "vite.log"), "w"), stderr=subprocess.STDOUT)
    until(lambda: port_open(1427), 60)
    driver_port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
    procs["driver"] = subprocess.Popen([TAURI_DRIVER, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", NATIVE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run = None
    try:
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(scratch, "shots"), app=app)
        run.boot()
        info = run.d.invoke("daemon_call", {"method": "daemon.info", "params": {}}).get("ok") or {}
        check("the app is connected to this script's daemon", info.get("pid") == d.proc.pid, info.get("pid"))
        if info.get("pid") != d.proc.pid:
            raise SystemExit("the app is not on this script's daemon: stopping before any scenario")
        scenario(run, d)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        if run is not None:
            run.d.quit()
        for name in ("driver", "vite"):
            p = procs.get(name)
            if p and p.poll() is None:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
                else:
                    p.terminate()
        assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
        d.stop()
        shutil.rmtree(WORK, ignore_errors=True)
        shutil.rmtree(DATA, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"mode": PREFIX, "passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(0 if BEFORE or not failed else 1)


if __name__ == "__main__":
    main()
