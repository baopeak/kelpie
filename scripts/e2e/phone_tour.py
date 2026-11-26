"""A screenshot tour of the phone page, for design reviews: every screen and state, on an iPhone-sized and a Pixel-sized Chromium, in
English and Vietnamese, dark and light. Nothing here asserts; it records what a person would see, and measures each screen's tap targets
and text fields.

    python -X utf8 scripts/e2e/phone_tour.py <kelpied.exe> <out-dir> [--devices iphone,pixel] [--locales en,vi]

<kelpied.exe> is a debug build (only that build trusts this script as the app, which approving a phone needs), built after
`cd app && pnpm build:phone`: the page shown is the one the daemon embeds. Needs Playwright for Python with Chromium, `cryptography` and
`websockets`.

Isolation as the other phone scripts (phone_support.isolate): this process's own endpoint and data folder, set before anything starts, so
the user's daemon and data are never reached; the daemon this script starts is stopped at the end, with its sessions. The page is served on
127.0.0.1 with a certificate made for this run and opened at `localhost`, which a passkey needs; the app lock is set up with Chromium's
virtual authenticator. The agents are small Python programs that speak Claude's stream format, never a real agent.

Writes <out-dir>/<device>-<locale>-<theme>/<NN-state>.png and <out-dir>/measures.json (per screen: targets under 44 px, fields whose text is
under 16 px, which makes Safari zoom in on focus).
"""
import argparse
import json
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import phone_support as ps  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("exe")
ap.add_argument("out")
ap.add_argument("--devices", default="iphone,pixel")
ap.add_argument("--locales", default="en,vi")
ap.add_argument("--readme", action="store_true", help="the README's sessions instead: the demo projects and simulated agents of the Safari tour "
                "(phone_safari_sim.demo_sessions), a `gh` that finds no pull request and stand-ins for the claude and codex CLIs (Unix)")
ARGS = ap.parse_args()

WORK = ps.isolate("phone-tour")
OUT = os.path.abspath(ARGS.out)
os.makedirs(OUT, exist_ok=True)
PORT = 48190
IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1"
PIXEL_UA = "Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36"
DEVICES = {
    # iPhone 15: 390×844 CSS pixels at 3×; the keyboard with its suggestion bar takes about 336 of them.
    "iphone": {"viewport": {"width": 390, "height": 844}, "device_scale_factor": 3, "user_agent": IPHONE_UA, "keyboard": 336},
    # Pixel 7: 412×915 at 2.625; Gboard about 300.
    "pixel": {"viewport": {"width": 412, "height": 915}, "device_scale_factor": 2.625, "user_agent": PIXEL_UA, "keyboard": 300},
}
THEMES = ("dark", "light")
MEASURES = {}
MISSED = []

TICKER = r'''
import sys, time, itertools
files = ["src/lib/layout.test.ts", "src/chat/chat-model.test.ts", "phone/src/link.test.ts", "phone/src/lock.test.ts", "src/lib/store.test.ts"]
for i in itertools.count():
    f = files[i % len(files)]
    print(f" \u2713 {f} ({12 + i % 30} tests) {20 + (i * 7) % 300}ms", flush=True)
    time.sleep(0.8)
'''

MENU = r'''
import sys
def key():
    try:
        import msvcrt
        return msvcrt.getwch()
    except ImportError:
        return sys.stdin.read(1)
print("Bash(rm -rf node_modules/.vite && pnpm build)")
print(" Do you want to proceed?")
print(" \u276f 1. Yes")
print("   2. Yes, and don't ask again for pnpm build")
print("   3. No, and tell Claude what to do differently (esc)")
key()
'''

