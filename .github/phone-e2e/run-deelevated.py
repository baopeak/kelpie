"""Runs one e2e script, its output to e2e-out/<name>.log and its exit code and duration to e2e-out/<name>.exit, as run-one.sh.

    python .github/phone-e2e/run-deelevated.py <name> <timeout-seconds> <command...>

By default the script runs as the runner's own user, in this process. With E2E_DEELEVATE=1 it runs as a standard local user (E2E_USER,
E2E_PASS, made by the workflow) instead: the app does not share the screen while it runs as administrator, and on the hosted runner (UAC
off) every process of the runner's user is elevated, `runas /trustlevel` included. Start-Process -Credential starts this file again in the
same desktop session as that user (`--child`), which takes this process's environment from a file, keeps its own profile's folders, and runs
the command; everything it starts (tauri-driver, the app, its daemon, the script's windows and browsers) then runs as that user.
"""
import json
import os
import subprocess
import sys
import time

# The user's own folders and identity: the child keeps its own.
OWN = {"USERNAME", "USERPROFILE", "HOMEDRIVE", "HOMEPATH", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "USERDOMAIN", "USERDOMAIN_ROAMINGPROFILE", "LOGONSERVER"}


def child(name):
    work = os.path.abspath("e2e-work")
    spec_path = os.path.join(work, f"deelevated-{name}.json")
    with open(spec_path, encoding="utf-8") as f:
        spec = json.load(f)
    os.remove(spec_path)
    env = {k: v for k, v in spec["env"].items() if k.upper() not in OWN}
    env.update({k: v for k, v in os.environ.items() if k.upper() in OWN})
    with open(spec["log"], "w", encoding="utf-8") as f:
        who = subprocess.run(["whoami", "/groups"], capture_output=True, text=True, errors="replace").stdout
        me = subprocess.run(["whoami"], capture_output=True, text=True).stdout.strip()
        groups = [l.split()[0] + " " + l.split()[1] for l in who.splitlines() if "Mandatory Label" in l or l.startswith("BUILTIN\\Administrators")]
        f.write(f"as {me}: {'; '.join(groups)}\n")
        f.flush()
        try:
            code = subprocess.run(spec["cmd"], stdout=f, stderr=subprocess.STDOUT, env=env, timeout=spec["timeout"]).returncode
        except subprocess.TimeoutExpired:
            code = 124
        except OSError as e:
            f.write(f"could not start: {e}\n")
            code = 127
    with open(spec["rc"], "w") as f:
        f.write(f"{code}\n")


if sys.argv[1] == "--child":
    child(sys.argv[2])
    sys.exit(0)

name, timeout, cmd = sys.argv[1], int(sys.argv[2]), sys.argv[3:]
out, work = os.path.abspath("e2e-out"), os.path.abspath("e2e-work")
os.makedirs(out, exist_ok=True)
os.makedirs(work, exist_ok=True)
log, rc = os.path.join(out, f"{name}.log"), os.path.join(work, f"{name}.rc")
for path in (log, rc):
    if os.path.exists(path):
        os.remove(path)
if os.environ.get("E2E_DEELEVATE") != "1":
    print(f"== {name}: {subprocess.list2cmdline(cmd)}", flush=True)
    began = time.time()
    with open(log, "w", encoding="utf-8") as f:
        try:
            code = subprocess.run(cmd, stdout=f, stderr=subprocess.STDOUT, timeout=timeout).returncode
        except subprocess.TimeoutExpired:
            code = 124
    print(open(log, encoding="utf-8", errors="replace").read()[-20000:], flush=True)
    with open(os.path.join(out, f"{name}.exit"), "w") as f:
        f.write(f"{code} {int(time.time() - began)}\n")
    print(f"== {name}: exit {code}", flush=True)
    sys.exit(code)

# The environment goes in a file the child deletes once read; never the password or the runner's tokens.
env = {k: v for k, v in os.environ.items() if not k.startswith("ACTIONS_") and k not in ("E2E_PASS", "E2E_DEELEVATE")}
with open(os.path.join(work, f"deelevated-{name}.json"), "w", encoding="utf-8") as f:
    json.dump({"env": env, "cmd": cmd, "log": log, "rc": rc, "timeout": timeout}, f)
args = subprocess.list2cmdline(["-X", "utf8", os.path.abspath(__file__), "--child", name])
ps = ("$c = New-Object System.Management.Automation.PSCredential($env:E2E_USER, (ConvertTo-SecureString $env:E2E_PASS -AsPlainText -Force)); "
      "Start-Process -FilePath $env:E2E_CHILD_EXE -ArgumentList $env:E2E_CHILD_ARGS -WorkingDirectory $env:E2E_CHILD_DIR -Credential $c -LoadUserProfile -WindowStyle Hidden")
print(f"== {name} (as {os.environ.get('E2E_USER')}): {subprocess.list2cmdline(cmd)}", flush=True)
began = time.time()
started = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps], capture_output=True, text=True,
                         env={**os.environ, "E2E_CHILD_EXE": sys.executable, "E2E_CHILD_ARGS": args, "E2E_CHILD_DIR": os.getcwd()})
if started.returncode:
    print(f"Start-Process failed: {started.stdout} {started.stderr}", flush=True)
shown = 0
while started.returncode == 0 and not os.path.exists(rc) and time.time() - began < timeout + 30:
    time.sleep(2)
    if os.path.exists(log):
        text = open(log, encoding="utf-8", errors="replace").read()
        print(text[shown:], end="", flush=True)
        shown = len(text)
time.sleep(1)
if os.path.exists(log):
    print(open(log, encoding="utf-8", errors="replace").read()[shown:], flush=True)
code = int(open(rc).read().strip() or 1) if os.path.exists(rc) else 124
with open(os.path.join(out, f"{name}.exit"), "w") as f:
    f.write(f"{code if os.path.exists(rc) else 'timeout'} {int(time.time() - began)}\n")
print(f"== {name}: exit {code}", flush=True)
sys.exit(code)
