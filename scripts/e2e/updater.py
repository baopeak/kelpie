"""Updating Kelpie never stops the sessions; installers never touch a daemon they do not own.

    python scripts/e2e/updater.py <kelpied.exe>

Isolated daemon (own endpoint and data folder) — never the installed Kelpie:
1. `kelpied stop --wait` with three shells: returns only once the daemon process is gone, the
   shells are saved as running, and a new daemon brings them back interrupted (resumable).
2. `scripts/release/latest_json.py` writes the updater manifest from signed artifacts.
3. The installer's stop-daemon.ps1 (install mode) stops only programs directly in the install
   folder (a daemon from Kelpie 0.1.x): a same-named program elsewhere keeps running.
4. A daemon running from its copy (`<install dir>/daemon/<build>/`, as the app starts it) and
   its session keep running through an install: same pid, same session, output keeps arriving.
   Uninstall mode stops it.
"""
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

exe = os.path.abspath(sys.argv[1])
repo = os.path.join(os.path.dirname(__file__), "..", "..")
os.environ["KELPIE_ENDPOINT"] = f"kelpie-updater-test-{os.getpid()}"
os.environ["KELPIE_DATA_DIR"] = data = tempfile.mkdtemp(prefix="kelpie-updater-")
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


def start_daemon():
    proc = subprocess.Popen([exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(150):
        try:
            return proc, kc.Conn()
        except OSError:
            time.sleep(0.1)
    raise SystemExit("daemon did not start")


def wait_for(fn, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.3)
    return None


# 1. stop --wait keeps sessions resumable -------------------------------------------------------
proc, c = start_daemon()
try:
    cwd = tempfile.mkdtemp(prefix="kelpie-updater-cwd-")
    ids = [c.call("session.create", {"kind": "shell", "cwd": cwd, "name": f"upd-{i}"})["id"] for i in range(3)]
    wait_for(lambda: all(s["alive"] for s in c.call("session.list") if s["id"] in ids), 60)
    pid = c.call("daemon.info")["pid"]
    t0 = time.time()
    r = subprocess.run([exe, "stop", "--wait", "--timeout-ms", "20000"], capture_output=True, text=True, timeout=60)
    took = time.time() - t0
    check("`kelpied stop --wait` succeeds", r.returncode == 0, r.stderr)
    check("…and returns only after the daemon process is gone", proc.poll() is not None or not wait_for(lambda: proc.poll() is None, 0.1), f"pid {pid}, took {took:.1f}s")
    proc.wait(timeout=10)
    saved = json.load(open(os.path.join(data, "sessions.json"), encoding="utf-8"))
    entries = saved if isinstance(saved, list) else saved.get("sessions", [])
    alive = {e.get("spec", e).get("id"): e.get("alive") for e in entries}
    check("shells are saved as running", all(alive.get(i) for i in ids), alive)
finally:
    if proc.poll() is None:
        proc.kill()

proc, c = start_daemon()
try:
    listed = {s["id"]: s for s in c.call("session.list")}
    check("a new daemon brings them back interrupted", all(listed.get(i, {}).get("interrupted") for i in ids), {i: listed.get(i, {}).get("interrupted") for i in ids})
    r = subprocess.run([exe, "stop", "--wait"], capture_output=True, text=True, timeout=60)
    check("stop --wait on the restored daemon", r.returncode == 0 and proc.wait(timeout=10) is not None, r.stderr)
finally:
    if proc.poll() is None:
        proc.kill()
r = subprocess.run([exe, "stop", "--wait"], capture_output=True, text=True, timeout=30)
check("stop --wait with no daemon fails cleanly (the installer ignores it)", r.returncode != 0, r.returncode)

# 2. latest.json --------------------------------------------------------------------------------
art = tempfile.mkdtemp(prefix="kelpie-updater-art-")
win = os.path.join(art, "Kelpie_9.9.9_x64-setup.exe")
mac = os.path.join(art, "Kelpie.app.tar.gz")
for path, sig in ((win, "sig-win"), (mac, "sig-mac")):
    open(path, "wb").write(b"x")
    open(path + ".sig", "w").write(sig + "\n")
notes = os.path.join(art, "notes.md")
open(notes, "w", encoding="utf-8").write("Faster typing.\n")
out = subprocess.run([sys.executable, os.path.join(repo, "scripts", "release", "latest_json.py"), "--version", "v9.9.9",
                      "--base-url", "https://github.com/baopeak/kelpie/releases/download/v9.9.9/", "--notes-file", notes,
                      "--artifact", f"windows-x86_64={win}", "--artifact", f"darwin-aarch64={mac}"], capture_output=True, text=True)
try:
    m = json.loads(out.stdout)
    check("latest.json: version without the v", m["version"] == "9.9.9", m)
    check("latest.json: windows url and signature", m["platforms"]["windows-x86_64"] == {"signature": "sig-win", "url": "https://github.com/baopeak/kelpie/releases/download/v9.9.9/Kelpie_9.9.9_x64-setup.exe"}, m["platforms"])
    check("latest.json: macOS entry and notes", m["platforms"]["darwin-aarch64"]["signature"] == "sig-mac" and m["notes"] == "Faster typing.", m)
except Exception as e:  # noqa: BLE001
    check("latest.json parses", False, f"{e}: {out.stderr}")
os.remove(mac + ".sig")
r = subprocess.run([sys.executable, os.path.join(repo, "scripts", "release", "latest_json.py"), "--version", "1", "--base-url", "x", "--artifact", f"darwin-aarch64={mac}"], capture_output=True, text=True)
check("latest.json refuses an unsigned artifact", r.returncode != 0, r.stdout)
shutil.rmtree(art, ignore_errors=True)

# 3. installer script: legacy programs in the install folder only ------------------------------
ps1 = os.path.join(repo, "app", "src-tauri", "installer", "stop-daemon.ps1")


def run_ps1(inst, mode, env=None, data_dir=None):
    extra = ["-DataDir", data_dir] if data_dir else []
    return subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", ps1,
                           "-InstallDir", inst, "-Mode", mode, *extra], capture_output=True, text=True, timeout=180, env=env)


