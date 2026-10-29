"""GUI e2e for Sessions panel → Export as Markdown… (Claude, Codex and OMP conversations).

    python -X utf8 scripts/e2e/session_export.py <screenshot-dir> <scratch-dir> <app.exe>

Needs the debug app and daemon (`cargo build -p kelpied`, `cd app && pnpm tauri build --debug --no-bundle`),
`tauri-driver` on PATH (or in ~/.cargo/bin) and `msedgedriver.exe` matching the installed WebView2 (on PATH, or
in MSEDGEDRIVER). The script starts its own kelpied (isolated endpoint and data folder) and tauri-driver (free
`--port` and `--native-port`) with the same environment, and stops both on exit. Everything it sets is set in
this process before anything starts: KELPIE_ENDPOINT, KELPIE_DATA_DIR, and the agents' folders —
CLAUDE_CONFIG_DIR, CODEX_HOME and KELPIE_AGENT_CONFIG_HOME (OMP's sessions, honoured with KELPIE_E2E only) —
all under <scratch-dir>, holding copies of the synthetic fixture transcripts in
app/src-tauri/tests/fixtures/session-export. The user's daemon, transcripts and keyring are never reached; no
network, no agent runs. The save dialog is answered through `window.__kelpie.sessionExport.replacePicker`;
everything else is the UI.

1. The Sessions panel lists exactly the three fixture conversations, each with an export button.
2. Cancelling the save dialog writes nothing and says nothing.
3. Each conversation exported through its row's button equals its golden file byte for byte (LF endings), the
   seeded secrets are gone, the toast names the file, no temporary file is left; the suggested name carries
   agent, id and day only.
4. Exporting again over an existing file replaces it whole.
5. The button's label is translated (Vietnamese).
"""
import importlib.util
import os
import re
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
FIXTURES = os.path.abspath(os.path.join(HERE, "..", "..", "app", "src-tauri", "tests", "fixtures", "session-export"))

ENDPOINT = f"kelpie-session-export-{os.getpid()}"
os.makedirs(scratch, exist_ok=True)
DATA = tempfile.mkdtemp(prefix="kelpie-session-export-data-", dir=scratch)
WORK = os.path.join(scratch, f"session-export-{os.getpid()}")
HOME = os.path.join(WORK, "home")
PROJ = os.path.join(WORK, "proj")
OUT = os.path.join(WORK, "xuất ra")  # a space and non-ASCII letters in the chosen folder

# (agent, conversation id, fixture name, where the agent keeps it under HOME, the title the panel shows)
CASES = [
    ("claude", "0c1d2e3f-0000-4000-8000-000000000001", "claude",
     os.path.join(".claude", "projects", "D--work-demo", "0c1d2e3f-0000-4000-8000-000000000001.jsonl"), "Fix the flaky build"),
    ("codex", "019a1b2c-3d4e-7f00-8000-000000000002", "codex",
     os.path.join(".codex", "sessions", "2026", "10", "24", "rollout-2026-10-24T02-00-00-019a1b2c-3d4e-7f00-8000-000000000002.jsonl"), "Move the API key out of config.ts"),
    ("omp", "omp-fixture-0003", "omp",
     os.path.join(".omp", "agent", "sessions", "--C--Users-dev-demo--", "2026-10-24T03-00-00-000Z_omp-fixture-0003.jsonl"), "Clean up the logger"),
]
SECRETS = ["ghp_FAKEfakeFAKEfakeFAKEfakeFAKEfake1234", "sk-proj-FAKEfakeFAKEfake0123456789", "abcdEFGHijklMNOPqrst1234",
           "Sup3rS3cretPw", "wJalrXUtnFEMIbPxRfiCYFAKEKEY99", "hunter2hunter2"]

