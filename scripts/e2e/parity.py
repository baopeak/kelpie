"""E2E for the Orca-parity features: agent presets and permissions, startup prompts, terminal
CLI and transcript reads, workspace comments, git push/pull/sync/abort, worktree ignored-file
copy/link, review notes, inbox, floating terminal, hibernation, keep-awake, terminal themes,
terminal context menu, rich viewers, word wrap, find in folder, create-worktree search row,
automations, editors list and the settings pages.

    python -X utf8 scripts/e2e/parity.py <screenshot-dir> <scratch-dir> [--ai]

`--ai` also runs one real one-shot agent call (commit message from a diff; a small model call).
"""
import base64
import json
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run, daemon, restart_daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
with_ai = "--ai" in sys.argv
S = "window.__kelpie.store"
repo = os.path.join(scratch, "parity-repo")
remote = os.path.join(scratch, "parity-remote.git")
other = os.path.join(scratch, "parity-other")
# The daemon the app was started with, else the release build of this checkout.
kelpied = os.environ.get("KELPIE_DAEMON_PATH") or os.path.join(REPO, "target", "release", "kelpied.exe" if os.name == "nt" else "kelpied")


def _writable(func, path, _exc):
    os.chmod(path, 0o666)
    func(path)


def rm(path):
    # Windows: a process that just exited may still hold the folder for a moment.
    for attempt in range(10):
        if not os.path.exists(path):
            return
        try:
            shutil.rmtree(path, onexc=_writable)
            return
        except PermissionError:
            if attempt == 9:
                raise
            time.sleep(0.5)


def git(cwd, *args, check=True):
    return subprocess.run(["git", "-C", cwd, *args], capture_output=True, text=True, check=check).stdout


def cli(*args):
    """`kelpied …` against the dev daemon (KELPIE_ENDPOINT comes from the environment)."""
    p = subprocess.run([kelpied, *args], capture_output=True, text=True, timeout=60)
    return p.returncode, p.stdout, p.stderr


def write(path, text, mode="w"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, mode, **({} if "b" in mode else {"encoding": "utf-8"})) as f:
        f.write(text)


def fresh_repos():
    for p in (repo, repo + "-worktrees", remote, other):
        rm(p)
    os.makedirs(repo)
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", remote], check=True)
    git(repo, "init", "-q", "-b", "main")
    for r in (repo,):
        git(r, "config", "user.email", "e2e@kelpie.test")
        git(r, "config", "user.name", "Kelpie e2e")
    write(os.path.join(repo, "README.md"), "parity\n")
    write(os.path.join(repo, ".gitignore"), ".env\nnode_modules/\n")
    write(os.path.join(repo, ".worktreeinclude"), "# private copies\n.env\n")
    write(os.path.join(repo, ".kelpie.yaml"), f"# {time.time()}\nlink:\n  - node_modules\n")
    write(os.path.join(repo, "data.csv"), 'name,count\n"b, quoted",2\na,10\nc,1\n')
    write(os.path.join(repo, "flow.mmd"), "graph TD\n  A[Start] --> B[Done]\n")
    write(os.path.join(repo, "nb.ipynb"), json.dumps({"cells": [
        {"cell_type": "markdown", "source": ["# Notebook title"], "metadata": {}},
        {"cell_type": "code", "source": ["print(1 + 1)"], "execution_count": 1, "metadata": {}, "outputs": [{"output_type": "stream", "name": "stdout", "text": ["2\n"]}]},
    ], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}))
    write(os.path.join(repo, "long.txt"), "word " * 400 + "\n")
    write(os.path.join(repo, "src", "deep", "needle.txt"), "parity-needle\n")
    write(os.path.join(repo, "top-needle.txt"), "parity-needle\n")
    # Smallest valid one-page PDF.
    pdf = b"%PDF-1.1\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj 2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj 3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n"
    write(os.path.join(repo, "doc.pdf"), pdf, "wb")
    git(repo, "add", ".")
    git(repo, "commit", "-q", "-m", "init")
    git(repo, "remote", "add", "origin", remote)
    # Ignored files the worktree should get.
    write(os.path.join(repo, ".env"), "SECRET=1\n")
    write(os.path.join(repo, "node_modules", "pkg", "index.js"), "module.exports = 1\n")


