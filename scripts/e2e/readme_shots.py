"""Capture Kelpie's native desktop using eight isolated demo repositories.

    python -X utf8 scripts/e2e/readme_shots.py <out-dir> <demo-root> --native-driver <msedgedriver.exe>

Creates the fixtures with readme_demo.seed when projects.json is absent. Requires
built kelpie/kelpied binaries, tauri-driver and a WebView2-matching msedgedriver.
Optional --chat codex|claude|omp makes one real model call using your CLI sign-in.
--agents runs four agents side by side on one read-only task each, with your own sign-ins
(real model calls): Claude Code, Codex and OMP in their terminals and Claude Code in chat
view, then PowerShell in a tab of its own. Claude Code and Codex may ask once to trust the
demo folder; the tour answers (this records the folder in their own config files).
--browser also captures ChatGPT web beside OMP (Windows native capture, no ChatGPT sign-in).
It expects Windows' light app theme: the page then renders light, which is how the tour
tells a painted page from an empty pane (a dark theme fails that check).
Without these options, no agent is launched; no conversation is fabricated.
The base tour also captures the welcome screen, a notebook, the Processes panel (the
catalog server on port 8765), a run waiting for approval, Try with several agents, the
Finish dialog, the SSH hosts page (documentation addresses, never connected) and the demo
plugin awaiting review.
--extra adds one picture per further feature (sidebar, quick answer, split picker, broadcast,
file links, theme editor, History and Stashes, Review tab, Review with, event automation, PDF,
design mode, host drawer, settings pages, sub-sessions, owed checks, past conversations, usage);
agent sessions there are simulated (demo_agents.py), and the Sessions tab and the Usage page
read synthetic transcripts in this run's own agent homes. --only picks some of them.
--clip-finish DIR records a worktree taken to Finish on a throwaway clone (Windows, ffmpeg).
Kelpie's own projects, sessions, settings and default daemon are never touched. With
--agents, the real CLIs keep their usual transcripts and usage records of these demo turns
in their own folders (~/.claude, ~/.codex, ~/.omp), as any session of yours would.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import socket
import subprocess
import sys
import tempfile
import time

from smoke_support import Run, daemon, default_app
from readme_demo import seed

S = "window.__kelpie.store"
# What the tour types into a shell: PowerShell on Windows, a POSIX shell (zsh or bash) elsewhere.
WINDOWS = os.name == "nt"
# zsh marks output that ends without a newline with a reverse "%": not in pictures.
PROMPT = "function prompt { 'PS> ' }; Clear-Host" if WINDOWS else "PS1='$ '; [ -n \"$ZSH_VERSION\" ] && unsetopt PROMPT_SP; clear"
ASK = 'Read-Host "Run the next demo check (y/n)"' if WINDOWS else "printf 'Run the next demo check (y/n) '; read answer"


def go_to(folder):
    return f"Set-Location '{folder}'; Clear-Host" if WINDOWS else f"cd '{folder}'; clear"


def capture(run, projects, out, chat=None):
    d = run.d
    run.boot()
    d.set_window_size(1920, 1080)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    d.js(f"""{S}.setSettings({{ theme: 'dark', density: 'comfortable', uiScale: 1,
        usageMeters: false, notifications: false, shortcutHints: false, tips: false,
        sidebarWidth: 280, rightPanelWidth: 360, fontSize: 13,
        quickCommands: [{{id:'demo-checks',label:'Run demo checks',text:'python smoke.py',action:'terminal',enter:true,projectId:null}},
        {{id:'demo-review',label:'Review checkout',text:'Review src/checkout.py for validation and rounding.',action:'agent',enter:false,projectId:null}}] }})""")
    shots = []

    def shot(name, wait=0.8):
        time.sleep(wait)
        d.screenshot(str(out / (name + ".png")))
        shots.append(name)
        print("SHOT " + name, flush=True)

    # First launch, before any project: the welcome screen with Kip.
    d.wait("return !!document.querySelector('.welcome')", 20)
    shot("welcome", 1.5)
    ids = []
    for p in projects:
        pid = d.js(f"return {S}.addProject(arguments[0], {{open:false}})", p["path"])
        ids.append(pid)
        d.js(f"{S}.setProjectStatus(arguments[0], arguments[1])", pid, p["status"])
        daemon(d, "meta.set", {"path": p["path"], "comment": p["comment"]})
    atlas = ids[0]
    d.js(f"{S}.togglePin(arguments[0]); {S}.switchProject(arguments[0])", atlas)
    d.wait(f"return {S}.projectInfo[arguments[0]]?.is_repo", 30, 0.25, atlas)
    d.js_async("const done=arguments[arguments.length-1]; window.__kelpie.worktrees.addExistingWorktrees(window.__kelpie.store.state.projects.find(p=>p.id===arguments[0])).then(done,e=>done(String(e)))", atlas)
    kids = d.js(f"return {S}.childrenOf(arguments[0]).map(p=>({{id:p.id,path:p.path}}))", atlas)
    for k, status, comment in zip(kids, ["progress", "review"], ["Demo task: validate coupon boundaries", "Demo task: review free-shipping threshold"]):
        d.js(f"{S}.setProjectStatus(arguments[0],arguments[1])", k["id"], status)
        daemon(d, "meta.set", {"path": k["path"], "comment": comment})
    run.check("eight demo projects and two Git worktrees", len(ids) == 8 and len(kids) == 2)

    def esc():
        d.keys("escape")
        time.sleep(0.3)

    def command(cid):
        d.js("window.__kelpie.commands.find(c=>c.id===arguments[0]).run()", cid)

    def session(pid, name, where="tab"):
        info = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],'shell',{{name:arguments[1],where:arguments[2]}}).then(done)", pid, name, where)
        if not info:
            raise RuntimeError("shell did not start: " + name)
        time.sleep(2)
        daemon(d, "session.input", {"handle": info["handle"], "data": PROMPT + "\r"})
        time.sleep(0.5)
        return info

    def send(info, text):
        daemon(d, "session.input", {"handle": info["handle"], "data": text + "\r"})

    first = session(atlas, "Checkout checks")
    send(first, "python smoke.py; python src/checkout.py")
    second = session(atlas, "Branch & changes", "right")
    send(second, "git log --oneline --graph --all -8; git diff --stat; git status -sb")
    background = []
    for pid, p in zip(ids[1:], projects[1:]):
        info = session(pid, p["name"].split("-")[0].title() + " checks")
        send(info, "python smoke.py")
        background.append(info)
    for k in kids:
        info = session(k["id"], "Worktree checks")
        send(info, "python smoke.py; git status -sb")
    d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('files')", atlas)
    time.sleep(2)
    screen = daemon(d, "session.read", {"handle": first["handle"], "lines": 100})
    run.check("real checkout check output reached daemon terminal", "PASS" in str(screen))
    shot("workspace", 2)

    d.keys("ctrl+shift+p")
    d.keys("atlas")
    shot("search")
    esc()
    d.js(f"{S}.showRightPanel('git')")
    d.wait("return !!document.querySelector('.rightpanel')", 10)
    shot("changes", 2)
    d.js(f"{S}.openFile(arguments[0],'src/checkout.py')", atlas)
    d.wait("return document.querySelectorAll('.cm-content span[class]').length > 5", 30)
    shot("editor")

    d.js(f"{S}.openFile(arguments[0],'src/checkout.py',{{diff:true}})", atlas)
    d.wait("return !!document.querySelector('.cm-note-gutter')", 20)
    d.js(f"{S}.addNote(arguments[0],{{path:'src/checkout.py',line:38,side:'new',text:'coupon = coupon.strip().upper()',note:'Demo review: add a whitespace + lowercase coupon assertion; preserve integer-cent rounding.'}})", atlas)
    d.wait("return !!document.querySelector('.cm-note-mark.has')", 10)
    shot("review-notes")

    d.js(f"{S}.showRightPanel('files'); {S}.openFile(arguments[0],'docs/architecture.md')", atlas)
    d.wait("return !!document.querySelector('.markdown svg')", 40)
    shot("markdown")
    d.js(f"{S}.openFile(arguments[0],'data/latency.csv')", atlas)
    d.wait("return !!document.querySelector('.data-table tbody tr')", 20)
    shot("csv")
    d.js(f"{S}.showRightPanel('plan')")
    d.wait("return !!document.querySelector('.plan-group')", 20)
    d.js("document.querySelectorAll('.plan-group-head[aria-expanded=false]').forEach(b=>b.click())")
    shot("plan")
    def plan_files():
        repo = Path(projects[0]["path"])
        return {p: p.read_bytes() for folder in ("plans", "specs") for p in sorted((repo / folder).rglob("*.md"))}
    before = plan_files()
    box = d.js("const b=document.querySelector('.plan-task .plan-box'); if(b){b.click();return true} return false")
    time.sleep(1)
    after = plan_files()
    changed = [p.name for p in before if before[p] != after[p]]
    run.check("Plan checkbox writes one demo plan file", box and len(changed) == 1, ", ".join(changed))
    if box:
        d.js("document.querySelector('.plan-task .plan-box').click()")
        time.sleep(1)

    d.js(f"{S}.selectTab(arguments[0],{S}.workspace(arguments[0]).tabs[0].id); {S}.showRightPanel('files')", atlas)
    d.keys("ctrl+shift+n")
    shot("new-session")
    esc()
    send(background[0], ASK)
    d.wait(f"return Object.values({S}.sessions).some(s=>s.handle===arguments[0] && s.status==='waiting')", 30, 0.25, background[0]["handle"])
    d.keys("ctrl+shift+m")
    shot("dashboard")
    esc()
    waiting = next(s for s in daemon(d, "session.list") if s["handle"] == background[0]["handle"])
    daemon(d, "session.answer", {"handle":waiting["handle"],"waiting_since":waiting["waiting_since"],"data":"y","enter":True})
    run.check("answered a real shell approval prompt", True)
    d.keys("ctrl+shift+j")
    shot("board")
    esc()
    command("layout.menu")
    shot("layouts")
    esc()
    command("composer.open")
    d.wait("return !!document.querySelector('[data-composer]')", 10)
    shot("composer")
    esc()
    d.js(f"{S}.setSettings({{terminalTheme:'dracula'}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)")
    d.js("document.querySelector('.pane .term-wrap').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,clientX:650,clientY:350}))")
    d.wait("return !!document.querySelector('.ctx-menu .ctx-item')", 10)
    shot("terminal-menu")
    esc()
    d.js(f"{S}.setSettings({{terminalTheme:'default'}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)")
    d.keys("ctrl+shift+`")
    d.wait("return !!document.querySelector('.floating-term .xterm')", 20)
    fh = d.js(f"return {S}.sessions[{S}.state.floating.active].handle")
    time.sleep(2)
    daemon(d, "session.input", {"handle": fh, "data": ("function prompt { 'PS> ' }; " if WINDOWS else PROMPT.replace("clear", "")) + go_to(projects[0]["path"]) + "\r"})
    time.sleep(0.8)
    daemon(d, "session.input", {"handle": fh, "data": "git worktree list; python smoke.py\r"})
    shot("floating", 2)
    d.keys("ctrl+shift+`")

    # Clearly marked scenario data; not invented agent accomplishments.
    d.js(f"""{S}.addInbox({{kind:'worktree',title:'Demo: coupon-validation worktree',body:'Sample branch ready for isolated work',projectId:arguments[0]}});
        {S}.addInbox({{kind:'finished',title:'Demo: checkout review',body:'Sample notification — inspect the checkout diff and review note',projectId:arguments[0]}})""", atlas)
    d.click(".inbox-anchor")
    d.wait("return !!document.querySelector('.inbox .inbox-item')", 10)
    shot("inbox")
    esc()
    # Leave the bell, so its hover hint does not stay in later shots.
    d.pointer([("move", 700, 600)])
    command("worktree.new")
    d.wait("return !!document.querySelector('.worktree-dialog input')", 10)
    shot("worktree-dialog")
    esc()

    for name, kind, prompt, every, at in [
        ("Demo · nightly dependency review", "claude", "Review dependencies and report risks; do not modify files.", "daily", "02:00"),
        ("Demo · weekly release notes", "codex", "Draft release notes from this demo repository's commits.", "weekly", "17:00"),
        ("Demo · weekday test review", "omp", "Run python smoke.py and summarize failures.", "weekdays", "07:30"),
    ]:
        daemon(d, "automation.save", {"id":"", "name":name, "enabled":False, "kind":kind,"prompt":prompt,"cwd":projects[0]["path"],"project_id":atlas,"args":[],"every":every,"time":at,"weekday":5})
    d.js(f"{S}.openSettings('automations')")
    d.wait("return !!document.querySelector('.automation-name')", 10)
    shot("automations")
    esc()
    for section, name in [("agents","agents"),("appearance","settings"),("terminal","terminal-settings"),("shortcuts","shortcuts")]:
        d.js(f"{S}.openSettings(arguments[0])", section)
        time.sleep(0.8)
        if section == "agents":
            d.js("[...document.querySelectorAll('.settings-page *')].find(e=>e.childElementCount===0 && e.textContent==='Agent CLIs')?.scrollIntoView({block:'start'})")
        shot(name)
        esc()

    # Features beyond the main story, each on the fixture's own files and data.
    by_name = dict(zip((p["name"] for p in projects), ids))
    d.js(f"{S}.switchProject(arguments[0]); {S}.openFile(arguments[0],'notebooks/revenue-summary.ipynb')", by_name["pulse-analytics"])
    d.wait("return !!document.querySelector('.nb-out')", 30)
    shot("notebook")

    beacon = by_name["beacon-api"]
    d.js(f"{S}.switchProject(arguments[0])", beacon)
    server = session(beacon, "Catalog server", "right")
    send(server, "python src/app.py --serve")
    d.js(f"{S}.showRightPanel('processes')")
    d.wait("return !!document.querySelector('.proc-row .port-chip')", 30)
    shot("processes", 1.5)
    run.check("the Processes panel shows the catalog server's real port",
              "8765" in d.js("return document.querySelector('.proc-panel').innerText"))
    daemon(d, "session.input", {"handle": server["handle"], "data": "\x03"})
    d.js(f"{S}.showRightPanel('files')")

    # Every task waits for an approval or a dependency: loading the run starts no agent.
    spec = json.loads(Path(projects[0]["path"], "runs", "checkout-hardening.json").read_text(encoding="utf-8"))
    tasks = [{"deps": [], "approve": False, "worktree": False, **t} for t in spec["tasks"]]
    created = d.js_async("const done=arguments[arguments.length-1]; const r=window.__kelpie.runs; r.createRun({name:arguments[0], objective:arguments[1], cwd:arguments[2], project_id:arguments[3], tasks:arguments[4]}).then(()=>r.loadRuns()).then(()=>done(true), e=>done(String(e)))",
                         spec["name"], spec["objective"], projects[0]["path"], atlas, tasks)
    run.check("the fixture's run file loads as a run", created is True, str(created))
    d.js(f"{S}.switchProject(arguments[0]); {S}.setOverlay('runs')", atlas)
    d.wait("return document.querySelectorAll('.run-detail .run-task-title').length >= 3", 20)
    shot("runs", 1.5)
    d.js(f"{S}.setOverlay(null)")

    d.js("window.__kelpie.review.openTryAgents(arguments[0], arguments[1])", atlas,
         "Add smoke assertions for the free-shipping boundary at 9,999 and 10,000 discounted cents. Do not change pricing rules.")
    d.wait("return !!document.querySelector('.try-dialog')", 10)
    shot("try")
    d.js(f"{S}.setOverlay(null)")

    # Finish wants a clean main checkout: the review story's uncommitted change waits in a
    # stash meanwhile. The base branch is confirmed and the checks approved as a user would.
    git = lambda *a: subprocess.run(["git", "-C", projects[0]["path"], *a], check=True, capture_output=True, text=True).stdout
    stashes = git("stash", "list")
    git("stash", "push", "-q", "-m", "readme-shots: review change")
    # A clean checkout stashes nothing: pop only the entry this tour added.
    stashed = git("stash", "list") != stashes
    try:
        coupon = next(k for k in kids if "coupon-validation" in k["path"])
        d.js(f"{S}.switchProject(arguments[0]); window.__kelpie.review.openFinish(arguments[0])", coupon["id"])
        d.wait("return [...document.querySelectorAll('.finish-dialog button')].some(b=>b.textContent.includes('Approve these checks'))", 30)
        # Approving re-reads the plan, so the base branch is confirmed after it.
        d.js("[...document.querySelectorAll('.finish-dialog button')].find(b=>b.textContent.includes('Approve these checks'))?.click()")
        d.wait("return !document.querySelector('.finish-dialog').innerText.includes('have not been approved')", 20)
        time.sleep(1)
        d.js("const box=[...document.querySelectorAll('.finish-dialog .check-label')].find(l=>l.textContent.includes('Merge into'))?.querySelector('input'); if(box && !box.checked) box.click();")
        d.wait("const box=[...document.querySelectorAll('.finish-dialog .check-label')].find(l=>l.textContent.includes('Merge into'))?.querySelector('input'); return !box || box.checked", 10)
        d.js("document.querySelector('.finish-dialog').scrollTop = 0")
        shot("finish", 2)
        run.check("Finish shows the approved check from .kelpie.yaml",
                  "python smoke.py" in d.js("return document.querySelector('.finish-dialog').innerText"))
        d.js(f"{S}.setOverlay(null); {S}.switchProject(arguments[0])", atlas)
    finally:
        if stashed:
            try:
                git("stash", "pop", "-q")
            except subprocess.CalledProcessError as error:
                # The tour's own failure stays the one reported; say where the change waits.
                print(f"could not restore the review change (git stash pop: {error.stderr.strip()}); "
                      f"it is the newest entry of `git -C {projects[0]['path']} stash list`", flush=True)

    # Documentation addresses (RFC 5737): listed, never connected to.
    def host(label, alias, group, tags, hostname, user, color, notes):
        return {"id": "", "label": label, "alias": alias, "group": group, "tags": tags, "hostname": hostname,
                "port": 22, "user": user, "auth": "key", "identityFile": "", "jump": "", "forwardAgent": False,
                "keepalive": 30, "env": {}, "startup": "", "folder": "", "tmux": True, "notes": notes,
                "color": color, "terminalTheme": "", "os": "", "lastConnected": 0}
    hosts = [host("Staging API", "staging-api", "Staging", ["api", "demo"], "192.0.2.10", "deploy", "#e8c47a", "Demo entry with a documentation address; not a real server."),
             host("Build runner", "build-runner", "CI", ["builds", "demo"], "198.51.100.20", "ci", "#7fb4ef", "Demo entry; not a real server."),
             host("Production DB", "prod-db", "Production", ["database", "demo"], "203.0.113.30", "ops", "#ef7f7f", "Demo entry: production panes get this red stripe and theme.")]
    saved = d.js_async("const done=arguments[arguments.length-1]; const h=window.__kelpie.sshHosts; h.saveVault({...h.vault(), hosts:arguments[0]}).then(()=>done(true), e=>done(String(e)))", hosts)
    run.check("three demo SSH hosts saved in the isolated data folder", saved is True, str(saved))
    command("hosts")
    d.wait("return document.body.innerText.includes('Production DB')", 15)
    shot("hosts", 1.5)
    d.js(f"{S}.setOverlay(null)")

    # Installed into the isolated data folder; it stays off until reviewed.
    plugin = str(Path(projects[0]["path"]).parent / "plugins" / "kelpie-demo-tools")
    installed = d.js_async("const done=arguments[arguments.length-1]; const p=window.__kelpie.plugins; p.pluginInstallFolder(arguments[0]).then(()=>p.loadPlugins()).then(()=>done(true), e=>done(String(e)))", plugin)
    run.check("the demo plugin installs from its folder", installed is True, str(installed))
    d.js(f"{S}.openSettings('plugins')")
    d.wait("return document.body.innerText.includes('Kelpie demo tools')", 15)
    shot("plugins", 1)
    esc()

    if chat:
        sid = d.js_async("const done=arguments[arguments.length-1]; window.__kelpie.chat.newChat(arguments[0],arguments[1],'tab').then(i=>done(i?.id))", atlas, chat)
        if not sid:
            raise RuntimeError("chat did not start")
        time.sleep(3)
        d.js("window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]).send('Read README.md and src/checkout.py in this demo repository. Explain its purpose in three short bullets and suggest one validation edge case. Do not edit files or run commands.')", sid)
        d.wait("const c=window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return c.state.items.some(i=>i.kind==='assistant') && !c.state.busy", 240, 1, sid)
        shot("chat", 2)
        run.check("real agent chat produced a completed answer", True)

    d.js(f"{S}.switchProject(arguments[0]); {S}.selectTab(arguments[0],{S}.workspace(arguments[0]).tabs[0].id); {S}.setSettings({{theme:'light'}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)", atlas)
    shot("light", 2)
    d.js(f"{S}.setSettings({{theme:'dark',language:'vi'}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)")
    shot("vietnamese", 2)
    d.js(f"{S}.setSettings({{language:'en'}})")
    version = json.loads((Path(__file__).resolve().parents[2] / "app" / "src-tauri" / "tauri.conf.json").read_text(encoding="utf-8"))["version"]
    # open() rather than write_text(newline=), which macOS's own Python 3.9 lacks.
    with open(out / "capture.json", "w", encoding="utf-8", newline="\n") as f:
        f.write(json.dumps({
        "application": "Kelpie", "app_version": version, "platform": platform.platform(terse=True),
        "native": True, "viewport": [1920, 1080],
        "projects": [p["name"] for p in projects], "worktrees": ["feature/coupon-validation", "fix/shipping-threshold"],
        "screenshots": shots, "chat_agent": chat,
        "scenario_data": "Local demo repositories with synthetic data; seeded board stages, worktree comments, a review note, "
                         "labelled inbox samples, disabled automations, a run waiting for approval, documentation-address SSH hosts "
                         "that are never connected to, and a demo plugin that stays disabled. Shell output, the catalog server's "
                         "port and the optional chat answer are real.",
        "checks": run.results}, indent=2) + "\n")
    return shots


def capture_agents(run, projects, out, browser_pid=None):
    """Four real agents at work in one 2x2 grid: Claude Code, Codex and OMP in their own
    terminal interfaces and Claude Code in chat view, each on one read-only task with the
    user's own sign-in (real model calls). PowerShell gets a tab of its own."""
    d = run.d
    atlas = d.js(f"return {S}.state.projects.find(p=>p.path===arguments[0]).id", projects[0]["path"])
    names = []
    DOWN, ENTER = "\x1b[B", "\r"

    def shot(name):
        # Windows may shrink a window taller than the work area (taskbar) while agents
        # start; every agent shot asks for the full 1920 x 1080 again.
        try:
            d.set_window_size(1920, 1080)
        except (RuntimeError, TimeoutError) as error:
            print("window size not restored: " + str(error), flush=True)
        time.sleep(1)
        d.screenshot(str(out / (name + ".png")))
        names.append(name)
        print("SHOT " + name, flush=True)

    def launch(kind, name, where="tab", **opts):
        info = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],arguments[1],arguments[2]).then(done)", atlas, kind, {"name": name, "where": where, **opts})
        if not info:
            raise RuntimeError("could not launch " + name)
        info["launched_ms"] = time.time() * 1000
        time.sleep(3)
        return info

    def send(info, text):
        daemon(d, "session.input", {"handle": info["handle"], "data": text + "\r"})

    def screen(info):
        return daemon(d, "session.read", {"handle": info["handle"], "lines": 100})["text"]

    def state(info):
        return next(s for s in daemon(d, "session.list") if s["id"] == info["id"])

    def trust(info, marker, keys):
        # A folder the CLI has not seen asks to be trusted first. Claude Code's "Yes, I
        # trust" is its second choice; Codex's "Yes" is its first (its second quits).
        end = time.monotonic() + 40
        while time.monotonic() < end:
            if marker in screen(info).lower():
                for key in keys:
                    daemon(d, "session.input", {"handle": info["handle"], "data": key})
                    time.sleep(0.4)
                return
            if state(info)["status"] == "working":
                return
            time.sleep(0.5)

    def settle(info, label, timeout=300):
        """Waits for the agent's turn to end: it worked, then stayed quiet, or it asks you.
        The agents run at once, so a turn may already be over before its first look here:
        output well after the launch counts as having worked."""
        worked = False
        waiting_since = None
        end = time.monotonic() + timeout
        s = {}
        while time.monotonic() < end:
            s = state(info)
            if s.get("failed") or not s["alive"]:
                raise RuntimeError(label + " stopped: " + screen(info)[-2000:])
            if s["status"] == "working":
                worked = True
            elif s["status"] == "waiting":
                # A starting TUI can look like a prompt for a moment; only a wait that
                # lasts is returned (and then reported by the guard below).
                waiting_since = waiting_since or time.monotonic()
                if time.monotonic() - waiting_since > 45:
                    return s
            elif worked or s["last_activity"] > info["launched_ms"] + 5000:
                if time.time() * 1000 - s["last_activity"] > 6000:
                    return s
            if s["status"] != "waiting":
                waiting_since = None
            time.sleep(1)
        raise RuntimeError(f"{label} did not finish its read-only task (status {s.get('status')}): " + screen(info)[-1500:])

    def reveal(session_id):
        d.js(f"{S}.revealSession(arguments[0]); {S}.setRightPanel(null)", session_id)

    def full(session_id, name):
        reveal(session_id)
        loc = d.js(f"return {S}.paneLocation(arguments[0])", session_id)
        panes = d.js(f"return window.__kelpie.layout.allPanes({S}.activeTab(arguments[0]).root).length", atlas)
        if panes > 1:
            d.js(f"{S}.toggleZoom(arguments[0],arguments[1])", loc["tabId"], loc["paneId"])
        shot(name)
        if panes > 1:
            d.js(f"{S}.toggleZoom(arguments[0],arguments[1])", loc["tabId"], loc["paneId"])

    def agents_at_work(claude):
        grid = d.js(f"return {S}.paneLocation(arguments[0]).tabId", claude["id"])
        codex = launch("codex", "Codex · missing assertion", "right", prompt="Read smoke.py and src/checkout.py. Suggest one missing smoke assertion for the free-shipping boundary, in at most 6 lines." + rules)
        omp = launch("omp", "OMP · checkout review", "down", prompt="Read README.md and src/checkout.py. Explain the demo purpose and two checkout boundary cases in at most 8 lines." + rules)
        chat_id = d.js_async("const done=arguments[arguments.length-1]; window.__kelpie.chat.newChat(arguments[0],'claude','down').then(i=>done(i?.id))", atlas)
        if not chat_id:
            raise RuntimeError("Claude Code chat did not start")
        time.sleep(3)
        d.js("window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]).send(arguments[1])", chat_id,
             "Read README.md and src/checkout.py. Explain this demo's purpose in three short bullets and name one validation edge case." + rules)
        d.js(f"{S}.arrangeTab(arguments[0],arguments[1],{{mode:'rows',counts:[2,2]}}); {S}.renameTab(arguments[0],arguments[1],'Four agents · one checkout')", atlas, grid)
        trust(claude, "trust this folder", [DOWN, ENTER])
        trust(codex, "trust", [ENTER])
        for info, label in ((claude, "Claude Code"), (codex, "Codex"), (omp, "OMP")):
            settle(info, label)
        try:
            d.wait("const c=window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return c.state.items.some(i=>i.kind==='assistant') && !c.state.busy", 300, 1, chat_id)
        except TimeoutError:
            kinds = d.js("const c=window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]); return {busy:c.state.busy, items:c.state.items.map(i=>i.kind)}", chat_id)
            pane = d.js("const p=[...document.querySelectorAll('.pane')].find(e=>e.querySelector('.chat')); return p ? p.innerText.slice(-1500) : ''")
            print(f"--- Claude Code chat state: {kinds} ---", pane, "--- end ---", sep="\n", flush=True)
            raise RuntimeError("Claude Code chat did not answer within 300 s (its state is in the log above)")
        # A sign-in page, a quota or a model error is not an answer: stop before any shot.
        problems = ("is not supported when", "401 Unauthorized", "Waiting for browser authentication",
                    "total limit", "Sign in with ChatGPT", "Please log in", "/login")
        for info, label in ((claude, "Claude Code"), (codex, "Codex"), (omp, "OMP")):
            hit = next((p for p in problems if p in screen(info)), None)
            if hit:
                raise RuntimeError(f"{label} did not answer (its screen shows {hit!r}); sign it in again and rerun")
            # A real prompt keeps the session waiting; a passing look like one does not.
            # OMP has no hooks: its status is read from the screen, and a tip line ending in
            # "?" can read as a question, so only its screen text (above) decides for it.
            if label == "OMP":
                continue
            end = time.monotonic() + 30
            while state(info)["status"] == "waiting" and time.monotonic() < end:
                time.sleep(1)
            if state(info)["status"] == "waiting":
                print(f"--- {label} screen ---\n" + screen(info) + "\n--- end ---", flush=True)
                raise RuntimeError(f"{label} is waiting on a prompt instead of answering (its screen is in the log above)")

        # Codex can receive the terminal's colour reply (OSC 10/11) as typed text in its
        # prompt; that is a known defect, not the demo, so the prompt is cleared (Ctrl+U).
        if "]10;rgb" in screen(codex) or "]11;rgb" in screen(codex):
            print("Codex prompt held a terminal colour reply; clearing it", flush=True)
            daemon(d, "session.input", {"handle": codex["handle"], "data": "\x15"})
            time.sleep(1)
        run.check("Codex prompt holds no stray terminal reply", "]10;rgb" not in screen(codex) and "]11;rgb" not in screen(codex))

        for pid in d.js(f"return {S}.state.projects.filter(p=>p.id!==arguments[0]).map(p=>p.id)", atlas):
            d.js(f"{S}.setCollapsed(arguments[0],true)", pid)
        d.js(f"{S}.setSettings({{sidebarWidth:260}}); {S}.selectTab(arguments[0],arguments[1]); {S}.setRightPanel(null)", atlas, grid)
        shot("workspace")
        full(claude["id"], "claude")
        full(codex["id"], "codex")
        full(omp["id"], "omp")
        full(chat_id, "chat")

        shell_args = ["pwsh.exe", "-NoLogo"] if shutil.which("pwsh") else []
        pwsh = launch("shell", "PowerShell · demo checks", args=shell_args)
        send(pwsh, "function prompt { 'pwsh> ' }; Clear-Host; $PSVersionTable | Format-Table -AutoSize; python smoke.py; python src/checkout.py")
        time.sleep(3)
        full(pwsh["id"], "powershell")

        d.js(f"{S}.selectTab(arguments[0],arguments[1]); {S}.arrangeTab(arguments[0],arguments[1],{{mode:'rows',counts:[2,2]}})", atlas, grid)
        for theme, language, name in [("light", "en", "light"), ("dark", "vi", "vietnamese")]:
            d.js(f"{S}.setSettings({{theme:arguments[0],language:arguments[1]}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)", theme, language)
            shot(name)
        d.js(f"{S}.setSettings({{language:'en'}})")

        if browser_pid:
            # OMP gets a tab of its own (the PowerShell tab, whose shell joins the grid), and
            # the browser opens beside it. A pane moved into a tab that already holds a browser
            # pane leaves that page unpainted, so the browser is the one that arrives last.
            o = d.js(f"return {S}.paneLocation(arguments[0])", omp["id"])
            w = d.js(f"return {S}.paneLocation(arguments[0])", pwsh["id"])
            d.js(f"{S}.movePaneToTab(arguments[0],arguments[1],arguments[2],arguments[3])", atlas, o["tabId"], o["paneId"], w["tabId"])
            d.js(f"{S}.movePaneToTab(arguments[0],arguments[1],arguments[2],arguments[3])", atlas, w["tabId"], w["paneId"], grid)
            o = d.js(f"return {S}.paneLocation(arguments[0])", omp["id"])
            d.js(f"{S}.focusPane(arguments[0],arguments[1],arguments[2]); {S}.setRightPanel(null); window.__kelpie.browser.openBrowser('https://chatgpt.com','right',arguments[0])", atlas, o["tabId"], o["paneId"])
            d.wait("return Object.values(window.__kelpie.browser.browserState).some(b=>b.title?.includes('ChatGPT'))", 45)
            time.sleep(10)  # The remote page's title arrives before its client-rendered UI.
            # The site's cookie notice covers a page this narrow: its "Reject non-essential"
            # button sits centred, 111 px above the bottom (trusted input into the page).
            page = d.js("const r=document.querySelector('.browser-host').getBoundingClientRect(); return [r.width, r.height]")
            browser_pane = d.js(f"return window.__kelpie.layout.allPanes({S}.activeTab(arguments[0]).root).map(p=>p.id).find(id=>id!==arguments[1])", atlas, o["paneId"])
            clicked = d.invoke("browser_test_input", {"label": f"browser-{browser_pane}", "events": [
                {"method": "Input.dispatchMouseEvent", "params": {"type": kind, "x": page[0] / 2, "y": page[1] - 111, "button": "left", "clickCount": 1}}
                for kind in ("mouseMoved", "mousePressed", "mouseReleased")]})
            if "err" in clicked:
                print("cookie notice left open: " + clicked["err"], flush=True)
            time.sleep(3)
            # A click that missed the notice could follow a link; the page must still be ChatGPT.
            page_now = d.js("return Object.values(window.__kelpie.browser.browserState).map(b=>[b.url, b.title])")
            run.check("the browser pane still shows chatgpt.com after the cookie click",
                      any("chatgpt.com" in (u or "") and "ChatGPT" in (t or "") for u, t in page_now), str(page_now))
            # WebDriver omits child webviews: PrintWindow captures the real native window,
            # cropped to its content area. The page is light and Kelpie's placeholder behind it
            # dark (as is the dimmed page under the cookie notice), so a dark area means retry.
            host = d.js("const r=document.querySelector('.browser-host').getBoundingClientRect(), k=devicePixelRatio; return [r.x*k, r.y*k, r.width*k, r.height*k].map(Math.round).join(',')")
            for attempt in range(3):
                result = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(Path(__file__).resolve().parents[1] / "capture-window.ps1"), "-ProcessId", str(browser_pid), "-Out", str(out / "chatgpt.png"), "-ClientOnly", "-MeasureRect", host], check=True, capture_output=True, text=True)
                mean = int(next(l.split()[1] for l in result.stdout.splitlines() if l.startswith("mean")).replace(",", ""))
                if mean > 200:
                    break
                time.sleep(5)
            run.check("ChatGPT web is painted in the browser pane", mean > 200, f"mean brightness {mean}")
            names.append("chatgpt")
        answers = d.js("return window.__kelpie.chat.getController(window.__kelpie.store.sessions[arguments[0]]).state.items.filter(i=>i.kind==='assistant').length", chat_id)
        alive = {label: state(i)["alive"] for i, label in ((claude, "claude"), (codex, "codex"), (omp, "omp"))}
        run.check("four agents answered in the grid: Claude Code, Codex, OMP and Claude Code chat",
                  all(alive.values()) and answers > 0, f"alive={alive} chat_answers={answers}")
        run.check("PowerShell ran the real demo checks", "PASS atlas-commerce" in screen(pwsh))
        return names

    d.js(f"{S}.switchProject(arguments[0])", atlas)
    rules = " Do not edit files or run commands."
    claude = launch("claude", "Claude Code · checkout risks", prompt="Read src/checkout.py and docs/checkout-rules.md. Name the two riskiest checkout boundaries in at most 6 lines." + rules)
    try:
        return agents_at_work(claude)
    finally:
        # Claude Code counts a killed launch against its fullscreen mode in the user's own
        # ~/.claude.json, so it leaves through /exit before the daemon shuts down, also
        # when the tour failed half-way.
        try:
            daemon(d, "session.input", {"handle": claude["handle"], "data": "\x1b"})
            time.sleep(0.5)
            # Enter goes on its own: a TUI reads a fast Enter as part of pasted text.
            daemon(d, "session.input", {"handle": claude["handle"], "data": "/exit"})
            time.sleep(0.5)
            daemon(d, "session.input", {"handle": claude["handle"], "data": "\r"})
            end = time.monotonic() + 20
            while time.monotonic() < end and state(claude)["alive"]:
                time.sleep(0.5)
            run.check("Claude Code left through /exit", not state(claude)["alive"])
        except Exception as error:  # the tour's own failure is the one to report
            print("could not end Claude Code cleanly: " + str(error), flush=True)



