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
        daemon(d, "session.input", {"handle": info["handle"], "data": "function prompt { 'PS> ' }; Clear-Host\r"})
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
    send(background[0], 'Read-Host "Run the next demo check (y/n)"')
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
    daemon(d, "session.input", {"handle": fh, "data": "function prompt { 'PS> ' }; Set-Location '" + projects[0]["path"] + "'; Clear-Host\r"})
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
    (out / "capture.json").write_text(json.dumps({
        "application": "Kelpie", "app_version": version, "platform": platform.platform(terse=True),
        "native": True, "viewport": [1920, 1080],
        "projects": [p["name"] for p in projects], "worktrees": ["feature/coupon-validation", "fix/shipping-threshold"],
        "screenshots": shots, "chat_agent": chat,
        "scenario_data": "Local demo repositories with synthetic data; seeded board stages, worktree comments, a review note, "
                         "labelled inbox samples, disabled automations, a run waiting for approval, documentation-address SSH hosts "
                         "that are never connected to, and a demo plugin that stays disabled. Shell output, the catalog server's "
                         "port and the optional chat answer are real.",
        "checks": run.results}, indent=2) + "\n", encoding="utf-8", newline="\n")
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
    with tempfile.TemporaryDirectory(prefix="kelpie-readme-state-") as data:
        app = args.app or default_app()
        # An empty SSH folder: the Hosts page never lists the user's own keys or known hosts.
        ssh_dir = Path(data) / "ssh-home"
        ssh_dir.mkdir()
        os.environ.update(KELPIE_SSH_DIR=str(ssh_dir), KELPIE_DATA_DIR=data,KELPIE_ENDPOINT=f"kelpie-readme-{os.getpid()}",KELPIE_E2E="1",KELPIE_DAEMON_PATH=str(Path(app).with_name("kelpied.exe" if os.name == "nt" else "kelpied")))
        for key in ("KELPIE_SESSION_ID","KELPIE_SESSION_TOKEN","KELPIE_SESSION_ENDPOINT"):
            os.environ.pop(key, None)
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
              finally:
                try:
                    if run:
                        try:
                            daemon(run.d,"daemon.shutdown",{"kill_sessions":True})
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
