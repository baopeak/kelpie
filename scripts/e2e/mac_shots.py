#!/usr/bin/env python3
"""README pictures of the Mac app: the real debug app and its daemon, on the demo projects of readme_demo.py.

    python3 scripts/e2e/mac_shots.py <out-dir> <demo-root> [--target DIR]

<demo-root> shows in some pictures (the floating terminal, search): /Users/Shared/kelpie-demo reads well. Without its projects.json
the kit is made there first (readme_demo.py, which needs Python 3.10+: macOS's own python3 is 3.9, so seed it with python3.12).

The Mac app has no WebDriver, so the app is a debug build made with plain `cargo build -p kelpie -p kelpied -p kelpie-screen`
(no bundle): such a build loads its page from the Vite dev server (devUrl, port 1427), which this script starts through
mac_bridge_vite.mjs with a bridge that runs the tour's code in the page. `--target` is the Cargo target folder (default
CARGO_TARGET_DIR, else ./target); `cd app && pnpm install` first. The window is moved and sized through Accessibility
(mac_window.swift) and captured with `screencapture -l`, so the shell running this needs Accessibility and Screen Recording
(Terminal, or sshd over SSH); over SSH, unlock the login keychain first.

What it captures, into <out-dir> as mac-<name>.png:
- the whole tour of readme_shots.capture (welcome, workspace, search, changes, editor, review notes, markdown, CSV, plan, new
  session, dashboard, board, layouts, composer, terminal menu, floating terminal, inbox, worktree dialog, automations, settings
  pages, notebook, processes, runs, Try, Finish, SSH hosts, plugins, light theme, Vietnamese), with zsh in place of PowerShell;
- chat view with tool activity, a chat waiting for an approval and the dashboard with agents waiting, from the simulated agents of
  demo_agents.py (never a real agent; marked "simulated" in media.json);
- Settings → Mobile with the Mac's Screen Recording and Accessibility rows, and other settings pages;
- `mac-hero`: the whole 1600x1200 screen with the app filling it (the Dock is hidden while it is taken, then restored).
<out-dir>/media.json lists each picture with what made it.

Isolation: this process's own daemon endpoint and data folder (phone_support.isolate) and an empty SSH folder, before anything
starts; Kelpie's real data and daemon are never reached. Only the processes this script started are stopped. Port 1427 must be
free (no other Vite dev server of Kelpie running).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import phone_support as ps  # noqa: E402
import readme_shots  # noqa: E402
from demo_agents import write_programs  # noqa: E402
from readme_demo import seed  # noqa: E402
from smoke_support import Run, daemon  # noqa: E402
from wd import Driver  # noqa: E402

REPO = HERE.parents[1]
BRIDGE = "http://localhost:1427/__shots"
FRAME = (40, 60, 1440, 900)  # the window for every picture but the hero
S = readme_shots.S

# Keys as the Windows tour names them: a chord runs the command bound to it on Windows (the Mac binds some to other keys), a named
# key is sent to the focused element, and text is typed into the focused field.
KEYS_JS = r"""
const chord = arguments[0];
const named = { escape: "Escape", enter: "Enter", tab: "Tab" };
if (chord.length > 1 && chord.includes("+")) {
  const norm = (c) => {
    const parts = c.toLowerCase().split("+").map((x) => (x === "ctrl" ? "mod" : x));
    const key = parts.pop();
    const order = ["mod", "ctrl", "meta", "shift", "alt"];
    return [...new Set(parts)].sort((a, b) => order.indexOf(a) - order.indexOf(b)).concat(key).join("+");
  };
  const c = window.__kelpie.commands.find((x) => x.win && norm(x.win) === norm(chord));
  if (!c) throw new Error("no command for " + chord);
  c.run();
  return c.id;
}
const t = document.activeElement || document.body;
if (named[chord]) {
  for (const type of ["keydown", "keyup"]) t.dispatchEvent(new KeyboardEvent(type, { key: named[chord], code: named[chord], bubbles: true, cancelable: true }));
  return chord;
}
if (!t || !("value" in t)) throw new Error("no field to type into");
const set = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(t), "value").set;
set.call(t, t.value + chord);
t.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: chord }));
return "typed";
"""


def bridge(code, args=(), timeout=60):
    body = json.dumps({"code": code, "args": list(args), "timeout": timeout}).encode()
    req = urllib.request.Request(BRIDGE, data=body, headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout + 10) as r:
        out = json.loads(r.read())
    if "error" in out:
        raise RuntimeError(out["error"][:800])
    return out["value"]


def pages():
    try:
        with urllib.request.urlopen(BRIDGE, timeout=3) as r:
            return json.loads(r.read())["pages"]
    except (urllib.error.URLError, OSError, ValueError, KeyError):
        return None


class MacDriver(Driver):
    """The few things the Windows tour asks of WebDriver, done through the bridge, Accessibility and `screencapture`."""

    def __init__(self, pid, helper):  # noqa: super().__init__ starts a WebDriver session: not here
        self.pid, self.helper = pid, helper

    def _req(self, method, path, body=None):
        return None  # WebDriver's own settings (timeouts): nothing to set

    def s(self, path):
        return path

    def js(self, script, *args):
        return bridge("return await (async function () {\n" + script + "\n}).apply(null, arguments);", args)

    def js_async(self, script, *args):
        return bridge("const a = Array.from(arguments);\nreturn await new Promise((done, fail) => { try { (function () {\n" + script
                      + "\n}).apply(null, [...a, done]); } catch (e) { fail(e); } });", args, 240)

    def window(self, frame=None):
        out = subprocess.run([self.helper, str(self.pid), *map(str, frame or ())], capture_output=True, text=True, timeout=30).stdout.split()
        if len(out) != 5:
            raise RuntimeError(f"no window for the app: {' '.join(out)}")
        return [int(x) for x in out]

    def set_window_size(self, width, height):
        self.window(FRAME)

    def screenshot(self, path):
        wid = self.window()[0]
        subprocess.run(["screencapture", "-x", "-o", "-l", str(wid), path], check=True, timeout=30)

    def click(self, css):
        self.js("const e = document.querySelector(arguments[0]); if (!e) throw new Error('nothing to click: ' + arguments[0]); e.click();", css)

    def keys(self, *chords):
        for chord in chords:
            self.js(KEYS_JS, chord)
            time.sleep(0.15)

    def pointer(self, steps):
        pass  # the tour only moves the mouse off a hover hint; no mouse is moved here

    def quit(self):
        pass


class MacRun(Run):
    def __init__(self, shots, driver):  # noqa: Run.__init__ starts WebDriver
        self.shots = shots
        self.results = []
        self.d = driver


def capture_mac(run, projects, out, programs, port):
    """The Mac's own pictures, after the shared tour: simulated agents, Settings → Mobile, more settings pages, the hero."""
    d = run.d
    shots = {}

    def shot(name, how, agent=None, wait=1.0):
        time.sleep(wait)
        d.screenshot(str(out / (name + ".png")))
        shots[name] = {"how": how, **({"agent": agent} if agent else {})}
        print("SHOT " + name, flush=True)

    ids = d.js(f"return {S}.state.projects.filter(p => !p.parentId).map(p => [p.id, p.path])")
    by_path = {path: pid for pid, path in ids}
    by_name = {p["name"]: by_path[p["path"]] for p in projects if p["path"] in by_path}
    atlas, beacon, relay = by_name["atlas-commerce"], by_name["beacon-api"], by_name["relay-workers"]
    py = sys.executable

    def chat(pid, name, program, where="tab"):
        return d.js("""const [pid, name, argv, where] = arguments; const k = window.__kelpie;
            const p = k.store.state.projects.find((x) => x.id === pid);
            const info = await k.ipc.createSession({ kind: 'claude', cwd: p.path, project_id: pid, name, chat_argv: argv });
            k.store.setSessions(info.id, info);
            k.store.placePane(pid, { id: 'demo-' + info.id, kind: 'chat', sessionId: info.id }, where);
            return info;""", pid, name, [py, "-u", programs[program]], where)

    def shell(pid, name, command, where="tab"):
        info = d.js_async(f"const done = arguments[arguments.length-1]; {S}.newSession(arguments[0], 'shell', {{name: arguments[1], where: arguments[2]}}).then(done)", pid, name, where)
        time.sleep(2)
        daemon(d, "session.input", {"handle": info["handle"], "data": readme_shots.PROMPT + "\r"})
        time.sleep(0.5)
        if command:
            daemon(d, "session.input", {"handle": info["handle"], "data": command + "\r"})
        return info

    def items_of(sid):
        return d.js("const s = window.__kelpie.store.sessions[arguments[0]]; const c = s && window.__kelpie.chat.getController(s); return c ? c.state.items.map(i => i.kind + ':' + (i.text || i.name || '')) : []", sid)

    simulated = "simulated: a local program speaking Claude Code's stream format (demo_agents.py); no model was called"

    # Chat view with tool activity, finished.
    d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('files')", atlas)
    work = chat(atlas, "Fix pasted coupon codes", "chat_work")
    run.check("the simulated chat shows its answer", bool(readme_shots_wait(lambda: any("Fixed." in i for i in items_of(work["id"])), 30)))
    shot("chat", "chat view fed by a simulated agent", "simulated", 2)

    # A chat waiting for an approval.
    d.js(f"{S}.switchProject(arguments[0])", beacon)
    approval = chat(beacon, "Release 0.4.0", "chat_approval")
    run.check("the simulated chat asks for an approval", bool(readme_shots_wait(lambda: any(i.startswith("permission") for i in items_of(approval["id"])), 30)))
    shot("chat-approval", "chat view waiting for an approval, fed by a simulated agent", "simulated", 2)

    # A shell session waiting at a menu, a working one, then the dashboard with all of them.
    menu = shell(relay, "Migration", f"python3 -u '{programs['menu']}'")
    shell(relay, "Checks (watch)", f"python3 -u '{programs['ticker']}'", "right")
    readme_shots_wait(lambda: any(s["handle"] == menu["handle"] and s.get("status") == "waiting" for s in daemon(d, "session.list")), 30)
    d.keys("ctrl+shift+m")
    shot("dashboard-agents", "Agent Dashboard: simulated chat sessions and shells waiting or working", "simulated", 2)
    d.keys("escape")
    d.js(f"{S}.setOverlay(null)")

    # Settings → Mobile, with phone access on (this run's loopback certificate) and the Mac's permission rows.
    cert, key = ps.certificate_for_loopback(str(out.parent))
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": port, "cert": cert, "key": key})
    d.js(f"{S}.openSettings('mobile')")
    readme_shots_wait(lambda: d.js("return !!document.querySelector('[data-mobile-advanced]')"), 15)
    shot("settings-mobile", "Settings → Mobile, phone access on in an isolated daemon (no pairing code shown)")
    d.js("const e = document.querySelector('[data-mobile-advanced]'); e.open = true; e.dispatchEvent(new Event('toggle'));")
    found = readme_shots_wait(lambda: d.js("const e = document.querySelector('[data-mac-permissions]'); if (!e) return false; e.scrollIntoView({block: 'start'}); "
                                         "let s = e.parentElement; while (s && !(s.scrollHeight > s.clientHeight && /auto|scroll/.test(getComputedStyle(s).overflowY))) s = s.parentElement; "
                                         "if (s) s.scrollTop -= 170; return true"), 20)
    run.check("Settings → Mobile shows the Mac's Screen Recording and Accessibility rows", bool(found))
    shot("mobile-permissions", "Settings → Mobile → Advanced: the Mac's Screen Recording and Accessibility rows")
    d.keys("escape")
    for section in ("notifications", "layouts", "remote", "commands", "daemon", "voice", "about"):
        d.js(f"{S}.openSettings(arguments[0])", section)
        shot(f"settings-{section}", f"Settings → {section}", wait=1.2)
        d.keys("escape")
    d.js(f"{S}.setOverlay(null)")

    # The hero: the whole screen, the app filling it, busy with demo work.
    d.js(f"{S}.switchProject(arguments[0])", atlas)
    first = d.js(f"return {S}.workspace(arguments[0]).tabs[0].id", atlas)
    d.js(f"{S}.selectTab(arguments[0], arguments[1]); {S}.showRightPanel('files')", atlas, first)
    hero_chat = chat(atlas, "Coupon fix (simulated)", "chat_work", "right")
    readme_shots_wait(lambda: any("Fixed." in i for i in items_of(hero_chat["id"])), 30)
    dock = subprocess.run(["defaults", "read", "com.apple.dock", "autohide"], capture_output=True, text=True).stdout.strip()
    try:
        if dock != "1":
            subprocess.run(["defaults", "write", "com.apple.dock", "autohide", "-bool", "true"], check=True)
            subprocess.run(["killall", "Dock"])
        subprocess.run([d.helper, "--close-notifications"], capture_output=True, timeout=30)
        time.sleep(2)  # the Dock gives its room back
        d.window((0, 0, 1600, 1200))
        d.window((0, 0, 1600, 1200))
        time.sleep(2.5)
        subprocess.run(["screencapture", "-x", str(out / "hero.png")], check=True, timeout=30)
        shots["hero"] = {"how": "whole 1600x1200 screen: two shells and a chat fed by a simulated agent", "agent": "simulated"}
        print("SHOT hero", flush=True)
    finally:
        if dock != "1":
            subprocess.run(["defaults", "write", "com.apple.dock", "autohide", "-bool", "false"])
            subprocess.run(["killall", "Dock"])
        d.window(FRAME)
    return shots, simulated