EXTRA_SCENES = ["sidebar", "quick-answer", "split-picker", "broadcast", "file-link", "theme-editor", "git-history", "review-queue",
                "review-with", "automation-event", "pdf", "design-mode", "host-edit", "notifications", "mobile-settings", "agent-setup",
                "voice", "settings-export", "chat-approval", "sub-sessions", "owed-checks", "past-conversations", "usage"]
SIMULATED = "simulated: a local program (scripts/e2e/demo_agents.py) stands in for the agent; no model was called"


def demo_pdf(path):
    """A one-page PDF (a demo release checklist), written with the standard library."""
    lines = [("F2", 20, "atlas-commerce 0.9 - release checklist (demo)"), ("F1", 12, ""),
             ("F1", 12, "[x] python smoke.py passes on main"), ("F1", 12, "[x] Coupon codes are trimmed and upper-cased"),
             ("F1", 12, "[x] Free shipping from 10,000 discounted cents"), ("F1", 12, "[ ] Release notes reviewed"),
             ("F1", 12, "[ ] Tag v0.9.0 and push the tag"), ("F1", 12, ""),
             ("F1", 10, "Sample document of the Kelpie demo projects; no real product.")]
    text = "BT 72 740 Td 18 TL\n" + "".join(f"/{font} {size} Tf ({line}) Tj T*\n" for font, size, line in lines) + "ET"
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>",
            f"<< /Length {len(text)} >>\nstream\n{text}\nendstream",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold >>"]
    out = b"%PDF-1.4\n"
    offsets = []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n{o}\nendobj\n".encode("latin-1")
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode() + "".join(f"{o:010d} 00000 n \n" for o in offsets).encode()
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    Path(path).write_bytes(out)


