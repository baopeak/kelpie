"""GUI e2e for settings export / import: a round trip from one Kelpie into a fresh one.

    python -X utf8 scripts/e2e/config_export.py <screenshot-dir> <scratch-dir> <app.exe>

Needs the debug app and daemon (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH (or in ~/.cargo/bin) and `msedgedriver.exe` matching the installed WebView2 (on PATH, or
in MSEDGEDRIVER). Each phase starts its own kelpied (isolated endpoint and data folder) and tauri-driver (free
`--port` and `--native-port`) with the same environment, and stops both when it ends. KELPIE_ENDPOINT,
KELPIE_DATA_DIR and KELPIE_SSH_DIR are set in this process before anything talks to a daemon, so the user's own
Kelpie, ~/.ssh and agent folders are never reached; nothing here touches the OS credential store. No network.

The native file dialogs cannot be driven by WebDriver: like the other scripts (plugins.py installs a folder
through `window.__kelpie.plugins`), this one calls the functions the Export / Import buttons call after their
dialog, `window.__kelpie.configTransfer.exportTo(path)` / `.previewImportFile(path)`. Everything else is the UI:
the "include values" box, the preview's counts and the Import button.

A. Seed settings, quick commands, a layout, agent arguments and variables, two automations (one an approved
   command), an SSH host and two plugins, with credentials in every place one could hide. Export (with values,
   and without): the file has the format header; no credential, approval hash or account folder is in it;
   variable names are, values only when asked and never secret ones.
B. A fresh data folder and daemon: a corrupt and an oversized file are refused and change nothing. Import the
   export: the preview counts, the Import button, then settings, quick commands, layouts, agents, hosts and
   automations are there; automations are off and their command unapproved; a backup was kept; missing variable
   values and plugins are reported. Exporting again gives the same sections (less what B was told to fill in).
"""
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run  # noqa: E402

if len(sys.argv) < 4:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

os.makedirs(scratch, exist_ok=True)
WORK = tempfile.mkdtemp(prefix="kelpie-config-export-", dir=scratch)
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)
os.environ["KELPIE_E2E"] = "1"
# Set before any phase: the first daemon or app must never start without an isolated endpoint.
os.environ["KELPIE_ENDPOINT"] = f"kelpie-config-export-{os.getpid()}-a"
os.environ["KELPIE_DATA_DIR"] = os.path.join(WORK, "data-a")
os.environ["KELPIE_SSH_DIR"] = os.path.join(WORK, "ssh")

NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver") or shutil.which(os.path.join(os.path.expanduser("~"), ".cargo", "bin", "tauri-driver"))
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)

S = "window.__kelpie.store"
CT = "window.__kelpie.configTransfer"
GH = "ghp_0123456789abcdefghijABCDEFGHIJ012345"
# An agent argument's value on each computer: masked in the file, B keeps its own.
API_KEY_A = "a-local-api-key-1234"
API_KEY_B = "b-local-api-key-5678"
ANT = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123"
DB_PW = "hunter2hunter2"
HOST_TOKEN = "tok_abcdef123456"
ACCOUNT_DIR = os.path.join(WORK, "claude-work-account")
AUTO_DIR = os.path.join(WORK, "auto-project")
PLUGINS_SRC = os.path.join(WORK, "plugin-src")
EXPORT = os.path.join(WORK, "export-values.json")
EXPORT_PLAIN = os.path.join(WORK, "export-plain.json")
EXPORT_B = os.path.join(WORK, "export-b.json")
results = []
# A fake credential store: the import's "forget this host's password" is recorded here and never reaches the
# OS store. (Tauri's invoke cannot be wrapped from the page: it is a read-only property.)
SECRET_FAKE = "window.__secretCalls = []; window.__kelpie.configTransfer.replacePasswordStore((id) => { window.__secretCalls.push(id); return Promise.resolve(); });"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def _writable(func, path, _exc):
    os.chmod(path, 0o666)
    func(path)