inst = tempfile.mkdtemp(prefix="kelpie-hook inst ")  # spaces, like a real profile path
other = tempfile.mkdtemp(prefix="kelpie-hook-other-")
copy_dir = os.path.join(inst, "daemon", "0.0.1-test")
os.makedirs(copy_dir)
ping = os.path.join(os.environ["SystemRoot"], "System32", "PING.EXE")
for d in (inst, other, copy_dir):
    shutil.copy(ping, os.path.join(d, "OpenConsole.exe"))
procs = {d: subprocess.Popen([os.path.join(d, "OpenConsole.exe"), "-n", "120", "127.0.0.1"], stdout=subprocess.DEVNULL) for d in (inst, other, copy_dir)}
time.sleep(1)
run_ps1(inst, "install")
time.sleep(0.5)
check("install: stops OpenConsole.exe directly in the install folder", procs[inst].poll() is not None)
check("install: leaves the daemon copy's console host running", procs[copy_dir].poll() is None)
check("install: leaves a same-named program elsewhere running", procs[other].poll() is None)
run_ps1(inst, "uninstall")
time.sleep(0.5)
check("uninstall: stops the daemon copy's console host too", procs[copy_dir].poll() is not None)
check("uninstall: still leaves programs elsewhere running", procs[other].poll() is None)
# A legacy daemon while the installer runs inside one of its sessions: refuse (exit 3), stop nothing.
shutil.copy(ping, os.path.join(inst, "kelpied.exe"))
legacy = subprocess.Popen([os.path.join(inst, "kelpied.exe"), "-n", "120", "127.0.0.1"], stdout=subprocess.DEVNULL)
time.sleep(1)
r = run_ps1(inst, "install", env=dict(os.environ, KELPIE_SESSION_ID="x"))
check("install from a legacy daemon's session: exit 3, nothing stopped", r.returncode == 3 and legacy.poll() is None, r.returncode)
for p in list(procs.values()) + [legacy]:
    if p.poll() is None:
        p.kill()
time.sleep(0.5)
shutil.rmtree(inst, ignore_errors=True)
shutil.rmtree(other, ignore_errors=True)

