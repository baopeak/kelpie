"""E2E for plugins (Settings → Plugins).

    python -X utf8 scripts/e2e/plugins.py <screenshot-dir> <scratch-dir> [--port 47813]

Installs a test plugin from a folder and from a git repository (served over plain http on
127.0.0.1), checks that nothing it declares is used before the user reviews and enables it,
then every contribution: a plugin agent (launched, in the daemon's table, kept for restarts),
palette commands (terminal and split, with variables), a quick command, a terminal theme, a
layout (from the layout menu), status items (output and failure). Editing the manifest turns
the plugin off until it is reviewed again; a stale approval is refused; disabling and removing
withdraw everything. Also: invalid manifests, creating a plugin, settings search.
"""
import json
import stat
import os
import shutil
import subprocess
import sys
import threading
import time
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
port = int(sys.argv[sys.argv.index("--port") + 1]) if "--port" in sys.argv else 47813
S = "window.__kelpie.store"
P = "window.__kelpie.plugins"
proj = os.path.join(scratch, "plugproj")
src = os.path.join(scratch, "plugin-src", "acme-tools")


def rmtree(path):
    # Git marks its object files read-only.
    shutil.rmtree(path, onexc=lambda f, p, e: (os.chmod(p, stat.S_IWRITE), f(p)))


for d_ in (proj, src):
    if os.path.exists(d_):
        rmtree(d_)
    os.makedirs(d_)
with open(os.path.join(proj, "notes.txt"), "w") as f:
    f.write("hello\n")
subprocess.run(["git", "init", "-q", "-b", "plug-branch"], cwd=proj, check=True)

COLORS = ["#101418", "#d0d0d0", "#ffcc00"] + ["#%02x%02x%02x" % (i * 15, 80, 200 - i * 10) for i in range(16)]
MANIFEST = {
    "id": "acme-tools",
    "name": "Acme Tools",
    "version": "1.2.0",
    "description": "Test plugin",
    "contributes": {
        "agents": [{"id": "echo", "label": "Acme Echo", "bin": "python", "launch": ["-c", "print('ACME AGENT READY'); input()"]}],
        "commands": [
            {"id": "hello", "label": "Say hello", "run": "echo HELLO-${projectName}-${branch}", "in": "terminal"},
            {"id": "split", "label": "Hello in a split", "run": "echo SPLIT-${projectName}", "in": "split"},
        ],
        "quickCommands": [{"label": "Acme list", "text": "echo ACME-QC", "action": "terminal"}],
        "terminalThemes": [{"id": "mono", "label": "Acme Mono", "dark": True, "colors": COLORS}],
        "layouts": [{"name": "Acme trio", "template": {"type": "split", "dir": "row", "sizes": [0.5, 0.5], "children": [
            {"type": "leaf", "kind": "shell"},
            {"type": "split", "dir": "col", "sizes": [0.5, 0.5], "children": [{"type": "leaf", "kind": "shell"}, {"type": "leaf", "kind": None}]},
        ]}}],
        "statusItems": [
            {"id": "ok", "label": "Acme", "command": "echo STATUS-OK", "intervalSeconds": 5},
            {"id": "bad", "command": "exit 3", "intervalSeconds": 5},
        ],
    },
}


def write_manifest(folder, m):
    with open(os.path.join(folder, "kelpie-plugin.json"), "w", encoding="utf-8") as f:
        json.dump(m, f, indent=2)


write_manifest(src, MANIFEST)

run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def wait(cond, timeout=20, interval=0.3):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def screen(handle, lines=60):
    return daemon(d, "session.read", {"handle": handle, "lines": lines})["text"]


def listed():
    return js_await(f"{P}.pluginList()")


def info(pid):
    return next((p for p in listed() if p["id"] == pid), None)


def plugin_command_ids():
    return d.js("return window.__kelpie.commands.filter(c => c.id.startsWith('plugin.')).map(c => c.id)")


def presets():
    return {p["id"]: p for p in daemon(d, "agent.presets")}


def settings_page():
    d.js(f"{S}.openSettings('plugins')")
    d.wait("return !!document.querySelector('[data-setting=\"plugins\"]')", 10)