def rm(path):
    for _ in range(20):
        if not os.path.exists(path):
            return
        try:
            shutil.rmtree(path, onexc=_writable)
        except OSError:
            time.sleep(0.5)
    print(f"could not remove {path}", flush=True)


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def make_fixtures():
    os.makedirs(os.environ["KELPIE_SSH_DIR"], exist_ok=True)
    os.makedirs(ACCOUNT_DIR, exist_ok=True)
    write(os.path.join(AUTO_DIR, "hook.cmd"), "@echo %1\n")
    write(os.path.join(AUTO_DIR, "notes.md"), "x\n")
    for pid, name in (("acme-tools", "Acme Tools"), ("beta-tools", "Beta Tools")):
        write(os.path.join(PLUGINS_SRC, pid, "kelpie-plugin.json"), json.dumps({"id": pid, "name": name, "version": "1.2.0", "contributes": {"quickCommands": [{"label": "Hi", "text": "echo hi"}]}}))


# ---------- one Kelpie (daemon + driver + app) per phase ----------

class Phase:
    def __init__(self, tag):
        self.tag = tag
        self.endpoint = f"kelpie-config-export-{os.getpid()}-{tag}"
        self.data = os.path.join(WORK, f"data-{tag}")
        self.procs = {}
        self.run = None

    def __enter__(self):
        os.makedirs(self.data, exist_ok=True)
        os.environ["KELPIE_ENDPOINT"] = self.endpoint
        os.environ["KELPIE_DATA_DIR"] = self.data
        port = free_port()
        os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{port}"
        assert os.environ["KELPIE_ENDPOINT"] == self.endpoint
        self.spawn("daemon", [kelpied, "serve"])
        if not until(self.conn, 15, 0.1):
            self.close()
            raise SystemExit(f"{self.tag}: the daemon did not start")
        self.spawn("driver", [TAURI_DRIVER, "--port", str(port), "--native-port", str(free_port()), "--native-driver", NATIVE])
        if not until(lambda: self.port_open(port), 30):
            self.close()
            raise SystemExit(f"{self.tag}: tauri-driver did not open its port")
        try:
            self.run = Run(shots, app=app)
            self.run.results = results
            self.run.boot()
        except Exception:
            self.close()
            raise
        d = self.run.d
        d.set_window_size(1480, 920)
        d._req("POST", d.s("/timeouts"), {"script": 120000})
        d.wait("return typeof window.__kelpie?.configTransfer?.replacePasswordStore === 'function' || null", 20)
        d.js(SECRET_FAKE)
        if d.js("return Array.isArray(window.__secretCalls)") is not True:
            self.close()
            raise SystemExit(f"{self.tag}: could not install the fake password store; refusing to run an import")
        return self.run

    def __exit__(self, *exc):
        self.close()
        return False

    def spawn(self, name, args):
        self.procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    @staticmethod
    def port_open(port):
        with socket.socket() as s:
            return s.connect_ex(("127.0.0.1", port)) == 0

    def conn(self):
        try:
            return kc.Conn()
        except (OSError, SystemExit):
            return None

    def close(self):
        """Quits the app first (it would start a daemon of its own if the test daemon went away), then the driver
        tree this phase started, then its daemon."""
        if self.run is not None:
            try:
                self.run.d.quit()
            except Exception:
                pass
            self.run = None
        if "driver" in self.procs:
            if os.name == "nt":
                subprocess.run(["taskkill", "/PID", str(self.procs["driver"].pid), "/T", "/F"], capture_output=True)
            else:
                self.procs["driver"].kill()
        assert os.environ["KELPIE_ENDPOINT"] == self.endpoint
        try:
            kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
        except (Exception, SystemExit):
            pass
        for p in self.procs.values():
            try:
                p.wait(timeout=10)
            except Exception:
                p.kill()
        self.procs = {}
        try:
            os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", self.endpoint + ".token"))
        except OSError:
            pass


def call(d, expr, *args):
    """Awaits `expr` in the page; a rejection comes back as {"err": text}."""
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e && e.message || e)}}));", *args)


def ok(r):
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def open_config_row(d):
    d.js(f"{S}.openSettings('daemon', 'config-transfer')")
    d.wait("return !!document.querySelector('[data-setting=\"config-transfer\"] [data-act=\"export\"]') || null", 30)


def configuration(d):
    """What an import may change, as this Kelpie holds it."""
    return call(d, f"Promise.all([{S}.state.settings, window.__kelpie.ipc.daemon('automation.list'), window.__TAURI_INTERNALS__.invoke('config_hosts_current')]).then(([s, a, v]) => JSON.parse(JSON.stringify({{settings: s, automations: a, vault: v}})))")


