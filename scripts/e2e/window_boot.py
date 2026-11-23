"""Boot check of a built app through the window helper (Windows): the app starts, its interface is there as an accessibility
outline, the way into the settings opens them, a picture is taken, and closing the window ends everything it started.

    python scripts/e2e/window_boot.py <scratch-dir> <app> <kelpie-window> [--owed] [--out result.json]

--owed also opens a check still owed with the daemon, finds it in Activity, closes it with evidence through the window, and asks the
daemon what it holds.

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
# Lines only the main view has (the welcome button only without a project): gone while the settings cover it. Compared in lower case.
MAIN_ONLY = ['button "Agent dashboard"', 'button "Toggle sidebar"', 'button "Command palette']
# The ones present on every main view, waited for when the settings close.
MAIN_ALWAYS = MAIN_ONLY[:2]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scratch")
    ap.add_argument("app")
    ap.add_argument("helper")
    ap.add_argument("--out")
    ap.add_argument("--owed", action="store_true", help="also open a check still owed, see it in Activity and close it with evidence")
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

    project, owed_what, evidence = "p-window-owed", "Open the settings on the real app", "seen on the runner"
    if args.owed:
        # The window knows a project only from its own state file: one is put there before the app starts.
        repo = os.path.join(scratch, "repo")
        os.makedirs(repo, exist_ok=True)
        state = {
            "version": 3, "projects": [{"id": project, "name": "repo", "path": repo.replace("\\", "/"), "color": "#4f8cff"}], "activeProject": project, "workspaces": {},
            "settings": {}, "recent": [], "collapsed": {}, "keymap": {}, "approvedSetups": {}, "hibernated": [], "inbox": [],
            "reviewNotes": {}, "floating": {"sessions": [], "active": None, "open": False},
        }
        with open(os.path.join(scratch, "data", "app-state.json"), "w", encoding="utf-8") as f:
            json.dump(state, f)

    def daemon(method, params):
        r = subprocess.run([kelpied, "call", method, json.dumps(params)], env=env, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60)
        return r.returncode, (r.stdout + r.stderr).strip()

    def button(outline, name, after=""):
        """The id of the first button with this name, looked for after a piece of text when one is given."""
        # A heading the interface shows in capitals reads in capitals in the outline.
        start = outline.lower().find(after.lower()) if after else 0
        found = re.search(r'button "' + re.escape(name) + r'" \[([0-9.\-]+)\]', outline[max(start, 0):])
        return found.group(1) if found and start >= 0 else None

    def owed_scenario():
        code, said = daemon("owed.open", {"project": project, "kind": "real_app", "what": owed_what, "closes_by": "a picture of the settings page"})
        if not check("the daemon takes a check still owed", code == 0, said[:200]):
            return
        bell = button(outline_with(['button "Activity and notifications"'], 15), "Activity and notifications")
        if not check("the Activity bell can be pressed", bell):
            return
        kw("act", window, bell, "invoke", ok=False)
        panel = ""
        for _ in range(15):
            panel = kw("tree", window, "--max", "1500", ok=False)
            if "checks still owed" in panel.lower() and owed_what.lower() in panel.lower():
                break
            time.sleep(1)
        shown = "checks still owed" in panel.lower() and owed_what.lower() in panel.lower()
        if not shown:
            with open(os.path.join(scratch, "window-boot-owed-outline.txt"), "w", encoding="utf-8") as f:
                f.write(panel)
        kw("shot", window, os.path.join(scratch, "window-boot-owed.png"), ok=False)
        if not check("Activity shows the check still owed", shown, f"{len(panel.splitlines())} lines"):
            return
        # The window's own title bar has a Close button too: the one meant is after the group's heading.
        close = button(panel, "Close", after="Checks still owed")
        if not check("the entry can be closed", close):
            return
        kw("act", window, close, "invoke", ok=False)
        asking = outline_with(['button "Close check"'], 10)
        field = re.search(r'edit "Evidence[^"]*" \[([0-9.\-]+)\]', asking)
        if not check("closing asks for the evidence", field, asking[max(asking.lower().find("checks still owed"), 0):][:400]):
            return
        check("the evidence is taken", "set" in kw("act", window, field.group(1), "set", evidence, ok=False))
        confirm = button(outline_with(['button "Close check"'], 5), "Close check")
        said = kw("act", window, confirm, "invoke", ok=False) if confirm else "no button"
        check("the close is pressed", "invoked" in said, said.strip())
        gone = False
        for _ in range(15):
            time.sleep(1)
            if owed_what.lower() not in kw("tree", window, "--max", "1500", ok=False).lower():
                gone = True
                break
        check("the entry leaves Activity", gone)
        kw("shot", window, os.path.join(scratch, "window-boot-owed-closed.png"), ok=False)
        code, said = daemon("owed.list", {"project": project})
        check("the daemon holds it as closed with the evidence", code == 0 and evidence in said and '"closed_ms"' in said, said[:300])
        # The panel is closed again, so that the settings are reached from the main view.
        bell = button(kw("tree", window, "--max", "1500", ok=False), "Activity and notifications")
        if bell:
            kw("act", window, bell, "invoke", ok=False)
            time.sleep(1)

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

        if args.owed:
            owed_scenario()
            first = kw("tree", window, "--max", "1500", ok=False)

        found = re.search(r'button "Settings" \[([0-9.\-]+)\]', first)
        if check("the way into the settings can be pressed", found):
            check("pressing it is taken", "invoked" in kw("act", window, found.group(1), "invoke", ok=False))
            after = outline_with(SETTINGS, 20)
            missing = [w for w in SETTINGS if w not in after]
            check("the settings open", not missing, f"missing {missing}; {len(after.splitlines())} lines")
            kw("shot", window, os.path.join(scratch, "window-boot-settings.png"), ok=False)

            # The main view stays mounted under the settings, but it must not be reachable by a screen reader or the keyboard.
            leaked = [w for w in MAIN_ONLY if w.lower() in after.lower()]
            check("the view behind the settings is not in the outline", not leaked, f"still there: {leaked}")
            # With the main view out of the outline, the settings' own way back is the only button called Settings.
            back = button(after, "Settings") or button(after, "Back (Esc)")
            if check("the settings have a way back that can be pressed", back):
                kw("act", window, back, "invoke", ok=False)
                returned = ""
                for _ in range(20):
                    returned = kw("tree", window, "--max", "1500", ok=False).lower()
                    if all(w.lower() in returned for w in MAIN_ALWAYS):
                        break
                    time.sleep(1)
                lost = [w for w in MAIN_ALWAYS if w.lower() not in returned]
                check("the main view is back in the outline after the settings close", not lost, f"missing {lost}")
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