DEMO_PAGE = """<!doctype html><html><head><meta charset="utf-8"><title>Atlas storefront (demo)</title><style>
body{font:16px system-ui,sans-serif;margin:0;background:#f6f7fb;color:#1d2433}header{background:#24324a;color:#fff;padding:18px 28px;font-size:20px}
main{padding:24px 28px;display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:18px}.card{background:#fff;border-radius:10px;padding:16px;box-shadow:0 1px 3px #0002}
.price{font-weight:700;margin:8px 0}button{background:#2d7ff9;color:#fff;border:0;border-radius:8px;padding:10px 16px;font-size:15px}
.note{grid-column:1/-1;color:#667}</style></head><body><header>Atlas storefront &middot; demo page</header><main>
<div class="card"><b>Trail kit</b><div class="price">$100.00</div><button id="add-kit">Add to cart</button></div>
<div class="card"><b>Rain shell</b><div class="price">$64.50</div><button id="add-shell">Add to cart</button></div>
<div class="card"><b>Camp mug</b><div class="price">$12.00</div><button id="add-mug">Add to cart</button></div>
<p class="note">Local sample page of the Kelpie demo projects, served on this computer only.</p></main></body></html>"""


def seed_transcripts(projects, claude_home, codex_home):
    """Synthetic Claude Code and Codex transcripts about the demo projects, in the run's own agent homes (never the user's)."""
    import uuid
    from datetime import datetime, timedelta, timezone
    by = {p["name"]: p["path"] for p in projects}
    now = datetime.now(timezone.utc)
    def iso(t):
        return t.isoformat().replace("+00:00", "Z")
    claude = [("atlas-commerce", "Fix pasted coupon codes", "Pasted coupon codes like ' welcome10 ' are rejected. Fix it and add a smoke check.", 3, 900_000, 21_000),
              ("atlas-commerce", "Free-shipping boundary review", "Review the free-shipping threshold at 9,999 and 10,000 cents.", 26, 420_000, 9_000),
              ("beacon-api", "Tag filter for the catalog", "Add a ?tag= filter to /v1/products with a test.", 50, 1_300_000, 30_000),
              ("relay-workers", "Retry with backoff", "Explain how relay-workers retries a failed job.", 75, 260_000, 6_000)]
    for name, title, prompt, hours, read, out in claude:
        conv = str(uuid.uuid4())
        folder = Path(claude_home) / "projects" / "".join(c if c.isalnum() else "-" for c in by[name])
        folder.mkdir(parents=True, exist_ok=True)
        t = now - timedelta(hours=hours)
        lines = [{"type": "user", "timestamp": iso(t), "cwd": by[name], "sessionId": conv, "message": {"role": "user", "content": prompt}},
                 {"type": "ai-title", "aiTitle": title, "sessionId": conv}]
        for i in range(3):
            lines.append({"type": "assistant", "timestamp": iso(t + timedelta(minutes=4 * i + 1)), "cwd": by[name], "sessionId": conv, "isSidechain": False,
                          "message": {"id": f"msg_demo_{conv[:8]}_{i}", "model": "claude-opus-5-5", "role": "assistant", "content": [{"type": "text", "text": "Demo transcript line."}],
                                      "usage": {"input_tokens": 1200, "cache_read_input_tokens": read // 3, "cache_creation_input_tokens": 20_000, "output_tokens": out // 3}}})
        with open(folder / f"{conv}.jsonl", "w", encoding="utf-8", newline="\n") as f:
            f.writelines(json.dumps(l, separators=(",", ":")) + "\n" for l in lines)
    for name, prompt, hours in [("atlas-commerce", "Suggest one missing smoke assertion for the coupon rules.", 8),
                                ("pulse-analytics", "Summarise the revenue notebook in five lines.", 30)]:
        conv = str(uuid.uuid4())
        t = now - timedelta(hours=hours)
        folder = Path(codex_home) / "sessions" / t.strftime("%Y") / t.strftime("%m") / t.strftime("%d")
        folder.mkdir(parents=True, exist_ok=True)
        lines = [{"timestamp": iso(t), "type": "session_meta", "payload": {"id": conv, "cwd": by[name], "timestamp": iso(t)}},
                 {"timestamp": iso(t), "type": "event_msg", "payload": {"type": "task_started"}},
                 {"timestamp": iso(t), "type": "event_msg", "payload": {"type": "user_message", "message": prompt}}]
        with open(folder / f"rollout-{t.strftime('%Y-%m-%dT%H-%M-%S')}-{conv}.jsonl", "w", encoding="utf-8", newline="\n") as f:
            f.writelines(json.dumps(l, separators=(",", ":")) + "\n" for l in lines)


def capture_extras(run, projects, out, programs, browser_pid=None, only=None, mobile_port=48771):
    """More pictures after the base tour, on the same fixtures: one scene per feature. A scene that fails is reported and
    skipped; the others still run. Agents in these scenes are simulated (demo_agents.py) and marked so."""
    d = run.d
    shots = {}
    failed = []
    ids = d.js(f"return {S}.state.projects.filter(p => !p.parentId).map(p => [p.id, p.path])")
    by_path = {os.path.normcase(path): pid for pid, path in ids}
    by_name = {p["name"]: by_path[os.path.normcase(p["path"])] for p in projects}
    atlas, beacon, relay, pulse = by_name["atlas-commerce"], by_name["beacon-api"], by_name["relay-workers"], by_name["pulse-analytics"]
    kids = d.js(f"return {S}.childrenOf(arguments[0]).map(p=>({{id:p.id,path:p.path}}))", atlas)
    shipping = next(k for k in kids if "shipping" in k["path"])
    py = sys.executable

    def shot(name, how, agent=None, wait=1.0):
        time.sleep(wait)
        d.screenshot(str(out / (name + ".png")))
        shots[name] = {"how": how, **({"agent": agent} if agent else {})}
        print("SHOT " + name, flush=True)

    def native(name, how):
        subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(Path(__file__).resolve().parents[1] / "capture-window.ps1"),
                        "-ProcessId", str(browser_pid), "-Out", str(out / (name + ".png")), "-ClientOnly"], check=True, capture_output=True, text=True)
        shots[name] = {"how": how}
        print("SHOT " + name, flush=True)

    def until(fn, timeout=20, step=0.4):
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

    def must(fn, what, timeout=20):
        v = until(fn, timeout)
        if not v:
            raise RuntimeError("not reached: " + what)
        return v

    def calm():
        for _ in range(2):
            d.keys("escape")
            time.sleep(0.2)
        d.js(f"{S}.setOverlay(null)")
        # A scrollIntoView can scroll the page itself; every picture starts from the top.
        d.js("document.scrollingElement.scrollTop = 0; document.body.scrollTop = 0;")
        d.pointer([("move", 900, 700)])

    def shell(pid, name, command=None, where="tab"):
        info = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],'shell',{{name:arguments[1],where:arguments[2]}}).then(done)", pid, name, where)
        if not info:
            raise RuntimeError("shell did not start: " + name)
        time.sleep(2)
        daemon(d, "session.input", {"handle": info["handle"], "data": PROMPT + "\r"})
        time.sleep(0.5)
        if command:
            daemon(d, "session.input", {"handle": info["handle"], "data": command + "\r"})
        return info

    def chat(pid, name, program, where="tab"):
        return d.js_async("""const [pid, name, argv, where, done] = arguments; const k = window.__kelpie;
            const p = k.store.state.projects.find((x) => x.id === pid);
            k.ipc.createSession({ kind: 'claude', cwd: p.path, project_id: pid, name, chat_argv: argv }).then((info) => {
              k.store.setSessions(info.id, info); k.store.placePane(pid, { id: 'demo-' + info.id, kind: 'chat', sessionId: info.id }, where); done(info); }, (e) => done(null));""",
                          pid, name, [py, "-u", programs[program]], where)

    def status(handle):
        return next((s.get("status") for s in daemon(d, "session.list") if s["handle"] == handle), None)

    def click_text(selector, text):
        ok = d.js("const b = [...document.querySelectorAll(arguments[0])].find((b) => b.textContent.trim() === arguments[1] && !b.disabled); if (b) b.click(); return !!b;", selector, text)
        if not ok:
            raise RuntimeError(f"no {selector} {text!r}")

    def scene(name):
        def wrap(fn):
            if only and name not in only:
                return fn
            calm()
            try:
                fn()
            except Exception as error:  # noqa: BLE001 - one scene's failure must not cost the others
                failed.append(name)
                print(f"SCENE FAILED {name}: {error}", flush=True)
            return fn
        return wrap

    @scene("sidebar")
    def _():
        for pid in by_name.values():
            d.js(f"{S}.setCollapsed(arguments[0], false)", pid)
        # The screen is cleared first, so only the program's own output shows (not the path it runs from).
        cls = "Clear-Host" if WINDOWS else "clear"
        menu = shell(relay, "Migration", f'{cls}; python -u "{programs["menu"]}"')
        shell(relay, "Checks (watch)", f'{cls}; python -u "{programs["ticker"]}"', "right")
        must(lambda: status(menu["handle"]) == "waiting", "the menu session waits")
        d.js(f"{S}.switchProject(arguments[0]); {S}.selectTab(arguments[0], {S}.workspace(arguments[0]).tabs[0].id); {S}.showRightPanel('files')", atlas)
        shot("sidebar", "the sidebar with a waiting session (a menu), a working one (a ticker) and the demo projects' comments, branches and worktrees", "simulated", 2)

    @scene("quick-answer")
    def _():
        d.keys("ctrl+shift+m")
        must(lambda: d.js("return !!document.querySelector('.dash-col .card .dash-answer')"), "a dashboard card with Answer")
        d.js("const c = [...document.querySelectorAll('.dash-col .card')].find(c => c.querySelector('.name')?.textContent === 'Migration') || document.querySelector('.dash-col .card'); c.querySelector('.dash-answer').click();")
        must(lambda: d.js("return !!document.querySelector('.quick-answer')"), "the quick-answer popover")
        shot("quick-answer", "Agent Dashboard: Answer on a waiting session opens the quick answer without leaving the board", "simulated", 1.2)

    @scene("split-picker")
    def _():
        info = shell(atlas, "Split here", "git status -sb")
        d.js(f"{S}.revealSession(arguments[0])", info["id"])
        time.sleep(1)
        d.js("const v = window.__kelpie.views().find(x => x.sessionId === arguments[0]); v && v.term.focus();", info["id"])
        d.keys("ctrl+shift+d")
        must(lambda: d.js("return !!document.querySelector('.empty-pane')"), "the empty pane")
        shot("split-picker", "Ctrl+Shift+D on a shell: the new empty pane asks what to run")

    @scene("broadcast")
    def _():
        first = shell(pulse, "Check 1", "python smoke.py")
        for i, where in ((2, "right"), (3, "right"), (4, "right")):
            shell(pulse, f"Check {i}", "Clear-Host" if WINDOWS else "clear", where)
        d.js(f"{S}.switchProject(arguments[0]); {S}.setRightPanel(null)", pulse)
        tab = d.js(f"return {S}.paneLocation(arguments[0]).tabId", first["id"])
        d.js(f"{S}.arrangeTab(arguments[0], arguments[1], {{mode:'rows', counts:[2,2]}}); {S}.setBroadcast(arguments[0], arguments[1], true)", pulse, tab)
        must(lambda: d.js("return document.querySelectorAll('.pane.broadcast').length === 4"), "four panes in broadcast")
        d.js("document.querySelectorAll('.pane[data-pane] .xterm-helper-textarea')[0].focus()")
        d.keys("python smoke.py", "enter")
        time.sleep(2.5)
        shot("broadcast", "Tile all (2 x 2) and Broadcast input: one line typed once runs in all four shells")
        d.js(f"{S}.setBroadcast(arguments[0], arguments[1], false)", pulse, tab)

    @scene("file-link")
    def _():
        info = shell(atlas, "Review notes", "Write-Output 'Review: src/checkout.py:38 rounds before the coupon check'" if WINDOWS else "echo 'Review: src/checkout.py:38 rounds before the coupon check'")
        d.js(f"{S}.switchProject(arguments[0]); {S}.revealSession(arguments[1]); {S}.showRightPanel('files')", atlas, info["id"])
        time.sleep(1.5)
        spot = must(lambda: d.js("""const v = window.__kelpie.views().find(x => x.sessionId === arguments[0]); if (!v) return null; const t = v.term;
            const el = t.element.querySelector('.xterm-screen'); const r = el.getBoundingClientRect(); const cw = r.width / t.cols, ch = r.height / t.rows;
            for (let y = 0; y < t.rows; y++) { const line = t.buffer.active.getLine(t.buffer.active.viewportY + y)?.translateToString(true) || '';
              const i = line.indexOf('src/checkout.py:38'); if (i >= 0 && !line.includes("'")) return [r.left + (i + 6) * cw, r.top + (y + 0.5) * ch]; } return null;""", info["id"]), "the printed file link")
        d.pointer([("move", spot[0] - 40, spot[1]), ("move", spot[0], spot[1], 200), ("pause", 1200)])
        # WebDriver hovers without the real pointer, so the native "Ctrl+click to open" tooltip is not in the picture: the underline is.
        shot("file-link", "a path printed by a shell is a link: hovered, it is underlined (Ctrl+click opens the file at that line)", wait=0.5)

    @scene("theme-editor")
    def _():
        d.js(f"{S}.openSettings('terminal')")
        must(lambda: d.js("return !!document.querySelector('[data-action=edit-themes]')"), "Edit themes")
        d.click("[data-action=edit-themes]")
        must(lambda: d.js("return !!document.querySelector('[data-theme-editor]')"), "the theme editor")
        d.js("const s = document.querySelector('[data-action=theme-duplicate]'); s.value = 'dracula'; s.dispatchEvent(new Event('change', {bubbles: true}));")
        must(lambda: d.js("return !!document.querySelector('[data-field=theme-name]')"), "the duplicated theme")
        shot("theme-editor", "Settings → Terminal → Edit themes: Dracula duplicated into a theme of one's own, with its preview")

    @scene("git-history")
    def _():
        # One stashed draft of its own (only that path), so the review change in the working tree stays as it is.
        repo = Path(projects[0]["path"])
        draft = repo / "docs" / "coupon-notes.md"
        if "WIP: coupon notes draft" not in subprocess.run(["git", "-C", str(repo), "stash", "list"], capture_output=True, text=True).stdout:
            draft.write_text("# Coupon notes (draft)\n\n- Trim and upper-case pasted codes.\n", encoding="utf-8")
            subprocess.run(["git", "-C", str(repo), "stash", "push", "-u", "-q", "-m", "WIP: coupon notes draft", "--", "docs/coupon-notes.md"], check=True)
        d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('git')", atlas)
        must(lambda: d.js("return !!document.querySelector('.group-label[data-section=\"log\"]')"), "History")
        if d.js("return document.querySelector('.group-label[data-section=\"log\"]').getAttribute('aria-expanded') !== 'true'"):
            d.click('.group-label[data-section="log"]')
        must(lambda: d.js("return document.querySelectorAll('.commit-row').length >= 1"), "History's commits")
        shot("git-history", "Changes panel: the branch, the changed files and History unfolded", wait=1.5)
        # One section is open at a time: Stashes next, with the demo stash.
        d.click('.group-label[data-section="stashes"]')
        must(lambda: d.js("return document.querySelectorAll('.stash-row').length >= 1"), "the stash row")
        shot("git-stashes", "Changes panel: Stashes unfolded, one demo stash with Apply and Drop", wait=1.0)

    @scene("review-queue")
    def _():
        # The worktree in Review gets one demo commit of its own (once per demo kit), so its card has a diff to show.
        git = lambda *a: subprocess.run(["git", "-C", shipping["path"], *a], check=True, capture_output=True, text=True).stdout  # noqa: E731
        if git("rev-list", "--count", "main..HEAD").strip() == "0":
            smoke = Path(shipping["path"]) / "smoke.py"
            text = smoke.read_text(encoding="utf-8")
            line = 'assert quote([LineItem("kit", 9999)]).shipping_cents == 595\nassert quote([LineItem("kit", 10000)]).shipping_cents == 0\n'
            smoke.write_text(text.replace("\nprint(", "\n" + line + "print(", 1) if "\nprint(" in text else text + line, encoding="utf-8")
            git("commit", "-qam", "test: free shipping at 9,999 and 10,000 cents")
        d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('review')", atlas)
        must(lambda: d.js("return !!document.querySelector('.rightpanel') && document.querySelector('.rightpanel').innerText.includes('shipping')"), "the Review tab with the shipping worktree")
        time.sleep(2)
        d.js("[...document.querySelectorAll('.rightpanel button')].find(b => b.textContent.trim() === 'Open diff')?.click()")
        shot("review-queue", "the Review tab: a worktree waiting for review with its diff, Request changes, Review with… and Approve → Finish", wait=3)

    @scene("review-with")
    def _():
        d.js("window.__kelpie.review.openReviewWith(arguments[0])", shipping["id"])
        must(lambda: d.js("return !!document.querySelector('.review-with')"), "the Review with dialog")
        shot("review-with", "Review with another agent: the dialog, cancelled (no agent started)")

    @scene("automation-event")
    def _():
        d.js(f"{S}.openSettings('automations')")
        must(lambda: d.js("return [...document.querySelectorAll('.btn')].some(b => b.textContent.includes('New automation'))"), "New automation")
        d.js("[...document.querySelectorAll('.btn')].find(b => b.textContent.includes('New automation')).click()")
        must(lambda: d.js("return !!document.querySelector('.automation-editor')"), "the automation editor")
        def fill(label, value, tag="input"):
            d.js("const f = [...document.querySelectorAll('.automation-editor .field')].find(f => f.querySelector('span')?.textContent === arguments[0]); if (!f) return; const el = f.querySelector(arguments[2]); if (!el) return; el.value = arguments[1]; el.dispatchEvent(new Event(arguments[2] === 'select' ? 'change' : 'input', {bubbles: true}));", label, value, tag)
        fill("Name", "Demo · keep the docs index current")
        fill("Starts", "event", "select")
        time.sleep(0.3)
        fill("When", "file", "select")
        time.sleep(0.3)
        fill("Files", "docs/**/*.md")
        fill("Prompt", "A Markdown file under docs/ changed: update docs/index.md so it lists every page. Do not edit other files.", "textarea")
        shot("automation-event", "Settings → Automations → New automation: started by an event (a Markdown file under docs/ changes); not saved")

    @scene("pdf")
    def _():
        pdf = Path(projects[0]["path"]) / "docs" / "release-checklist.pdf"
        if not pdf.exists():
            demo_pdf(pdf)
        d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('files'); {S}.openFile(arguments[0], 'docs/release-checklist.pdf')", atlas)
        time.sleep(4)
        if browser_pid:
            # The PDF plugin's frame is left out of WebDriver's picture: the native window is captured instead.
            native("pdf", "the PDF viewer on a demo release checklist (native window capture)")
        else:
            shot("pdf", "the PDF viewer on a demo release checklist", wait=1)

    @scene("design-mode")
    def _():
        if not browser_pid:
            raise RuntimeError("needs the native window (Windows)")
        import http.server, threading
        site = Path(tempfile.mkdtemp(prefix="demo-site-"))
        (site / "index.html").write_text(DEMO_PAGE, encoding="utf-8")
        handler = lambda *a, **k: http.server.SimpleHTTPRequestHandler(*a, directory=str(site), **k)  # noqa: E731
        http.server.SimpleHTTPRequestHandler.log_message = lambda *a: None
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{server.server_address[1]}/"
        info = shell(beacon, "Storefront", None)
        d.js(f"{S}.switchProject(arguments[0]); {S}.revealSession(arguments[1]); {S}.setRightPanel(null); window.__kelpie.browser.openBrowser(arguments[2], 'right', arguments[0])", beacon, info["id"], url)
        must(lambda: d.js("return Object.values(window.__kelpie.browser.browserState).some(b => b.title?.includes('Atlas storefront'))"), "the demo page in a browser pane", 30)
        time.sleep(2)
        pane = d.js("const b = Object.entries(window.__kelpie.browser.browserState).find(([k, v]) => v.title?.includes('Atlas storefront')); return b && b[0]")
        label = f"browser-{pane}"
        d.js("document.querySelector('.browser-bar .icon-btn[title^=\"Design mode\"]').click()")
        time.sleep(0.8)
        def page(*events):
            r = d.invoke("browser_test_input", {"label": label, "events": list(events)})
            if "err" in r:
                raise RuntimeError(r["err"])
        mouse = lambda kind, x, y, **e: {"method": "Input.dispatchMouseEvent", "params": {"type": kind, "x": x, "y": y, **e}}  # noqa: E731
        page(mouse("mouseMoved", 60, 160), mouse("mouseMoved", 90, 175))
        page(mouse("mousePressed", 90, 175, button="left", clickCount=1), mouse("mouseReleased", 90, 175, button="left", clickCount=1))
        must(lambda: d.js("return Object.values(window.__kelpie.browser.browserState).some(b => (b.notes || []).length)"), "a picked element")
        page(mouse("mouseMoved", 300, 175))
        time.sleep(1.5)
        native("design-mode", "a browser pane on a local demo page in design mode, one element picked (native window capture: WebDriver leaves child webviews out)")
        d.js("const k = window.__kelpie.browser; for (const [id, b] of Object.entries(k.browserState)) if (b.design) document.querySelector('.browser-bar .icon-btn[title^=\"Design mode\"]')?.click();")
        server.shutdown()

    @scene("host-edit")
    def _():
        d.js("window.__kelpie.commands.find(c => c.id === 'hosts').run()")
        must(lambda: d.js("return !!document.querySelector('[data-host=prod-db]')"), "the Production DB host")
        d.click("[data-host=prod-db]")
        time.sleep(0.5)
        click_text(".hosts-page button", "Edit")
        must(lambda: d.js("return !!document.querySelector('[data-host-editor]')"), "the host editor")
        d.js("const o = [...document.querySelectorAll('[data-host-editor] details.drawer-opt')].find((x) => x.querySelector('[data-terminal-preview]')); if (o) { o.open = true; o.scrollIntoView(); }")
        shot("host-edit", "SSH hosts: a demo host's drawer with its appearance (label colour, theme, font, cursor) and live preview; documentation address, never connected")

    @scene("notifications")
    def _():
        d.js(f"{S}.openSettings('notifications')")
        shot("notifications", "Settings → Notifications: desktop, ntfy, Telegram and webhook channels (none set up)", wait=1.5)

    @scene("mobile-settings")
    def _():
        import phone_support as ps
        cert, key = ps.certificate_for_loopback(str(out))
        daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": mobile_port, "cert": cert, "key": key})
        d.js(f"{S}.openSettings('mobile')")
        must(lambda: d.js("return !!document.querySelector('[data-mobile-advanced]')"), "Settings → Mobile")
        shot("mobile-settings", "Settings → Mobile with phone access on in an isolated daemon on this computer's loopback (no pairing code shown)", wait=1.5)
        for f in (cert, key):
            os.remove(f)

    @scene("agent-setup")
    def _():
        mcp = Path(projects[0]["path"]) / ".mcp.json"
        if not mcp.exists():
            mcp.write_text(json.dumps({"mcpServers": {"demo-catalog": {"command": "python", "args": ["tools/catalog_mcp.py"]}}}, indent=2) + "\n", encoding="utf-8")
        d.js(f"{S}.switchProject(arguments[0]); {S}.openSettings('agent-setup')", atlas)
        time.sleep(3)
        shot("agent-setup", "Settings → Agent setup: the demo project's MCP server and the agents' instruction files (an isolated agent home, not the user's)")

    @scene("voice")
    def _():
        d.js(f"{S}.openSettings('voice')")
        shot("voice", "Settings → Voice dictation: on-device speech models to download (no microphone on the test machine)", wait=1.5)

    @scene("settings-export")
    def _():
        d.js(f"{S}.openSettings('daemon', 'config-transfer')")
        must(lambda: d.js("return !!document.querySelector('[data-setting=\"config-transfer\"] [data-act=\"export\"]')"), "the export row", 30)
        d.js("document.querySelector('[data-setting=\"config-transfer\"]').scrollIntoView({block: 'center'})")
        shot("settings-export", "Settings → Session daemon → Export and import settings, with the parts to include")

    @scene("chat-approval")
    def _():
        d.js(f"{S}.switchProject(arguments[0])", beacon)
        info = chat(beacon, "Release 0.4.0", "chat_approval")
        if not info:
            raise RuntimeError("the simulated chat did not start")
        must(lambda: d.js("const s = window.__kelpie.store.sessions[arguments[0]]; const c = s && window.__kelpie.chat.getController(s); return c && c.state.items.some(i => i.kind === 'permission')", info["id"]), "the approval card", 30)
        shot("chat-approval", "chat view waiting for approval of a risky command (tag and push), fed by a simulated agent", "simulated", 2)

    @scene("sub-sessions")
    def _():
        agents = [{"id": "demo-lead", "label": "Demo agent (simulated)", "bin": py, "launch": ["-u", programs["lead"]], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []},
                  {"id": "demo-helper", "label": "Demo helper (simulated)", "bin": py, "launch": ["-u", programs["helper"]], "yolo": [], "resume_latest": [], "prompt": "typed", "acp": []}]
        daemon(d, "agent.set_custom", {"agents": agents})
        d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.agents.loadAgentPresets().then(() => done(true), () => done(false))")
        daemon(d, "daemon.set_policy", {"sub_sessions": "ask", "sub_max": 4})
        lead = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],'demo-lead',{{name:'Release prep (simulated)',where:'tab'}}).then(done)", atlas)
        if not lead:
            raise RuntimeError("the demo agent did not start")
        d.js(f"{S}.switchProject(arguments[0]); {S}.revealSession(arguments[1]); {S}.showRightPanel('files')", atlas, lead["id"])
        must(lambda: d.js("return !!document.querySelector('.sub-card [data-sub-primary]')"), "the sub-session request card", 30)
        shot("sub-sessions-ask", "an agent asks to open a sub-session: the card with Allow once, Allow for this session, Always allow and Deny", "simulated", 1.2)
        time.sleep(0.6)
        click_text(".sub-card button", "Allow for this session")
        must(lambda: d.js("return document.querySelectorAll('.sub-strip .sub-chip:not(.more)').length >= 3"), "the family strip", 40)
        time.sleep(6)
        shot("sub-sessions", "a simulated agent with two sub-sessions: the sub-tab strip over its pane and the tree in the sidebar", "simulated", 1)

    @scene("owed-checks")
    def _():
        d.click(".inbox-anchor")
        must(lambda: d.js("return !!document.querySelector('.inbox .inbox-item.owed')"), "a check still owed in Activity", 40)
        shot("owed-checks", "Activity: a sub-session's report with a check it could not run itself (look at it on a phone)", "simulated", 1.5)

    @scene("past-conversations")
    def _():
        d.js(f"{S}.switchProject(arguments[0]); {S}.selectTab(arguments[0], {S}.workspace(arguments[0]).tabs[0].id); {S}.showRightPanel('history')", atlas)
        must(lambda: d.js("return document.querySelector('.rightpanel')?.innerText.includes('coupon')"), "the demo conversations", 40)
        shot("past-conversations", "the Sessions tab: past Claude Code and Codex conversations of this project, from synthetic demo transcripts", "simulated", 1.5)

    @scene("usage")
    def _():
        d.js(f"{S}.setOverlay('cost')")
        must(lambda: d.js("return !!document.querySelector('.cost-page')"), "the Usage & cost page")
        d.js("document.querySelector('.cost-page [data-by=project]')?.click(); document.querySelector('.cost-page [data-range=all]')?.click();")
        must(lambda: d.js("const p = document.querySelector('.cost-page'); return !p.innerText.includes('Counting') && p.querySelectorAll('.cost-row .label').length >= 2"), "the demo projects' totals", 90)
        shot("usage", "Usage & cost by project, from synthetic demo transcripts (Opus 5.5 prices)", "simulated", 2)

    calm()
    return shots, failed


