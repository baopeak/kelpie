"""E2E for the SSH hosts page (Termius-style hosts, keys, known hosts, forwards, files).

    python -X utf8 scripts/e2e/ssh_hosts.py <screenshot-dir> <scratch-dir>

Start tauri-driver with KELPIE_E2E=1, an isolated KELPIE_DATA_DIR / KELPIE_ENDPOINT, KELPIE_SSH_DIR
(Kelpie's stand-in for ~/.ssh: keys and known_hosts the test creates never touch the real ones) and KELPIE_FILES_HOME
(the Files tab's home: its local pane starts there, never in the real Downloads); run this script with the same
KELPIE_SSH_DIR and KELPIE_FILES_HOME. Starts `ssh_test_server.py` on 127.0.0.1:47824 with
password login and its own $HOME (KELPIE_TEST_HOME), so "installing a key on the host" writes a
scratch authorized_keys.

Covers: saving a host with a password (credential store, never in files), the generated ssh
config, an unknown host key shown with fingerprints before trusting it, logging in with the saved
password (askpass), a terminal on the host in its remote project, files on the host (list, new
folder, upload, download), local / remote / SOCKS forwards carrying traffic and stopping, a new
key installed on the host and used to log in, known hosts, importing the SSH config, Claude Code
on the host.
"""
import http.server
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
PORT, WEB, LOCAL_FW, REMOTE_FW, SOCKS = 47824, 47830, 47831, 47832, 47833
PASSWORD = f"pw-{int(time.time())}"
ssh_dir = os.environ.get("KELPIE_SSH_DIR")
if not ssh_dir:
    raise SystemExit("set KELPIE_SSH_DIR (the same value tauri-driver was started with)")
# The Files tab's local pane must never be the real profile: the test writes there.
files_home = os.environ.get("KELPIE_FILES_HOME")
if not files_home or os.path.normcase(os.path.abspath(files_home)) == os.path.normcase(os.path.expanduser("~")):
    raise SystemExit("set KELPIE_FILES_HOME to a scratch folder (the same value tauri-driver was started with)")
os.makedirs(os.path.join(files_home, "Downloads"), exist_ok=True)
fwd = lambda p: p.replace("\\", "/")
BASH = os.environ.get("KELPIE_TEST_BASH", r"C:\Program Files\Git\bin\bash.exe")
# The test host is Git Bash: C:\x -> /c/x, and the temp folder is /tmp.
posix = lambda p: subprocess.run([BASH, "-c", 'cygpath -u "$1"', "_", p], capture_output=True, text=True).stdout.strip()

