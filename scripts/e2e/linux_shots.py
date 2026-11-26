#!/usr/bin/env python3
"""README pictures of the Linux app: the real app and its daemon under an X server (Xvfb on a runner), on the demo projects of
readme_demo.py.

    xvfb-run -a -s "-screen 0 1920x1200x24" python3 scripts/e2e/linux_shots.py <out-dir> <demo-root> [--app target/debug/kelpie]

<app> is a build with its frontend embedded (`cd app && pnpm tauri build --debug --no-bundle`) with `kelpied` next to it. Needs
tauri-driver (PATH or ~/.cargo/bin) and WebKitWebDriver (`apt install webkit2gtk-driver`). Settings → Mobile stores the phone link's key in
the Secret Service: run inside `dbus-run-session` with an unlocked gnome-keyring (the workflow readme-media.yml does).

What it captures, into <out-dir> as linux-<name>.png, at 1920x1080:
- the whole tour of readme_shots.capture (welcome, workspace, search, changes, editor, review notes, markdown, CSV, plan, new session,
  dashboard, board, layouts, composer, terminal menu, floating terminal, inbox, worktree dialog, automations, settings pages, notebook,
  processes, runs, Try, Finish, SSH hosts, plugins, light theme, Vietnamese), with bash in place of PowerShell;
- the pictures mac_shots.capture_mac adds (chat view, a chat waiting for approval and the dashboard with agents, from the simulated agents
  of demo_agents.py, never a real agent; Settings → Mobile and other settings pages; a hero), without the Mac's permission rows.
<out-dir>/media.json lists each picture with what made it.

Isolation: this process's own daemon endpoint (a socket under /tmp), data folder, XDG data/config/cache folders and an empty SSH folder,
set before anything starts; Kelpie's real data and daemon are never reached. Only the processes this script started are stopped.
"""
import argparse
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import mac_shots  # noqa: E402
import readme_shots  # noqa: E402
from demo_agents import write_programs  # noqa: E402
from readme_demo import seed  # noqa: E402
from smoke_support import Run, daemon, native_driver, native_driver_hint, tauri_driver  # noqa: E402
from wd import Driver  # noqa: E402

REPO = HERE.parents[1]
SIZE = (1920, 1080)


class LinuxDriver(Driver):
    """WebDriver through tauri-driver and WebKitWebDriver. Scripts run as async functions (mac_shots' extra pictures await in them), and
    keys go through the app's own commands as in mac_shots, so a chord does the same whatever the X server's keymap."""

    def js(self, script, *args):
        out = self.js_async("const done = arguments[arguments.length-1]; const a = Array.prototype.slice.call(arguments, 0, -1);\n"
                            "(async function () {\n" + script + "\n}).apply(null, a).then((v) => done({v: v === undefined ? null : v}), "
                            "(e) => done({e: String((e && e.stack) || e)}));", *args)
        if out and "e" in out:
            raise RuntimeError(out["e"][:800])
        return (out or {}).get("v")

    def keys(self, *chords):
        for chord in chords:
            self.js(mac_shots.KEYS_JS, chord)
            time.sleep(0.15)

    def window(self, frame=None):
        self.set_window_size(*SIZE)

    def set_window_size(self, width, height):
        super().set_window_size(*SIZE)


