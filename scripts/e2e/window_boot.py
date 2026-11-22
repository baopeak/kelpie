"""Boot check of a built app through the window helper (Windows): the app starts, its interface is there as an accessibility
outline, the way into the settings opens them, a picture is taken, and closing the window ends everything it started.

    python scripts/e2e/window_boot.py <scratch-dir> <app> <kelpie-window> [--out result.json]

<app> is a built app binary (`target/release/kelpie.exe`) with `kelpied` next to it; <kelpie-window> is the helper
(`cargo build --release -p kelpie-window`). No WebDriver is involved: the helper reads the window through UI Automation and presses
through it, so this runs where a driver session cannot be opened.

Everything runs isolated, set in this process before anything starts: the app gets its own daemon endpoint (KELPIE_ENDPOINT) and
data folder (KELPIE_DATA_DIR) under <scratch-dir>, and the helper its own list of started programs (KELPIE_WINDOW_SCOPE). The
user's daemon and data are never reached. The script ends only what it started.

The app's window comes to the front while this runs (the helper's own desktop does not hold the app yet): meant for a runner, or a
desktop nobody is typing on.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

# What the first screen and the settings must show, by the names a person reads. Checked as text of the outline.
FIRST_SCREEN = ['button "Add project folder"', 'button "Settings"', 'text "0 sessions"']
SETTINGS = ['text "WORKFLOW"', 'button "Agents"', 'button "Session Daemon"']


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scratch")
    ap.add_argument("app")
    ap.add_argument("helper")
    ap.add_argument("--out")
    args = ap.parse_args()
    if sys.platform != "win32":
        raise SystemExit("the window helper runs on Windows")

    scratch = os.path.abspath(args.scratch)
    os.makedirs(os.path.join(scratch, "data"), exist_ok=True)
    tag = f"window-boot-{os.getpid()}"
    helper, app = os.path.abspath(args.helper), os.path.abspath(args.app)
    kelpied = os.path.join(os.path.dirname(app), "kelpied.exe")
    for what, path in (("app", app), ("daemon next to the app", kelpied), ("window helper", helper)):
        if not os.path.exists(path):
            raise SystemExit(f"{what} not found: {path}")
    # The same isolation the boot smoke gives the app (boot_smoke.py).
    env = dict(os.environ, KELPIE_ENDPOINT=f"kelpie-{tag}", KELPIE_DATA_DIR=os.path.join(scratch, "data"), KELPIE_DAEMON_PATH=kelpied, KELPIE_E2E="1", KELPIE_WINDOW_SCOPE=tag)
    env.pop("KELPIE_WINDOW_DESKTOP", None)

    def kw(*a, ok=True):
        r = subprocess.run([helper, *a], env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
        if ok and r.returncode != 0:
            raise RuntimeError(f"kelpie-window {' '.join(a[:2])}: {r.stderr.strip() or r.stdout.strip()}")
        return r.stdout

    checks, numbers, window = [], {}, None

    def check(name, passed, detail=""):
        checks.append({"name": name, "ok": bool(passed), "detail": detail})
        print(("ok   " if passed else "FAIL ") + name + (f": {detail}" if detail and not passed else ""))
        return passed

    def outline_with(wanted, seconds):
        """The outline once it holds every wanted line, or the last one read."""
        until, text = time.time() + seconds, ""
        while time.time() < until:
            text = kw("tree", window, "--max", "1500", ok=False)
            if all(w in text for w in wanted):
                break
            time.sleep(1)
        return text

    try:
        t0 = time.time()
        started = kw("launch", app)
        main_windows = [line.split()[0] for line in started.splitlines() if '"Kelpie' in line and " dialog " not in line]
        if not check("the app shows its window", main_windows, started.strip()):
            raise RuntimeError("no window")
        window = main_windows[0]
        numbers["window_ms"] = round((time.time() - t0) * 1000)

        first = outline_with(FIRST_SCREEN, 45)
        numbers["interface_ms"] = round((time.time() - t0) * 1000)
        missing = [w for w in FIRST_SCREEN if w not in first]
        check("the first screen is in the outline", not missing, f"missing {missing}; {len(first.splitlines())} lines")
        kw("shot", window, os.path.join(scratch, "window-boot-first.png"), ok=False)

        found = re.search(r'button "Settings" \[([0-9.\-]+)\]', first)
        if check("the way into the settings can be pressed", found):
            check("pressing it is taken", "invoked" in kw("act", window, found.group(1), "invoke", ok=False))
            after = outline_with(SETTINGS, 20)
            missing = [w for w in SETTINGS if w not in after]
            check("the settings open", not missing, f"missing {missing}; {len(after.splitlines())} lines")
            kw("shot", window, os.path.join(scratch, "window-boot-settings.png"), ok=False)
    except Exception as e:  # noqa: BLE001 - whatever went wrong is a failed check with its reason
        check("the check ran to its end", False, str(e))
    finally:
        if window:
            kw("close", window, ok=False)
            gone = False
            for _ in range(20):
                time.sleep(1)
                if "kelpie.exe" not in kw("list", ok=False):
                    gone = True
                    break
            check("closing the window ends the app", gone)
        # The app's own daemon stops with it or stays for its sessions; this one was started for the check, so it is ended by its endpoint.
        subprocess.run([kelpied, "stop"], env=env, capture_output=True, timeout=30)

    passed = sum(c["ok"] for c in checks)
    result = {"passed": passed, "failed": len(checks) - passed, "numbers": numbers, "checks": checks}
    print(json.dumps({k: result[k] for k in ("passed", "failed", "numbers")}))
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(result, f, indent=2)
    sys.exit(0 if passed == len(checks) and checks else 1)


if __name__ == "__main__":
    main()
