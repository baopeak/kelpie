"""Capture Kelpie's native desktop using eight isolated demo repositories.

    python -X utf8 scripts/e2e/readme_shots.py <out-dir> <demo-root> --native-driver <msedgedriver.exe>

Creates the fixtures with readme_demo.seed when projects.json is absent. Requires
built kelpie/kelpied binaries, tauri-driver and a WebView2-matching msedgedriver.
Optional --chat codex|claude|omp makes one real model call using your CLI sign-in.
--agents captures OMP (one read-only model call), Claude, Codex onboarding and PowerShell.
--browser also captures ChatGPT web beside OMP (Windows native capture, no ChatGPT sign-in).
Without these options, no agent is launched; no conversation is fabricated.
Personal projects, sessions, histories and default daemon are never modified.
"""
import argparse
import json
import os
from pathlib import Path
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
    d.set_window_size(1600, 1000)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    d.js(f"""{S}.setSettings({{ theme: 'dark', density: 'comfortable', uiScale: 1,
        usageMeters: false, notifications: false, shortcutHints: false,
        sidebarWidth: 280, rightPanelWidth: 360, fontSize: 13,
        quickCommands: [{{id:'demo-checks',label:'Run demo checks',text:'python smoke.py',action:'terminal',enter:true,projectId:null}},
        {{id:'demo-review',label:'Review checkout',text:'Review src/checkout.py for validation and rounding.',action:'agent',enter:false,projectId:null}}] }})""")
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
    shots = []

    def shot(name, wait=0.8):
        time.sleep(wait)
        d.screenshot(str(out / (name + ".png")))
        shots.append(name)
        print("SHOT " + name, flush=True)

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
    before = Path(projects[0]["path"], "specs/release/tasks.md").read_bytes()
    box = d.js("const b=document.querySelector('.plan-task .plan-box'); if(b){b.click();return true} return false")
    time.sleep(1)
    after = Path(projects[0]["path"], "specs/release/tasks.md").read_bytes()
    run.check("Plan checkbox writes the actual demo spec", box and before != after)
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
    (out / "capture.json").write_text(json.dumps({"application":"Kelpie", "native":True,"viewport":[1600,1000],"projects":[p["name"] for p in projects],"screenshots":shots,"chat_agent":chat,"scenario_data":"Local demo repositories, board comments, review note and inbox samples; disabled automation examples. Shell output and optional chat are real.","checks":run.results}, indent=2), encoding="utf-8")
    return shots