LONG_CHAT = r'''
import json, sys
def out(m):
    sys.stdout.write(json.dumps(m, ensure_ascii=False) + "\n"); sys.stdout.flush()
def user(text):
    out({"type": "user", "isReplay": True, "message": {"role": "user", "content": text}})
def say(mid, blocks):
    out({"type": "assistant", "message": {"id": mid, "role": "assistant", "content": blocks}})
def result(tid, text, err=False):
    out({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": text, "is_error": err}]}})
user(sys.argv[1])
say("m1", [{"type": "thinking", "thinking": "I should read the merged pull requests since the last tag, then group them by area."}])
say("m2", [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "git log --oneline v0.1.20..HEAD", "description": "List the commits since the last release"}}])
result("t1", "bca9fff fix(storage): survive a power cut and restore state from its previous save (#71)\n449fbe3 feat(sidebar): compact worktree rows and fold quiet worktrees (#70)\n3d1e2aa fix(chat): keep the queue when a turn is cut mid-way (#72)")
say("m3", [{"type": "tool_use", "id": "t2", "name": "Read", "input": {"file_path": "CHANGELOG.md"}}])
result("t2", "# Changelog\n\n## 0.1.20\n- Phone: remote screen gestures\n- Relay: TURN for peer to peer")
say("m4", [{"type": "text", "text": "Here is a draft for **0.1.21**:\n\n- Sessions survive a power cut: state is written atomically and the previous save is restored if the last one is torn.\n- The sidebar folds quiet worktrees and shows compact rows.\n- A chat keeps its queue when a turn is cut mid-way.\n\nThe entry in CHANGELOG.md would be:\n\n```markdown\n## 0.1.21\n- Survive a power cut (#71)\n- Compact worktree rows (#70)\n- Keep the chat queue mid-turn (#72)\n```"}])
user(sys.argv[2])
say("m5", [{"type": "tool_use", "id": "t3", "name": "Edit", "input": {"file_path": "CHANGELOG.md", "old_string": "# Changelog\n", "new_string": "# Changelog\n\n## 0.1.21\n- Survive a power cut (#71)\n"}}])
result("t3", "The file was updated.")
say("m6", [{"type": "text", "text": "Done: CHANGELOG.md has the 0.1.21 entry. Next I would push the release branch so CI can build it."}])
out({"type": "control_request", "request_id": "req-push", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "git push --force-with-lease origin release/0.1.21", "description": "Push the release branch"}}})
for line in sys.stdin:
    pass
'''

DONE_CHAT = r'''
import json, sys
def out(m):
    sys.stdout.write(json.dumps(m, ensure_ascii=False) + "\n"); sys.stdout.flush()
out({"type": "user", "isReplay": True, "message": {"role": "user", "content": "Rename the helper to kebab-case and fix the imports."}})
out({"type": "assistant", "message": {"id": "a1", "role": "assistant", "content": [{"type": "text", "text": "Renamed `phoneState.ts` to `phone-state.ts` and updated 14 imports. The tests pass."}]}})
out({"type": "result", "subtype": "success", "is_error": False, "result": "ok", "duration_ms": 41000, "num_turns": 3})
for line in sys.stdin:
    pass
'''


def write(name, text):
    path = os.path.join(WORK, name)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    return path


MEASURE_JS = """() => {
  const out = { small: [], zoom: [] };
  const seen = (el) => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el); return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && r.bottom > 0 && r.top < innerHeight; };
  const name = (el) => (el.getAttribute('aria-label') || el.textContent || el.getAttribute('placeholder') || el.tagName).trim().replace(/\\s+/g, ' ').slice(0, 40);
  for (const el of document.querySelectorAll('button, a[href], select, input:not([type=hidden]), textarea, [role=button], summary, label.row-check')) {
    if (!seen(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.height < 44 || r.width < 44) out.small.push(`${name(el)} ${Math.round(r.width)}x${Math.round(r.height)}`);
    if (/^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName) && parseFloat(getComputedStyle(el).fontSize) < 16) out.zoom.push(`${name(el)} ${getComputedStyle(el).fontSize}`);
  }
  return out;
}"""


class Phone:
    def __init__(self, browser, device, locale):
        spec = DEVICES[device]
        self.device, self.locale, self.keyboard = device, locale, spec["keyboard"]
        self.viewport = spec["viewport"]
        self.ctx = browser.new_context(viewport=spec["viewport"], device_scale_factor=spec["device_scale_factor"], is_mobile=True, has_touch=True,
                                       user_agent=spec["user_agent"], ignore_https_errors=True, locale="vi-VN" if locale == "vi" else "en-US",
                                       color_scheme="dark")
        self.page = self.ctx.new_page()
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        self.cdp = self.ctx.new_cdp_session(self.page)
        self.name = f"{device}-{locale}"

    def shot(self, state, themes=THEMES):
        if ARGS.readme:
            # News about another phone (its permission changed, a device was paired) shows for 8 s and then goes on its own: README
            # pictures wait for that, as a person looking a moment later would see the page.
            try:
                self.page.wait_for_selector("[data-banner=device-news]", state="detached", timeout=12000)
            except Exception:  # noqa: BLE001 - noted in the measures, the picture is still taken
                MISSED.append(f"{self.name} {state}: device news still shown")
        for theme in themes:
            self.page.emulate_media(color_scheme=theme)
            time.sleep(0.25)
            folder = os.path.join(OUT, f"{self.name}-{theme}")
            os.makedirs(folder, exist_ok=True)
            self.page.screenshot(path=os.path.join(folder, f"{state}.png"))
        self.page.emulate_media(color_scheme="dark")
        MEASURES.setdefault(self.name, {})[state] = self.page.evaluate(MEASURE_JS)
        print(f"  {self.name} {state}", flush=True)

    def has(self, sel):
        return self.page.locator(sel).count() > 0

    def wait(self, sel, timeout=15000, state="visible"):
        return self.page.wait_for_selector(sel, timeout=timeout, state=state)

    def back(self):
        self.page.click("[data-back], header button.icon")
        time.sleep(0.4)

    def open_row(self, text):
        self.page.locator(".row, [data-id]").filter(has_text=text).first.click()
        time.sleep(0.6)

    def keyboard_up(self, field):
        """Focus a field and take the keyboard's height off the viewport, as a phone that resizes for it."""
        self.page.focus(field)
        self.page.set_viewport_size({"width": self.viewport["width"], "height": self.viewport["height"] - self.keyboard})
        time.sleep(0.4)

    def keyboard_down(self):
        self.page.evaluate("() => document.activeElement?.blur()")
        self.page.set_viewport_size(self.viewport)
        time.sleep(0.3)