def readme_shots_wait(fn, timeout, step=0.5):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
        except Exception:  # noqa: BLE001 - the page may be between states
            v = None
        if v:
            return v
        time.sleep(step)
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("demo_root", type=Path)
    ap.add_argument("--target", type=Path, default=Path(os.environ.get("CARGO_TARGET_DIR") or REPO / "target"))
    args = ap.parse_args()
    if sys.platform != "darwin":
        raise SystemExit(__doc__)
    app, kelpied = args.target / "debug" / "kelpie", args.target / "debug" / "kelpied"
    for path in (app, kelpied, args.target / "debug" / "kelpie-screen"):
        if not path.is_file():
            raise SystemExit(f"missing {path}: cargo build -p kelpie -p kelpied -p kelpie-screen")
    if pages() is not None:
        raise SystemExit("something already serves the app's dev port 1427: stop that Vite dev server first")

    root = args.demo_root.resolve()
    manifest = root / "projects.json"
    if not manifest.exists() and sys.version_info < (3, 10):
        raise SystemExit(f"readme_demo needs Python 3.10+: python3.12 scripts/e2e/readme_demo.py {root}, then run this again")
    projects = json.loads(manifest.read_text(encoding="utf-8")) if manifest.exists() else seed(root)
    for p in projects:
        subprocess.run([sys.executable, "smoke.py"], cwd=p["path"], check=True, capture_output=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)

    # Folders this run makes show on some pages (Session Daemon, Mobile): short ones under /tmp.
    tempfile.tempdir = "/tmp"
    data = ps.isolate("mac-shots")
    work = Path(tempfile.mkdtemp(prefix="kelpie-mac-shots-"))
    stage = work / "stage"
    stage.mkdir()
    # The demo's commands say `python`; macOS has only python3.
    shim = work / "bin"
    shim.mkdir()
    (shim / "python").symlink_to(sys.executable)
    ssh_dir = work / "ssh-home"
    ssh_dir.mkdir()
    os.environ.update(PATH=f"{shim}{os.pathsep}{os.environ['PATH']}", KELPIE_SSH_DIR=str(ssh_dir), KELPIE_E2E="1", KELPIE_DAEMON_PATH=str(kelpied))
    os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)
    helper = str(work / "mac_window")
    subprocess.run(["xcrun", "swiftc", "-O", str(HERE / "mac_window.swift"), "-o", helper], check=True, capture_output=True, timeout=300)
    programs = write_programs(str(work / "agents"))

    vite = appp = None
    run = None
    try:
        with open(work / "vite.log", "w") as log:
            vite = subprocess.Popen(["node", str(HERE / "mac_bridge_vite.mjs"), str(REPO / "app")], stdout=log, stderr=subprocess.STDOUT)
        if readme_shots_wait(lambda: pages() is not None, 60) is None:
            raise SystemExit(f"the Vite bridge did not start: {(work / 'vite.log').read_text()[-600:]}")
        appp = subprocess.Popen([str(app)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not readme_shots_wait(lambda: pages(), 90):
            raise SystemExit("the app did not load its page from the dev server: build it with plain `cargo build` (not `tauri build`)")
        driver = MacDriver(appp.pid, helper)
        driver.window(FRAME)
        run = MacRun(str(stage), driver)
        tour = readme_shots.capture(run, projects, stage)
        extra, simulated = capture_mac(run, projects, stage, programs, 48700 + os.getpid() % 200)
    finally:
        if run is not None:
            try:
                daemon(run.d, "daemon.shutdown", {"kill_sessions": True})
            except Exception:  # noqa: BLE001
                pass
        for p in (appp, vite):
            if p is not None and p.poll() is None:
                p.terminate()
                try:
                    p.wait(15)
                except subprocess.TimeoutExpired:
                    p.kill()
    failed = [r for r in run.results if not r[1]] if run else ["no run"]
    if failed:
        raise SystemExit(f"checks failed; {args.out_dir} is unchanged, this run's pictures are in {stage}")
    media = {}
    for name in tour:
        media[f"mac-{name}"] = {"how": "readme_shots.capture tour on the Mac app (zsh shells, demo projects)"}
    for name, info in extra.items():
        media[f"mac-{name}"] = info
    for name in media:
        shutil.copy2(stage / (name[len("mac-"):] + ".png"), args.out_dir / (name + ".png"))
    (args.out_dir / "media.json").write_text(json.dumps({"platform": "macOS", "simulated_agent": simulated, "pictures": media}, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(work, ignore_errors=True)
    shutil.rmtree(data, ignore_errors=True)
    print(f"published {len(media)} pictures to {args.out_dir}")


if __name__ == "__main__":
    main()