def capture_agents(run, projects, out, codex_home, browser_pid=None):
    """Real installed CLIs; Codex uses a clean onboarding home, never copied credentials."""
    d = run.d
    atlas = d.js(f"return {S}.state.projects.find(p=>p.path===arguments[0]).id", projects[0]["path"])
    names = []

    def shot(name):
        time.sleep(1)
        d.screenshot(str(out / (name + ".png")))
        names.append(name)
        print("SHOT " + name, flush=True)

    def launch(kind, name, where="tab", **opts):
        info = d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],arguments[1],arguments[2]).then(done)", atlas, kind, {"name": name, "where": where, **opts})
        if not info:
            raise RuntimeError("could not launch " + name)
        time.sleep(3)
        return info

    def send(info, text):
        daemon(d, "session.input", {"handle":info["handle"], "data":text + "\r"})

    def reveal(info):
        d.js(f"{S}.revealSession(arguments[0]); {S}.setRightPanel(null)", info["id"])

    def full(info, name):
        reveal(info)
        loc = d.js(f"return {S}.paneLocation(arguments[0])", info["id"])
        panes = d.js(f"return window.__kelpie.layout.allPanes({S}.activeTab(arguments[0]).root).length", atlas)
        if panes > 1:
            d.js(f"{S}.toggleZoom(arguments[0],arguments[1])", loc["tabId"], loc["paneId"])
        shot(name)
        if panes > 1:
            d.js(f"{S}.toggleZoom(arguments[0],arguments[1])", loc["tabId"], loc["paneId"])

    d.js(f"{S}.switchProject(arguments[0])", atlas)
    prompt = "Read README.md and src/checkout.py. Explain the demo purpose and two checkout boundary cases in at most 8 lines. Do not edit files."
    # OMP accepts an initial message as argv: no paste/Enter timing dependency.
    omp = launch("omp", "OMP · checkout review", args=[prompt])
    reveal(omp)
    end = time.monotonic() + 180
    while time.monotonic() < end:
        text = daemon(d, "session.read", {"handle":omp["handle"],"lines":100})["text"]
        info = next(s for s in daemon(d,"session.list") if s["id"] == omp["id"])
        if info.get("failed") or not info["alive"]:
            raise RuntimeError("OMP task failed: " + text[-2000:])
        # Observe completion, not particular words chosen by the model.
        if "Read (" in text and info["status"] != "working" and time.time() * 1000 - info["last_activity"] > 3000:
            break
        time.sleep(1)
    else:
        raise RuntimeError("OMP did not finish the read-only demo task: " + text[-2000:])
    full(omp, "omp")
    grid = d.js(f"return {S}.paneLocation(arguments[0]).tabId", omp["id"])
    shell_args = ["pwsh.exe", "-NoLogo"] if shutil.which("pwsh") else []
    pwsh = launch("shell", "PowerShell · demo checks", "right", args=shell_args)
    send(pwsh, "function prompt { 'pwsh> ' }; Clear-Host; $PSVersionTable | Format-Table -AutoSize; python smoke.py; python src/checkout.py")
    time.sleep(2)
    full(pwsh, "powershell")
    codex_home.mkdir(parents=True, exist_ok=True)
    codex = launch("codex", "Codex · sign-in", "down", env={"CODEX_HOME":str(codex_home)})
    end = time.monotonic() + 45
    while time.monotonic() < end:
        text = daemon(d, "session.read", {"handle":codex["handle"],"lines":100})["text"]
        if "Sign in with ChatGPT" in text:
            break
        time.sleep(0.5)
    else:
        raise RuntimeError("Codex did not reach its demo sign-in screen: " + text[-2000:])
    full(codex, "codex")
    git = launch("shell", "Git · branches & worktrees", "right", args=shell_args)
    send(git, "function prompt { 'pwsh> ' }; Clear-Host; git log --oneline --graph --all -8; git worktree list; git diff --stat")
    d.js(f"{S}.arrangeTab(arguments[0],arguments[1],{{mode:'rows',counts:[2,2]}}); {S}.renameTab(arguments[0],arguments[1],'Agents · checks · review')", atlas, grid)
    for pid in d.js(f"return {S}.state.projects.filter(p=>p.id!==arguments[0]).map(p=>p.id)", atlas):
        d.js(f"{S}.setCollapsed(arguments[0],true)", pid)
    d.js(f"{S}.setSettings({{sidebarWidth:260}})")
    shot("workspace")
    claude = launch("claude", "Claude Code · demo workspace")
    time.sleep(4)
    text = daemon(d, "session.read", {"handle":claude["handle"],"lines":80})["text"]
    if "Yes, I trust this folder" in text:
        # This is the generated fixture folder, not an unreviewed repository.
        daemon(d, "session.input", {"handle":claude["handle"], "data":"\x1b[B\r"})
        time.sleep(4)
    full(claude, "claude")

    if browser_pid:
        d.js("window.__kelpie.commands.find(c=>c.id==='browser.open').run()")
        d.wait("return !!document.querySelector('.browser-address input')", 10)
        d.js("const i=document.querySelector('.browser-address input'); i.value='https://chatgpt.com'; i.dispatchEvent(new InputEvent('input',{bubbles:true})); i.form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))")
        d.wait("return Object.values(window.__kelpie.browser.browserState).some(b=>b.title?.includes('ChatGPT'))", 45)
        tab = d.js(f"return {S}.activeTab(arguments[0]).id", atlas)
        loc = d.js(f"return {S}.paneLocation(arguments[0])", omp["id"])
        d.js(f"{S}.movePaneToTab(arguments[0],arguments[1],arguments[2],arguments[3]); {S}.selectTab(arguments[0],arguments[3])", atlas, loc["tabId"], loc["paneId"], tab)
        time.sleep(10)  # The remote page's title arrives before its client-rendered UI.
        # WebDriver omits child webviews: PrintWindow captures the real native window.
        subprocess.run(["powershell","-NoProfile","-ExecutionPolicy","Bypass","-File",str(Path(__file__).resolve().parents[1] / "capture-window.ps1"),"-ProcessId",str(browser_pid),"-Out",str(out / "chatgpt.png")], check=True)
        names.append("chatgpt")
        loc = d.js(f"return {S}.paneLocation(arguments[0])", omp["id"])
        d.js(f"{S}.movePaneToTab(arguments[0],arguments[1],arguments[2],arguments[3])", atlas, loc["tabId"], loc["paneId"], grid)
    d.js(f"{S}.selectTab(arguments[0],arguments[1]); {S}.arrangeTab(arguments[0],arguments[1],{{mode:'rows',counts:[2,2]}})", atlas, grid)
    for theme, language, name in [("light","en","light"),("dark","vi","vietnamese")]:
        d.js(f"{S}.setSettings({{theme:arguments[0],language:arguments[1]}}); for(const v of window.__kelpie.views()) v.applySettings({S}.state.settings)", theme, language)
        shot(name)
    d.js(f"{S}.setSettings({{language:'en'}})")
    screens = {kind:daemon(d,"session.read",{"handle":info["handle"],"lines":100})["text"]
               for kind, info in [("omp",omp),("powershell",pwsh),("codex",codex),("claude",claude)]}
    run.check("real OMP, PowerShell, Codex onboarding and Claude interfaces captured",
              "Read (" in screens["omp"] and "PASS atlas-commerce" in screens["powershell"]
              and "Sign in with ChatGPT" in screens["codex"] and "Claude Code" in screens["claude"])
    return names


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
    for p in projects:
        if not Path(p["path"]).resolve().is_relative_to(root):
            parser.error("all demo repositories must be inside demo_root")
        subprocess.run([sys.executable,"smoke.py"], cwd=p["path"], check=True)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="kelpie-readme-state-") as data:
        app = args.app or default_app()
        os.environ.update(KELPIE_DATA_DIR=data,KELPIE_ENDPOINT=f"kelpie-readme-{os.getpid()}",KELPIE_E2E="1",KELPIE_DAEMON_PATH=str(Path(app).with_name("kelpied.exe" if os.name == "nt" else "kelpied")))
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
                time.sleep(1)
                run = Run(str(args.out_dir), app)
                capture(run, projects, args.out_dir, args.chat)
                if args.agents or args.browser:
                    pid = native_app_pid(driver.pid, app) if args.browser else None
                    extra = capture_agents(run, projects, args.out_dir, root / "codex-demo-home", pid)
                    evidence_path = args.out_dir / "capture.json"
                    evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
                    evidence["screenshots"] = sorted(set(evidence["screenshots"] + extra))
                    evidence["agent_views"] = {"omp":"real read-only answer", "codex":"clean sign-in screen; no credentials copied", "claude":"real CLI, no model turn", "powershell":"real shell output", "chatgpt":"signed-out website" if args.browser else None}
                    evidence["checks"] = run.results
                    evidence_path.write_text(json.dumps(evidence, indent=2), encoding="utf-8")
            finally:
                if run:
                    try:
                        daemon(run.d,"daemon.shutdown",{"kill_sessions":True})
                    finally:
                        run.d.quit()
                driver.terminate()
                driver.wait(timeout=15)
            if any(not ok for _, ok, _ in run.results):
                raise SystemExit("demo checks failed; inspect capture.json")


if __name__ == "__main__":
    main()