os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_E2E": "1",
    "KELPIE_AGENT_CONFIG_HOME": HOME,
    "CLAUDE_CONFIG_DIR": os.path.join(HOME, ".claude"), "CODEX_HOME": os.path.join(HOME, ".codex"),
})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)
for p in (DATA, os.environ["KELPIE_AGENT_CONFIG_HOME"], os.environ["CLAUDE_CONFIG_DIR"], os.environ["CODEX_HOME"]):
    assert os.path.abspath(p).startswith(scratch), f"{p} must live under the scratch folder"


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
NATIVE = os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")
TAURI_DRIVER = shutil.which("tauri-driver") or shutil.which(os.path.join(os.path.expanduser("~"), ".cargo", "bin", "tauri-driver"))
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit("needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver (PATH or MSEDGEDRIVER)")
if not os.path.exists(kelpied):
    raise SystemExit(f"no daemon next to the app: {kelpied}")

spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kc)
assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT

procs = {}
S = "window.__kelpie.store"
SX = "window.__kelpie.sessionExport"


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def rm(path):
    for _ in range(20):
        if not os.path.exists(path):
            return
        try:
            shutil.rmtree(path)
        except OSError:
            time.sleep(0.5)
    print(f"could not remove {path}", flush=True)


def read_bytes(path):
    with open(path, "rb") as f:
        return f.read()


def make_fixtures():
    for _, xid, name, rel, title in CASES:
        dest = os.path.join(HOME, rel)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(os.path.join(FIXTURES, name + ".jsonl"), dest)
        if name == "codex":
            # Codex keeps thread names in their own index.
            with open(os.path.join(HOME, ".codex", "session_index.jsonl"), "w", encoding="utf-8", newline="\n") as f:
                f.write('{"id":"%s","thread_name":"%s","updated_at":"2026-10-24T02:04:00Z"}\n' % (xid, title))
    os.makedirs(PROJ)
    os.makedirs(OUT)


def toast(d):
    return d.js("return [...document.querySelectorAll('.toast')].map(t => t.innerText).join('\\n')") or ""


def row(agent):
    return f'.history-row[data-agent="{agent}"]'


def export_row(d, agent, target):
    """Answers the save dialog with `target` (None: Cancel) and clicks the row's export button."""
    d.js(f"window.__suggested = null; const target = arguments[0]; {SX}.replacePicker((p) => {{ window.__suggested = p; return Promise.resolve(target); }});", target)
    d.js("document.querySelector(arguments[0]).click()", f'{row(agent)} [data-act="export"]')
    return until(lambda: d.js("return window.__suggested"), 10)