def step(phone, label, fn):
    """One state: a failure is noted and the tour goes on."""
    try:
        fn()
    except Exception as e:  # noqa: BLE001
        MISSED.append(f"{phone.name} {label}: {e}".splitlines()[0])
        print(f"  MISSED {phone.name} {label}: {str(e).splitlines()[0]}", flush=True)
        traceback.print_exc(limit=1)


def pair(d, phone):
    page = phone.page
    page.goto(f"https://localhost:{PORT}/")
    phone.wait("[data-stay-in-browser], [data-scan]", 20000)
    if phone.has("[data-stay-in-browser]"):
        phone.shot("01-install")
        page.click("[data-stay-in-browser]")
    phone.wait("[data-scan]")
    phone.shot("02-pair-scan")
    page.click("[data-scan-start]")
    step(phone, "scan-no-camera", lambda: (phone.wait("[data-scan-problem]", 8000), phone.shot("03-pair-no-camera")))
    link = d.call("mobile.pair_start")["url"].replace(f"https://127.0.0.1:{PORT}/", f"https://localhost:{PORT}/")
    page.fill("[data-paste]", link)
    page.click("[data-paste-go]")
    phone.wait("[data-pair-name], .card input.field")
    phone.shot("04-pair-name")
    name = f"Tour {phone.name}"
    page.fill("[data-pair-name], .card input.field", name)
    page.click("[data-pair-go], .card .btn.primary")
    code = phone.wait("[data-pair-code]").text_content()
    phone.shot("05-pair-code")
    request = ps.pending_request(d, name)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": code, "permission": "write"})
    phone.wait("[data-lock-screen]", 20000)
    phone.shot("06-lock-setup")
    phone.cdp.send("WebAuthn.enable", {"enableUI": False})
    phone.cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})
    page.click("[data-lock-go]")
    phone.wait("[data-lock-screen]", 20000, state="detached")
    return next(x for x in d.call("mobile.status")["devices"] if x["name"] == name)


def sessions(d):
    """The computer's sessions, in every state the inbox shows."""
    if ARGS.readme:
        import phone_safari_sim
        phone_safari_sim.demo_sessions(d, WORK)
        return
    projects = {"p-kelpie": "kelpie", "p-site": "website", "p-docs": "Tài liệu hướng dẫn"}
    d.call("mobile.set_projects", {"projects": projects})
    folders = {}
    for pid in projects:
        folders[pid] = os.path.join(WORK, pid)
        os.makedirs(folders[pid], exist_ok=True)
    py = sys.executable
    ticker, menu = write("ticker.py", TICKER), write("menu.py", MENU)
    long_chat, done_chat = write("long_chat.py", LONG_CHAT), write("done_chat.py", DONE_CHAT)
    made = {}

    def shell(name, pid, command=None):
        info = d.call("session.create", {"kind": "shell", "cwd": folders.get(pid, WORK), "name": name, **({"project_id": pid} if pid else {})})
        made[name] = info
        if command:
            time.sleep(2.0)
            d.call("session.input", {"handle": info["handle"], "data": command + "\r"})
        return info

    def chat(name, pid, argv):
        made[name] = d.call("session.create", {"kind": "claude", "cwd": folders[pid], "chat_argv": [py, "-u", *argv], "name": name, "project_id": pid})

    shell("test-watch", "p-kelpie", f'python "{ticker}"')
    shell("build-release", "p-kelpie", f'python "{menu}"')
    chat("release-notes", "p-site", [long_chat, "Write the release notes for 0.1.21 from the merged pull requests.", "Looks good. Add it to CHANGELOG.md and push the release branch."])
    chat("rename-helpers", "p-kelpie", [done_chat])
    shell("Sửa hướng dẫn sử dụng tiếng Việt cho phần điện thoại", "p-docs")
    shell("deploy-preview", "p-site", f'python "{ticker}"')
    shell("scratch", None)
    old = shell("old-shell", "p-site")
    time.sleep(2.0)
    d.call("session.input", {"handle": old["handle"], "data": "exit\r"})
    time.sleep(4.0)
    return made