def card(pid):
    return f'[data-plugin="{pid}"]'


def card_text(pid):
    return d.js("const c = document.querySelector(arguments[0]); return c ? c.innerText : null", card(pid))


def click_button(container_css, label):
    return d.js("""const c = document.querySelector(arguments[0]); if (!c) return false;
        const b = [...c.querySelectorAll('button')].find(b => b.innerText.trim() === arguments[1]); if (!b || b.disabled) return false; b.click(); return true;""", container_css, label)


def active_sessions():
    return d.js(f"const s = {S}; const p = s.state.activeProject; const t = s.activeTab(p); if (!t) return []; return window.__kelpie.layout.allPanes(t.root).map(x => x.sessionId ? s.sessions[x.sessionId] : null).filter(Boolean)")


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    for p in listed():
        js_await(f"{P}.pluginRemove(arguments[0])", p["id"])
    js_await(f"{P}.loadPlugins()")
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ terminalTheme: undefined, shortcutHints: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)
    data_dir = d.js_async("const done = arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke('platform_info').then(i => done(i.data_dir));")
    plugins_dir = js_await(f"{P}.pluginDir()")
    run.check("plugins folder is in the data folder", os.path.normcase(os.path.dirname(plugins_dir)) == os.path.normcase(os.path.normpath(data_dir)) and os.path.isdir(plugins_dir), plugins_dir)
    run.check("no plugins at the start", listed() == [])

    # ---------- install from a folder ----------
    r = js_await(f"{P}.pluginInstallFolder(arguments[0])", src)
    run.check("install from a folder returns the manifest id", r == "acme-tools", str(r))
    run.check("plugin copied into the plugins folder", os.path.isfile(os.path.join(plugins_dir, "acme-tools", "kelpie-plugin.json")))
    i = info("acme-tools")
    run.check("installed plugin is listed, valid and off", i and not i["error"] and not i["approved"] and not i["changed"] and len(i["hash"]) == 64, str(i))
    r = js_await(f"{P}.pluginInstallFolder(arguments[0])", src)
    run.check("installing the same id again is refused", isinstance(r, dict) and "already installed" in r.get("err", ""), str(r))

    # ---------- nothing is used before approval ----------
    js_await(f"{P}.loadPlugins()")
    run.check("unapproved plugin adds no palette commands", plugin_command_ids() == [])
    run.check("unapproved plugin adds no quick commands", not d.js("return window.__kelpie.plugins.pluginQuickCommands().length"))
    run.check("unapproved plugin adds no layouts or themes", not d.js("return window.__kelpie.plugins.pluginLayouts().length") and not d.js("return document.querySelector('[data-status-item]')"))
    run.check("unapproved plugin agent is not in the daemon", "acme-tools-echo" not in presets())
    r = js_await(f"{P}.pluginStatus('acme-tools', 'ok', arguments[0])", proj)
    run.check("unapproved status command is refused", isinstance(r, dict) and "not approved" in r.get("err", ""), str(r))

    # ---------- review and enable in Settings ----------
    settings_page()
    d.wait(f"return !!document.querySelector('{card('acme-tools')}')", 10)
    run.check("settings card shows name, version and what it adds", "Acme Tools" in card_text("acme-tools") and "v1.2.0" in card_text("acme-tools") and "2 commands" in card_text("acme-tools") and "Off" in card_text("acme-tools"), card_text("acme-tools"))
    click_button(card("acme-tools"), "Review & enable")
    d.wait(f"return !!document.querySelector('[data-review=\"acme-tools\"]')", 5)
    review = d.js("return document.querySelector('[data-review=\"acme-tools\"]').innerText")
    run.check("review lists every program it would run", all(x in review for x in ["python -c print('ACME AGENT READY'); input()", "echo HELLO-${projectName}-${branch}", "echo ACME-QC", "echo STATUS-OK", "exit 3", "every 5 s"]), review)
    run.check("review mentions themes and layouts", "1 terminal theme(s) and 1 layout(s)" in review)
    run.shot("plugins-review")
    click_button('[data-review="acme-tools"]', "Enable plugin")
    ok = wait(lambda: "Enabled" in (card_text("acme-tools") or ""), 10)
    run.check("enabling marks the plugin enabled", ok, card_text("acme-tools"))
    approvals = json.load(open(os.path.join(plugins_dir, "approvals.json")))
    run.check("approval stored with the manifest hash", approvals.get("acme-tools") == info("acme-tools")["hash"])

    # ---------- contributions ----------
    ids = plugin_command_ids()
    run.check("palette commands added", ids == ["plugin.acme-tools.hello", "plugin.acme-tools.split"], str(ids))
    run.check("command label carries the plugin name", d.js("return window.__kelpie.commands.find(c => c.id === 'plugin.acme-tools.hello').label") == "Acme Tools: Say hello")
    pr = wait(lambda: presets().get("acme-tools-echo"), 10)
    run.check("plugin agent is in the daemon's agent table and found installed", pr and pr["label"] == "Acme Echo" and pr["installed"], str(pr))
    run.check("plugin agent kept for daemon restarts", "acme-tools-echo" in open(os.path.join(data_dir, "custom-agents.json"), encoding="utf-8").read())
    d.js(f"{S}.setOverlay(null)")
    run.check("plugin agent offered on the project's start page", wait(lambda: "Acme Echo" in d.js("return document.body.innerText"), 5))

    # Palette search finds the command.
    d.js(f"{S}.setOverlay('palette')")
    d.wait("return !!document.querySelector('.palette input')", 5)
    d.js("const i = document.querySelector('.palette input'); i.value = 'acme tools say'; i.dispatchEvent(new InputEvent('input', {bubbles: true}))")
    found = wait(lambda: "Acme Tools: Say hello" in d.js("return document.querySelector('.palette').innerText"), 5)
    run.check("palette search lists the plugin command", found)
    d.js(f"{S}.setOverlay(null)")

    d.js("window.__kelpie.commands.find(c => c.id === 'plugin.acme-tools.hello').run()")
    sh = wait(lambda: next((s for s in active_sessions() if s["kind"] == "shell"), None), 15)
    out = wait(lambda: sh and "HELLO-plugproj-plug-branch" in screen(sh["handle"]), 20)
    run.check("command runs in a terminal with ${projectName} and ${branch}", out, screen(sh["handle"]) if sh else "no shell")
    n = len(active_sessions())
    d.js("window.__kelpie.commands.find(c => c.id === 'plugin.acme-tools.split').run()")
    ok = wait(lambda: len(active_sessions()) == n + 1, 15)
    new = [s for s in active_sessions() if s["handle"] != sh["handle"]]
    run.check("split command opens a new terminal beside it and runs there", ok and new and wait(lambda: "SPLIT-plugproj" in screen(new[-1]["handle"]), 20))
    d.js(f"{S}.openFile(arguments[0], 'notes.txt')", pid)
    exp = d.js("return window.__kelpie.plugins.expandVariables('${file}|${projectPath}|${unknown}')")
    run.check("${file} is the focused file's full path; unknown variables stay", exp == f"{os.path.join(proj, 'notes.txt')}|{proj}|${{unknown}}", exp)

    qc = d.js(f"return {S} && window.__kelpie.plugins.pluginQuickCommands()")
    run.check("quick command added for every project", qc and qc[0]["label"] == "Acme list" and qc[0]["projectId"] is None, str(qc))
    d.js(f"{S}.revealSession(arguments[0])", sh["id"])
    d.js(f"{S}.setOverlay('palette')")
    d.wait("return !!document.querySelector('.palette input')", 5)
    d.js("const i = document.querySelector('.palette input'); i.value = 'acme list'; i.dispatchEvent(new InputEvent('input', {bubbles: true}))")
    wait(lambda: "Acme list" in d.js("return document.querySelector('.palette').innerText"), 5)
    d.keys("enter")
    run.check("quick command runs in the focused terminal", wait(lambda: "ACME-QC" in screen(sh["handle"]), 15), screen(sh["handle"])[-300:])

    # Theme.
    d.js(f"{S}.openSettings('terminal')")
    d.wait("return !!document.querySelector('[data-setting=\"terminal-theme\"] select')", 10)
    opts = d.js("return [...document.querySelectorAll('[data-setting=\"terminal-theme\"] option')].map(o => o.value + '=' + o.textContent)")
    run.check("terminal theme offered in Settings → Terminal", "acme-tools.mono=Acme Mono (Acme Tools)" in opts, str(opts))
    d.js(f"{S}.setSettings({{ terminalTheme: 'acme-tools.mono' }})")
    bg = wait(lambda: d.js("return getComputedStyle(document.documentElement).getPropertyValue('--terminal-bg').trim()") == COLORS[0], 5)
    run.check("choosing the plugin theme colours the terminals", bg)
    d.js(f"{S}.setOverlay(null)")

    # Layout from the layout menu.
    tabs = d.js(f"return {S}.workspace(arguments[0]).tabs.length", pid)
    d.js("document.querySelector('.layout-btn').click()")
    d.wait("return !!document.querySelector('[data-layout=\"plugin:acme-tools:0\"]')", 5)
    run.check("layout menu lists the plugin layout", "Acme trio" in d.js("return document.querySelector('[data-layout=\"plugin:acme-tools:0\"]').innerText"))
    run.shot("plugins-layout-menu")
    d.js("document.querySelector('[data-layout=\"plugin:acme-tools:0\"]').click()")
    ok = wait(lambda: d.js(f"return {S}.workspace(arguments[0]).tabs.length", pid) == tabs + 1, 10)
    panes = d.js(f"const s = {S}; const t = s.activeTab(arguments[0]); return window.__kelpie.layout.allPanes(t.root).map(p => p.kind + ':' + (p.sessionId ? s.sessions[p.sessionId]?.kind : ''))", pid)
    run.check("plugin layout opens as a new tab: two terminals and an empty pane", ok and sorted(panes) == ["empty:", "terminal:shell", "terminal:shell"], str(panes))

    # Status items.
    ok = wait(lambda: d.js("const e = document.querySelector('[data-status-item=\"acme-tools.ok\"]'); return e && e.innerText.includes('STATUS-OK')"), 15)
    run.check("status item shows its command's output", ok)
    bad = wait(lambda: d.js("const e = document.querySelector('[data-status-item=\"acme-tools.bad\"]'); return e && e.classList.contains('error') && e.innerText.includes('error')"), 15)
    run.check("failing status command is shown as an error", bad)
    run.shot("plugins-status")

    # Agent.
    d.js(f"{S}.newSession(arguments[0], 'acme-tools-echo', {{ where: 'tab' }})", pid)
    ag = wait(lambda: next((s for s in daemon(d, "session.list") if s["kind"] == "acme-tools-echo"), None), 15)
    run.check("plugin agent launches", ag and wait(lambda: "ACME AGENT READY" in screen(ag["handle"]), 20), str(ag))
    run.check("plugin agent shows its label", d.js("return document.body.innerText.includes('Acme Echo')"))

    # ---------- an edited plugin turns off ----------
    old_hash = info("acme-tools")["hash"]
    m2 = json.loads(json.dumps(MANIFEST))
    m2["contributes"]["statusItems"][0]["command"] = "echo CHANGED"
    write_manifest(os.path.join(plugins_dir, "acme-tools"), m2)
    js_await(f"{P}.loadPlugins()")
    i = info("acme-tools")
    run.check("edited manifest is no longer approved and is marked changed", not i["approved"] and i["changed"], str(i))
    run.check("its contributions are withdrawn", plugin_command_ids() == [] and wait(lambda: not d.js("return document.querySelector('[data-status-item]')"), 5))
    run.check("its agent leaves the daemon's table", wait(lambda: "acme-tools-echo" not in presets(), 10))
    r = js_await(f"{P}.pluginStatus('acme-tools', 'ok', arguments[0])", proj)
    run.check("its status command is refused", isinstance(r, dict) and "not approved" in r.get("err", ""))
    r = js_await(f"{P}.pluginApprove('acme-tools', arguments[0])", old_hash)
    run.check("approving with the old hash is refused", isinstance(r, dict) and "changed" in r.get("err", ""), str(r))
    settings_page()
    d.wait(f"return !!document.querySelector('{card('acme-tools')}')", 10)
    run.check("settings shows it needs reviewing", "Changed — review" in card_text("acme-tools"))
    click_button(card("acme-tools"), "Review & enable")
    d.wait("return !!document.querySelector('[data-review=\"acme-tools\"]')", 5)
    run.check("review shows the new command", "echo CHANGED" in d.js("return document.querySelector('[data-review=\"acme-tools\"]').innerText"))
    click_button('[data-review="acme-tools"]', "Enable plugin")
    run.check("re-enabled after review", wait(lambda: "Enabled" in card_text("acme-tools"), 10) and len(plugin_command_ids()) == 2)
    d.js(f"{S}.setOverlay(null)")
    run.check("status item runs the new command", wait(lambda: d.js("const e = document.querySelector('[data-status-item=\"acme-tools.ok\"]'); return e && e.innerText.includes('CHANGED')"), 15))

    # ---------- disable ----------
    settings_page()
    d.wait(f"return !!document.querySelector('{card('acme-tools')}')", 10)
    click_button(card("acme-tools"), "Disable")
    run.check("disable turns it off", wait(lambda: "Off" in card_text("acme-tools"), 10) and plugin_command_ids() == [])
    run.check("disabled plugin's theme is gone (terminals fall back)", d.js("return getComputedStyle(document.documentElement).getPropertyValue('--terminal-bg').trim()") != COLORS[0])
    run.check("disabled plugin's approval is withdrawn", "acme-tools" not in json.load(open(os.path.join(plugins_dir, "approvals.json"))))

    # ---------- invalid plugins ----------
    bad_dir = os.path.join(plugins_dir, "broken")
    os.makedirs(bad_dir, exist_ok=True)
    write_manifest(bad_dir, {"id": "other-id", "name": "Broken"})
    bad2 = os.path.join(plugins_dir, "unknown-section")
    os.makedirs(bad2, exist_ok=True)
    write_manifest(bad2, {"id": "unknown-section", "name": "X", "contributes": {"scripts": [{"run": "x"}]}})
    js_await(f"{P}.loadPlugins()")
    run.check("id not matching its folder is reported", "does not match" in (info("broken")["error"] or ""))
    run.check("unknown contribution is reported", "unknown contribution" in (info("unknown-section")["error"] or ""))
    d.wait(f"return !!document.querySelector('{card('broken')}')", 10)
    run.check("invalid plugin shows Invalid without an enable button", "Invalid" in card_text("broken") and not click_button(card("broken"), "Review & enable"))
    r = js_await(f"{P}.pluginApprove('broken', arguments[0])", info("broken")["hash"])
    run.check("an invalid plugin cannot be approved", isinstance(r, dict) and "err" in r, str(r))
    run.shot("plugins-settings")

    # ---------- create ----------
    d.js("const r = document.querySelector('[data-setting=\"plugin-create\"]'); const [id, name] = r.querySelectorAll('input'); id.value = 'my-tools'; id.dispatchEvent(new InputEvent('input', {bubbles: true})); name.value = 'My Tools'; name.dispatchEvent(new InputEvent('input', {bubbles: true}));")
    # Create stays disabled while the page is still busy with the previous action; wait until it can be clicked.
    run.check("Create can be clicked", wait(lambda: click_button('[data-setting="plugin-create"]', "Create"), 10))
    ok = wait(lambda: info("my-tools"), 10)
    run.check("create makes a valid starter plugin", ok and not ok["error"] and ok["manifest"]["name"] == "My Tools", str(ok))
    run.check("starter plugin is listed in settings", wait(lambda: d.js(f"return !!document.querySelector('{card('my-tools')}')"), 10))
    r = js_await(f"{P}.pluginCreate('my-tools', 'Again')")
    run.check("creating an existing id is refused", isinstance(r, dict) and "already exists" in r.get("err", ""))
    r = js_await(f"{P}.pluginCreate('Bad Id', 'x')")
    run.check("invalid ids are refused", isinstance(r, dict) and "err" in r)

    # ---------- install from git ----------
    repo_root = os.path.join(scratch, "plugin-git")
    if os.path.exists(repo_root):
        rmtree(repo_root)
    work = os.path.join(repo_root, "work")
    shutil.copytree(src, work)
    gm = json.loads(json.dumps(MANIFEST))
    gm["id"], gm["name"] = "git-tools", "Git Tools"
    write_manifest(work, gm)
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@example.test"]
    subprocess.run(git + ["init", "-q"], cwd=work, check=True)
    subprocess.run(git + ["add", "."], cwd=work, check=True)
    subprocess.run(git + ["commit", "-qm", "plugin"], cwd=work, check=True)
    subprocess.run(["git", "clone", "-q", "--bare", work, os.path.join(repo_root, "srv", "git-tools.git")], check=True)
    subprocess.run(["git", "update-server-info"], cwd=os.path.join(repo_root, "srv", "git-tools.git"), check=True)

    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *a):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", port), partial(Quiet, directory=os.path.join(repo_root, "srv")))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    d.js("const i = document.querySelector('[data-setting=\"plugin-install-git\"] input'); i.value = arguments[0]; i.dispatchEvent(new InputEvent('input', {bubbles: true}))", f"http://127.0.0.1:{port}/git-tools.git")
    click_button('[data-setting="plugin-install-git"]', "Install")
    ok = wait(lambda: info("git-tools"), 60)
    run.check("install from a git URL clones the plugin", ok and not ok["error"] and not ok["approved"], str(ok))
    run.check("the clone keeps no .git folder", not os.path.exists(os.path.join(plugins_dir, "git-tools", ".git")))
    run.check("no temporary clone left behind", not [x for x in os.listdir(plugins_dir) if x.startswith(".install-")])
    msg = wait(lambda: d.js("const e = document.querySelector('[data-plugin-message]'); return e && e.innerText"), 5)
    run.check("settings confirms the install", msg and "Installed git-tools" in msg, str(msg))
    for url, why in [("file:///" + work.replace("\\", "/"), "local file URL"), ("http://example.com/x.git", "plain http to another host"), ("ext::sh -c id", "ext transport")]:
        r = js_await(f"{P}.pluginInstallGit(arguments[0])", url)
        run.check(f"git install refuses a {why}", isinstance(r, dict) and "repository URL" in r.get("err", ""), str(r))
    r = js_await(f"{P}.pluginInstallGit(arguments[0])", f"http://127.0.0.1:{port}/missing.git")
    run.check("a missing repository reports git's error", isinstance(r, dict) and r.get("err"), str(r))
    server.shutdown()

    # ---------- settings search ----------
    d.js("const i = document.querySelector('.settings-search input, input.settings-search'); i.value = 'extension'; i.dispatchEvent(new InputEvent('input', {bubbles: true}))")
    run.check("settings search finds Plugins", wait(lambda: d.js("return !!document.querySelector('[data-setting=\"plugins\"]')"), 5))
    d.js("const i = document.querySelector('.settings-search input, input.settings-search'); i.value = ''; i.dispatchEvent(new InputEvent('input', {bubbles: true}))")

    # ---------- remove ----------
    js_await(f"{P}.pluginApprove('acme-tools', arguments[0])", info("acme-tools")["hash"])
    js_await(f"{P}.loadPlugins()")
    wait(lambda: "acme-tools-echo" in presets(), 10)
    d.js("window.confirm = () => true")
    d.wait(f"return !!document.querySelector('{card('acme-tools')}')", 10)
    d.js("document.querySelector(arguments[0] + ' button[title=\"Remove plugin\"]').click()", card("acme-tools"))
    run.check("remove deletes the plugin folder", wait(lambda: not os.path.exists(os.path.join(plugins_dir, "acme-tools")), 10))
    run.check("remove forgets its approval", wait(lambda: "acme-tools" not in json.load(open(os.path.join(plugins_dir, "approvals.json"))), 5))
    run.check("removed plugin's agent leaves the daemon", wait(lambda: "acme-tools-echo" not in presets(), 10) and "acme-tools-echo" not in open(os.path.join(data_dir, "custom-agents.json"), encoding="utf-8").read())
    run.check("removed plugin's commands are gone", plugin_command_ids() == [])
except Exception as e:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    for p in (listed() or []):
        js_await(f"{P}.pluginRemove(arguments[0])", p["id"])
    js_await(f"{P}.loadPlugins()")
    d.js(f"{S}.setSettings({{ terminalTheme: undefined }})")
    run.finish()
