"""Merges a real (sample) pull request from the phone's own page, in a phone-sized Chromium, through a real daemon.

    python -X utf8 scripts/e2e/phone_merge_page.py <kelpied.exe> <clone folder> --number N [--method squash] [--out result-dir]

`kelpied.exe` is a DEBUG build with the phone client embedded (`KELPIE_REQUIRE_PHONE=1 cargo build -p kelpied`, after
`cd app && pnpm build:phone`): only that build takes this script for the Kelpie app (approving a pairing, raising a phone to control) and
answers the person's verification itself. The clone folder is a git checkout of the sample's head branch with `origin` on github.com; `gh`
must be signed in (in CI: `GH_TOKEN`, which the daemon passes on to the `gh` it runs). Needs Playwright for Python with Chromium and
`cryptography`.

What phone_merge.py checks at the protocol, this checks on the page a person uses: pairing in the browser, the app lock set up with
Chromium's virtual authenticator (CDP `WebAuthn`, no person's face), a session in the clone's project, and in its view:
1. With the phone allowed to type but not to take control, the pull request's line says it can be merged and gives the reason in place of
   a Merge button.
2. Raised to control, the line offers Merge. The sheet names the repository, the number, the head and the method; Cancel merges nothing.
3. Continue: the sheet shows the exact line the passkey signs; Merge with passkey: the page says Merged, GitHub says MERGED, the audit holds
   the merge. The page raised no script error.

The sample pull request is really merged, once. Everything the script starts is its own: a daemon with its own endpoint and data folder,
phone access on 127.0.0.1 with a certificate of this run, opened at `localhost` (a passkey needs a name).
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import phone_support as ps  # noqa: E402

results = []  # (name, ok, detail)
IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1"
PROJECT = "sample-project"
SESSION = "sample-shell"


def check(name, ok, detail=""):
    results.append((name, bool(ok), str(detail)))
    shown = str(detail).encode("ascii", "replace").decode()
    print(("ok   " if ok else "FAIL ") + name + (f"  ({shown[:300]})" if shown and not ok else ""), flush=True)
    return bool(ok)


class Stop(Exception):
    """What follows cannot run: the check that said why has already failed."""


def need(name, ok, detail=""):
    if not check(name, ok, detail):
        raise Stop(name)


def gh_state(repo, number):
    done = subprocess.run(["gh", "pr", "view", str(number), "-R", f"github.com/{repo}", "--json", "state,headRefOid,title"], capture_output=True, text=True, timeout=90, check=False)
    if done.returncode != 0:
        raise RuntimeError("gh pr view failed: " + (done.stderr or done.stdout).strip()[:300])
    return json.loads(done.stdout)


def git_out(folder, *args):
    done = subprocess.run(["git", "-C", folder, *args], capture_output=True, text=True, timeout=30, check=False)
    if done.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {done.stderr.strip()[:300]}")
    return done.stdout.strip()


def repo_of_origin(folder):
    url = git_out(folder, "remote", "get-url", "origin")
    found = re.match(r"^(?:https://(?:[^/@]+@)?github\.com/|git@github\.com:|ssh://git@github\.com/)([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+?)(?:\.git)?/?$", url)
    if not found:
        raise RuntimeError("the clone's origin is not a repository on github.com")
    return f"{found.group(1)}/{found.group(2)}"


class Page:
    def __init__(self, browser, out):
        self.ctx = browser.new_context(viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True,
                                       user_agent=IPHONE_UA, ignore_https_errors=True, locale="en-US")
        self.page = self.ctx.new_page()
        self.page.set_default_timeout(20000)
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        self.cdp = self.ctx.new_cdp_session(self.page)
        self.out = out
        self.n = 0

    def shot(self, name):
        if not self.out:
            return
        self.n += 1
        try:
            self.page.screenshot(path=os.path.join(self.out, f"{self.n:02d}-{name}.png"))
        except Exception as e:  # noqa: BLE001
            print(f"  (no picture {name}: {e})", flush=True)

    def wait(self, sel, timeout=20000, state="visible"):
        return self.page.wait_for_selector(sel, timeout=timeout, state=state)

    def has(self, sel):
        return self.page.locator(sel).count() > 0

    def text(self, sel):
        return (self.page.locator(sel).first.text_content() or "").strip()


def pair(d, ph, port):
    page = ph.page
    page.goto(f"https://localhost:{port}/")
    ph.wait("[data-stay-in-browser], [data-scan]")
    if ph.has("[data-stay-in-browser]"):
        page.click("[data-stay-in-browser]")
    ph.wait("[data-scan]")
    page.click("[data-scan-start]")
    try:
        ph.wait("[data-scan-problem]", 8000)
    except Exception:  # noqa: BLE001
        pass
    link = d.call("mobile.pair_start")["url"].replace(f"https://127.0.0.1:{port}/", f"https://localhost:{port}/")
    page.fill("[data-paste]", link)
    page.click("[data-paste-go]")
    ph.wait("[data-pair-name], .card input.field")
    name = "Merge phone"
    page.fill("[data-pair-name], .card input.field", name)
    page.click("[data-pair-go], .card .btn.primary")
    code = ph.wait("[data-pair-code]").text_content()
    request = ps.pending_request(d, name)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": code, "permission": "write"})
    ph.wait("[data-lock-screen]")
    ph.cdp.send("WebAuthn.enable", {"enableUI": False})
    ph.cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True,
                                                                 "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})
    page.click("[data-lock-go]")
    ph.wait("[data-lock-screen]", state="detached")
    return next(x for x in d.call("mobile.status")["devices"] if x["name"] == name)


def open_session(ph):
    """The inbox's row of the sample session, opened."""
    ph.wait("[data-inbox-view], .view-switch")
    deadline = time.time() + 30
    while time.time() < deadline:
        row = ph.page.locator(".row, [data-id]").filter(has_text=SESSION)
        if row.count() > 0:
            row.first.click()
            return
        time.sleep(1.0)
    raise RuntimeError("the sample session never showed in the inbox")


