"""E2E for remote projects over SSH.

    python -X utf8 scripts/e2e/remote.py <screenshot-dir> <scratch-dir>

Starts `ssh_test_server.py` (needs paramiko: set KELPIE_SSH_TEST_PYTHON to a Python that has it)
on 127.0.0.1:47823 with a fresh key, and a client config Kelpie is pointed at (Settings →
Remote: SSH config file). The "remote host" is this machine through Git Bash: the project
folder is a POSIX path (/c/Users/…), commands are bash, git is Git for Windows.

Covers: hosts from the SSH config, the Add remote project dialog (connect, host check, folder
browsing), the sidebar host label, branch info, file tree, viewing / editing / saving a file,
create / rename / delete, search in files, quick open, the Changes panel (status, stage,
commit), a terminal running on the host, chat view offered only for local sessions, resuming
the remote terminal after the daemon restarts, removing the project.
"""
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run, daemon, restart_daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
PORT = 47823
kelpied = os.path.join(REPO, "target", "release", "kelpied.exe" if os.name == "nt" else "kelpied")
ssh_dir = os.path.join(scratch, "ssh-remote-test")
os.makedirs(ssh_dir, exist_ok=True)
if os.name == "nt":
    # Kelpie includes this config from its own; Windows' OpenSSH refuses an included config others
    # may write (the temp folder can carry extra entries), as it does for ~/.ssh/config.
    subprocess.run(["icacls", ssh_dir, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:(OI)(CI)F", "/grant:r", "SYSTEM:(OI)(CI)F", "/grant:r", "Administrators:(OI)(CI)F"], capture_output=True)
key = os.path.join(ssh_dir, "id_test")
if not os.path.exists(key):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", key], check=True)
    if os.name == "nt":
        subprocess.run(["icacls", key, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:R"], capture_output=True)
cfg = os.path.join(ssh_dir, "config")
fwd = lambda p: p.replace("\\", "/")
with open(cfg, "w") as f:
    f.write(f"Host kelpie-remote\n  HostName 127.0.0.1\n  Port {PORT}\n  User tester\n  IdentityFile {fwd(key)}\n  IdentitiesOnly yes\n"
            f"  StrictHostKeyChecking no\n  UserKnownHostsFile {fwd(os.path.join(ssh_dir, 'known_hosts'))}\n  LogLevel ERROR\n")
server = subprocess.Popen([os.environ.get("KELPIE_SSH_TEST_PYTHON", sys.executable), os.path.join(os.path.dirname(__file__), "ssh_test_server.py"), str(PORT), key + ".pub", os.path.join(ssh_dir, "host_key")],
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
server.stdout.readline()  # "ssh test server on …"

# The project on the "host".
repo = os.path.join(scratch, "remote-repo")
if os.path.exists(repo):
    shutil.rmtree(repo, onexc=lambda f, p, e: (os.chmod(p, 0o666), f(p)))
os.makedirs(os.path.join(repo, "src"))
git = lambda *a: subprocess.run(["git", "-C", repo, *a], capture_output=True, text=True, check=True).stdout
subprocess.run(["git", "init", "-q", "-b", "main", repo], check=True)
git("config", "user.email", "e2e@kelpie.test")
git("config", "user.name", "Kelpie e2e")
open(os.path.join(repo, "README.md"), "w", newline="\n").write("# Remote\nhello\n")
open(os.path.join(repo, "src", "app.txt"), "w", newline="\n").write("the remote-needle is here\n")
git("add", ".")
git("commit", "-q", "-m", "init")
posix = "/" + repo[0].lower() + fwd(repo[2:])  # C:\x\y -> /c/x/y (Git Bash)

run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def inv(cmd, payload):
    r = d.invoke(cmd, payload)
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def wait(cond, timeout=30, interval=0.4):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def sess(sid):
    return next((s for s in daemon(d, "session.list") if s["id"] == sid), None)


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ sshConfig: arguments[0], shortcutHints: false, tips: false, resumeSessions: 'shown' }});", cfg)
    time.sleep(0.5)

    # ---------- hosts and the dialog ----------
    hosts = inv("remote_hosts", {})
    run.check("hosts come from the SSH config", "kelpie-remote" in hosts, str(hosts))
    d.js(f"{S}.setOverlay('remote')")
    d.wait("return !!document.querySelector('.remote-dialog input')", 5)
    d.js("const i = document.querySelector('.remote-dialog input'); i.value = 'kelpie-remote'; i.dispatchEvent(new InputEvent('input', {bubbles: true}));")
    d.js("[...document.querySelectorAll('.remote-dialog .btn.primary')].find(b => b.textContent.includes('Connect')).click()")
    ok = d.wait("return !!document.querySelector('.remote-check')", 30)
    check = d.js("return document.querySelector('.remote-check').textContent")
    run.check("Connect checks the host (OS, git)", bool(ok) and "MINGW" in check and "git ✓" in check, check)
    run.check("the host check lists agent CLIs found there", "agents:" in check and "Claude Code" in check, check)
    d.wait("return document.querySelectorAll('.remote-folder').length > 0", 20)
    run.check("folders of the host's home are listed", d.js("return document.querySelectorAll('.remote-folder').length") > 0)
    # Go to the repository's parent by typing it, then into the repository by clicking.
    parent = posix.rsplit("/", 1)[0]
    d.js("const i = document.querySelectorAll('.remote-dialog input.mono')[0]; i.value = arguments[0]; i.dispatchEvent(new InputEvent('input', {bubbles: true})); i.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}));", parent)
    d.wait("return [...document.querySelectorAll('.remote-folder')].some(f => f.textContent.trim() === 'remote-repo')", 20)
    d.js("[...document.querySelectorAll('.remote-folder')].find(f => f.textContent.trim() === 'remote-repo').click()")
    d.wait("return document.querySelector('.remote-dialog input.mono').value.endsWith('/remote-repo')", 20)
    run.shot("01-remote-dialog")
    d.js("[...document.querySelectorAll('.remote-dialog .btn.primary')].find(b => b.textContent.includes('Add project')).click()")
    p = wait(lambda: d.js(f"return {S}.state.projects.find(p => p.remote) ?? null"), 10)
    run.check("remote project added with its ssh root", p and p["path"] == f"ssh://kelpie-remote{posix}" and p["remote"]["dest"] == "kelpie-remote", str(p))
    root, pid = p["path"], p["id"]
    run.check("sidebar shows the host", bool(d.wait("return [...document.querySelectorAll('.remote-host')].some(e => e.textContent.includes('kelpie-remote'))", 10)))
    info = wait(lambda: d.js(f"return {S}.projectInfo[arguments[0]]?.is_repo ? {S}.projectInfo[arguments[0]] : null", pid), 30)
    run.check("branch info over ssh", info and info["branch"] == "main", str(info))

    # ---------- files ----------
    entries = inv("fs_list_dir", {"root": root, "path": ""})
    names = [(e["name"], e["is_dir"]) for e in entries]
    run.check("file tree lists the remote folder (folders first)", names[:1] == [("src", True)] and ("README.md", False) in names, str(names))
    d.js(f"{S}.showRightPanel('files')")
    run.check("explorer shows remote files", bool(d.wait("return [...document.querySelectorAll('.tree-row, .file-row, [data-path]')].some(e => (e.textContent || '').includes('README.md'))", 20)))
    content = inv("fs_read_file", {"root": root, "path": "README.md"})
    run.check("reading a remote file", content["text"] == "# Remote\nhello\n" and content["mtime"] > 0, str(content)[:200])
    w = inv("fs_write_file", {"root": root, "path": "README.md", "content": "# Remote\nhello again\n", "expectedMtime": content["mtime"]})
    run.check("saving a remote file", w["ok"] and open(os.path.join(repo, "README.md")).read() == "# Remote\nhello again\n")
    stale = inv("fs_write_file", {"root": root, "path": "README.md", "content": "x", "expectedMtime": content["mtime"] - 60000})
    run.check("a stale save is refused as a conflict", stale["conflict"] and open(os.path.join(repo, "README.md")).read() == "# Remote\nhello again\n")
    inv("fs_create", {"root": root, "path": "notes/todo.md", "dir": False})
    inv("fs_rename", {"root": root, "from": "notes/todo.md", "to": "notes/done.md"})
    run.check("create and rename on the host", os.path.isfile(os.path.join(repo, "notes", "done.md")))
    inv("fs_delete", {"root": root, "path": "notes"})
    run.check("delete on the host", not os.path.exists(os.path.join(repo, "notes")))
    escape = d.invoke("fs_read_file", {"root": root, "path": "../../outside.txt"})
    run.check("paths cannot leave the project folder", "err" in escape)

    # ---------- search / quick open ----------
    found = inv("search_text", {"root": root, "query": "remote-needle", "regex": False, "caseSensitive": False})
    run.check("search in files on the host", any(h["path"] == "src/app.txt" and h["line"] == 1 for h in found["hits"]), str(found)[:200])
    qo = inv("quick_open", {"root": root, "query": "app", "limit": 10})
    run.check("quick open lists remote files", any(h["path"] == "src/app.txt" for h in qo), str(qo)[:200])

    # ---------- git ----------
    st = inv("git_status", {"root": root})
    run.check("git status over ssh", st["is_repo"] and any(f["path"] == "README.md" for f in st["files"]), str(st)[:200])
    inv("git_stage", {"root": root, "paths": ["README.md"], "stage": True})
    inv("git_commit", {"root": root, "message": "Edit README remotely", "amend": False})
    run.check("stage and commit on the host", git("log", "-1", "--format=%s").strip() == "Edit README remotely")
    d.js(f"{S}.showRightPanel('git')")
    time.sleep(2)
    run.shot("02-remote-changes")

    # ---------- a terminal on the host ----------
    info = js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    sid = info["id"]
    s = sess(sid)
    run.check("the session runs over ssh in the remote folder", s["remote"] and s["remote"]["dest"] == "kelpie-remote" and s["cwd"] == posix and s["args"] == [] , str({k: s[k] for k in ("remote", "cwd")}))
    time.sleep(3)
    daemon(d, "session.input", {"handle": s["handle"], "data": "echo remote-$((40+2)) $(pwd)\r"})
    out = wait(lambda: (lambda t: t if "remote-42" in t else None)(daemon(d, "session.read", {"handle": sess(sid)["handle"], "lines": 50})["text"]), 30)
    run.check("commands run on the host, in the project folder", bool(out) and posix in out.replace("\\", "/"), (out or "")[-300:])
    run.shot("03-remote-terminal")
    run.check("chat view is not offered for remote sessions", not d.js("return !!document.querySelector('.pane .icon-btn[title=\"Open in chat view\"]')"))
    d.js(f"{S}.setOverlay('newsession')")
    d.wait("return !!document.querySelector('.modal')", 5)
    kinds = d.js("return [...document.querySelectorAll('.modal .launch-card')].map(c => c.textContent.trim())")
    run.check("new-session dialog offers the host's agents", any("Claude" in k for k in kinds), str(kinds))
    d.js(f"{S}.setOverlay(null)")

    # ---------- resume after the daemon restarts ----------
    restart_daemon(d, kelpied)
    resumed = wait(lambda: (sess(sid) or {}).get("alive"), 30)
    run.check("the remote terminal resumes after the daemon restarts", bool(resumed))
    time.sleep(3)
    daemon(d, "session.input", {"handle": sess(sid)["handle"], "data": "echo again-$((1+1))\r"})
    run.check("and runs on the host again", bool(wait(lambda: "again-2" in daemon(d, "session.read", {"handle": sess(sid)["handle"], "lines": 50})["text"], 30)))
    daemon(d, "session.close", {"handle": sess(sid)["handle"]})

    d.js(f"{S}.removeProject(arguments[0])", pid)
    run.check("remote project removed (files on the host untouched)", os.path.isfile(os.path.join(repo, "README.md")))
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    server.kill()
    run.finish()