def secrets_absent(run, label, text, approved_hash=None):
    leaked = [s for s in (GH, ANT, DB_PW, HOST_TOKEN, API_KEY_A, API_KEY_B, ACCOUNT_DIR.replace("\\", "\\\\"), ACCOUNT_DIR, "claude-work-account") if s in text]
    if approved_hash:
        leaked += [approved_hash] if approved_hash in text else []
    for key in ('"approved_hash"', '"approval"', '"command_hash"', '"accounts":', '"activeAccounts":', '"password":', '"passphrase":', '"lastConnected"'):
        if key in text:
            leaked.append(key)
    run.check(f"{label}: no credential, approval or account folder in the file", not leaked, str(leaked))


def without_name_only(entries):
    return [e for e in entries if "value" in e]


# ---------- A: seed and export ----------

def phase_a(run):
    d = run.d
    ok(call(d, f"{S}.setSettings(arguments[0])", {
        "fontSize": 15, "scrollback": 9000, "theme": "light", "wordWrap": True, "shellArgs": ["powershell.exe", "-NoLogo"],
        "integrations": {"linear": True, "jira": {"url": "https://jira.example.com", "email": "me@example.com"}, "gitlabHosts": ["gitlab.example.com"]},
        "quickCommands": [
            {"id": "qc-publish", "label": "Publish", "text": f"export GITHUB_TOKEN={GH} && npm publish", "action": "terminal", "enter": True, "projectId": None},
            {"id": "qc-review", "label": "Review", "text": "Review the uncommitted changes and list problems.", "action": "agent", "enter": True, "projectId": None},
        ],
        "layouts": [{"id": "lay-pair", "name": "Pair", "template": {"type": "split", "dir": "row", "sizes": [0.5, 0.5], "children": [{"type": "leaf", "kind": "claude"}, {"type": "leaf", "kind": "shell"}]}}],
        "accounts": [{"id": "acc-work", "agent": "claude", "label": "Work", "dir": ACCOUNT_DIR}],
    }))
    ok(call(d, f"{S}.setProfile('claude', arguments[0])", {"args": ["--model", "opus", "--api-key", API_KEY_A], "permission": "plan", "env": {"PUBLIC_FLAG": "1", "GITHUB_TOKEN": GH, "DB_PASSWORD": DB_PW}}))
    ok(call(d, f"{S}.setProfile('codex', arguments[0])", {"args": ["--search"]}))
    ok(call(d, "window.__kelpie.sshHosts.saveHost(window.__kelpie.sshHosts.newHost(arguments[0]))", {
        "label": "Build box", "hostname": "10.0.0.5", "user": "dev", "auth": "password", "identityFile": "~/.ssh/id_ed25519", "forwardAgent": True,
        "env": {"API_TOKEN": HOST_TOKEN, "LANG": "C.UTF-8"}, "notes": f"deploy key {GH}",
    }))
    for pid in ("acme-tools", "beta-tools"):
        ok(call(d, "window.__kelpie.plugins.pluginInstallFolder(arguments[0])", os.path.join(PLUGINS_SRC, pid)))
    daemon = "window.__kelpie.ipc.daemon"
    cmd = ok(call(d, f"{daemon}('automation.save', arguments[0])", {
        "id": "", "name": "Format notes", "enabled": True, "kind": "", "prompt": "", "cwd": AUTO_DIR, "project_id": None, "args": [],
        "every": "daily", "time": "09:00", "weekday": 1, "last_run": 0, "last_session": None, "last_error": None, "next_run": 0,
        "action": "shell", "shell": {"program": ".\\hook.cmd", "argv": ["{file}"]}, "trigger": {"event": "file", "glob": "*.md", "daily_cap": 10},
    }))
    approved = ok(call(d, f"{daemon}('automation.approve', arguments[0])", {"id": cmd["id"], "hash": cmd["command_hash"]}))
    run.check("A: the command automation is approved here", approved.get("command_approved") is True, str(approved.get("command_approved")))
    ok(call(d, f"{daemon}('automation.save', arguments[0])", {
        "id": "", "name": "Morning summary", "enabled": True, "kind": "claude", "prompt": f"Summarise the repo; key {ANT}", "cwd": AUTO_DIR, "project_id": None,
        "args": [], "env": {"GITHUB_TOKEN": GH, "PUBLIC_FLAG": "1"}, "every": "weekdays", "time": "08:15", "weekday": 1,
        "last_run": 0, "last_session": None, "last_error": None, "next_run": 0, "action": "agent",
    }))

    open_config_row(d)
    run.check("A: the export row lists every section, automations available", d.js("return [...document.querySelectorAll('[data-part=\"export-sections\"] input')].map(i => i.dataset.section + (i.disabled ? '-off' : '')).join(',')") == "settings,quickCommands,layouts,automations,sshHosts,plugins,agents")
    run.check("A: values are not included by default", d.js("return document.querySelector('[data-part=\"include-values\"]').checked") is False)
    d.click('[data-part="include-values"]')
    warn = d.wait("return document.querySelector('[data-setting=\"config-transfer\"] .config-warning')?.textContent || null", 5)
    run.check("A: ticking values shows the plain-text warning", "plain text" in (warn or ""), warn)
    run.shot("cfg-01-export-row")
    r = ok(call(d, f"{CT}.exportTo(arguments[0])", EXPORT))
    run.check("A: export reports every section written", sorted(r["counts"]) == sorted(["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"]), json.dumps(r["counts"]))
    msg = d.wait("return document.querySelector('[data-setting=\"config-transfer\"] [data-part=\"message\"]')?.textContent || null", 10)
    run.check("A: the row says where the file went", EXPORT in (msg or ""), msg)
    ok(call(d, f"{CT}.exportTo(arguments[0], undefined, false)", EXPORT_PLAIN))

    text = open(EXPORT, encoding="utf-8").read()
    doc = json.loads(text)
    sec = doc["sections"]
    run.check("file: kind, version, exportedAt and app version", doc["kind"] == "kelpie-config" and doc["version"] == 1 and doc["exportedAt"] and doc["appVersion"], json.dumps({k: doc[k] for k in ("kind", "version", "exportedAt", "appVersion")}))
    secrets_absent(run, "file with values", text, cmd["command_hash"])
    secrets_absent(run, "file without values", open(EXPORT_PLAIN, encoding="utf-8").read(), cmd["command_hash"])
    claude = next(a for a in sec["agents"] if a["kind"] == "claude")
    run.check("file: agent variable names always, values only non-secret ones", claude["env"] == [{"name": "DB_PASSWORD"}, {"name": "GITHUB_TOKEN"}, {"name": "PUBLIC_FLAG", "value": "1"}], json.dumps(claude["env"]))
    plain_claude = next(a for a in load(EXPORT_PLAIN)["sections"]["agents"] if a["kind"] == "claude")
    run.check("file without values: names only", plain_claude["env"] == [{"name": "DB_PASSWORD"}, {"name": "GITHUB_TOKEN"}, {"name": "PUBLIC_FLAG"}], json.dumps(plain_claude["env"]))
    run.check("file: the quick command keeps its text around the masked token", sec["quickCommands"][0]["text"] == "export GITHUB_TOKEN=[redacted] && npm publish", sec["quickCommands"][0]["text"])
    run.check("file: automations are written switched off", [a["enabled"] for a in sec["automations"]] == [False, False])
    host = sec["sshHosts"][0]
    run.check("file: the host keeps its key path and loses secret variables", host["identityFile"] == "~/.ssh/id_ed25519" and host["env"] == {"LANG": "C.UTF-8"} and "os" not in host, json.dumps(host))
    run.check("file: plugins listed with name and version", sec["plugins"] == [{"id": "acme-tools", "name": "Acme Tools", "version": "1.2.0"}, {"id": "beta-tools", "name": "Beta Tools", "version": "1.2.0"}], json.dumps(sec["plugins"]))
    run.check("file: settings without sign-ins or local files", "accounts" not in sec["settings"] and "sshConfig" not in sec["settings"] and sec["settings"]["fontSize"] == 15)
    return doc