# The client's ~/.ssh stand-in: an importable host in its config, no keys or known hosts yet.
shutil.rmtree(ssh_dir, ignore_errors=True)
os.makedirs(ssh_dir)
if os.name == "nt":
    # Windows' OpenSSH refuses an included config (and keys) others may write: give the folder
    # what a real ~/.ssh has (the temp folder can carry extra entries).
    subprocess.run(["icacls", ssh_dir, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:(OI)(CI)F", "/grant:r", "SYSTEM:(OI)(CI)F", "/grant:r", "Administrators:(OI)(CI)F"], capture_output=True, check=True)
open(os.path.join(ssh_dir, "config"), "w").write("Host imported-box\n  HostName 127.0.0.1\n  Port 2299\n  User someone\n")
# The host's own $HOME and the key the server is started with (unused by the client).
home = os.path.join(scratch, "ssh-hosts-home")
shutil.rmtree(home, ignore_errors=True)
os.makedirs(os.path.join(home, ".ssh"))
server_dir = os.path.join(scratch, "ssh-hosts-server")
os.makedirs(server_dir, exist_ok=True)
server_key = os.path.join(server_dir, "id_unused")
if not os.path.exists(server_key):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", server_key], check=True)
server = subprocess.Popen([os.environ.get("KELPIE_SSH_TEST_PYTHON", sys.executable), os.path.join(os.path.dirname(__file__), "ssh_test_server.py"), str(PORT), server_key + ".pub", os.path.join(server_dir, "host_key")],
                          env=dict(os.environ, KELPIE_TEST_SSH_PASSWORD=PASSWORD, KELPIE_TEST_HOME=home), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
server.stdout.readline()

# A web server on this machine, reached through the forwards.
web_root = os.path.join(scratch, "ssh-hosts-web")
os.makedirs(web_root, exist_ok=True)
open(os.path.join(web_root, "hello.txt"), "w").write("hello through the tunnel")
handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=web_root, **k)
http.server.SimpleHTTPRequestHandler.log_message = lambda *a: None
web = http.server.ThreadingHTTPServer(("127.0.0.1", WEB), handler)
threading.Thread(target=web.serve_forever, daemon=True).start()

run = Run(shots)
d = None


def wait(fn, timeout=30, step=0.3):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:
            pass
        time.sleep(step)
    return None


def invoke(cmd, args=None):
    r = d.invoke(cmd, args or {})
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def fill(css, value):
    d.js("const el = document.querySelector(arguments[0]); el.value = arguments[1]; el.dispatchEvent(new Event('input', {bubbles: true}));", css, value)


def click_text(text, scope=".hosts-page"):
    ok = d.js("const b = [...document.querySelectorAll(arguments[1] + ' button')].find((b) => b.textContent.trim() === arguments[0] && !b.disabled); if (b) b.click(); return !!b;", text, scope)
    if not ok:
        raise RuntimeError(f"no button {text!r}")


def exists(css):
    return d.js("return !!document.querySelector(arguments[0])", css)


def text_of(css):
    return d.js("return document.querySelector(arguments[0])?.textContent ?? ''", css)


def get(url):
    with urllib.request.urlopen(url, timeout=5) as r:
        return r.read().decode()


def connects(port):
    try:
        socket.create_connection(("127.0.0.1", port), timeout=1).close()
        return True
    except OSError:
        return False


def open_hosts():
    d.js("window.__kelpie.commands.find((c) => c.id === 'hosts').run()")
    wait(lambda: exists(".hosts-page"), 10)
    time.sleep(0.5)  # the page reloads the vault when it opens


try:
    run.boot()
    d = run.d
    d.set_window_size(1480, 920)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    for h in invoke("ssh_vault_load")["hosts"]:
        invoke("ssh_secret_set", {"kind": "password", "target": h["id"], "value": ""})
    invoke("ssh_vault_save", {"vault": {"version": 1, "hosts": [], "forwards": []}})

    # ---- a host with a password ----
    open_hosts()
    run.shot("01-hosts-empty")
    d.click("[data-action=new-host]")
    fill("[data-field=label]", "Test box")
    fill("[data-field=user]", "tester")
    fill("[data-field=hostname]", "127.0.0.1")
    fill("[data-field=port]", str(PORT))
    d.click("[data-auth=password]")
    fill("[data-field=password]", PASSWORD)
    run.shot("02-host-editor")
    d.click("[data-action=save-host]")
    host = wait(lambda: next((h for h in invoke("ssh_vault_load")["hosts"] if h["label"] == "Test box"), None), 10)
    run.check("the host is saved with an alias from its name", host and host["alias"] == "test-box" and host["auth"] == "password", str(host))
    run.check("its password is in the credential store", invoke("ssh_secret_has", {"kind": "password", "target": host["id"]}))
    data_dir = d.js_async("const done = arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke('platform_info').then(i => done(i.data_dir));")
    files_text = open(os.path.join(data_dir, "ssh", "hosts.json")).read()
    config = open(invoke("ssh_paths")["config"]).read()
    run.check("the password is in no file", PASSWORD not in files_text and PASSWORD not in config)
    run.check("the generated ssh config has the host and includes the user's config",
              f"Host test-box\n  HostName 127.0.0.1\n  Port {PORT}\n  User tester" in config and f'Include "{fwd(os.path.join(ssh_dir, "config"))}"' in config, config)

    # ---- unknown host key: fingerprints first, then the saved password logs in ----
    d.click("[data-action=check]")
    run.check("an unknown host key asks for a fingerprint check", wait(lambda: exists("[data-trust] .trust-fp"), 30), text_of("[data-host-error]"))
    run.shot("03-trust-host-key")
    d.click("[data-action=trust]")
    check = wait(lambda: text_of("[data-host-check]"), 40)
    run.check("trusted: the saved password logs in (askpass)", check and ("MINGW" in check or "MSYS" in check or "Linux" in check), (check or "") + text_of("[data-host-error]"))
    known = open(os.path.join(ssh_dir, "known_hosts")).read()
    run.check("the host key went to known_hosts", f"[127.0.0.1]:{PORT}" in known)
    run.shot("04-host-checked")

    # ---- a terminal on the host ----
    d.click("[data-action=terminal]")
    sess = wait(lambda: next((s for s in daemon(d, "session.list") if (s.get("remote") or {}).get("dest") == "test-box" and s["alive"]), None), 30)
    run.check("a terminal starts on the host", sess is not None)
    project = d.js(f"return {S}.state.projects.find((p) => p.remote?.dest === 'test-box')")
    run.check("in a remote project for the host's folder", project and project["remote"]["path"] == posix(home), str(project and project.get("remote")))
    run.check("its ssh can ask Kelpie for the password", sess and (sess.get("env") or {}).get("SSH_ASKPASS", "").lower().endswith("kelpied.exe"), str(sess and sess.get("env")))
    time.sleep(2)
    daemon(d, "session.input", {"handle": sess["handle"], "data": "echo hosts-marker-$((40+2))\r"})
    run.check("commands run on the host", wait(lambda: "hosts-marker-42" in daemon(d, "session.read", {"handle": sess["handle"], "lines": 50})["text"], 30))
    run.shot("05-terminal-on-host")

    # ---- files on the host ----
    open_hosts()
    d.click("[data-host=test-box]")
    d.click("[data-action=files]")
    open(os.path.join(home, "existing.txt"), "w").write("on the host")
    # The host is the right pane; the local (left) one starts in KELPIE_FILES_HOME's Downloads, which this run owns.
    run.check("the local pane is the scratch home, not the real profile", wait(lambda: os.path.normcase(files_home) in os.path.normcase(d.js("return document.querySelector('[data-pane=left] [data-field=pane-path]')?.value || ''")), 20))
    run.check("files on the host are listed", wait(lambda: exists('[data-pane=right] [data-entry=".ssh"]') or exists('[data-pane=right] [data-entry="existing.txt"]'), 30))
    d.click("[data-pane=right] [data-action=new-folder]")
    fill("[data-pane=right] [data-field=ask]", "made-by-e2e")
    d.click("[data-pane=right] [data-action=ask-ok]")
    run.check("new folder on the host", wait(lambda: os.path.isdir(os.path.join(home, "made-by-e2e")) and exists('[data-pane=right] [data-entry="made-by-e2e"]'), 20))
    run.check("nothing was made on this computer", not os.path.exists(os.path.join(files_home, "Downloads", "made-by-e2e")))
    up = os.path.join(scratch, "upload-me.txt")
    open(up, "w").write("uploaded by kelpie")
    def xfer(job):
        jid = daemon(d, "xfer.start", job)["id"]
        return wait(lambda: next((j for j in daemon(d, "xfer.list")["jobs"] if j["id"] == jid and j["state"] in ("done", "failed", "cancelled")), None), 60)
    j = xfer({"src": {}, "dst": {"dest": "test-box"}, "items": [up], "dst_dir": posix(home) + "/made-by-e2e", "conflict": "replace"})
    run.check("upload (transfer queue)", j and j["state"] == "done" and open(os.path.join(home, "made-by-e2e", "upload-me.txt")).read() == "uploaded by kelpie", str(j))
    down = os.path.join(scratch, "downloads")
    shutil.rmtree(down, ignore_errors=True)
    os.makedirs(down)
    j = xfer({"src": {"dest": "test-box"}, "dst": {}, "items": [posix(home) + "/existing.txt"], "dst_dir": down, "conflict": "replace"})
    run.check("download (transfer queue)", j and j["state"] == "done" and open(os.path.join(down, "existing.txt")).read() == "on the host", str(j))
    run.shot("06-files")

    # ---- port forwards ----
    def add_forward(kind_label, bind_port, dest_port=WEB):
        d.click("[data-action=new-forward]")
        click_text(kind_label)
        fill("[data-field=bind-port]", str(bind_port))
        if dest_port:
            fill("[data-field=dest-host]", "127.0.0.1")
            fill("[data-field=dest-port]", str(dest_port))
        d.click("[data-action=save-forward]")
        f = wait(lambda: next((x for x in invoke("ssh_vault_load")["forwards"] if x["bindPort"] == bind_port), None), 10)
        d.js("document.querySelector(`[data-forward='${arguments[0]}'] [data-action=toggle-forward]`).click()", f["id"])
        wait(lambda: (invoke("ssh_forward_status").get(f["id"]) or {}).get("running"), 15)
        return f

    local = add_forward("Local (-L)", LOCAL_FW)
    run.check("a local forward carries traffic", wait(lambda: get(f"http://127.0.0.1:{LOCAL_FW}/hello.txt") == "hello through the tunnel", 15))
    remote = add_forward("Remote (-R)", REMOTE_FW)
    run.check("a remote forward carries traffic", wait(lambda: get(f"http://127.0.0.1:{REMOTE_FW}/hello.txt") == "hello through the tunnel", 15))
    socks = add_forward("SOCKS (-D)", SOCKS, dest_port=None)
    run.check("a SOCKS forward listens", wait(lambda: connects(SOCKS), 15))
    run.shot("07-forwards")
    d.js("document.querySelector(`[data-forward='${arguments[0]}'] [data-action=toggle-forward]`).click()", local["id"])
    run.check("stopping a forward frees its port", wait(lambda: not invoke("ssh_forward_status").get(local["id"], {}).get("running") and not connects(LOCAL_FW), 15))
    for f in (remote, socks):
        invoke("ssh_forward_stop", {"id": f["id"]})

    # ---- a new key, installed on the host, then used to log in ----
    d.click("[data-tab=keys]")
    d.click("[data-action=generate-key]")
    fill("[data-field=key-name]", "e2e_key")
    d.click("[data-action=create-key]")
    run.check("a key is generated and listed", wait(lambda: exists("[data-key=e2e_key]"), 20) and os.path.exists(os.path.join(ssh_dir, "e2e_key.pub")))
    run.shot("08-keychain")
    d.js("[...document.querySelectorAll('[data-key=e2e_key] button')].find((b) => b.textContent.includes('Install on host')).click()")
    click_text("Test box", "[data-key=e2e_key] .key-install")
    pub = open(os.path.join(ssh_dir, "e2e_key.pub")).read().split()[1]
    run.check("the key is installed on the host", wait(lambda: pub in open(os.path.join(home, ".ssh", "authorized_keys")).read(), 20))
    v = invoke("ssh_vault_load")
    for h in v["hosts"]:
        if h["id"] == host["id"]:
            h["auth"], h["identityFile"] = "key", os.path.join(ssh_dir, "e2e_key")
    invoke("ssh_vault_save", {"vault": v})
    invoke("ssh_secret_set", {"kind": "password", "target": host["id"], "value": ""})
    ok = invoke("remote_check", {"dest": "test-box", "agents": []})
    run.check("the host now logs in with the key alone", ok and ok.get("home"))

    # ---- known hosts ----
    d.js(f"{S}.setOverlay(null)")
    open_hosts()  # the page reloads the vault changed above
    d.click("[data-tab=known]")
    run.check("known hosts are listed with fingerprints", wait(lambda: "SHA256:" in text_of("[data-known]"), 10))
    run.shot("09-known-hosts")

    # ---- import the user's ssh config ----
    d.click("[data-tab=hosts]")
    click_text("Import")
    run.check("hosts import from the SSH config", wait(lambda: any(h["alias"] == "imported-box" and h["port"] == 2299 for h in invoke("ssh_vault_load")["hosts"]), 10))

    # ---- Claude Code on the host ----
    d.click("[data-host=test-box]")
    d.click("[data-action=check]")
    agents = wait(lambda: text_of("[data-host-check]"), 30) or ""
    # "Install Claude Code" names it too: only a host check that lists it as found starts it.
    if "Claude Code" in agents.replace("Install Claude Code", ""):
        d.click("[data-action=claude]")
        c = wait(lambda: next((s for s in daemon(d, "session.list") if s["kind"] == "claude" and (s.get("remote") or {}).get("dest") == "test-box"), None), 30)
        run.check("Claude Code starts on the host (in tmux when it has one)", c is not None, agents)
        if c:
            daemon(d, "session.kill", {"handle": c["handle"]})
    else:
        run.check("the host check offers to install Claude Code", "Install Claude Code" in d.js("return document.querySelector('.host-detail')?.textContent ?? ''"), agents)
    run.shot("10-host-detail")

    # ---- clean up ----
    for s in daemon(d, "session.list"):
        if (s.get("remote") or {}).get("dest") == "test-box":
            daemon(d, "session.close", {"handle": s["handle"]})
    for h in invoke("ssh_vault_load")["hosts"]:
        invoke("ssh_secret_set", {"kind": "password", "target": h["id"], "value": ""})
    invoke("ssh_vault_save", {"vault": {"version": 1, "hosts": [], "forwards": []}})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setOverlay(null);")
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    server.kill()
    web.shutdown()
    run.finish()