# Installed in a custom folder: the copies live in the data folder, outside the install folder.
inst = tempfile.mkdtemp(prefix="kelpie-hook custom ")
data_root = tempfile.mkdtemp(prefix="kelpie-hook-data ")
copy_dir = os.path.join(data_root, "daemon", "0.0.3-test")
os.makedirs(copy_dir)
shutil.copy(ping, os.path.join(copy_dir, "OpenConsole.exe"))
host = subprocess.Popen([os.path.join(copy_dir, "OpenConsole.exe"), "-n", "120", "127.0.0.1"], stdout=subprocess.DEVNULL)
time.sleep(1)
run_ps1(inst, "install", data_dir=data_root)
time.sleep(0.5)
check("custom folder, install: the copy in the data folder keeps running", host.poll() is None)
run_ps1(inst, "uninstall", data_dir=data_root)
time.sleep(0.5)
check("custom folder, uninstall: stops and removes the copies in the data folder", host.poll() is not None and not os.path.exists(os.path.join(data_root, "daemon")))
if host.poll() is None:
    host.kill()
time.sleep(0.5)
shutil.rmtree(inst, ignore_errors=True)
shutil.rmtree(data_root, ignore_errors=True)

# 4. sessions keep running through an install ---------------------------------------------------
inst = tempfile.mkdtemp(prefix="kelpie-upd inst ")
copy_dir = os.path.join(inst, "daemon", "0.0.2-test")
os.makedirs(copy_dir)
for name in ("kelpied.exe", "conpty.dll", "OpenConsole.exe"):
    src = os.path.join(os.path.dirname(exe), name)
    if os.path.isfile(src):
        shutil.copy(src, os.path.join(copy_dir, name))
shutil.copy(exe, os.path.join(inst, "kelpied.exe"))  # the installed (not running) daemon binary
copied = os.path.join(copy_dir, "kelpied.exe")
proc = subprocess.Popen([copied, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
c = None
for _ in range(150):
    try:
        c = kc.Conn()
        break
    except OSError:
        time.sleep(0.1)
try:
    cwd = tempfile.mkdtemp(prefix="kelpie-upd-cwd-")
    s1 = c.call("session.create", {"kind": "shell", "cwd": cwd, "name": "keeps-running"})
    wait_for(lambda: any(x["id"] == s1["id"] and x["status"] == "idle" for x in c.call("session.list")) and "PS " in c.call("session.read", {"handle": s1["handle"]})["text"], 150)
    pid = c.call("daemon.info")["pid"]
    r = run_ps1(inst, "install")
    check("install over a running copy: script succeeds", r.returncode == 0, r.stderr[-300:])
    info = c.call("daemon.info")
    check("install over a running copy: daemon still running (same pid)", proc.poll() is None and info["pid"] == pid)
    check("daemon.info names the copy it runs from", os.path.normcase(info.get("exe", "")) == os.path.normcase(copied), info.get("exe"))
    marker = f"still-here-{os.getpid()}"
    c.call("session.input", {"handle": s1["handle"], "data": f"echo {marker}\r"})
    seen = wait_for(lambda: marker in c.call("session.read", {"handle": s1["handle"], "lines": 40})["text"].split("echo")[-1], 60)
    alive = any(x["id"] == s1["id"] and x["alive"] for x in c.call("session.list"))
    check("install over a running copy: the session keeps running and answering", bool(seen) and alive)
    try:
        os.replace(os.path.join(inst, "kelpied.exe"), os.path.join(inst, "kelpied.old"))
        replaced = True
    except OSError:
        replaced = False
    check("install over a running copy: the install folder's daemon file is free to replace", replaced)
    r = run_ps1(inst, "uninstall")
    proc.wait(timeout=30)
    check("uninstall stops the running copy", proc.poll() is not None, r.stderr[-300:])
finally:
    if proc.poll() is None:
        proc.kill()
    time.sleep(0.5)
    shutil.rmtree(inst, ignore_errors=True)

# cleanup: this endpoint's auth token and data folder
try:
    os.remove(os.path.join(os.environ["LOCALAPPDATA"], "Kelpie", "auth", os.environ["KELPIE_ENDPOINT"] + ".token"))
except OSError:
    pass
shutil.rmtree(data, ignore_errors=True)

failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)}/{len(results)} passed")
sys.exit(1 if failed else 0)
