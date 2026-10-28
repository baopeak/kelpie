"""GUI e2e for Settings -> Agent setup: MCP servers, instruction files, skills, "Find more agents".

    python -X utf8 scripts/e2e/agent_setup.py <screenshot-dir> <scratch-dir> [app.exe]

Needs the debug daemon and app (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH and `msedgedriver.exe` matching the installed WebView2 (on PATH, or in MSEDGEDRIVER).
The script starts its own kelpied (isolated endpoint and data folder), tauri-driver (free ports, the
native driver's too) and a fake ACP registry on 127.0.0.1, and stops them on exit.

Nothing outside <scratch-dir> is touched. The agents' config homes are fixture folders:
KELPIE_AGENT_CONFIG_HOME (honoured only with KELPIE_E2E) replaces the user's home for the app, and
CLAUDE_CONFIG_DIR / CODEX_HOME / GEMINI_CLI_HOME / OPENCODE_CONFIG_DIR point at the same fixtures. As a
last guard the script compares size and mtime (never the content) of the real ~/.claude.json,
~/.codex/config.toml and ~/.gemini/settings.json before and after.

1. Opening reads once (nothing in the background), within 50 ms; env values are masked; the project's
   server is tagged project / untrusted, the user's are user.
2. Toggle: the preview shows the diff and writes nothing; applying writes, with a backup.
3. Copy project -> user shows the exact command and cannot be applied until it is confirmed.
4. Env values never reach a project file; a diff never shows them.
5. Backups: list, restore (preview first).
6. A write that leaves the file unparseable is undone at once.
7. Add a server once to Claude, Codex and Gemini.
8. One file for all agents.
9. Skills and commands are listed.
10. Find more agents: no request until the button, one request per click, full command, a confirm, nothing run.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import threading
import time
import importlib.util
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from smoke_support import Run, daemon, default_app  # noqa: E402

if len(sys.argv) < 3:
    raise SystemExit(__doc__)
shots, scratch = sys.argv[1], os.path.abspath(sys.argv[2])
app = os.path.abspath(sys.argv[3]) if len(sys.argv) > 3 else default_app()
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")

ENDPOINT = f"kelpie-agentsetup-{os.getpid()}"
WORK = os.path.join(scratch, f"agentsetup-{os.getpid()}")
DATA = os.path.join(WORK, "data")
HOME = os.path.join(WORK, "home")
PROJ = os.path.join(WORK, "proj dự án")
for d_ in (DATA, HOME, PROJ):
    os.makedirs(d_, exist_ok=True)
assert all(os.path.abspath(p).startswith(scratch) for p in (DATA, HOME, PROJ)), "fixtures must live under the scratch folder"

SECRET = "sk-live-ZXCVBNMASDFGHJKLQWERTY123456"
REGISTRY = {"version": "1.0.0", "extensions": [], "agents": [
    {"id": "alpha-acp", "name": "Alpha", "version": "1.4.0", "description": "A test agent", "repository": "https://example.com/alpha",
     "distribution": {"npx": {"package": "@acme/alpha", "args": ["--acp"]}}},
    {"id": "gamma", "name": "Gamma", "version": "3.0.0", "description": "Binary only",
     "distribution": {"binary": {"windows-x86_64": {"archive": "https://example.com/g.zip", "cmd": "./g.exe"}}}},
]}
registry_hits = []


class Registry(BaseHTTPRequestHandler):
    def do_GET(self):
        registry_hits.append(self.path)
        body = json.dumps(REGISTRY).encode()
        self.send_response(200 if self.path == "/registry.json" else 404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


REGISTRY_PORT = free_port()
DRIVER_PORT = free_port()
NATIVE_PORT = free_port()
os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "KELPIE_AGENT_CONFIG_HOME": HOME,
    "CLAUDE_CONFIG_DIR": os.path.join(HOME, ".claude"), "CODEX_HOME": os.path.join(HOME, ".codex"),
    "GEMINI_CLI_HOME": HOME, "OPENCODE_CONFIG_DIR": os.path.join(HOME, ".config", "opencode"),
    "KELPIE_ACP_REGISTRY_URL": f"http://127.0.0.1:{REGISTRY_PORT}/registry.json",
    "KELPIE_E2E_DRIVER": f"http://127.0.0.1:{DRIVER_PORT}",
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)

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
real_home = os.path.expanduser("~")
REAL_FILES = [os.path.join(real_home, ".claude.json"), os.path.join(real_home, ".codex", "config.toml"), os.path.join(real_home, ".gemini", "settings.json")]


def stat_of(path):
    try:
        st = os.stat(path)
        return (st.st_size, st.st_mtime_ns)
    except OSError:
        return None


def until(fn, timeout=30, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(interval)
    return None


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def js_await(d, expr, *args):
    r = d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)
    if isinstance(r, dict) and "err" in r:
        raise RuntimeError(r["err"])
    return r


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


CLAUDE_JSON = json.dumps({
    "numStartups": 12, "theme": "dark",
    "mcpServers": {
        "files": {"command": "npx", "args": ["-y", "@acme/files"], "env": {"API_KEY": SECRET, "MODE": "fast"}},
        "remote": {"type": "http", "url": "https://mcp.example.com/mcp", "headers": {"Authorization": f"Bearer {SECRET}"}},
    },
    "projects": {"C:/work": {"allowedTools": []}},
}, indent=2) + "\n"
CODEX_TOML = '# my codex\nmodel = "gpt-5"\n\n[mcp_servers.docs]\ncommand = "npx"\nargs = ["-y", "@acme/docs"]\n'
GEMINI_JSON = '{\n  "theme": "dark"\n}\n'
PROJECT_MCP = (
    '{\n  "mcpServers": {\n    "build": { "command": "node", "args": ["./scripts/mcp.js", "--fast"],'
    ' "env": { "NODE_OPTIONS": "--require ./x.js", "GH": "${GITHUB_TOKEN}" } }\n  }\n}\n'
)


def make_fixtures():
    write(os.path.join(HOME, ".claude.json"), CLAUDE_JSON)
    write(os.path.join(HOME, ".codex", "config.toml"), CODEX_TOML)
    write(os.path.join(HOME, ".gemini", "settings.json"), GEMINI_JSON)
    write(os.path.join(HOME, ".claude", "skills", "pdf", "SKILL.md"), "---\nname: pdf\ndescription: Work with PDF files\n---\nbody\n")
    write(os.path.join(HOME, ".claude", "commands", "review.md"), "---\ndescription: Review the diff\n---\nGo\n")
    write(os.path.join(HOME, ".codex", "prompts", "ship.md"), "Ship it\n")
    write(os.path.join(PROJ, ".mcp.json"), PROJECT_MCP)
    write(os.path.join(PROJ, "CLAUDE.md"), "# Rules\nUse tabs.\n")
    write(os.path.join(PROJ, ".claude", "agents", "planner.md"), "---\ndescription: Plans the work\n---\n")


def page_text(d):
    return d.js("return document.querySelector('[data-agent-setup]')?.innerText || ''")


def click(d, css, text=None):
    return d.js("""const els = [...document.querySelectorAll(arguments[0])];
        const el = arguments[1] ? els.find(e => e.innerText.trim() === arguments[1]) : els[0];
        if (!el || el.disabled) return false; el.click(); return true;""", css, text)


def exists(d, css):
    return bool(d.js("return !!document.querySelector(arguments[0])", css))


def count(d, css):
    return d.js("return document.querySelectorAll(arguments[0]).length", css)


def set_input(d, css, value):
    d.js("""const el = document.querySelector(arguments[0]); el.value = arguments[1];
        el.dispatchEvent(new Event('input', { bubbles: true })); el.dispatchEvent(new Event('change', { bubbles: true }));""", css, value)


def panel_ready(d):
    return until(lambda: exists(d, "[data-change-panel] .as-diff") or exists(d, "[data-change-panel] [data-noop]"), 20)


def panel_text(d):
    return d.js("return document.querySelector('[data-change-panel]')?.innerText || ''")


def apply_pending(d):
    ok = click(d, "[data-change-panel] [data-apply]")
    until(lambda: not exists(d, "[data-change-panel]") or exists(d, "[data-change-error]"), 20)
    return ok


def open_page(d):
    d.js(f"{S}.openSettings('agent-setup')")
    d.wait("return !!document.querySelector('[data-agent-setup] [data-mcp-agent]')", 20)


def calls(d, prefix):
    """How many times the page asked the app for `prefix*` (the IPC requests show up as resource timings)."""
    return d.js("return performance.getEntriesByType('resource').filter(e => new URL(e.name).pathname.slice(1).startsWith(arguments[0])).length", prefix)


def scenario(run):
    d = run.d
    check = run.check
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    d.js(f"{S}.setSettings({{ shortcutHints: false }})")
    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0])", pid)
    real_before = [stat_of(p) for p in REAL_FILES]
    sessions_before = len(daemon(d, "session.list"))
    # Count what the page asks the app for from here on.
    d.js("performance.setResourceTimingBufferSize(10000); performance.clearResourceTimings();")

    # ---------- 1. opening ----------
    open_page(d)
    run.shot("01-agent-setup")
    time.sleep(3)
    check("opening reads once and nothing runs in the background",
          calls(d, "agent_setup_overview") == 1 and calls(d, "agent_setup_preview") == 0 and calls(d, "agent_setup_apply") == 0 and calls(d, "agent_registry_fetch") == 0,
          str(d.js("return performance.getEntriesByType('resource').map(e => e.name).filter(n => n.includes('agent_'))")))
    check("looking writes nothing: no backups folder", not os.path.exists(os.path.join(DATA, "agent-config-backups")))
    ms = [js_await(d, "window.__TAURI_INTERNALS__.invoke('agent_setup_overview', { project: arguments[0] })", PROJ)["elapsed_ms"] for _ in range(5)]
    print(f"overview read: best {min(ms):.1f} ms, runs {[round(m, 1) for m in ms]}", flush=True)
    check("reading everything takes at most 50 ms", min(ms) <= 50, f"{min(ms):.1f} ms")
    check("the user's servers are listed under Claude Code", count(d, '[data-server-file="claude:user"]') == 2)
    check("the project's server is listed and tagged project / untrusted",
          exists(d, '[data-server-file="claude:project"][data-server="build"]') and "untrusted" in d.js("return document.querySelector('[data-mcp-file=\"claude:project\"] .as-chip').innerText"))
    check("the user's file is tagged user", d.js("return document.querySelector('[data-mcp-file=\"claude:user\"] .as-chip').dataset.scope") == "user")
    text = page_text(d)
    check("env values are masked, names shown", SECRET not in text and "API_KEY=••••••••" in text, text[:200])
    check("headers show names only", "Headers: Authorization" in text and SECRET not in text)
    check("the page tells how long the read took", exists(d, "[data-read-ms]"))
    first = int(d.js("return (document.querySelector('[data-read-ms]')?.innerText.match(/\\d+/) || ['-1'])[0]"))
    print(f"first read on opening: {first} ms", flush=True)
    # The first read of fresh files also pays for the disk cache and the virus scanner; the budget is
    # for the repeat reads below, this one only has to stay in the same order.
    check("the read that opened the page was quick (cold: at most 150 ms)", 0 <= first <= 150, f"{first} ms")

    # ---------- 2. toggle ----------
    codex = os.path.join(HOME, ".codex", "config.toml")
    check("a Codex server has a switch", exists(d, '[data-server-file="codex:user"][data-server="docs"] .switch'))
    check("Claude's user servers have none (it has no such setting)", not exists(d, '[data-server-file="claude:user"] .switch'))
    d.js("document.querySelector('[data-server-file=\"codex:user\"][data-server=\"docs\"] .switch').click()")
    check("the toggle shows a preview", bool(panel_ready(d)))
    ptext = panel_text(d)
    check("the diff shows the exact line", "enabled = false" in ptext and "mcp_servers.docs" in ptext, ptext[:300])
    check("the preview wrote nothing", read(codex) == CODEX_TOML and not os.path.exists(os.path.join(DATA, "agent-config-backups")))
    run.shot("02-toggle-preview")
    apply_pending(d)
    check("applying writes the setting and keeps the comment", "enabled = false" in read(codex) and "# my codex" in read(codex), read(codex))
    check("a backup of the file as it was exists", any(f.endswith(".bak") and read(os.path.join(DATA, "agent-config-backups", f)) == CODEX_TOML for f in os.listdir(os.path.join(DATA, "agent-config-backups"))))
    check("the row shows it is off", until(lambda: exists(d, '[data-server-file="codex:user"][data-server="docs"].off'), 10) is not None)

    # ---------- 3. project -> user copy ----------
    claude_json = os.path.join(HOME, ".claude.json")
    click(d, '[data-server-file="claude:project"][data-server="build"] [data-copy-open]')
    d.wait("return !!document.querySelector('[data-copy-form]')", 10)
    d.js("document.querySelector('[data-copy-target=\"claude:user\"]').click()")
    check("the copy form warns that the server is from the project", "comes from the project" in d.js("return document.querySelector('[data-copy-form]').innerText"))
    click(d, "[data-copy-preview]")
    check("the copy shows a preview", bool(panel_ready(d)))
    ptext = panel_text(d)
    mcp_js = os.path.join(PROJ, "scripts", "mcp.js")
    argv = d.js("return document.querySelector('[data-untrusted] [data-argv]')?.innerText || ''")
    check("the exact command is shown before it can be trusted, with the relative path made absolute",
          exists(d, "[data-untrusted]") and argv == (f'node "{mcp_js}" --fast' if " " in mcp_js else f"node {mcp_js} --fast"), argv)
    check("the confirmation says what the relative path became", exists(d, "[data-untrusted] [data-resolved]") and "relative to the project" in ptext, ptext[:500])
    uv_node = d.js("return document.querySelector('[data-untrusted] [data-uv=\"NODE_OPTIONS\"]')?.innerText || ''")
    uv_gh = d.js("return document.querySelector('[data-untrusted] [data-uv=\"GH\"]')?.innerText || ''")
    check("every environment entry is listed as the repository wrote it", "--require ./x.js" in uv_node and "not copied" in uv_node, uv_node)
    check("a reference to the user's variable is named, not expanded", "reads your variable GITHUB_TOKEN" in uv_gh and "${GITHUB_TOKEN}" not in uv_gh, uv_gh)
    check("it cannot be applied before the confirmation", d.js("return document.querySelector('[data-apply]').disabled") is True)
    click(d, "[data-change-panel] [data-apply]")
    check("a click on the disabled button writes nothing", read(claude_json) == CLAUDE_JSON)
    run.shot("03-untrusted-confirm")
    d.js("document.querySelector('[data-confirm-untrusted]').click()")
    check("confirming enables it", d.js("return document.querySelector('[data-apply]').disabled") is False)
    apply_pending(d)
    written = json.loads(read(claude_json))["mcpServers"]["build"]
    check("the confirmed server is in the user's file with the absolute path", written["args"][0] == mcp_js, str(written))
    check("nothing of the project's environment was written, and no reference to the user's variables was invented", "env" not in written and "NODE_OPTIONS" not in read(claude_json) and "${GH}" not in read(claude_json), str(written))
    check("the rest of the user's file is untouched", json.loads(read(claude_json))["theme"] == "dark" and json.loads(read(claude_json))["mcpServers"]["files"]["env"]["API_KEY"] == SECRET)
    check("the project's own file is unchanged", read(os.path.join(PROJ, ".mcp.json")) == PROJECT_MCP)

    # ---------- 3b. the opt-in lists and writes exactly what the repository declares ----------
    click(d, '[data-server-file="claude:project"][data-server="build"] [data-copy-open]')
    d.wait("return !!document.querySelector('[data-copy-form]')", 10)
    d.js("document.querySelector('[data-copy-target=\"codex:user\"]').click()")
    check("the project server's environment can be opted in for a file of the user's", d.js("return document.querySelector('[data-copy-env]').disabled") is False)
    d.js("document.querySelector('[data-copy-env]').click()")
    click(d, "[data-copy-preview]")
    panel_ready(d)
    uv_node = d.js("return document.querySelector('[data-untrusted] [data-uv=\"NODE_OPTIONS\"]')?.innerText || ''")
    check("ticked, the listing says which values will be written", "will be written" in uv_node, uv_node)
    d.js("document.querySelector('[data-confirm-untrusted]').click()")
    apply_pending(d)
    check("exactly those values were written to Codex's file", 'NODE_OPTIONS = "--require ./x.js"' in read(codex) and "GITHUB_TOKEN" not in read(codex), read(codex))

    # ---------- 4. env never in project files ----------
    click(d, '[data-server-file="claude:user"][data-server="files"] [data-copy-open]')
    d.wait("return !!document.querySelector('[data-copy-form]')", 10)
    d.js("document.querySelector('[data-copy-target=\"cursor:project\"]').click()")
    check("the environment opt-in is disabled for a project-only copy", d.js("return document.querySelector('[data-copy-env]').disabled") is True)
    click(d, "[data-copy-preview]")
    panel_ready(d)
    ptext = panel_text(d)
    check("the diff of a secret-bearing copy shows no value", SECRET not in ptext and "${env:API_KEY}" in ptext, ptext[:400])
    apply_pending(d)
    cursor_file = os.path.join(PROJ, ".cursor", "mcp.json")
    check("the project file got a reference, not the value", os.path.exists(cursor_file) and SECRET not in read(cursor_file) and "${env:API_KEY}" in read(cursor_file))
    leaked = [os.path.join(r, f) for r, _, fs in os.walk(PROJ) for f in fs if SECRET in read(os.path.join(r, f))]
    check("no file in the project holds the secret", not leaked, str(leaked))
    # To a user-level file the values need the opt-in.
    click(d, '[data-server-file="claude:user"][data-server="files"] [data-copy-open]')
    d.wait("return !!document.querySelector('[data-copy-form]')", 10)
    d.js("document.querySelector('[data-copy-target=\"gemini:user\"]').click()")
    check("the opt-in is available for a user-level file", d.js("return document.querySelector('[data-copy-env]').disabled") is False)
    click(d, "[data-copy-preview]")
    panel_ready(d)
    gemini = os.path.join(HOME, ".gemini", "settings.json")
    apply_pending(d)
    check("without the opt-in the Gemini file holds a reference", SECRET not in read(gemini) and "${API_KEY}" in read(gemini), read(gemini))

    # ---------- 5. backups ----------
    click(d, "[data-backups] summary")
    d.wait("return document.querySelectorAll('[data-backup]').length > 0", 10)
    n_backups = count(d, "[data-backup]")
    check("the backups are listed", n_backups >= 4, str(n_backups))
    backed = sorted(os.listdir(os.path.join(DATA, "agent-config-backups")))
    check("each backup is a copy plus a record", len([f for f in backed if f.endswith(".bak")]) >= 3 and len([f for f in backed if f.endswith(".json")]) >= 4, str(backed))
    run.shot("04-backups")
    # Restore the Gemini file to how it was before the copy.
    d.js("""const rows = [...document.querySelectorAll('[data-backup]')].filter(r => r.innerText.includes('settings.json'));
        rows[0].querySelector('[data-restore]').click();""")
    check("restoring shows the difference first", bool(panel_ready(d)) and "Restore" in panel_text(d))
    check("the preview of a restore wrote nothing", "${API_KEY}" in read(gemini))
    apply_pending(d)
    check("the file is back as it was", read(gemini) == GEMINI_JSON, read(gemini))

    # ---------- 6. auto-restore ----------
    before = read(codex)
    open(os.path.join(DATA, "agent-setup-corrupt-next"), "w").close()
    d.js("document.querySelector('[data-server-file=\"codex:user\"][data-server=\"docs\"] .switch').click()")
    panel_ready(d)
    apply_pending(d)
    err = until(lambda: d.js("return document.querySelector('[data-change-error]')?.innerText || ''"), 10) or ""
    check("a write that breaks the file is reported", "no longer parses" in err and "back as they were" in err, err)
    check("the file was put back at once", read(codex) == before)
    check("the injected failure fired once", not os.path.exists(os.path.join(DATA, "agent-setup-corrupt-next")))
    run.shot("05-auto-restore")
    click(d, "[data-change-panel] [data-cancel]")
    d.js("document.querySelector('[data-server-file=\"codex:user\"][data-server=\"docs\"] .switch').click()")
    panel_ready(d)
    apply_pending(d)
    check("the next attempt works", "enabled = true" in read(codex), read(codex))

    # ---------- 7. add once, usable in three agents ----------
    click(d, "[data-add-server] summary")
    set_input(d, "[data-new-name]", "shared tool".replace(" ", "-"))
    set_input(d, "[data-new-command]", 'node "C:\\Program Files\\tool\\srv.js" --root "D:\\ünï côdé"')
    for t in ("claude:user", "codex:user", "gemini:user"):
        d.js("document.querySelector(arguments[0]).click()", f'[data-new-target="{t}"]')
    click(d, "[data-add-preview]")
    check("adding shows a preview of three files", bool(panel_ready(d)) and count(d, "[data-diff-file]") == 3)
    apply_pending(d)
    claude_doc = json.loads(read(claude_json))
    srv = claude_doc["mcpServers"].get("shared-tool", {})
    check("Claude has it", srv.get("command") == "node" and srv.get("args") == ["C:\\Program Files\\tool\\srv.js", "--root", "D:\\ünï côdé"], str(srv))
    check("Codex has it", "[mcp_servers.shared-tool]" in read(codex) and "# my codex" in read(codex))
    check("Gemini has it", json.loads(read(gemini))["mcpServers"]["shared-tool"]["command"] == "node")

    # ---------- 7b. Claude's own lock ----------
    lock_dir = claude_json + ".lock"
    os.mkdir(lock_dir)
    d.js("document.querySelector('[data-server-file=\"claude:user\"][data-server=\"remote\"] [data-remove]').click()")
    panel_ready(d)
    before_remove = read(claude_json)
    apply_pending(d)
    err = until(lambda: d.js("return document.querySelector('[data-change-error]')?.innerText || ''"), 15) or ""
    check("while Claude holds its lock nothing is written, and it says so", "Claude Code is saving" in err and read(claude_json) == before_remove, err)
    check("the other program's lock is left alone", os.path.isdir(lock_dir))
    os.rmdir(lock_dir)
    click(d, "[data-apply]")
    check("once the lock is free the change is saved", bool(until(lambda: not exists(d, "[data-change-panel]"), 20)) and '"remote"' not in read(claude_json))
    check("Kelpie's own lock is gone afterwards", not os.path.exists(lock_dir))

    # ---------- 8. one file for all agents ----------
    click(d, '[data-tab="instructions"]')
    d.wait("return !!document.querySelector('[data-one-file]')", 10)
    check("the instruction files are listed", exists(d, '[data-instruction="claude"]') and exists(d, '[data-instruction="agents"]'))
    check("the status says CLAUDE.md does not import AGENTS.md", "does not import" in d.js("return document.querySelector('[data-claude-imports]').innerText"))
    click(d, "[data-unify]")
    check("one file for all agents shows a preview", bool(panel_ready(d)))
    apply_pending(d)
    check("AGENTS.md holds the text and CLAUDE.md imports it",
          read(os.path.join(PROJ, "AGENTS.md")) == "# Rules\nUse tabs.\n" and read(os.path.join(PROJ, "CLAUDE.md")) == "@AGENTS.md\n")
    check("Gemini lists AGENTS.md in its settings", "AGENTS.md" in read(os.path.join(PROJ, ".gemini", "settings.json")))
    check("the status updates", until(lambda: "● CLAUDE.md imports" in d.js("return document.querySelector('[data-claude-imports]')?.innerText || ''"), 10) is not None)
    run.shot("06-instructions")

    # ---------- 9. skills ----------
    click(d, '[data-tab="skills"]')
    d.wait("return !!document.querySelector('[data-skill-kind]')", 10)
    stext = page_text(d)
    check("skills, commands and subagents are listed with their descriptions",
          all(x in stext for x in ("pdf", "Work with PDF files", "review", "Review the diff", "planner", "Plans the work", "ship")), stext[:300])
    check("they are read-only: no switch, no remove", count(d, "[data-skill-kind] .switch") == 0 and count(d, "[data-skill-kind] [data-remove]") == 0)
    run.shot("07-skills")

    # ---------- 10. find more agents ----------
    click(d, '[data-tab="find"]')
    d.wait("return !!document.querySelector('[data-registry-search]')", 10)
    time.sleep(1)
    check("the registry is not asked until the button is pressed", len(registry_hits) == 0 and calls(d, "agent_registry_fetch") == 0)
    click(d, "[data-registry-search]")
    check("one click is one request", bool(until(lambda: exists(d, '[data-registry-agent="alpha-acp"]'), 15)) and len(registry_hits) == 1, str(registry_hits))
    atext = d.js("return document.querySelector('[data-registry-agent=\"alpha-acp\"]').innerText")
    check("the full command is shown with the version pinned", "npx -y @acme/alpha@1.4.0 --acp" in atext, atext)
    check("an entry shipped as a program download cannot be saved here", not exists(d, '[data-registry-save="gamma"]') and "downloaded" in d.js("return document.querySelector('[data-registry-agent=\"gamma\"]').innerText"))
    click(d, '[data-registry-save="alpha-acp"]')
    d.wait("return !!document.querySelector('[data-registry-confirm]')", 10)
    check("saving asks first, with the exact command", "npx -y @acme/alpha@1.4.0 --acp" in d.js("return document.querySelector('[data-registry-confirm]').innerText"))
    check("nothing is saved before the confirmation", not (d.js(f"return {S}.state.settings.registryAgents") or []) and "acp-alpha-acp" not in [p["id"] for p in daemon(d, "agent.presets")])
    run.shot("08-registry-confirm")
    click(d, "[data-registry-confirm] .btn:not(.primary)")
    check("cancelling saves nothing", not exists(d, "[data-registry-confirm]") and not (d.js(f"return {S}.state.settings.registryAgents") or []))
    click(d, '[data-registry-save="alpha-acp"]')
    d.wait("return !!document.querySelector('[data-registry-confirm]')", 10)
    click(d, "[data-registry-confirm-yes]")
    saved = until(lambda: "acp-alpha-acp" in [p["id"] for p in daemon(d, "agent.presets")], 15)
    check("after the confirmation the daemon has the custom agent (agent.set_custom)", bool(saved))
    preset = next((p for p in daemon(d, "agent.presets") if p["id"] == "acp-alpha-acp"), {})
    check("it is started through npx with the pinned version", preset.get("bin") == "npx" and preset.get("acp") == ["-y", "@acme/alpha@1.4.0", "--acp"], str(preset))
    check("saving ran nothing and asked the registry no more", len(daemon(d, "session.list")) == sessions_before and len(registry_hits) == 1)
    check("it is listed as saved", exists(d, '[data-saved-agent="acp-alpha-acp"]'))
    run.shot("09-registry-saved")
    click(d, '[data-forget="acp-alpha-acp"]')
    check("removing it takes it from the daemon again", bool(until(lambda: "acp-alpha-acp" not in [p["id"] for p in daemon(d, "agent.presets")], 15)))

    # ---------- 11. a project folder that links out ----------
    outside = os.path.join(WORK, "outside")
    os.makedirs(outside, exist_ok=True)
    write(os.path.join(outside, "settings.json"), '{"theirs": true}\n')
    gem = os.path.join(PROJ, ".gemini")
    os.rename(gem, gem + "-real")
    made = subprocess.run(["cmd", "/C", "mklink", "/J", gem, outside], capture_output=True).returncode == 0 if os.name == "nt" else (os.symlink(outside, gem) is None)
    check("a directory link out of the project could be made for the test", made)
    try:
        click(d, '[data-tab="mcp"]')
        click(d, "[data-refresh]")
        until(lambda: exists(d, '[data-mcp-file="gemini:project"] [data-file-error]'), 10)
        ferr = d.js("return document.querySelector('[data-mcp-file=\"gemini:project\"] [data-file-error]')?.innerText || ''")
        check("the project's Gemini file is not read: the reason names where the link leads", "resolves outside the project" in ferr and os.path.basename(outside) in ferr, ferr)
        click(d, '[data-tab="mcp"]')
        d.wait("return !!document.querySelector('[data-mcp-agent]')", 10)
        click(d, '[data-server-file="claude:user"][data-server="files"] [data-copy-open]')
        d.wait("return !!document.querySelector('[data-copy-form]')", 10)
        d.js("document.querySelector('[data-copy-target=\"gemini:project\"]').click()")
        click(d, "[data-copy-preview]")
        msg_text = until(lambda: d.js("return document.querySelector('[data-message]')?.innerText || ''"), 10) or ""
        check("a copy into the linked folder is refused with the real target", "resolves outside the project" in msg_text and "outside" in msg_text, msg_text)
        check("the folder it leads to is untouched", read(os.path.join(outside, "settings.json")) == '{"theirs": true}\n' and os.listdir(outside) == ["settings.json"])
        click(d, '[data-tab="instructions"]')
        d.wait("return !!document.querySelector('[data-unify]')", 10)
        before_agents = read(os.path.join(PROJ, "AGENTS.md"))
        click(d, "[data-unify]")
        msg_text = until(lambda: d.js("return document.querySelector('[data-message]')?.innerText || ''"), 10) or ""
        check("one file for all agents refuses too, and writes nothing", "resolves outside the project" in msg_text and read(os.path.join(PROJ, "AGENTS.md")) == before_agents, msg_text)
        run.shot("10-link-refused")
    finally:
        if os.path.isdir(gem):
            os.rmdir(gem)
        if os.path.isdir(gem + "-real") and not os.path.exists(gem):
            os.rename(gem + "-real", gem)

    # ---------- the real files ----------
    check("the real agent config files were not touched", [stat_of(p) for p in REAL_FILES] == real_before)
    check("nothing but fixtures changed: the project's .mcp.json is as it was", read(os.path.join(PROJ, ".mcp.json")) == PROJECT_MCP)


def main():
    make_fixtures()
    server = ThreadingHTTPServer(("127.0.0.1", REGISTRY_PORT), Registry)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT and os.environ["KELPIE_DATA_DIR"] == DATA
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
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", NATIVE])
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
        server.shutdown()
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
    shutil.rmtree(WORK, ignore_errors=True)


if __name__ == "__main__":
    main()
