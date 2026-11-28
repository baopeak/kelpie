"""The daemon samples the machine and each session's resources on its own, and learns what a command costs
(crates/kelpied/src/resources.rs, API 51).

    python scripts/e2e/resources_monitor.py <kelpied.exe>

Daemon only, on an isolated endpoint and data folder (never the user's daemon). A shell session runs a program
whose cost is known by construction: it holds 600 MiB, keeps two processes spinning and writes 256 MiB to disk for about
12 seconds. Around it the test sends the two hook reports Claude Code sends for a shell command (`PreToolUse` and
`PostToolUse` on `Bash`). It checks:
- the monitor samples with no app asking, and counts no process scan;
- every gauge the machine can tell is known;
- the session's memory and disk traffic are seen while the program runs, within 15 % of what it takes, and its CPU
  at least 85 % of what its workers report they got (on a shared machine that is less than two cores);
- the ledger learned the command with its memory, CPU and duration;
- the ledger file is written in the data folder.
"""
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time

exe = sys.argv[1]
os.environ["KELPIE_ENDPOINT"] = f"kelpie-res-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = data_dir = tempfile.mkdtemp(prefix="kelpie-res-")
os.environ["KELPIE_E2E"] = "1"
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN"):
    os.environ.pop(k, None)

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(os.path.dirname(__file__), "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

results = []
HOLD_MB, CORES, WRITE_MB, SECONDS = 600, 2, 256, 12


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


HOG = f"""
import multiprocessing, os, sys, time
def spin(until, used):
    while time.time() < until:
        pass
    used.put(time.process_time())
if __name__ == "__main__":
    began = time.time()
    until = began + {SECONDS}
    used = multiprocessing.Queue()
    workers = [multiprocessing.Process(target=spin, args=(until, used)) for _ in range({CORES})]
    for w in workers:
        w.start()
    held = bytearray({HOLD_MB} * 1024 * 1024)
    for i in range(0, len(held), 4096):
        held[i] = 1
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "written.bin")
    chunk = os.urandom(1024 * 1024)
    with open(path, "wb") as f:
        for _ in range({WRITE_MB}):
            f.write(chunk)
        f.flush()
        os.fsync(f.fileno())
    while time.time() < until:
        time.sleep(0.2)
    cpu = sum(used.get() for _ in workers)
    for w in workers:
        w.join()
    os.remove(path)
    # What the workers really got of the machine: CPU seconds over the time they spun.
    print("HOG-CPU %.2f" % (cpu / (time.time() - began)))
    print("HOG-DONE")
"""

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
    info = c.call("daemon.info")
    check("the daemon speaks API 51", info.get("api", 0) >= 51, info.get("api"))

    # 1. Always on: samples come with nobody asking, and none of them is a process scan.
    first = wait_for(lambda: c.call("daemon.info")["resource_samples"] >= 2, 30)
    info = c.call("daemon.info")
    check("the monitor samples with no app asking", bool(first), info.get("resource_samples"))
    check("…and makes no process scan", info.get("process_scans") == 0, info.get("process_scans"))
    snap = c.call("resources.get")
    gauges = snap["gauges"]
    check("cpu, ram, disk and space are known", all(gauges.get(k) in ("ok", "busy", "saturated") for k in ("cpu", "ram", "disk", "space")), gauges)
    m = snap["machine"]
    check("the machine reading has memory, commit, a disk and the data folder's volume",
          m["mem_total_mb"] and m["commit_limit_mb"] and m["disks"] and any(os.path.normcase(v["path"]) == os.path.normcase(data_dir) for v in m["volumes"]),
          {k: m[k] for k in ("mem_total_mb", "commit_limit_mb")})
    print("  gpu:", gauges.get("gpu"), [(g["id"][-12:], g["util_pct"]) for g in m["gpus"]], flush=True)

    # 2. A command of known cost, between the hook reports Claude Code sends for it.
    cwd = tempfile.mkdtemp(prefix="kelpie-res-cwd-")
    with open(os.path.join(cwd, "hog.py"), "w", encoding="utf-8") as f:
        f.write(HOG)
    s = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": "res-hog", "project_id": "res-proj"})
    sid, handle = s["id"], s["handle"]
    screen = lambda: c.call("session.read", {"handle": handle, "lines": 40})["text"]
    status = lambda: next((x["status"] for x in c.call("session.list") if x["id"] == sid), None)
    wait_for(lambda: "PS " in screen() and status() == "idle", 150)
    command = f'"{sys.executable}" -I hog.py'
    hook = lambda event, extra=None: c.call("hook.report", {"session_id": sid, "agent": "claude", "payload": {
        "hook_event_name": event, "session_id": "conv-res", "tool_name": "Bash", "tool_use_id": "toolu_res_1", "tool_input": {"command": command}, **(extra or {})}})
    hook("PreToolUse")
    c.call("session.input", {"handle": handle, "data": f"& {command}\r"})
    seen = {"private_mb": 0.0, "cores": 0.0, "io_mbps": 0.0, "running": None}

    def watch():
        snap = c.call("resources.get", {"session": sid})
        for sess in snap["sessions"]:
            now = sess["now"]
            seen["private_mb"] = max(seen["private_mb"], now["private_mb"])
            seen["cores"] = max(seen["cores"], now["cores"])
            seen["io_mbps"] = max(seen["io_mbps"], now["io_mbps"])
            if sess["commands"]:
                seen["running"] = sess["commands"][0]
        return "HOG-DONE" in screen()

    done = wait_for(watch, SECONDS + 60, 1.0)
    hook("PostToolUse", {"tool_response": {"stdout": "HOG-DONE"}})
    check("the program ran to its end", bool(done), screen()[-300:])
    check(f"the session's memory reached what the program holds (~{HOLD_MB} MiB)", seen["private_mb"] >= HOLD_MB * 0.85, round(seen["private_mb"]))
    import re
    got = float((re.search(r"HOG-CPU ([0-9.]+)", screen()) or [0, "0"])[1])
    print(f"  the workers got {got} cores of the machine (it has others' work too)", flush=True)
    check("its CPU reached what the workers really got", got > 0 and seen["cores"] >= got * 0.85, f"{round(seen['cores'], 2)} vs {got}")
    check("its disk writes were seen", seen["io_mbps"] >= 5, round(seen["io_mbps"], 1))
    run = seen["running"] or {}
    check("the command was listed as running, by its fingerprint", run.get("fingerprint") == "python", run)

    # 3. The ledger learned it.
    learned = wait_for(lambda: c.call("resources.ledger", {"project": "res-proj"})["commands"], 10)
    e = (learned or [{}])[0]
    print("  learned:", json.dumps({k: (round(v, 2) if isinstance(v, float) else v) for k, v in e.items()}), flush=True)
    check("the ledger learned the command once", e.get("fingerprint") == "python" and e.get("runs") == 1, e.get("runs"))
    check(f"…with its memory within 15 % of {HOLD_MB} MiB", abs(e.get("ram_mb", 0) - HOLD_MB) <= HOLD_MB * 0.15, round(e.get("ram_mb", 0)))
    check("…its peak CPU at least what the workers got", e.get("peak_cores", 0) >= got * 0.85, f"{round(e.get('peak_cores', 0), 2)} vs {got}")
    check("…sampled at the working pace from the start (a sample every 2 s or so)", run.get("samples", 0) >= SECONDS // 3, run.get("samples"))
    check(f"…its disk traffic at least the {WRITE_MB} MiB written", e.get("io_mb", 0) >= WRITE_MB * 0.85, round(e.get("io_mb", 0)))
    check(f"…and its duration (~{SECONDS} s)", SECONDS * 0.7 <= e.get("secs", 0) <= SECONDS + 40, e.get("secs"))
    ledger_file = os.path.join(data_dir, "command-ledger.json")
    written = wait_for(lambda: os.path.exists(ledger_file), 45, 1.0)
    check("the ledger is saved in the data folder", bool(written))
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