def _main_window(pid):
    """A process's largest visible top-level window and its frame in physical screen pixels (Windows): (hwnd, (x, y, w, h))."""
    import ctypes
    import ctypes.wintypes as wt
    u = ctypes.windll.user32
    u.SetProcessDPIAware()
    found = []

    def each(hwnd, _):
        owner = wt.DWORD()
        u.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and u.IsWindowVisible(hwnd):
            r = wt.RECT()
            ctypes.windll.dwmapi.DwmGetWindowAttribute(hwnd, 9, ctypes.byref(r), ctypes.sizeof(r))  # DWMWA_EXTENDED_FRAME_BOUNDS
            found.append((hwnd, (r.left, r.top, r.right - r.left, r.bottom - r.top)))
        return True
    u.EnumWindows(ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)(each), 0)
    if not found:
        raise RuntimeError("no window of process %d" % pid)
    return max(found, key=lambda f: f[1][2] * f[1][3])


def window_rect(pid):
    return _main_window(pid)[1]


def move_window(pid, x=0, y=0):
    """Puts a process's window at (x, y) of the screen, size unchanged: a recording or a desktop grab needs it all on screen."""
    import ctypes
    hwnd, _ = _main_window(pid)
    ctypes.windll.user32.SetWindowPos(hwnd, 0, x, y, 0, 0, 0x0001 | 0x0004)  # SWP_NOSIZE | SWP_NOZORDER
    time.sleep(0.5)