def scenario(run):
    d = run.d
    run.boot()
    d.set_window_size(1480, 920)
    d.wait(f"return typeof {SX}?.replacePicker === 'function' || null", 20)

    pid = d.js(f"return {S}.addProject(arguments[0])", PROJ)
    d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('history')", pid)
    d.wait("return document.querySelector('.history .history-scope') || null", 15)
    d.js("[...document.querySelectorAll('.history-scope .seg button')].find(b => b.textContent === 'All').click()")

    # 1. the fixture conversations, and nothing else
    rows = "return [...document.querySelectorAll('.history-row')].map(r => r.dataset.agent + ':' + r.dataset.id)"
    listed = until(lambda: (v := d.js(rows)) and len(v) >= 3 and v, 30) or d.js(rows)
    want = sorted(f"{a}:{i}" for a, i, *_ in CASES)
    run.check("the Sessions panel lists exactly the three fixture conversations", sorted(listed or []) == want, str(listed))
    titles = d.js("return Object.fromEntries([...document.querySelectorAll('.history-row')].map(r => [r.dataset.agent, r.querySelector('.history-title').textContent]))") or {}
    run.check("…with the titles the export uses", all(titles.get(a) == t for a, _, _, _, t in CASES), str(titles))
    buttons = d.js("return [...document.querySelectorAll('.history-row')].filter(r => r.querySelector('[data-act=\"export\"]')).length")
    run.check("…each with an Export as Markdown… button", buttons == 3 and d.js("return document.querySelector('[data-act=\"export\"]').title") == "Export as Markdown…", str(buttons))
    run.shot("01-sessions-panel")

    # 2. Cancel
    suggested = export_row(d, "claude", None)
    time.sleep(1)
    run.check("Cancel writes nothing and shows nothing", bool(suggested) and os.listdir(OUT) == [] and "xport" not in toast(d), f"{suggested} {os.listdir(OUT)} {toast(d)!r}")
    run.check("…the suggested name is agent, id and day only", bool(re.fullmatch(r"claude-0c1d2e3f-\d{4}-\d{2}-\d{2}\.md", suggested or "")), suggested)

    # 3. each agent, through its row's button
    for agent, xid, name, _, _ in CASES:
        target = os.path.join(OUT, f"{agent}.md")
        suggested = export_row(d, agent, target)
        written = until(lambda: os.path.exists(target), 20)
        said = until(lambda: (s := toast(d)) and target in s and "1 unreadable transcript line was left out." in s and s, 10)
        golden = read_bytes(os.path.join(FIXTURES, name + ".md"))
        got = read_bytes(target) if written else b""
        run.check(f"{agent}: the export equals {name}.md byte for byte", got == golden, f"{len(got)} vs {len(golden)} bytes")
        run.check(f"{agent}: LF line endings only", bool(got) and b"\r" not in got)
        text = got.decode("utf-8", "replace")
        raw = open(os.path.join(FIXTURES, name + ".jsonl"), encoding="utf-8").read()
        seeded = [s for s in SECRETS if s in raw]
        run.check(f"{agent}: the {len(seeded)} seeded secrets are gone", bool(seeded) and not any(s in text for s in seeded) and "[redacted]" in text, str([s for s in seeded if s in text]))
        run.check(f"{agent}: the toast names the file and the unreadable line", bool(said), said or toast(d))
        run.check(f"{agent}: suggested {suggested}", bool(re.fullmatch(rf"{agent}-{re.escape(xid[:8])}-\d{{4}}-\d{{2}}-\d{{2}}\.md", suggested or "")))
        run.check(f"{agent}: no temporary file is left beside it", not [f for f in os.listdir(OUT) if f.endswith(".tmp")], str(os.listdir(OUT)))
    run.shot("02-exported")

    # 4. over an existing file
    target = os.path.join(OUT, "claude.md")
    with open(target, "w", encoding="utf-8") as f:
        f.write("an older export\n" * 5000)
    export_row(d, "claude", target)
    # Wait for the toast, not on the file: an open read handle would make the rename over it fail.
    said = until(lambda: (s := toast(d)) and f"Exported to {target}" in s and s, 20)
    replaced = bool(said) and read_bytes(target) == read_bytes(os.path.join(FIXTURES, "claude.md"))
    run.check("exporting over an existing file replaces it whole", bool(replaced) and not [f for f in os.listdir(OUT) if f.endswith(".tmp")], f"{os.listdir(OUT)} {said or toast(d)!r}")

    # 5. Vietnamese
    d.js(f"{S}.setSettings({{ language: 'vi' }})")
    vi = until(lambda: d.js("return document.querySelector('[data-act=\"export\"]').title === 'Xuất ra Markdown…' || null"), 10)
    run.check("the button is translated", bool(vi), d.js("return document.querySelector('[data-act=\"export\"]').title"))
    d.js(f"{S}.setSettings({{ language: 'en' }})")


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def spawn(name, args):
    procs[name] = subprocess.Popen(args, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _conn():
    try:
        return kc.Conn()
    except (OSError, SystemExit):
        return None


def main():
    try:
        make_fixtures()
    except Exception:
        teardown(None)
        raise
    spawn("daemon", [kelpied, "serve"])
    if not until(_conn, 15, 0.1):
        teardown(None)
        raise SystemExit("the daemon did not start")
    spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE])
    if not until(port_open, 30):
        teardown(None)
        raise SystemExit("tauri-driver did not open its port")
    run = None
    try:
        run = Run(shots, app=app)
        scenario(run)
    except Exception:
        traceback.print_exc()
        if run is None:
            teardown(None)
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        if run is not None:
            teardown(run)
    run.finish()


def teardown(run):
    """Quits the app first (it would start a daemon of its own if the test daemon went away), then the driver
    tree this script started, then the test daemon; removes the test data and fixture folders."""
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
    except (Exception, SystemExit):
        pass
    for p in procs.values():
        try:
            p.wait(timeout=10)
        except Exception:
            p.kill()
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    rm(DATA)
    rm(WORK)


if __name__ == "__main__":
    main()