# ---------- B: a fresh Kelpie imports it ----------

def phase_b(run, exported):
    d = run.d
    ok(call(d, "window.__kelpie.plugins.pluginInstallFolder(arguments[0])", os.path.join(PLUGINS_SRC, "acme-tools")))
    # B already has a "build-box" that connects somewhere else, with agent forwarding and a port forward.
    # B's Claude already has an API key argument of its own: the import must keep it, not write the mask.
    ok(call(d, f"{S}.setProfile('claude', arguments[0])", {"args": ["--api-key", API_KEY_B]}))
    sh = "window.__kelpie.sshHosts"
    local = ok(call(d, f"{sh}.saveHost({sh}.newHost(arguments[0]))", {"label": "Build box", "alias": "build-box", "hostname": "10.9.9.9", "user": "dev", "auth": "password", "forwardAgent": True}))
    ok(call(d, f"{sh}.saveForward({{...{sh}.newForward(arguments[0]), label: 'web', bindPort: 18080, destHost: 'localhost', destPort: 80, autoStart: false}})", local["id"]))
    open_config_row(d)
    backups = os.path.join(os.environ["KELPIE_DATA_DIR"], "backups")

    corrupt = os.path.join(WORK, "corrupt.json")
    write(corrupt, '{"kind": "kelpie-config", "version": 1, "sections": {')
    big = os.path.join(WORK, "big.json")
    with open(big, "w", encoding="utf-8") as f:
        f.write('{"kind":"kelpie-config","version":1,"sections":{"settings":{"pad":"' + "x" * (2 * 1024 * 1024) + '"}}}')
    newer = os.path.join(WORK, "newer.json")
    write(newer, json.dumps({"kind": "kelpie-config", "version": 2, "sections": {}}))
    before = configuration(d)
    for path, why in ((corrupt, "not a Kelpie settings file"), (big, "larger than 2 MB"), (newer, "newer Kelpie")):
        r = call(d, f"{CT}.previewImportFile(arguments[0])", path)
        run.check(f"B: {os.path.basename(path)} is refused ({why})", isinstance(r, dict) and why in r.get("err", ""), json.dumps(r)[:200])
    run.check("B: …and nothing changed, no backup made", configuration(d) == before and not os.path.exists(backups))
    run.check("B: …and no preview is shown", not d.js("return !!document.querySelector('[data-part=\"import-preview\"]')"))

    ok(call(d, f"{CT}.previewImportFile(arguments[0])", EXPORT))
    d.wait("return !!document.querySelector('[data-part=\"import-preview\"]') || null", 10)
    counts = d.js("return Object.fromEntries([...document.querySelectorAll('[data-part=\"import-preview\"] .config-preview-row')].map(r => [r.dataset.section, r.querySelector('[data-part=\"counts\"]').textContent]))")
    run.check("B: preview: one automation new, the one whose prompt was masked left out", counts.get("automations", "").startswith("2 in the file: 1 new, 0 replacing") and "1 unusable entries left out" in counts.get("automations", ""), json.dumps(counts))
    run.check("B: preview counts the quick commands as new and the host as replacing", counts.get("sshHosts", "").startswith("1 in the file: 0 new, 1 replacing") and counts.get("quickCommands", "").startswith("2 in the file: 1 new") and "1 unusable entries left out" in counts.get("quickCommands", ""), json.dumps(counts))
    run.check("B: preview: the agents replace the default profiles", counts.get("agents", "").startswith("3 in the file: 0 new, 3 replacing"), counts.get("agents"))
    run.check("B: preview: one listed plugin is not installed here", counts.get("plugins", "").startswith("2 listed, 1 not installed"), counts.get("plugins"))
    notes = d.js("return document.querySelector('[data-part=\"import-preview\"]').textContent")
    run.check("B: preview says automations come in off and hosts without passwords", "switched off" in notes and "No passwords" in notes, notes[:300])
    run.check("B: preview marks the host's connection target as changed, with password and forward",
              "Connection target changed: build-box, dev@10.9.9.9 → dev@10.0.0.5. Its saved password will be forgotten and its 1 port forwards removed." in notes, notes[:600])

    # What runs: every value shown old → new, and Import needs its own tick.
    runs = d.js("return [...document.querySelectorAll('[data-part=\"runs\"] .config-run')].map(r => [r.dataset.section, r.dataset.field, r.querySelector('.config-run-what').textContent, r.querySelector('.config-run-before').textContent, r.querySelector('.config-run-after').textContent])")
    find = lambda field, item=None: next((r for r in runs or [] if r[1] == field and (item is None or item in r[2])), None)  # noqa: E731
    run.check("B: runs: the terminal program, old → new", find("shellArgs") and find("shellArgs")[3] == "(none)" and find("shellArgs")[4] == "powershell.exe -NoLogo", json.dumps(find("shellArgs")))
    run.check("B: runs: claude's arguments, permission and variable values", find("args", "Claude Code")[4] == "--model opus --api-key " + API_KEY_B and find("args", "Claude Code")[3] == "--api-key " + API_KEY_B and find("permission", "Claude Code")[4] == "plan" and find("env:PUBLIC_FLAG", "Claude Code")[4] == "1", json.dumps(runs)[:600])
    run.check("B: runs: quick command text, where it goes and Enter", find("text", "Review") and find("text", "Review")[4] == "> Review the uncommitted changes and list problems. ⏎", json.dumps(find("text", "Review")))
    run.check("B: runs: the masked quick command is not offered", find("text", "Publish") is None)
    run.check("B: runs: the automation's command and trigger", find("command", "Format notes")[4] == ".\\hook.cmd {file}" and find("when", "Format notes")[4] == "file *.md ≤10/day", json.dumps(runs)[:800])
    left = d.js("return [...document.querySelectorAll('[data-part=\"import-preview\"] [data-part=\"note\"]')].map(n => n.textContent).join('\\n')")
    run.check("B: preview names what was masked and is left out", "Left out, masked in the file and not set here: Publish (text)" in left and "Morning summary (prompt)" in left, left[:600])
    button_off = lambda: d.js("return document.querySelector('[data-act=\"apply-import\"]').disabled")  # noqa: E731
    run.check("B: Import stays off until the commands are checked", button_off() is True)
    run.shot("cfg-02-import-preview")
    d.click('[data-part="runs-checked"]')
    run.check("B: …and turns on with the tick", until(lambda: button_off() is False, 5))
    # The fake password store must still be in place (a reload would have brought back the real one).
    if d.js("return Array.isArray(window.__secretCalls) && window.__secretCalls.length === 0") is not True:
        raise SystemExit("the fake password store is gone; refusing to import")
    d.click('[data-act="apply-import"]')
    msg = d.wait("const m = document.querySelector('[data-setting=\"config-transfer\"] [data-part=\"message\"]'); return m && !document.querySelector('[data-part=\"import-preview\"]') ? m.textContent : null", 30)
    run.check("B: import reports success and the backup", (msg or "").startswith("Imported.") and backups.replace("\\", "/").lower() in (msg or "").replace("\\", "/").lower(), msg)
    run.check("B: …names the variables to fill in and the plugin to install", "DB_PASSWORD, GITHUB_TOKEN" in (msg or "") and "Beta Tools" in (msg or "") and "Publish (text)" in (msg or ""), msg)
    kept = [x for x in os.listdir(backups) if x.startswith("config-")] if os.path.isdir(backups) else []
    run.check("B: a backup of the previous configuration was kept", len(kept) == 1 and os.path.exists(os.path.join(backups, kept[0], "app-state.json")), str(kept))
    run.shot("cfg-03-imported")

    now = configuration(d)
    s = now["settings"]
    run.check("B: settings arrived", s["fontSize"] == 15 and s["scrollback"] == 9000 and s["theme"] == "light" and s["integrations"]["jira"]["url"] == "https://jira.example.com", json.dumps({k: s[k] for k in ("fontSize", "scrollback", "theme")}))
    run.check("B: quick commands and layouts arrived", [q["id"] for q in s["quickCommands"]] == ["qc-review"] and [l["name"] for l in s["layouts"]] == ["Pair"])
    claude = next(p for p in s["profiles"] if p["kind"] == "claude")
    run.check("B: agent arguments, permission and the non-secret variable arrived", claude["args"] == ["--model", "opus", "--api-key", API_KEY_B] and claude.get("permission") == "plan" and claude.get("env") == {"PUBLIC_FLAG": "1"}, json.dumps(claude))
    run.check("B: no sign-ins came along", s.get("accounts") == [], json.dumps(s.get("accounts")))
    hosts = now["vault"]["hosts"]
    run.check("B: the host replaced the local one, without credentials", len(hosts) == 1 and hosts[0]["id"] == local["id"] and hosts[0]["hostname"] == "10.0.0.5" and hosts[0]["auth"] == "password", json.dumps(hosts))
    run.check("B: …agent forwarding is off, though the file and the old host had it on", hosts[0]["forwardAgent"] is False)
    run.check("B: …its port forward was removed", now["vault"]["forwards"] == [], json.dumps(now["vault"]["forwards"]))
    secret_calls = d.js("return window.__secretCalls")
    run.check("B: …and its saved password was forgotten (fake store, never the OS one)", secret_calls == [local["id"]], json.dumps(secret_calls))
    run.check("B: the result names the moved host", "build-box now connects to dev@10.0.0.5" in (msg or ""), msg)
    autos = sorted(now["automations"], key=lambda a: a["name"])
    run.check("B: the automation arrived switched off; the masked one did not", [(a["name"], a["enabled"]) for a in autos] == [("Format notes", False)], json.dumps([(a["name"], a["enabled"]) for a in autos]))
    run.check("B: the redaction marker is nowhere in this computer's configuration", "[redacted]" not in json.dumps(now), [k for k in ("settings", "vault", "automations") if "[redacted]" in json.dumps(now[k])])
    command = autos[0]
    run.check("B: the command needs approving on this computer", command.get("command_hash") and command.get("command_approved") is False and not (command.get("shell") or {}).get("approved_hash"), json.dumps(command.get("shell")))
    d.js(f"{S}.openSettings('automations')")
    approve = d.wait("return [...document.querySelectorAll('.automation-item')].find(r => r.textContent.includes('Format notes'))?.textContent || null", 15)
    run.check("B: the Automations page offers Approve command for it", "Approve command" in (approve or ""), approve)
    run.shot("cfg-04-automations")
    open_config_row(d)

    # Round trip: B exports what A exported.
    ok(call(d, f"{CT}.exportTo(arguments[0], undefined, true)", EXPORT_B))
    text_b = open(EXPORT_B, encoding="utf-8").read()
    secrets_absent(run, "B's export", text_b)
    a_sec, b_sec = exported["sections"], json.loads(text_b)["sections"]
    # The host replaced B's own: it keeps B's id and B's notes (the file's were masked); agent forwarding never comes from a file.
    host_view = lambda hosts: [{k: v for k, v in h.items() if k not in ("id", "forwardAgent", "notes")} for h in hosts]  # noqa: E731
    # Items left out because what they need was masked are not in B; the rest must match.
    a_sec = {**a_sec, "quickCommands": [q for q in a_sec["quickCommands"] if q["label"] != "Publish"], "automations": [x for x in a_sec["automations"] if x["name"] != "Morning summary"]}
    a_sec, b_sec = {**a_sec, "sshHosts": host_view(a_sec["sshHosts"])}, {**b_sec, "sshHosts": host_view(b_sec["sshHosts"])}
    for name in ("settings", "quickCommands", "layouts", "sshHosts"):
        equal = a_sec[name] == b_sec[name]
        run.check(f"round trip: {name} equal", equal, "" if equal else f"{json.dumps(a_sec[name])[:300]} != {json.dumps(b_sec[name])[:300]}")
    strip = lambda items, key: [{**x, "env": without_name_only(x["env"])} for x in sorted(items, key=lambda y: y[key])]  # noqa: E731
    run.check("round trip: agents equal (less the variables B was told to fill in)", strip(a_sec["agents"], "kind") == strip(b_sec["agents"], "kind"), json.dumps(b_sec["agents"])[:400])
    run.check("round trip: automations equal (less variables without a value)", strip(a_sec["automations"], "name") == strip(b_sec["automations"], "name"), json.dumps(b_sec["automations"])[:400])
    run.check("round trip: B lists the plugin it has", b_sec["plugins"] == [p for p in a_sec["plugins"] if p["id"] == "acme-tools"], json.dumps(b_sec["plugins"]))


def main():
    make_fixtures()
    exported = None
    try:
        try:
            with Phase("a") as run:
                exported = phase_a(run)
        except Exception:
            traceback.print_exc()
            results.append(("phase A ran to completion", False, ""))
            print("FAIL phase A ran to completion", flush=True)
        if exported is not None:
            try:
                with Phase("b") as run:
                    phase_b(run, exported)
            except Exception:
                traceback.print_exc()
                results.append(("phase B ran to completion", False, ""))
                print("FAIL phase B ran to completion", flush=True)
    finally:
        rm(WORK)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