class Recorder:
    """The whole desktop recorded with ffmpeg's gdigrab (15 fps, no sound), cut afterwards to a window and compressed for the README."""

    def __init__(self, raw):
        self.raw = str(raw)
        self.p = subprocess.Popen(["ffmpeg", "-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", "15", "-i", "desktop",
                                   "-c:v", "libx264", "-preset", "ultrafast", "-crf", "14", "-pix_fmt", "yuv420p", self.raw], stdin=subprocess.PIPE)
        self.t0 = time.time()

    def stop(self):
        try:
            self.p.communicate(b"q", timeout=60)
        except subprocess.TimeoutExpired:
            self.p.kill()

    def finish(self, out_mp4, rect, start=0.0, speed=1.0, height=720, limit=3_000_000):
        """Cuts the recording to `rect` (physical pixels) from `start` s, plays it `speed` times faster, and compresses it under `limit` bytes."""
        import ctypes
        ctypes.windll.user32.SetProcessDPIAware()
        sw = ctypes.windll.user32.GetSystemMetrics(0)
        vw = int(subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width", "-of", "csv=p=0", self.raw],
                                capture_output=True, text=True, check=True).stdout.strip())
        k = vw / sw  # gdigrab's pixels per physical pixel (1 unless ffmpeg runs DPI-unaware on a scaled screen)
        x, y, w, h = (max(0, int(v * k)) for v in rect)
        w, h = w - w % 2, h - h % 2
        for crf in (28, 31, 34, 37):
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-ss", f"{start:.2f}", "-i", self.raw, "-an",
                            "-vf", f"crop={w}:{h}:{x}:{y},setpts=PTS/{speed},scale=-2:{height}:flags=lanczos,fps=15",
                            "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out_mp4)], check=True)
            if os.path.getsize(out_mp4) <= limit:
                break
        return os.path.getsize(out_mp4)


