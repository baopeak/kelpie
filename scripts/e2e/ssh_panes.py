"""E2E for SSH panes in any project: a local project runs terminal panes on saved SSH hosts.

    python -X utf8 scripts/e2e/ssh_panes.py <screenshot-dir> <scratch-dir>

Start tauri-driver with KELPIE_E2E=1, an isolated KELPIE_DATA_DIR / KELPIE_ENDPOINT and KELPIE_SSH_DIR
(the same KELPIE_SSH_DIR is passed to this script). Starts `ssh_test_server.py` on 127.0.0.1:47844
with password login and its own $HOME (KELPIE_TEST_HOME) under <scratch-dir>/ssh-panes.

Covers: a host added from the quick-connect box (drawer prefilled, Enter and button), its password,
trust of its key, OS detected by a check; a local project's split → "Run on" row → host → shell in a
chosen folder (remote session, `.on-host` pane, host chip on pane and tab, commands run on the host);
a local shell beside it; a split of the remote pane preselects the host; linking the host to the
project with a folder; Tab cycling machines; a second project linking the same host with another
folder; unlinking keeps the host; hosts page "Open in → current project"; a saved layout with a
remote pane reopened on its host; the per-host terminal theme saved from the drawer.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
PORT = 47844
ALIAS, LABEL = "panes-box", "Panes box"
PASSWORD = f"pw-{int(time.time())}"
ssh_dir = os.environ.get("KELPIE_SSH_DIR")
if not ssh_dir:
    raise SystemExit("set KELPIE_SSH_DIR (the same value tauri-driver was started with)")
BASH = os.environ.get("KELPIE_TEST_BASH", r"C:\Program Files\Git\bin\bash.exe")
# The test host is Git Bash: C:\x -> /c/x.
posix = lambda p: subprocess.run([BASH, "-c", 'cygpath -u "$1"', "_", p], capture_output=True, text=True).stdout.strip()

base = os.path.join(scratch, "ssh-panes")
os.makedirs(base, exist_ok=True)
# The client's ~/.ssh stand-in (shared with ssh_hosts.py, which resets it): keep what is there, but
# this port's known_hosts entry must start unknown so the trust step is exercised.
os.makedirs(ssh_dir, exist_ok=True)
if os.name == "nt":
    subprocess.run(["icacls", ssh_dir, "/inheritance:r", "/grant:r", f"{os.environ['USERNAME']}:(OI)(CI)F", "/grant:r", "SYSTEM:(OI)(CI)F", "/grant:r", "Administrators:(OI)(CI)F"], capture_output=True, check=True)
config_path = os.path.join(ssh_dir, "config")
if not os.path.exists(config_path):
    open(config_path, "w").write("Host imported-box\n  HostName 127.0.0.1\n  Port 2299\n  User someone\n")
known_path = os.path.join(ssh_dir, "known_hosts")
if os.path.exists(known_path):
    kept = [l for l in open(known_path).read().splitlines(keepends=True) if f"[127.0.0.1]:{PORT}" not in l]
    open(known_path, "w").write("".join(kept))

# The host's own $HOME and its folders; the local projects.
home = os.path.join(base, "home")
shutil.rmtree(home, ignore_errors=True)
os.makedirs(os.path.join(home, ".ssh"))
shell_work, linked_a, linked_b = (os.path.join(home, n) for n in ("shell-work", "linked-a", "linked-b"))
for p in (shell_work, linked_a, linked_b):
    os.makedirs(p)
proj_a, proj_b = os.path.join(base, "proj-a"), os.path.join(base, "proj-b")
for p in (proj_a, proj_b):
    shutil.rmtree(p, ignore_errors=True)
    os.makedirs(p)
    open(os.path.join(p, "README.md"), "w").write("ssh panes\n")

server_dir = os.path.join(base, "server")
os.makedirs(server_dir, exist_ok=True)
server_key = os.path.join(server_dir, "id_unused")
if not os.path.exists(server_key):
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", server_key], check=True)
s = socket.socket()
if s.connect_ex(("127.0.0.1", PORT)) == 0:
    raise SystemExit(f"port {PORT} is in use: not ours to stop")
s.close()
server = subprocess.Popen([os.environ.get("KELPIE_SSH_TEST_PYTHON", sys.executable), os.path.join(os.path.dirname(__file__), "ssh_test_server.py"), str(PORT), server_key + ".pub", os.path.join(server_dir, "host_key")],
                          env=dict(os.environ, KELPIE_TEST_SSH_PASSWORD=PASSWORD, KELPIE_TEST_HOME=home), stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
server.stdout.readline()

run = Run(shots)
d = None
pid_a = pid_b = None
host_id = None
remote_sid = None


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


def call(expr, *args):
    """A store call that may return a promise: resolves to its value or raises."""
    r = d.js_async("const done = arguments[arguments.length-1]; Promise.resolve().then(() => " + expr + ").then(v => done({ok: v === undefined ? null : v}), e => done({err: String(e)}));", *args)
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def fill(css, value):
    d.js("const el = document.querySelector(arguments[0]); el.value = arguments[1]; el.dispatchEvent(new Event('input', {bubbles: true}));", css, value)


def set_select(css, value):
    d.js("const el = document.querySelector(arguments[0]); el.value = arguments[1]; el.dispatchEvent(new Event('change', {bubbles: true}));", css, value)


def exists(css):
    return d.js("return !!document.querySelector(arguments[0])", css)


def count(css):
    return d.js("return document.querySelectorAll(arguments[0]).length", css)


def text_of(css):
    return d.js("return document.querySelector(arguments[0])?.textContent ?? ''", css)


def click_text(text, scope=".hosts-page"):
    ok = d.js("const b = [...document.querySelectorAll(arguments[1] + ' button')].find((b) => b.textContent.trim() === arguments[0] && !b.disabled); if (b) b.click(); return !!b;", text, scope)
    if not ok:
        raise RuntimeError(f"no button {text!r}")


def open_hosts():
    # The "hosts" command toggles the page: run it only when the page is not open.
    if d.js(f"return {S}.overlay() !== 'hosts'"):
        d.js("window.__kelpie.commands.find((c) => c.id === 'hosts').run()")
    wait(lambda: exists(".hosts-page"), 10)
    time.sleep(0.5)  # the page reloads the vault when it opens


def vault_host():
    return next((h for h in invoke("ssh_vault_load")["hosts"] if h["alias"] == ALIAS), None)


def sessions():
    return daemon(d, "session.list")


def on_host(x):
    return (x.get("remote") or {}).get("dest") == ALIAS


def find_session(pred):
    return next((x for x in sessions() if pred(x)), None)


def read(handle, lines=80):
    return daemon(d, "session.read", {"handle": handle, "lines": lines})["text"]


def tabs_of(pid):
    return d.js(f"return {S}.workspace(arguments[0]).tabs.map(t => ({{id: t.id, root: t.root}}));", pid)


def leaves(node):
    return [node["pane"]] if node["type"] == "leaf" else [p for c in node["children"] for p in leaves(c)]


def panes_of(pid, tab_id):
    t = next((x for x in tabs_of(pid) if x["id"] == tab_id), None)
    return leaves(t["root"]) if t else []


def session_info(sid):
    return d.js(f"return {S}.sessions[arguments[0]] ?? null;", sid)


def remote_session_in(pid, tab_id):
    """The first remote session shown in a tab of `pid` (its session record), or None."""
    for p in panes_of(pid, tab_id):
        if p.get("sessionId"):
            info = session_info(p["sessionId"])
            if info and (info.get("remote") or {}).get("dest") == ALIAS:
                return info
    return None


def tab_with(pid, pred):
    return next((t["id"] for t in tabs_of(pid) if pred(t)), None)


def pick_other_host(label):
    d.click('[data-action=other-host]')
    wait(lambda: exists(".ctx-menu"), 5)
    ok = d.js("const it = [...document.querySelectorAll('.ctx-menu .ctx-item')].find((e) => e.textContent.trim().startsWith(arguments[0])); if (it) it.click(); return !!it;", label)
    if not ok:
        raise RuntimeError(f"no host {label!r} in Other host menu")


def step(name, fn):
    """Runs one scenario section; an unexpected error fails that section only."""
    try:
        fn()
    except Exception as e:
        traceback.print_exc()
        run.check(f"{name}: section completed", False, str(e)[:400])


try:
    run.boot()
    d = run.d
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    for x in sessions():
        daemon(d, "session.close", {"handle": x["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, layouts: [], resumeSessions: 'shown', hibernate: false, tips: false }});")
    for h in invoke("ssh_vault_load")["hosts"]:
        invoke("ssh_secret_set", {"kind": "password", "target": h["id"], "value": ""})
    invoke("ssh_vault_save", {"vault": {"version": 1, "hosts": [], "forwards": []}})

    # ---------- quick connect: a typed address opens the drawer prefilled ----------
    def quick_connect():
        open_hosts()
        fill("[data-field=quick-connect]", f"tester@127.0.0.1:{PORT}")
        run.check("typing user@host:port offers to add it", wait(lambda: exists("[data-action=add-typed-host]"), 5))
        d.click("[data-field=quick-connect]")
        d.keys("enter")
        wait(lambda: exists("[data-host-editor]"), 5)
        prefilled = d.js("const g = (f) => document.querySelector('[data-field=' + f + ']')?.value; return [g('user'), g('hostname'), g('port')];")
        run.check("Enter opens the host drawer prefilled", prefilled == ["tester", "127.0.0.1", str(PORT)], str(prefilled))
        run.check("the drawer keeps a sticky Connect", exists("[data-host-editor] [data-action=connect-host]") and exists("[data-host-editor] [data-action=save-host]"))
        d.keys("escape")
        run.check("Esc closes the drawer without saving", wait(lambda: not exists("[data-host-editor]"), 5) and not vault_host())
        d.click("[data-field=quick-connect]")
        click_btn = d.js("return !!document.querySelector('[data-action=quick-connect]') && !document.querySelector('[data-action=quick-connect]').disabled")
        run.check("the Connect button is enabled for a typed address", click_btn)
        d.click("[data-action=quick-connect]")
        wait(lambda: exists("[data-host-editor]"), 5)
        run.shot("01-quick-connect-drawer")
        fill("[data-field=label]", LABEL)
        d.click("[data-auth=password]")
        fill("[data-field=password]", PASSWORD)
        run.shot("02-host-drawer")
        d.click("[data-action=save-host]")
        host = wait(lambda: vault_host(), 10)
        global host_id
        host_id = host and host["id"]
        run.check("the host is saved from the drawer", bool(host) and host["label"] == LABEL, str(host))
        time.sleep(0.5)
        run.shot("02b-hosts-after-save")
        # Saved from the typed address: the box is cleared, so the new host is listed instead of "Add …".
        run.check("saving from the quick-connect box clears it", d.js("return document.querySelector('[data-field=quick-connect]').value === ''"))
        run.check("the saved host is listed in the hosts page", wait(lambda: d.js(f"const el = document.querySelector('.host-item[data-host={ALIAS}]'); return !!el && el.getBoundingClientRect().height > 0;"), 5))
        run.check("the add-typed row is gone after saving", not exists("[data-action=add-typed-host]"))
        run.check("its password is in the credential store", host and invoke("ssh_secret_has", {"kind": "password", "target": host["id"]}))
    step("quick connect", quick_connect)

    # ---------- trust the host key and check it: OS is recorded ----------
    def trust_and_check():
        open_hosts()
        # The quick-connect text stays after the drawer closes and would filter the list to nothing.
        fill("[data-field=quick-connect]", "")
        wait(lambda: exists(f".host-item[data-host={ALIAS}]"), 5)
        d.click(f".host-item[data-host={ALIAS}]")
        d.click("[data-action=check]")
        run.check("an unknown host key asks for a fingerprint check", wait(lambda: exists("[data-trust] .trust-fp"), 30))
        run.shot("03-trust-host-key")
        d.click("[data-action=trust]")
        check = wait(lambda: text_of("[data-host-check]"), 40)
        run.check("trusted: the host checks through its saved password", bool(check), text_of("[data-host-error]"))
        h = vault_host()
        run.check("the check records the host's OS", bool(h and h.get("os")), str(h and h.get("os")))
        run.shot("04-host-checked")
        d.js(f"{S}.setOverlay(null)")
    step("trust and check", trust_and_check)

    # ---------- a local project: split → Run on → host → shell in a folder ----------
    def remote_pane():
        global pid_a, remote_sid
        pid_a = d.js(f"return {S}.addProject(arguments[0])", proj_a)
        call(f"{S}.newSession(arguments[0], 'shell')", pid_a)
        wait(lambda: exists(".pane .term-host .xterm"), 20)
        d.click(".pane .xterm")
        d.keys("ctrl+shift+d")
        wait(lambda: exists(".empty-pane"), 5)
        run.check("split: the empty pane shows a Run on row", exists("[data-run-on]") and exists('[data-run-on-host=own]') and exists('[data-action=other-host]'))
        run.shot("05-launcher-run-on")
        pick_other_host(LABEL)
        run.check("Other host… offers the saved host as a chip", wait(lambda: exists(f'[data-run-on-host="{host_id}"].on'), 5))
        fill("[data-field=run-on-path]", posix(shell_work))
        run.check("the Terminal kind is offered for the host", wait(lambda: exists('.empty-pane .empty-pane-kind[title="Terminal"]'), 30))
        d.click('.empty-pane .empty-pane-kind[title="Terminal"]')
        sess = wait(lambda: find_session(lambda x: on_host(x) and x.get("alive")), 40)
        remote_sid = sess and sess["id"]
        run.check("the shell starts on the host", sess is not None)
        if not sess:
            return
        run.check("session: remote.dest is the host alias", on_host(sess), str(sess.get("remote")))
        run.check("session: cwd is the folder picked in the launcher", sess.get("cwd") == posix(shell_work), f"{sess.get('cwd')} vs {posix(shell_work)}")
        wait(lambda: exists(".pane.on-host .pane-header .host-chip"), 10)
        pane_host = d.js("const p = document.querySelector('.pane[data-pane-host]'); return p ? [p.dataset.paneHost, !!p.querySelector('.pane-header .host-chip')] : null;")
        run.check("pane: data-pane-host and a header host chip", pane_host == [ALIAS, True], str(pane_host))
        tab_chip = wait(lambda: exists(".tabs [data-tab-id] .host-chip"), 5)
        run.check("tab title shows the host chip", bool(tab_chip))
        daemon(d, "session.input", {"handle": sess["handle"], "data": "echo ssh-panes-$((20+3))-$(pwd)\r"})
        out = wait(lambda: "ssh-panes-23-" in read(sess["handle"]), 30) and read(sess["handle"])
        line = next((l for l in (out or "").splitlines() if "ssh-panes-23-" in l and "$(pwd)" not in l), "")
        run.check("commands run on the host, in its folder", line.endswith("shell-work"), line[-120:])
        run.shot("06-mixed-tab")
    step("remote pane from a split", remote_pane)

    # ---------- a local shell next to it; a split of the remote pane preselects the host ----------
    def mixed_tab():
        tab = tab_with(pid_a, lambda t: remote_session_in(pid_a, t["id"]) is not None)
        panes = panes_of(pid_a, tab)
        run.check("the local pane and the remote pane share a tab", len(panes) >= 2 and remote_sid is not None)
        d.js("document.querySelector('.pane.on-host .pane-header .icon-btn[title^=\"Split right\"]').click()")
        wait(lambda: exists(".empty-pane"), 5)
        run.check("a split of the remote pane preselects its host", wait(lambda: exists(f'.empty-pane [data-run-on-host="{host_id}"].on'), 5))
        run.shot("07-split-preselects-host")
        before = {x["id"] for x in sessions()}
        d.click('.empty-pane [data-run-on-host="own"]')
        d.click('.empty-pane .empty-pane-kind[title="Terminal"]')
        local = wait(lambda: find_session(lambda x: x["id"] not in before and not x.get("remote") and x.get("alive")), 20)
        run.check("a local shell starts next to the remote pane", local is not None and local.get("project_id") == pid_a, str(local and local.get("cwd")))
        run.check("both kinds sit in the same tab", len(panes_of(pid_a, tab)) >= 3)
    step("mixed tab", mixed_tab)

    # ---------- link the host to the project with a folder, from the launcher ----------
    def link_from_launcher():
        d.click(".pane:not(.on-host) .xterm")
        d.keys("ctrl+shift+d")
        wait(lambda: exists(".empty-pane"), 5)
        pick_other_host(LABEL)
        wait(lambda: exists(f'.empty-pane [data-run-on-host="{host_id}"].on'), 5)
        fill("[data-field=run-on-path]", posix(linked_a))
        d.click('.empty-pane [data-action=link-host]')
        linked = wait(lambda: d.js(f"const p = {S}.state.projects.find(x => x.id === arguments[0]); return (p?.hosts ?? []).find(l => l.hostId === arguments[1])?.path ?? null;", pid_a, host_id), 5)
        run.check("linking saves the host and folder on the project", linked == posix(linked_a), str(linked))
        d.click(".empty-pane-title")
        wait(lambda: exists('.empty-pane [data-run-on-host="own"]'), 5)
        d.keys("tab")
        run.check("Tab moves to the next machine", wait(lambda: exists('.empty-pane [data-run-on-host="own"].on'), 3))
        d.keys("tab")
        run.check("Tab cycles back to the linked host", wait(lambda: exists(f'.empty-pane [data-run-on-host="{host_id}"].on'), 3))
        d.click(f'.empty-pane [data-run-on-host="{host_id}"]')
        d.click('.empty-pane .empty-pane-kind[title="Terminal"]')
        sess = wait(lambda: find_session(lambda x: on_host(x) and x.get("alive") and x.get("cwd") == posix(linked_a)), 40)
        run.check("a new pane starts in the linked folder", sess is not None)
        run.shot("08-linked-folder-pane")
    step("link from launcher", link_from_launcher)

    # ---------- second project, same host, another folder; unlink keeps the host ----------
    def two_projects():
        global pid_b
        pid_b = d.js(f"return {S}.addProject(arguments[0])", proj_b)
        call(f"{S}.switchProject(arguments[0])", pid_b)
        open_hosts()
        d.click(f".host-item[data-host={ALIAS}]")
        fill("[data-field=link-folder]", posix(linked_b))
        d.click("[data-action=link-current]")
        b_path = wait(lambda: d.js(f"const p = {S}.state.projects.find(x => x.id === arguments[0]); return (p?.hosts ?? []).find(l => l.hostId === arguments[1])?.path ?? null;", pid_b, host_id), 5)
        run.check("a second project links the same host with its own folder", b_path == posix(linked_b), str(b_path))
        rows = d.js("return [...document.querySelectorAll('[data-host-link]')].map(e => e.dataset.hostLink);")
        run.check("the host lists both projects", sorted(rows) == sorted([pid_a, pid_b]), str(rows))
        run.shot("09-host-used-in-projects")
        d.js(f"const s = {S}; s.setOverlay(null);")
        open_hosts()
        d.click(f".host-item[data-host={ALIAS}]")
        d.js("[...document.querySelectorAll('[data-host-link]')].find((e) => e.dataset.hostLink === arguments[0]).querySelector('button').click()", pid_b)
        unlinked = wait(lambda: d.js(f"return (({S}.state.projects.find(x => x.id === arguments[0])?.hosts) ?? []).length === 0;", pid_b), 5)
        run.check("unlinking removes that project's link", bool(unlinked))
        run.check("the host stays in the vault after unlinking", vault_host() is not None)
        still = d.js(f"return (({S}.state.projects.find(x => x.id === arguments[0])?.hosts) ?? []).map(l => l.hostId)", pid_a)
        run.check("the other project keeps its link", still == [host_id], str(still))
    step("two projects", two_projects)

    # ---------- hosts page: Open in → current project ----------
    def open_in_current():
        open_hosts()
        d.click(f".host-item[data-host={ALIAS}]")
        run.check("'Open in' offers the current project when it is not linked", wait(lambda: exists('[data-open-in="current"]'), 5))
        d.click('[data-open-in="current"]')
        d.click('[data-action=terminal]')
        sess = wait(lambda: find_session(lambda x: on_host(x) and x.get("alive") and x.get("project_id") == pid_b), 40)
        run.check("the terminal opens in the local project, not a new remote project", sess is not None)
        remotes = d.js(f"return {S}.state.projects.filter(p => p.remote?.dest === arguments[0]).length", ALIAS)
        run.check("no remote project was created for the host", remotes == 0, str(remotes))
        run.check("the local project is still the active one", d.js(f"return {S}.state.activeProject") == pid_b)
        d.js(f"{S}.setOverlay(null)")
    step("open in current", open_in_current)

    # ---------- a saved layout with a remote pane reopens it on the host ----------
    def layout_round_trip():
        tab = tab_with(pid_a, lambda t: remote_session_in(pid_a, t["id"]) is not None)
        call(f"{S}.switchProject(arguments[0])", pid_a)
        d.js(f"{S}.saveLayout(arguments[0], arguments[1], 'Remote pair')", pid_a, tab)
        layout = d.js(f"return ({S}.state.settings.layouts ?? []).find(l => l.name === 'Remote pair') ?? null;")
        run.check("the saved layout remembers the host and folder of its remote pane", layout is not None and f'"dest": "{ALIAS}"' in json.dumps(layout), json.dumps(layout)[:600])
        before = tabs_of(pid_a)
        call(f"{S}.applyLayout(arguments[0], {S}.state.settings.layouts.find(l => l.name === 'Remote pair').template)", pid_a)
        new_tab = wait(lambda: next((t["id"] for t in tabs_of(pid_a) if t["id"] not in [x["id"] for x in before]), None), 10)
        # Every remote pane of the layout comes back on the host, in the folder it had: the one
        # matching the project's usual folder for the host opens in that linked folder.
        def remote_cwds(tab_id):
            out = []
            for p in panes_of(pid_a, tab_id):
                info = session_info(p["sessionId"]) if p.get("sessionId") else None
                if info and on_host(info):
                    out.append(info.get("cwd"))
            return sorted(out)
        want = sorted([posix(shell_work), posix(linked_a)])
        got = wait(lambda: (lambda c: c if len(c) == 2 else None)(remote_cwds(new_tab)) if new_tab else None, 40)
        run.check("applying the layout reopens each remote pane on the host in its folder", got is not None and [x.rstrip("/") for x in got] == [x.rstrip("/") for x in want], f"{got} vs {want}")
        run.shot("10-layout-reopened")
        d.js(f"{S}.deleteLayout(arguments[0])", (layout or {}).get("id"))
    step("layout round trip", layout_round_trip)

    # ---------- per-host terminal theme from the drawer ----------
    def theme():
        open_hosts()
        d.click(f".host-item[data-host={ALIAS}]")
        click_text("Edit")
        wait(lambda: exists("[data-host-editor]"), 5)
        d.js("[...document.querySelectorAll('[data-host-editor] summary')].find((s) => s.textContent.includes(arguments[0])).click();", "Appearance")
        options = d.js("return [...document.querySelectorAll('[data-field=terminal-theme] option')].map(o => o.value).filter(Boolean);")
        run.check("the Appearance section lists terminal themes", len(options) >= 2, str(options[:3]))
        if options:
            set_select("[data-field=terminal-theme]", options[1])
            run.shot("11-hosts-drawer-appearance")
            d.click("[data-action=save-host]")
            saved = wait(lambda: (vault_host() or {}).get("terminalTheme") == options[1], 10)
            run.check("the per-host terminal theme is stored in the vault", bool(saved), str((vault_host() or {}).get("terminalTheme")))
    step("terminal theme", theme)

    # ---------- clean up ----------
    for x in sessions():
        if on_host(x) or x.get("project_id") in (pid_a, pid_b):
            daemon(d, "session.close", {"handle": x["handle"]})
    for h in invoke("ssh_vault_load")["hosts"]:
        invoke("ssh_secret_set", {"kind": "password", "target": h["id"], "value": ""})
    invoke("ssh_vault_save", {"vault": {"version": 1, "hosts": [], "forwards": []}})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setOverlay(null);")
except Exception:
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    server.kill()
    run.finish()