# The sessions each screen opens, by role: this tour's own, or the README's (phone_safari_sim.demo_sessions).
NAMES = {"ticker": "test-watch", "menu": "build-release", "approval": "release-notes", "done": "rename-helpers",
         "typed": "pnpm test --run", "reply": "Wait, push it after CI is green."}
if ARGS.readme:
    NAMES = {"ticker": "Checks (watch)", "menu": "Migration", "approval": "Release 0.4.0", "done": "How retries work",
             "typed": "python smoke.py", "reply": "Tag it after the release notes are reviewed."}


def readme_tools():
    """README mode, Unix: first on PATH for the daemon, a `gh` that finds no pull request (GitHub is never asked) and do-nothing
    `claude` and `codex`, so the new-session sheet lists the agents a person would have. None of them is ever started by the tour."""
    import phone_safari_sim
    phone_safari_sim.fake_gh(WORK)
    bin_dir = os.path.join(WORK, "fake-gh")
    for name in ("claude", "codex"):
        path = os.path.join(bin_dir, name)
        with open(path, "w", encoding="utf-8") as f:
            f.write("#!/bin/sh\nexit 0\n")
        os.chmod(path, 0o755)


def tour(d, phone, device_id):
    page = phone.page
    page.set_default_timeout(15000)

    def inbox():
        while phone.has("[data-back], header button.icon"):
            phone.back()

    step(phone, "inbox-status", lambda: (inbox(), page.click("[data-view=status]"), time.sleep(0.5), phone.shot("08-inbox-status")))
    step(phone, "inbox-workspace", lambda: (page.click("[data-view=workspace]"), time.sleep(0.5), phone.shot("09-inbox-workspace"), page.click("[data-view=status]")))
    step(phone, "inbox-scrolled", lambda: (page.evaluate("() => { const m = document.querySelector('main'); m.scrollTop = m.scrollHeight; }"), time.sleep(0.4), phone.shot("10-inbox-scrolled"), page.evaluate("() => document.querySelector('main').scrollTop = 0")))

    def terminal():
        inbox()
        phone.open_row(NAMES["ticker"])
        phone.wait(".screen, [data-terminal]")
        time.sleep(1.0)
        phone.shot("11-terminal")
        phone.keyboard_up(".composer textarea")
        page.keyboard.type(NAMES["typed"])
        phone.shot("12-terminal-keyboard", themes=("dark",))
        page.fill(".composer textarea", "")
        phone.keyboard_down()
    step(phone, "terminal", terminal)

    def prompt():
        inbox()
        phone.open_row(NAMES["menu"])
        phone.wait("[data-prompt]", 20000)
        time.sleep(0.5)
        phone.shot("13-terminal-prompt")
    step(phone, "terminal-prompt", prompt)

    def chat():
        inbox()
        phone.open_row(NAMES["approval"])
        phone.wait("[data-perm]", 20000)
        time.sleep(0.8)
        phone.shot("14-chat-approval")
        page.evaluate("() => { const l = document.querySelector('.chat-log, [data-chat-log]'); if (l) l.scrollTop = 0; }")
        time.sleep(0.4)
        phone.shot("15-chat-top")
        page.evaluate("() => { const l = document.querySelector('.chat-log, [data-chat-log]'); if (l) l.scrollTop = l.scrollHeight / 2; }")
        time.sleep(0.4)
        phone.shot("16-chat-middle")
        phone.keyboard_up(".chat-composer textarea")
        page.keyboard.type(NAMES["reply"])
        phone.shot("17-chat-keyboard", themes=("dark",))
        page.fill(".chat-composer textarea", "")
        phone.keyboard_down()
    step(phone, "chat", chat)

    def done_chat():
        inbox()
        phone.open_row(NAMES["done"])
        time.sleep(1.5)
        phone.shot("18-chat-done")
    step(phone, "chat-done", done_chat)

    def new_session():
        inbox()
        page.click("[data-open-new-session]")
        phone.wait("[data-new-session]")
        phone.wait("[data-new-start]")
        time.sleep(0.8)
        phone.shot("19-new-session")
        page.keyboard.press("Escape")
        page.mouse.click(10, 60)
        time.sleep(0.4)
        if phone.has("[data-new-session]"):
            page.locator("[data-new-session] button").last.click()
    step(phone, "new-session", new_session)

    def conn_sheet():
        inbox()
        page.click("header .conn, [data-conn]")
        phone.wait("[role=dialog]")
        time.sleep(0.4)
        phone.shot("20-connection-sheet")
        page.locator("[role=dialog] button").last.click()
        time.sleep(0.3)
    step(phone, "connection-sheet", conn_sheet)

    def settings():
        inbox()
        page.click("[data-open-settings]")
        phone.wait("[data-settings]")
        time.sleep(0.5)
        main = "main"
        for i in range(8):
            phone.shot(f"21-settings-{i + 1}")
            at = page.evaluate(f"() => {{ const m = document.querySelector('{main}'); const before = m.scrollTop; m.scrollTop += m.clientHeight * 0.85; return m.scrollTop > before; }}")
            time.sleep(0.3)
            if not at:
                break
        page.evaluate(f"() => document.querySelector('{main}').scrollTop = 0")
    step(phone, "settings", settings)

    def locked():
        if not phone.has("[data-settings]"):
            page.click("[data-open-settings]")
        page.click("[data-setting=lock-now]")
        phone.wait("[data-lock-screen]")
        time.sleep(0.4)
        phone.shot("22-locked")
        page.click("[data-lock-go]")
        phone.wait("[data-lock-screen]", 20000, state="detached")
    step(phone, "locked", locked)

    def screen_picker():
        d.call("mobile.set_permission", {"id": device_id, "permission": "control"})
        inbox()
        phone.wait("[data-see-screen]", 15000)
        time.sleep(0.4)
        phone.shot("23-inbox-control")
        page.click("[data-see-screen]")
        time.sleep(2.5)
        phone.shot("24-screen-picker")
        inbox()
    step(phone, "screen-picker", screen_picker)