run = Run(shots)
d = run.d


def invoke(cmd, payload):
    r = d.invoke(cmd, payload)
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def js_await(expr, *args):
    """Evaluates an async expression (a promise) in the app and returns its value or {err}."""
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def screen(handle, lines=60):
    return daemon(d, "session.read", {"handle": handle, "lines": lines})["text"]


def wait_text(handle, needle, timeout=30):
    end = time.time() + timeout
    while time.time() < end:
        if needle in screen(handle, 200):
            return True
        time.sleep(0.5)
    return False


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    # Sessions of an earlier run may still sit in the test folders.
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    for a in daemon(d, "automation.list"):
        daemon(d, "automation.delete", {"id": a["id"]})
    fresh_repos()
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, theme: 'dark', terminalTheme: 'default', agentPermissions: 'manual', hibernate: false, keepAwake: 'off', wordWrap: false, floatingTerminal: true }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", repo)
    d.wait(f"return {S}.projectInfo[arguments[0]]?.is_repo || null", 20, 0.25, pid)

    # ---------- agents ----------
    presets = daemon(d, "agent.presets")
    ids = [p["id"] for p in presets]
    run.check("agent.presets lists the preset agents", all(k in ids for k in ["claude", "codex", "gemini", "cursor", "opencode", "copilot", "aider"]), f"{len(ids)} presets; installed: {[p['id'] for p in presets if p['installed']]}")
    run.check("the app loaded the presets", (js_await("window.__kelpie.agents.agentPresets().length") or 0) >= 20)
    yolo = js_await(f"(async () => {{ {S}.setSettings({{ agentPermissions: 'yolo' }}); const a = {S}.profileLaunch('claude').args; {S}.setSettings({{ agentPermissions: 'manual' }}); return [a, {S}.profileLaunch('claude').args]; }})()")
    run.check("yolo adds the skip-permission flag; manual removes it", "--dangerously-skip-permissions" in yolo[0] and "--dangerously-skip-permissions" not in yolo[1], str(yolo))

    # ---------- startup prompt (typed once the shell is ready) ----------
    sh = js_await(f"{S}.newSession(arguments[0], 'shell', {{ prompt: 'echo parity-typed-42' }})", pid)
    run.check("startup command is typed into a new shell", wait_text(sh["handle"], "parity-typed-42", 40))
    daemon(d, "session.input", {"handle": sh["handle"], "data": "echo parity-scroll; " + "; ".join(f"echo row{i}" for i in range(30)) + "\r"})
    time.sleep(2)
    tail = screen(sh["handle"], 400)
    run.check("session.read lines includes scrollback", "parity-typed-42" in tail and "row29" in tail)

    # ---------- kelpied terminal CLI ----------
    code, out, err = cli("terminal", "list")
    run.check("kelpied terminal list", code == 0 and str(sh["handle"]) in out, err.strip())
    code, out, err = cli("terminal", "send", str(sh["handle"]), "echo", "cli-sent-ok", "--enter")
    run.check("kelpied terminal send", code == 0 and wait_text(sh["handle"], "cli-sent-ok", 20), err.strip())
    code, out, err = cli("terminal", "read", str(sh["handle"]), "--lines", "20")
    run.check("kelpied terminal read", code == 0 and "cli-sent-ok" in out, err.strip())
    code, out, err = cli("terminal", "wait", str(sh["handle"]), "--timeout-ms", "20000")
    run.check("kelpied terminal wait returns once idle", code == 0 and out.strip() in ("idle", "waiting"), out.strip() + err.strip())

    # ---------- Ctrl+C reaches programs, also when the app started the daemon ----------
    # The app starts kelpied in a new process group, which Windows marks as ignoring Ctrl+C;
    # sessions must not inherit that.
    restart_daemon(d, kelpied)
    cc = js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    time.sleep(4)
    daemon(d, "session.input", {"handle": cc["handle"], "data": ("ping -n 30 127.0.0.1" if os.name == "nt" else "sleep 30") + "\r"})
    time.sleep(1.5)
    daemon(d, "session.input", {"handle": cc["handle"], "data": "\x03"})
    time.sleep(1)
    daemon(d, "session.input", {"handle": cc["handle"], "data": "echo after-ctrl-c\r"})
    run.check("Ctrl+C stops a running program", bool(wait_text(cc["handle"], "after-ctrl-c", 10)) and "after-ctrl-c" in screen(cc["handle"], 40).split("127.0.0.1")[-1])
    daemon(d, "session.close", {"handle": cc["handle"]})

    # ---------- workspace comment from the CLI ----------
    p = subprocess.run([kelpied, "worktree", "set", "--comment", "reproduced bug; testing fix", "--status", "progress"], cwd=os.path.join(repo, "src"), capture_output=True, text=True, timeout=30)
    run.check("kelpied worktree set (from a subfolder)", p.returncode == 0, p.stderr.strip())
    ok = d.wait(f"return ({S}.metaFor({S}.state.projects.find(p => p.id === arguments[0]))?.comment || null)", 10, 0.25, pid)
    run.check("comment reaches the app", ok == "reproduced bug; testing fix", str(ok))
    run.check("agent status moves the board card", d.js(f"return {S}.state.projects.find(p => p.id === arguments[0]).status", pid) == "progress")
    run.check("sidebar shows the comment", d.wait("return document.querySelector('.project-comment')?.textContent || null", 5) == "reproduced bug; testing fix")
    code, out, _ = cli("worktree", "get", "--path", repo)
    run.check("kelpied worktree get", "reproduced bug" in out and "progress" in out, out.strip())
    run.shot("01-sidebar-comment")

    # ---------- git: publish, sync, pull, diverge, conflict + abort ----------
    st = invoke("git_repo_state", {"root": repo})
    run.check("repo state: remote, no operation", st["has_remote"] and st["operation"] is None, str(st))
    out = invoke("git_remote", {"root": repo, "action": "push"})
    run.check("push publishes the branch with upstream", git(repo, "rev-parse", "--abbrev-ref", "@{u}").strip() == "origin/main", out)
    rm(other)
    subprocess.run(["git", "clone", "-q", remote, other], check=True)
    git(other, "config", "user.email", "o@kelpie.test")
    git(other, "config", "user.name", "Other")
    write(os.path.join(other, "remote.txt"), "from elsewhere\n")
    git(other, "add", ".")
    git(other, "commit", "-q", "-m", "remote change")
    git(other, "push", "-q")
    write(os.path.join(repo, "local.txt"), "local\n")
    git(repo, "add", ".")
    git(repo, "commit", "-q", "-m", "local change")
    git(repo, "fetch", "-q")
    try:
        invoke("git_remote", {"root": repo, "action": "sync"})
        run.check("sync refuses a diverged branch (fast-forward only)", False)
    except RuntimeError as e:
        run.check("sync refuses a diverged branch (fast-forward only)", "fast-forward" in str(e).lower() or "diverg" in str(e).lower(), str(e)[:120])
    # Merge conflict → operation shown → abort.
    write(os.path.join(other, "README.md"), "theirs\n")
    git(other, "commit", "-qam", "theirs")
    git(other, "push", "-q")
    write(os.path.join(repo, "README.md"), "ours\n")
    git(repo, "commit", "-qam", "ours")
    git(repo, "fetch", "-q")
    git(repo, "merge", "origin/main", check=False)
    st = invoke("git_repo_state", {"root": repo})
    run.check("merge in progress is detected", st["operation"] == "merge", str(st))
    d.js(f"{S}.showRightPanel('git')")
    run.check("Changes panel shows the merge banner", d.wait("return document.querySelector('.git-note.warn')?.textContent.includes('merge in progress') || null", 10))
    run.shot("02-merge-banner")
    invoke("git_abort", {"root": repo})
    run.check("abort merge", invoke("git_repo_state", {"root": repo})["operation"] is None)
    git(repo, "pull", "-q", "--no-rebase", "-X", "ours", "--no-edit")
    out = invoke("git_remote", {"root": repo, "action": "sync"})
    run.check("sync pushes after the merge", git(repo, "status", "-sb").splitlines()[0].find("ahead") < 0, git(repo, "status", "-sb").splitlines()[0])
    refs = invoke("git_refs", {"root": repo})
    run.check("refs for the start-from picker", any(r["name"] == "origin/main" and r["kind"] == "remote" for r in refs) and any(r["name"] == "main" for r in refs))
    write(os.path.join(repo, "staged.txt"), "staged\n")
    git(repo, "add", "staged.txt")
    ctx = invoke("git_context", {"root": repo, "kind": "staged", "max": 5000})
    run.check("AI context: staged diff", "staged.txt" in ctx)
    git(repo, "commit", "-qm", "staged")
    pr = invoke("gh_pr_status", {"root": repo}) if shutil.which("gh") else None
    run.check("PR status is empty for a non-GitHub remote", pr is None, str(pr))

    # ---------- worktree: .worktreeinclude copy + .kelpie.yaml link ----------
    res = js_await(f"Promise.resolve(window.__kelpie.worktrees).then(async m => {{ const r = {S}.state.projects.find(p => p.id === arguments[0]); const cfg = await m.configAtRef(r, 'main'); const w = await m.createWorktree(r, {{ branch: 'wt/parity', base: 'main', path: arguments[1], config: cfg }}); return w.id; }})", pid, repo + "-worktrees\\parity" if os.name == "nt" else repo + "-worktrees/parity")
    wt = repo + "-worktrees" + os.sep + "parity"
    run.check("worktree created", isinstance(res, str), str(res))
    run.check(".worktreeinclude copied .env", os.path.isfile(os.path.join(wt, ".env")))
    nm = os.path.join(wt, "node_modules")
    run.check(".kelpie.yaml link shares node_modules", os.path.isfile(os.path.join(nm, "pkg", "index.js")) and (os.path.islink(nm) or (os.name == "nt" and bool(os.stat(nm, follow_symlinks=False).st_file_attributes & 0x400))))
    run.check("worktree creation lands in the inbox", d.js(f"return {S}.state.inbox.some(i => i.kind === 'worktree' && i.title.includes('wt/parity'))"))
    removed = js_await(f"Promise.resolve(window.__kelpie.worktrees).then(m => m.removeWorktree({S}.state.projects.find(p => p.id === arguments[0]), {{ force: true }}))", res)
    run.check("worktree with a linked folder removes cleanly", not os.path.exists(wt), str(removed))
    run.check("removing it keeps the shared folder's files", os.path.isfile(os.path.join(repo, "node_modules", "pkg", "index.js")))
    git(repo, "branch", "parity-tmp")
    invoke("git_branch_delete", {"root": repo, "branch": "parity-tmp", "force": False})
    run.check("branch delete", "parity-tmp" not in git(repo, "branch"))

    # ---------- palette: create worktree row ----------
    d.js(f"{S}.switchProject(arguments[0])", pid)
    d.js(f"{S}.setOverlay('jump')")
    time.sleep(0.4)
    d.keys("zz-new-task")
    row = d.wait("return [...document.querySelectorAll('.palette .item .label')].map(e => e.textContent).find(t => t.includes('Create worktree')) || null", 5)
    run.check("search offers Create worktree for an unknown name", row and "zz-new-task" in row, str(row))
    d.keys("escape")
    time.sleep(0.3)

    # ---------- review notes ----------
    write(os.path.join(repo, "README.md"), "ours\nsecond line\n")
    d.js(f"{S}.openFile(arguments[0], 'README.md', {{ diff: true }})", pid)
    d.wait("return document.querySelector('.cm-note-gutter') || null", 15)
    d.js(f"{S}.addNote(arguments[0], {{ path: 'README.md', line: 2, side: 'new', text: 'second line', note: 'Say what this line is for' }})", pid)
    run.check("notes bar appears", d.wait("return document.querySelector('.notes-bar')?.textContent.includes('1 review note') || null", 5))
    run.check("note marker in the gutter", d.wait("return document.querySelector('.cm-note-mark.has') || null", 5))
    prompt = js_await(f"Promise.resolve(window.__kelpie.diffView).then(m => m.notesPrompt({S}.notesFor(arguments[0])))", pid)
    run.check("notes prompt names file, line and note", "README.md" in prompt and "line 2" in prompt and "Say what this line is for" in prompt, prompt[:160])
    d.js("document.querySelector('.cm-note-gutter .cm-gutterElement:nth-child(3)')?.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, clientX: 300, clientY: 200 }))")
    run.check("gutter click opens the note editor", d.wait("return document.querySelector('.note-draft textarea') || null", 5))
    run.shot("03-review-notes")
    d.js("document.querySelector('.note-draft .btn')?.click()")
    d.js("document.querySelector('.notes-bar .btn.primary').dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: 600, clientY: 200 }))")
    menu = d.wait("return [...document.querySelectorAll('.ctx-menu .ctx-item')].map(e => e.textContent) || null", 5)
    run.check("Send to agent lists agents to start", any("New Claude Code session" in m for m in menu or []), str(menu)[:200])
    d.keys("escape")
    d.js(f"{S}.removeNotes(arguments[0], 'all')", pid)

    # ---------- viewers ----------
    def view(path, css, label, timeout=20):
        d.js(f"{S}.openFile(arguments[0], arguments[1])", pid, path)
        ok = d.wait(f"return document.querySelector(arguments[0]) || null", timeout, 0.25, css)
        run.check(label, ok)
        return ok

    view("data.csv", ".data-table tbody tr", "CSV opens as a table")
    run.check("CSV parses quoted fields", d.js("return [...document.querySelectorAll('.data-table tbody td')].some(td => td.textContent === 'b, quoted')"))
    d.js("document.querySelectorAll('.data-table th')[1].click()")
    run.check("CSV sorts numerically", d.js("return [...document.querySelectorAll('.data-table tbody tr')].map(r => r.cells[1].textContent).join(',')") == "1,2,10")
    run.shot("04-csv")
    view("nb.ipynb", ".nb-cell .nb-out", "notebook renders cells and outputs")
    run.check("notebook markdown cell", d.js("return document.querySelector('.nb-md h1')?.textContent") == "Notebook title")
    view("flow.mmd", ".diagram-canvas svg", "Mermaid diagram renders", 40)
    run.shot("05-mermaid")
    view("doc.pdf", "iframe.pdf-frame", "PDF opens in a viewer")
    view("long.txt", ".cm-content", "text opens in the editor")
    d.js(f"{S}.setSettings({{ wordWrap: true }})")
    run.check("word wrap applies to the editor", d.wait("return document.querySelector('.cm-lineWrapping') || null", 5))
    d.js(f"{S}.setSettings({{ wordWrap: false }})")
    run.check("word wrap turns off", d.wait("return !document.querySelector('.cm-lineWrapping') || null", 5))

    # ---------- find in folder ----------
    hits = invoke("search_text", {"root": repo, "query": "parity-needle", "regex": False, "caseSensitive": False, "folder": "src"})
    run.check("search limited to a folder", [h["path"] for h in hits["hits"]] == ["src/deep/needle.txt"], str([h["path"] for h in hits["hits"]]))
    hits = invoke("search_text", {"root": repo, "query": "parity-needle", "regex": False, "caseSensitive": False})
    run.check("search without a folder finds both", len(hits["hits"]) == 2)

    # ---------- terminal theme, context menu ----------
    d.js(f"{S}.revealSession(arguments[0])", sh["id"])
    time.sleep(0.8)
    d.js(f"{S}.setSettings({{ terminalTheme: 'dracula' }}); for (const v of window.__kelpie.views()) v.applySettings({S}.state.settings);")
    bg = d.wait("return getComputedStyle(document.querySelector('.pane .term-wrap')).backgroundColor || null", 5)
    run.check("terminal theme colours the terminal", bg == "rgb(40, 42, 54)", str(bg))
    run.shot("06-dracula")
    d.js(f"{S}.setSettings({{ terminalTheme: 'default' }}); for (const v of window.__kelpie.views()) v.applySettings({S}.state.settings);")
    d.js("document.querySelector('.pane .term-wrap').dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 500, clientY: 300 }))")
    items = d.wait("return [...document.querySelectorAll('.ctx-menu .ctx-item')].map(e => e.textContent) || null", 5)
    run.check("terminal context menu", items and any("Copy Context" in i for i in items) and any("Paste" in i for i in items), str(items)[:200])
    run.shot("07-terminal-menu")
    d.js("[...document.querySelectorAll('.ctx-menu .ctx-item')].find(e => e.textContent.includes('Copy Context'))?.click()")
    time.sleep(0.6)
    clip = js_await("window.__TAURI_INTERNALS__.invoke('plugin:clipboard-manager|read_text')")
    run.check("Copy Context copies the transcript", isinstance(clip, str) and "parity-typed-42" in clip, str(clip)[:80])

    # ---------- inbox ----------
    d.js(f"{S}.addInbox({{ kind: 'finished', title: 'Parity agent · parity-repo', body: 'Finished', sessionId: arguments[0], projectId: arguments[1] }})", sh["id"], pid)
    run.check("bell shows unread count", d.wait("return document.querySelector('.inbox-anchor .badge')?.textContent || null", 5))
    d.js("document.querySelector('.inbox-anchor').click()")
    run.check("inbox opens", d.wait("return document.querySelector('.inbox .inbox-item.unread') || null", 5))
    run.shot("08-inbox")
    d.js("document.querySelector('.inbox .inbox-item.unread').click()")
    run.check("opening an item marks it read and closes", d.wait(f"return (!document.querySelector('.inbox') && {S}.state.inbox.find(i => i.title.startsWith('Parity agent'))?.read) || null", 5))

    # ---------- floating terminal ----------
    d.keys("ctrl+shift+`")
    run.check("floating terminal opens with a shell", d.wait("return document.querySelector('.floating-term .xterm') || null", 20))
    fl = d.js(f"return {S}.state.floating.active")
    fh = d.js(f"return {S}.sessions[arguments[0]].handle", fl)
    run.check("floating shell starts in the home folder", os.path.normcase(d.js(f"return {S}.sessions[arguments[0]].cwd", fl)) == os.path.normcase(os.path.expanduser("~")))
    run.shot("09-floating")
    d.keys("ctrl+shift+`")
    run.check("floating terminal hides (session keeps running)", d.wait("return !document.querySelector('.floating-term') || null", 5) and any(s["handle"] == fh and s["alive"] for s in daemon(d, "session.list")))
    run.check("floating sessions are not listed as orphans", not d.js("return !!document.querySelector('.other-label')"))
    d.js(f"{S}.closeFloating(arguments[0])", fl)

    # ---------- hibernation: stop, then resume when shown ----------
    sh2 = js_await(f"{S}.newSession(arguments[0], 'shell', {{ where: 'tab' }})", pid)
    time.sleep(3)
    js_await(f"{S}.hibernateSessions([arguments[0]])", sh2["id"])
    run.check("hibernated session stops", d.wait(f"return ({S}.sessions[arguments[0]] && !{S}.sessions[arguments[0]].alive) || null", 10, 0.25, sh2["id"]))
    d.js(f"{S}.revealSession(arguments[0])", sh["id"])
    time.sleep(0.5)
    d.js(f"{S}.revealSession(arguments[0])", sh2["id"])
    run.check("showing it resumes it", d.wait(f"return {S}.sessions[arguments[0]]?.alive || null", 15, 0.25, sh2["id"]))
    run.check("hibernated list is cleared", d.js(f"return !{S}.state.hibernated.includes(arguments[0])", sh2["id"]))

    # ---------- keep awake ----------
    run.check("keep awake: always holds", daemon(d, "daemon.set_awake", {"mode": "always"})["held"] is True)
    run.check("keep awake: agents with none working releases", daemon(d, "daemon.set_awake", {"mode": "agents"})["held"] is False)
    daemon(d, "daemon.set_awake", {"mode": "off"})
    d.js("document.querySelector('.statusbar [title^=\"Keep computer awake\"]').click()")
    run.check("status bar coffee cycles the mode", d.js(f"return {S}.state.settings.keepAwake") == "agents")
    d.js(f"{S}.setSettings({{ keepAwake: 'off' }})")

    # ---------- automations ----------
    a = daemon(d, "automation.save", {"id": "", "name": "Parity automation", "enabled": False, "kind": "shell", "prompt": "echo automation-ran", "cwd": repo, "project_id": pid, "args": [], "every": "daily", "time": "09:00", "weekday": 1})
    run.check("automation saved with a next run", a["next_run"] > time.time() * 1000, str(a.get("next_run")))
    # A shell automation runs only once its command is approved in Kelpie.
    listed = next(x for x in daemon(d, "automation.list") if x["id"] == a["id"])
    daemon(d, "automation.approve", {"id": a["id"], "hash": listed["command_hash"]})
    # The reply says what the run did; `session` is the session it started.
    info = daemon(d, "automation.run", {"id": a["id"]})["session"]
    run.check("Run now starts its session with the prompt", wait_text(info["handle"], "automation-ran", 40))
    lst = daemon(d, "automation.list")
    run.check("run is recorded", any(x["id"] == a["id"] and x["last_run"] > 0 and x["last_session"] == info["id"] for x in lst))
    run.check("run lands in the inbox", d.wait(f"return {S}.state.inbox.some(i => i.kind === 'automation' && i.title.includes('Parity automation')) || null", 10))
    d.js(f"{S}.openSettings('automations')")
    run.check("Automations settings list it", d.wait("return [...document.querySelectorAll('.automation-name')].some(e => e.textContent === 'Parity automation') || null", 10))
    run.shot("10-automations")
    daemon(d, "automation.delete", {"id": a["id"]})
    run.check("automation deleted", not any(x["id"] == a["id"] for x in daemon(d, "automation.list")))
    d.js(f"{S}.openSettings('agents')")
    run.check("Agents settings show permissions and the agent list", d.wait("return (document.querySelector('[data-setting=agent-permissions]') && document.querySelectorAll('.agent-item').length > 5) || null", 10))
    run.shot("11-agents-settings")
    d.keys("escape")

    # ---------- editors ----------
    eds = invoke("editors_list", {})
    run.check("installed editors are listed", isinstance(eds, list), str(eds))

    # ---------- one-shot AI (optional, real model call) ----------
    if with_ai:
        text = js_await("window.__kelpie ? Promise.resolve(window.__kelpie.ipc).then(m => m.agentRun('claude', 'Reply with exactly: parity-ok', arguments[0])) : null", repo)
        run.check("one-shot agent run answers", isinstance(text, str) and "parity-ok" in text, str(text)[:120])

    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