def back_to_inbox(ph):
    while ph.has("[data-back]"):
        ph.page.click("[data-back]")
        time.sleep(0.5)


def wait_line(ph, want_can=True, timeout=90):
    """The pull request's line, read again (its refresh button) until GitHub has worked out whether it can merge."""
    ph.wait("[data-pull-line]", 30000)
    deadline = time.time() + timeout
    while time.time() < deadline:
        line = ph.page.locator("[data-pull-line]").first
        if (line.get_attribute("class") or "").split().count("can") == (1 if want_can else 0) and ph.has("[data-pull-headline]"):
            return
        if ph.page.locator("[data-pull-refresh]:not([disabled])").count() > 0:
            ph.page.click("[data-pull-refresh]")
        time.sleep(6)


def scenario(d, ph, args, repo, port):
    number, method = args.number, args.method
    head = git_out(args.clone, "rev-parse", "HEAD")
    before = gh_state(repo, number)
    need("the sample pull request is open, at the clone's head commit", before["state"] == "OPEN" and before["headRefOid"] == head, before)
    need("the daemon answers the person's verification itself (a debug build)", d.call("daemon.info").get("test_user_presence") == "approve")

    cert, key = ps.certificate_for_loopback(os.path.dirname(args.out) if args.out else os.getcwd())
    st = d.call("mobile.enable", {"ip": "127.0.0.1", "port": port, "cert": cert, "key": key})
    need("phone access serves HTTPS on the chosen address", bool(st.get("running")), st)
    d.call("mobile.set_projects", {"projects": {PROJECT: "Sample"}})
    d.call("mobile.set_launch", {"projects": [{"id": PROJECT, "name": "Sample", "path": args.clone}], "agents": []})
    d.call("session.create", {"kind": "shell", "cwd": args.clone, "name": SESSION, "project_id": PROJECT})

    dev = pair(d, ph, port)
    need("the phone paired in the browser and its app lock was set up with a passkey", dev.get("permission") == "write", dev)
    ph.shot("inbox")

    # ---------- 1. allowed to type, not to take control ----------
    open_session(ph)
    wait_line(ph)
    ph.shot("line-write")
    need("the session's view shows the pull request's line, saying it can be merged",
         "can" in (ph.page.locator("[data-pull-line]").first.get_attribute("class") or "").split(), ph.text("[data-pull-line]"))
    headline = ph.text("[data-pull-headline]")
    check("…naming the pull request's number", f"#{number}" in ph.text("[data-pull-line]"), ph.text("[data-pull-line]"))
    check("a phone that may not take control gets no Merge button", not ph.has("[data-pull-merge]"))
    check("…and the reason stands in its place", ph.has("[data-pull-merge-why]") and bool(ph.text("[data-pull-merge-why]")), ph.text("[data-pull-line]"))

    # ---------- 2. raised to control ----------
    d.call("mobile.set_permission", {"id": dev["id"], "permission": "control"})
    back_to_inbox(ph)
    deadline = time.time() + 30
    while time.time() < deadline and not ph.has("[data-see-screen]"):
        time.sleep(1.0)
    open_session(ph)
    wait_line(ph)
    try:
        ph.wait("[data-pull-merge]", 30000)
    except Exception:  # noqa: BLE001
        pass
    ph.shot("line-control")
    need("raised to control, the line offers Merge", ph.has("[data-pull-merge]"), ph.text("[data-pull-line]"))
    check("…and no longer gives a reason against it", not ph.has("[data-pull-merge-why]"))

    ph.page.click("[data-pull-merge]")
    ph.wait("[data-merge-sheet=choose]")
    ph.shot("sheet-choose")
    check("the sheet names the repository", ph.text("[data-merge-repo]") == repo, ph.text("[data-merge-repo]"))
    check("…the pull request", ph.text("[data-merge-pr]").startswith(f"#{number}"), ph.text("[data-merge-pr]"))
    check("…and the head commit it will merge", head.startswith(ph.text("[data-merge-head]")) and len(ph.text("[data-merge-head]")) >= 7, ph.text("[data-merge-head]"))
    ph.page.select_option("[data-merge-method]", method)
    ph.page.click("[data-merge-cancel]")
    ph.wait("[data-merge-sheet]", state="detached")
    time.sleep(2)
    check("Cancel closes the sheet and merges nothing", gh_state(repo, number)["state"] == "OPEN")

    # ---------- 3. the merge ----------
    ph.page.click("[data-pull-merge]")
    ph.wait("[data-merge-sheet=choose]")
    ph.page.select_option("[data-merge-method]", method)
    ph.page.click("[data-merge-continue]")
    ph.wait("[data-merge-sheet=verify]", 60000)
    ph.shot("sheet-verify")
    command = f"merge github.com {repo} #{number} {head} {method}"
    need("the passkey is asked over exactly the bound line", ph.text("[data-merge-command]") == command, ph.text("[data-merge-command]"))
    check("…and the plain words under it name the method, repository and number", f"{repo} #{number}" in ph.text("[data-merge-read]"), ph.text("[data-merge-read]"))
    ph.page.click("[data-merge-go]")
    ph.wait("[data-merge-sheet=done]", 120000)
    ph.shot("sheet-done")
    title = ph.text(".sheet h2")
    need("the page says Merged", title == "Merged", (title, ph.text("[data-merge-result]")))

    state = ""
    for _ in range(10):
        state = gh_state(repo, number)["state"]
        if state == "MERGED":
            break
        time.sleep(3)
    check("GitHub says the pull request is MERGED", state == "MERGED", state)
    lines = [e["summary"] for e in d.call("mobile.audit", {"limit": 50, "action": "pr_merge"})["entries"]]
    check("the phone audit holds the merge, with the number and the head", any(f"{repo} #{number} head {head} ({method})" in x and x.endswith(": merged") for x in lines), lines)
    ph.page.click("[data-merge-close]")
    ph.wait("[data-merge-sheet]", state="detached")
    check("the page raised no script error", not ph.errors, ph.errors[:3])
    return {"repo": repo, "number": number, "head": head, "state": state, "headline": headline, "audit": lines}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("kelpied", help="a debug kelpied with the phone client embedded")
    ap.add_argument("clone", help="a git checkout of the sample pull request's head branch, origin on github.com")
    ap.add_argument("--number", type=int, required=True, help="the sample pull request's number")
    ap.add_argument("--method", default="squash", choices=["squash", "merge", "rebase"])
    ap.add_argument("--out", help="a folder for result.json, the pictures and the daemon's log")
    args = ap.parse_args()
    args.kelpied = os.path.abspath(args.kelpied)
    args.clone = os.path.abspath(args.clone)
    if args.out:
        os.makedirs(os.path.abspath(args.out), exist_ok=True)
        args.out = os.path.join(os.path.abspath(args.out), "result.json")
    if not os.path.isfile(args.kelpied):
        raise SystemExit(f"no such daemon: {args.kelpied}")
    if not os.path.isdir(os.path.join(args.clone, ".git")):
        raise SystemExit(f"not a git checkout: {args.clone}")

    from playwright.sync_api import sync_playwright

    data_dir = ps.isolate("phone-merge-page")
    port = 48300 + os.getpid() % 500
    summary, d = {}, None
    try:
        repo = repo_of_origin(args.clone)
        d = ps.Daemon(args.kelpied, env={"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1"}).start()
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            try:
                ph = Page(browser, os.path.dirname(args.out) if args.out else None)
                try:
                    summary = scenario(d, ph, args, repo, port) or {}
                except Exception:
                    ph.shot("stopped")
                    raise
            finally:
                browser.close()
    except Stop as e:
        print(f"stopped: {e}", flush=True)
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False, "see the traceback above")
    finally:
        if d is not None:
            d.stop()
        failed = [r for r in results if not r[1]]
        print(f"{len(results) - len(failed)} ok, {len(failed)} failed", flush=True)
        log_file = os.path.join(data_dir, "logs", "kelpied.log")
        if args.out:
            with open(args.out, "w", encoding="utf-8") as f:
                json.dump({"ok": not failed and bool(results), "checks": [{"name": n, "ok": ok, "detail": detail[:500]} for n, ok, detail in results], **summary}, f, indent=2)
            if os.path.isfile(log_file):
                shutil.copyfile(log_file, os.path.join(os.path.dirname(args.out), "daemon.log"))
        time.sleep(0.2)
        shutil.rmtree(data_dir, ignore_errors=True)
    sys.exit(1 if failed or not results else 0)


if __name__ == "__main__":
    main()