def offline(phone):
    page = phone.page

    def go():
        phone.wait("[data-banner]", 30000)
        time.sleep(1.0)
        phone.shot("25-offline-inbox")
        phone.open_row(NAMES["ticker"])
        time.sleep(0.8)
        phone.shot("26-offline-terminal")
    step(phone, "offline", go)


def main():
    from playwright.sync_api import sync_playwright

    if ARGS.readme and os.name != "nt":
        readme_tools()
    d = ps.Daemon(ARGS.exe).start()
    cert, key = ps.certificate_for_loopback(WORK)
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert, "key": key})
    d.call("mobile.set_launch", {"projects": [{"id": "p-kelpie", "name": "kelpie", "path": WORK}, {"id": "p-site", "name": "website", "path": WORK}],
                                 "agents": [{"kind": "claude", "label": "Claude Code", "args": [], "env": {}}, {"kind": "codex", "label": "Codex", "args": [], "env": {}}]})
    phones = []
    try:
        with sync_playwright() as pw:
            browser = pw.chromium.launch()
            try:
                for device in ARGS.devices.split(","):
                    for locale in ARGS.locales.split(","):
                        phone = Phone(browser, device, locale)
                        print(f"pairing {phone.name}", flush=True)
                        dev = pair(d, phone)
                        step(phone, "inbox-empty", lambda: (phone.wait("[data-inbox-view], .view-switch"), time.sleep(0.6), phone.shot("07-inbox-empty")))
                        phones.append((phone, dev["id"]))
                sessions(d)
                time.sleep(3.0)
                for phone, dev_id in phones:
                    print(f"touring {phone.name}", flush=True)
                    tour(d, phone, dev_id)
                # The computer goes away: every page says so and shows what it last had.
                d.call("mobile.disable")
                for phone, _ in phones:
                    offline(phone)
                for phone, _ in phones:
                    if phone.errors:
                        MISSED.append(f"{phone.name} page errors: {phone.errors[:3]}")
            finally:
                browser.close()
    finally:
        d.stop()
        with open(os.path.join(OUT, "measures.json"), "w", encoding="utf-8") as f:
            json.dump({"measures": MEASURES, "missed": MISSED}, f, ensure_ascii=False, indent=1)
    print(f"done: {sum(len(v) for v in MEASURES.values())} screens, {len(MISSED)} missed", flush=True)
    for m in MISSED:
        print("  missed:", m)


main()