class LinuxRun(Run):
    def __init__(self, shots, app):  # noqa: Run.__init__ makes a plain Driver
        self.shots = shots
        self.results = []
        self.d = LinuxDriver(app)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out_dir", type=Path)
    ap.add_argument("demo_root", type=Path)
    ap.add_argument("--app", type=Path, default=REPO / "target" / "debug" / "kelpie")
    args = ap.parse_args()
    if not sys.platform.startswith("linux"):
        raise SystemExit(__doc__)
    app = args.app.resolve()
    kelpied = app.with_name("kelpied")
    for path in (app, kelpied):
        if not path.is_file():
            raise SystemExit(f"missing {path}")
    native, driver_exe = native_driver(), tauri_driver()
    if not native or not driver_exe:
        raise SystemExit(f"needs tauri-driver and {native_driver_hint()}")

    root = args.demo_root.resolve()
    manifest = root / "projects.json"
    projects = json.loads(manifest.read_text(encoding="utf-8")) if manifest.exists() else seed(root)
    for p in projects:
        subprocess.run([sys.executable, "smoke.py"], cwd=p["path"], check=True, capture_output=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)

    # Short folders under /tmp: some pages show them (Session Daemon), and a socket path must stay under 108 bytes.
    work = Path(tempfile.mkdtemp(prefix="kelpie-linux-shots-", dir="/tmp"))
    stage = work / "stage"
    stage.mkdir()
    ssh_dir = work / "ssh-home"
    ssh_dir.mkdir()
    endpoint = f"/tmp/kelpie-linux-shots-{os.getpid()}.sock"
    os.environ.update(KELPIE_ENDPOINT=endpoint, KELPIE_DATA_DIR=str(work / "data"), KELPIE_SSH_DIR=str(ssh_dir), KELPIE_E2E="1",
                      KELPIE_DAEMON_PATH=str(kelpied), XDG_DATA_HOME=str(work / "xdg-data"), XDG_CONFIG_HOME=str(work / "xdg-config"),
                      XDG_CACHE_HOME=str(work / "xdg-cache"))
    for key in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
        os.environ.pop(key, None)
    programs = write_programs(str(work / "agents"))
    port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{port}"

    run = None
    driver = None
    try:
        with open(args.out_dir / "driver.log", "w", encoding="utf-8") as log:
            # Own process group, so the driver's WebKitWebDriver and the app go down with it.
            driver = subprocess.Popen([driver_exe, "--port", str(port), "--native-port", str(free_port()), "--native-driver", native],
                                      stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        end = time.time() + 30
        while time.time() < end:
            with socket.socket() as s:
                if s.connect_ex(("127.0.0.1", port)) == 0:
                    break
            time.sleep(0.2)
        run = LinuxRun(str(stage), str(app))
        tour = readme_shots.capture(run, projects, stage)
        extra, simulated = mac_shots.capture_mac(run, projects, stage, programs, 48700 + os.getpid() % 200, mac=False)
    except BaseException:
        # What the tour saw before it stopped, for the run's artifact; the published names stay untouched.
        shutil.copytree(stage, args.out_dir / "failed-stage", dirs_exist_ok=True)
        raise
    finally:
        if run is not None:
            try:
                daemon(run.d, "daemon.shutdown", {"kill_sessions": True})
            except Exception:  # noqa: BLE001 - the reply may race the daemon exiting
                pass
            run.d.quit()
        if driver is not None:
            try:
                os.killpg(driver.pid, signal.SIGTERM)
                driver.wait(15)
            except (OSError, subprocess.TimeoutExpired):
                try:
                    os.killpg(driver.pid, signal.SIGKILL)
                except OSError:
                    pass
    failed = [r for r in run.results if not r[1]] if run else ["no run"]
    if failed:
        shutil.copytree(stage, args.out_dir / "failed-stage", dirs_exist_ok=True)
        raise SystemExit(f"checks failed ({failed}); {args.out_dir} is unchanged, this run's pictures are in {stage}")
    media = {}
    for name in tour:
        media[f"linux-{name}"] = {"how": "readme_shots.capture tour on the Linux app (bash shells, demo projects)"}
    for name, info in extra.items():
        media[f"linux-{name}"] = info
    for name in media:
        shutil.copy2(stage / (name[len("linux-"):] + ".png"), args.out_dir / (name + ".png"))
    (args.out_dir / "media.json").write_text(json.dumps({"platform": "Linux", "simulated_agent": simulated, "pictures": media}, indent=2) + "\n",
                                             encoding="utf-8")
    shutil.rmtree(work, ignore_errors=True)
    print(f"published {len(media)} pictures to {args.out_dir}")


if __name__ == "__main__":
    main()