def poster(mp4, at, png):
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-ss", f"{at:.2f}", "-i", str(mp4), "-frames:v", "1", str(png)], check=True)


def capture_clip_finish(run, projects, out, app_pid, media):
    """The clip of a worktree taken to Finish, on a throwaway clone of the demo checkout (the demo kit itself is not changed): New worktree,
    a shell commits a fix in it, its card goes to Review, Finish merges it into main. `media` is where the MP4 goes."""
    d = run.d
    work = Path(tempfile.mkdtemp(prefix="kelpie-clip-", dir=str(Path(projects[0]["path"]).parent.parent) if WINDOWS else None))
    repo = work / "storefront"
    subprocess.run(["git", "clone", "-q", projects[0]["path"], str(repo)], check=True)
    subprocess.run(["git", "-C", str(repo), "checkout", "-q", "main"], check=True)
    for k in ("user.name", "user.email"):
        subprocess.run(["git", "-C", str(repo), "config", k, "Kelpie demo" if k == "user.name" else "demo@example.com"], check=True)
    for pid in d.js(f"return {S}.state.projects.filter(p => !p.parentId).map(p => p.id)"):
        d.js(f"{S}.setCollapsed(arguments[0], true)", pid)
    pid = d.js(f"return {S}.addProject(arguments[0], {{open: true}})", str(repo))
    d.js(f"{S}.setProjectStatus(arguments[0], 'progress'); {S}.switchProject(arguments[0]); {S}.showRightPanel('files')", pid)
    d.wait(f"return {S}.projectInfo[arguments[0]]?.branch === 'main'", 30, 0.25, pid)
    d.js(f"{S}.approveSetup(arguments[0], '[]')", str(repo))
    # Smaller than the screen, so the taskbar never covers the window's bottom in the recording.
    d.set_window_size(1600, 900)
    move_window(app_pid, 0, 0)
    d.js(f"{S}.setSettings({{sidebarWidth: 260}})")
    rec = Recorder(work / "raw.mp4")
    try:
        time.sleep(1.5)
        start = time.time() - rec.t0
        d.js("window.__kelpie.commands.find(c => c.id === 'worktree.new').run()")
        d.wait("return !!document.querySelector('.worktree-dialog input')", 10)
        d.js("const i = document.querySelector('.worktree-dialog input'); i.focus(); i.select();")
        d.keys("fix/coupon-trim")
        time.sleep(1.2)
        d.js("[...document.querySelectorAll('.worktree-dialog .btn.primary')].find(b => b.textContent.includes('Create worktree')).click()")
        wt = d.wait(f"const p = {S}.state.projects.find(p => p.parentId === arguments[0] && (p.branch || '').includes('coupon-trim')); return p ? p.id : null", 60, 0.25, pid)
        time.sleep(1.5)
        d.js(f"{S}.switchProject(arguments[0])", wt)
        info = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],'shell',{{name:'Coupon fix',where:'tab'}}).then(done)", wt)
        time.sleep(2)
        daemon(d, "session.input", {"handle": info["handle"], "data": PROMPT + "\r"})
        time.sleep(0.5)
        check = 'assert quote([LineItem("kit", 9999)]).shipping_cents == 595'
        fix = (f"Add-Content smoke.py '{check}'; python smoke.py; git commit -qam 'test: no free shipping one cent below the threshold'; git log --oneline -2"
               if WINDOWS else f"echo '{check}' >> smoke.py; python smoke.py; git commit -qam 'test: no free shipping one cent below the threshold'; git log --oneline -2")
        daemon(d, "session.input", {"handle": info["handle"], "data": fix + "\r"})
        time.sleep(3.5)
        d.js(f"{S}.setProjectStatus(arguments[0], 'review'); {S}.setOverlay('board')", wt)
        d.wait("const b = [...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Finish')); return !!b && !b.disabled", 30, 0.25, wt)
        time.sleep(2)
        d.js("[...document.querySelectorAll(`.board-card[data-card='${arguments[0]}'] .review-actions .btn`)].find(b => b.textContent.includes('Finish')).click()", wt)
        d.wait("return !!document.querySelector('.finish-dialog') && !document.querySelector('.finish-dialog').textContent.includes('Reading the worktree')", 30)
        time.sleep(1.5)
        d.js("[...document.querySelectorAll('.finish-dialog button')].find(b => b.textContent.includes('Approve these checks'))?.click()")
        time.sleep(1)
        d.js("const l = [...document.querySelectorAll('.finish-dialog label')].find(l => l.textContent.trim().startsWith('Merge into')); const i = l && l.querySelector('input'); if (i && !i.checked) i.click();")
        d.wait("const b = [...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish'); return !!b && !b.disabled", 30)
        time.sleep(1.5)
        d.js("[...document.querySelectorAll('.finish-dialog .footer .btn.primary')].find(b => b.textContent.trim() === 'Finish').click()")
        d.wait("return document.querySelector('.finish-dialog .finish-note')?.textContent.includes('Finished.')", 120)
        time.sleep(2)
        d.js(f"{S}.setOverlay(null); {S}.switchProject(arguments[0]); {S}.showRightPanel('git')", pid)
        time.sleep(3)
        end = time.time() - rec.t0
    finally:
        rec.stop()
    merged = subprocess.run(["git", "-C", str(repo), "show", "main:smoke.py"], capture_output=True, text=True).stdout
    run.check("the clip's worktree was merged into main", "LineItem(\"kit\", 9999)" in merged, merged[-300:])
    rect = window_rect(app_pid)
    length = end - start
    speed = max(1.0, length / 19.0)
    mp4 = Path(media) / "clip-worktree-finish.mp4"
    size = rec.finish(mp4, rect, start, speed)
    poster(mp4, (length / speed) * 0.82, out / "clip-worktree-finish-poster.png")
    print(f"CLIP worktree-finish {length:.1f} s at x{speed:.2f}, {size} bytes", flush=True)
    d.js(f"{S}.removeProject(arguments[0])", pid)
    d.set_window_size(1920, 1080)
    shutil.rmtree(work, ignore_errors=True)
    return {"clip-worktree-finish.mp4": {"seconds": round(length / speed, 1), "bytes": size, "speed": round(speed, 2), "poster": "clip-worktree-finish-poster.png"}}


def native_app_pid(driver_pid, app):
    command = "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,ExecutablePath | ConvertTo-Json -Compress"
    processes = json.loads(subprocess.check_output(["powershell","-NoProfile","-Command",command], text=True))
    by_id = {p["ProcessId"]:p for p in processes}
    for p in processes:
        exe = p.get("ExecutablePath")
        if not exe or Path(exe).resolve() != Path(app).resolve():
            continue
        ancestor = p["ParentProcessId"]
        seen = set()
        while ancestor in by_id and ancestor not in seen:
            if ancestor == driver_pid:
                return p["ProcessId"]
            seen.add(ancestor)
            ancestor = by_id[ancestor]["ParentProcessId"]
    raise RuntimeError("could not identify this WebDriver's Kelpie window for native capture")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out_dir", type=Path)
    parser.add_argument("demo_root", type=Path)
    parser.add_argument("--native-driver", default=os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver"))
    parser.add_argument("--app", default=None)
    parser.add_argument("--chat", choices=["codex","claude","omp"])
    parser.add_argument("--agents", action="store_true")
    parser.add_argument("--browser", action="store_true", help="Windows: capture ChatGPT web alongside OMP; implies --agents")
    parser.add_argument("--extra", action="store_true", help="after the base tour, one picture per further feature (simulated agents, synthetic transcripts)")
    parser.add_argument("--only", default="", help="with --extra: a comma-separated subset of " + ", ".join(EXTRA_SCENES))
    parser.add_argument("--clip-finish", type=Path, default=None, metavar="DIR",
                        help="Windows: also record the clip of a worktree taken to Finish (ffmpeg on PATH) into DIR, with its poster among the shots")
    args = parser.parse_args()
    if not args.native_driver or not shutil.which("tauri-driver"):
        parser.error("tauri-driver and --native-driver (matching WebView2) are required")
    if args.browser and os.name != "nt":
        parser.error("--browser needs Windows native window capture")
    root = args.demo_root.resolve()
    manifest = root / "projects.json"
    projects = json.loads(manifest.read_text(encoding="utf-8")) if manifest.exists() else seed(root)
    # A kit generated by an older readme_demo.py lacks files the tour opens.
    needed = [root / "plugins" / "kelpie-demo-tools" / "kelpie-plugin.json",
              Path(projects[0]["path"], "runs", "checkout-hardening.json"),
              Path(projects[0]["path"], "plans", "checkout-hardening", "plan.md"),
              root / "pulse-analytics" / "notebooks" / "revenue-summary.ipynb"]
    missing = [str(p) for p in needed if not p.exists()]
    if missing:
        parser.error("the demo kit is older than this tour; generate a new one into an empty folder. Missing: " + ", ".join(missing))
    for p in projects:
        if not Path(p["path"]).resolve().is_relative_to(root):
            parser.error("all demo repositories must be inside demo_root")
        subprocess.run([sys.executable,"smoke.py"], cwd=p["path"], check=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    # Shots land in a staging folder and replace the published ones only when every check
    # passed, so a failed run never overwrites a good gallery.
    stage = Path(tempfile.mkdtemp(prefix="shots-", dir=root))
    # Some settings pages show the data folder: on Windows it sits beside the demo kit, not under the user's profile.
    with tempfile.TemporaryDirectory(prefix="kelpie-readme-state-", dir=str(root.parent) if WINDOWS else None) as data:
        app = args.app or default_app()
        # An empty SSH folder: the Hosts page never lists the user's own keys or known hosts.
        ssh_dir = Path(data) / "ssh-home"
        ssh_dir.mkdir()
        os.environ.update(KELPIE_SSH_DIR=str(ssh_dir), KELPIE_DATA_DIR=data,KELPIE_ENDPOINT=f"kelpie-readme-{os.getpid()}",KELPIE_E2E="1",KELPIE_DAEMON_PATH=str(Path(app).with_name("kelpied.exe" if os.name == "nt" else "kelpied")))
        for key in ("KELPIE_SESSION_ID","KELPIE_SESSION_TOKEN","KELPIE_SESSION_ENDPOINT"):
            os.environ.pop(key, None)
        programs = None
        if args.extra:
            # The agents' homes of this run only: synthetic transcripts for the Sessions tab and the Usage page, and
            # an empty home for Agent setup. The user's own ~/.claude, ~/.codex and agent configs are never read.
            homes = Path(data) / "agent-homes"
            os.environ.update(CLAUDE_CONFIG_DIR=str(homes / "claude"), CODEX_HOME=str(homes / "codex"), KELPIE_AGENT_CONFIG_HOME=str(homes / "home"),
                              KELPIE_FILES_HOME=str(homes / "files"))
            (homes / "home").mkdir(parents=True)
            (homes / "files").mkdir(parents=True)
            seed_transcripts(projects, homes / "claude", homes / "codex")
            from demo_agents import write_programs
            programs = write_programs(str(Path(data) / "demo-agents"))
        with socket.socket() as frontend, socket.socket() as native:
            frontend.bind(("127.0.0.1",0))
            native.bind(("127.0.0.1",0))
            port = frontend.getsockname()[1]
            native_port = native.getsockname()[1]
        os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{port}"
        with open(root / "driver.log", "w", encoding="utf-8") as log:
            driver = subprocess.Popen(["tauri-driver","--port",str(port),"--native-port",str(native_port),"--native-driver",args.native_driver], stdout=log, stderr=subprocess.STDOUT)
            run = None
            try:
              try:
                time.sleep(1)
                run = Run(str(stage), app)
                capture(run, projects, stage, args.chat)
                if args.agents or args.browser:
                    pid = native_app_pid(driver.pid, app) if args.browser else None
                    extra = capture_agents(run, projects, stage, pid)
                    evidence_path = stage / "capture.json"
                    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
                    evidence["screenshots"] = sorted(set(evidence["screenshots"] + extra))
                    evidence["agent_views"] = {"claude": "real read-only answer in its terminal interface", "codex": "real read-only answer with the user's own sign-in", "omp": "real read-only answer", "chat": "real Claude Code answer in chat view", "powershell": "real shell output", "chatgpt": "signed-out website" if args.browser else None}
                    evidence["chat_agent"] = "claude"
                    evidence["checks"] = run.results
                    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8", newline="\n")
                if args.extra:
                    pid = native_app_pid(driver.pid, app) if os.name == "nt" else None
                    only = {s.strip() for s in args.only.split(",") if s.strip()}
                    extra, failed = capture_extras(run, projects, stage, programs, pid, only)
                    evidence_path = stage / "capture.json"
                    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
                    evidence["extra_scenes"] = extra
                    evidence["extra_failed"] = failed
                    evidence["simulated_agent"] = SIMULATED
                    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8", newline="\n")
                    if failed:
                        print("extra scenes that failed: " + ", ".join(failed), flush=True)
                if args.clip_finish:
                    args.clip_finish.mkdir(parents=True, exist_ok=True)
                    clips = capture_clip_finish(run, projects, stage, native_app_pid(driver.pid, app), args.clip_finish)
                    evidence_path = stage / "capture.json"
                    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
                    evidence["clips"] = clips
                    evidence["checks"] = run.results
                    evidence_path.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8", newline="\n")
              finally:
                try:
                    if run:
                        try:
                            daemon(run.d,"daemon.shutdown",{"kill_sessions":True})
                        except RuntimeError as error:
                            # Already gone: nothing is left to stop, and the pictures stand on their own checks.
                            print(f"the tour's daemon could not be asked to stop ({error}); it is no longer connected", flush=True)
                        finally:
                            run.d.quit()
                finally:
                    # The driver goes whatever happened above, so no tauri-driver or
                    # msedgedriver outlives the tour.
                    driver.terminate()
                    try:
                        driver.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        driver.kill()
            except BaseException:
                print(f"the tour stopped; {args.out_dir} is unchanged, this run's shots are in {stage}", flush=True)
                raise
            if not run or any(not ok for _, ok, _ in run.results) or not (stage / "capture.json").exists():
                raise SystemExit(f"demo checks failed; {args.out_dir} is unchanged, this run's shots are in {stage}")
            for shot in stage.iterdir():
                if shot.suffix in (".png", ".json"):
                    shutil.copy2(shot, args.out_dir / shot.name)
            shutil.rmtree(stage, ignore_errors=True)
            print(f"published {len(list(args.out_dir.glob('*.png')))} screenshots to {args.out_dir}")


if __name__ == "__main__":
    main()
