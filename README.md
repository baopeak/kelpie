<p align="center"><img src="app/src/assets/brand/kip-peek.svg" alt="Kip, the Kelpie mascot, peeking out of a terminal" width="120"></p>

<h1 align="center">Kelpie</h1>

<p align="center"><strong>Every coding agent you use, side by side, in one workbench that never loses track.</strong></p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/baopeak/kelpie?label=download"></a>
  <img alt="Windows, macOS and Linux" src="https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-4c7a8a">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-4c7a8a"></a>
</p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><strong>Download Kelpie</strong></a> ·
  <a href="#see-it-in-action">See it in action</a> ·
  <a href="#try-the-demo-kit">Try the demo kit</a> ·
  <a href="docs/user-guide.md">User guide</a> ·
  <a href="README.vi.md">Tiếng Việt</a>
</p>

![Four agents at work in one Kelpie tab: Claude Code, Codex and OMP in their own terminals and Claude Code in chat view, all reviewing the same checkout, beside eight demo projects](docs/screenshots/workspace.png)

You start one agent in a terminal. Then a second agent takes another task, a test needs its own shell, a bug needs another branch, and the browser hides somewhere behind everything else. Soon the hard part is no longer starting the work. The hard part is remembering where it went: which agent is waiting for an answer, which checkout belongs to which task, and what is actually ready to review.

**Kelpie is a desktop workbench for that moment.** It runs Claude Code, Codex, OMP and twenty more agent CLIs next to your shells, across all of your projects and worktrees. It tells you which session needs you, helps you review what an agent changed, and merges the finished work safely. Its sessions live in a separate background process, so **updating Kelpie never interrupts an agent that is working**.

## Why people use it

- **You keep your own tools.** Kelpie launches the agent CLIs you already installed, with your own sign-ins, subscriptions and permission rules. It does not bundle a model, and it does not need an account.
- **You see what needs you.** A dashboard sorts every session into *Needs you*, *Working*, *Done* and *Failed*. You can answer a waiting prompt without opening its tab, from the bell in the status bar or from your phone.
- **Parallel work stays separate.** Each task can get its own Git worktree, its own sessions and a short comment that says why it matters. A board shows every task moving from *Todo* to *Done*.
- **Review is part of the flow.** You read the diff, leave a note on the line that matters, and send all notes back to an agent in one request. When the work is ready, **Finish** commits, merges, runs your approved checks and cleans up.
- **Updates do not stop the work.** The `kelpied` daemon owns every process. When Kelpie installs an update, only the window restarts, and each agent is still running when it comes back. (The one exception is described under [Install](docs/user-guide.md#install): the first update from an early 0.1 release stops sessions once.)

## What is inside

| | |
|---|---|
| **Multi-agent workspace** | Tabs, splits, grid presets up to 3×3 or any custom grid, saved layouts, broadcast input, a floating terminal and one search box for sessions, files, commands and settings. |
| **Terminal or chat, your choice** | Supported agents can switch between their own terminal interface and Kelpie's chat view in the same conversation, with tool cards, approvals, model controls and pictures. |
| **Attention you can trust** | The dashboard, quick answers, desktop and phone notifications, and ntfy, Telegram or webhook messages tell you when a session waits, finishes or fails. |
| **Worktrees and a board** | New worktrees come with optional setup commands, copied `.env` files and shared folders from `.kelpie.yaml`, and every one of them appears on the board. |
| **Review and Finish** | Diff notes, review by a second agent from another vendor, turn-by-turn undo, pull requests through `gh`, GitLab merge requests and a guarded merge with checks. |
| **Orchestration** | Runs split a job into agent tasks with dependencies and approvals. *Try with several agents* gives one prompt to up to five agents and compares their results. Agents can open **sub-sessions** that you can see, steer and close. |
| **Plans in your repository** | The Plan tab reads Markdown checklists from `plans/`, `specs/`, `.kiro/specs/` and `docs/plans/`, lets you tick them, and starts an agent on any task. |
| **Files, browser and processes** | An editor with Markdown, Mermaid, CSV, PDF and notebook viewers, a browser pane with a design mode, and a Processes panel that shows each session's programs and ports. |
| **Remote work** | SSH projects, a Termius-style hosts page with keys, port forwarding and file transfer, and agent panes on any server, kept alive with `tmux`. |
| **Your phone** | An optional, end-to-end encrypted phone app lists your sessions, buzzes when one needs you and lets you answer it. |
| **Make it yours** | Agent setup for MCP servers and instruction files, plugins, quick commands, automations, shortcuts, themes, offline voice dictation, settings export, and English or Vietnamese. |

## See it in action

The tour below follows one small checkout change through a working day. The projects and planning data are samples. The shell output and every agent answer are real executions with real sign-ins, and [capture.json](docs/screenshots/capture.json) records which parts are sample data.

### Give every task a home

You open the project, start an agent and put a shell beside it. The explanation, the test output and the branch are now visible together. When a task grows, you split the pane again or arrange the tab into a grid. When one detail needs your full attention, you maximize that pane without stopping its neighbours.

In the picture at the top, four agents work on the same checkout at once, in three different agent CLIs: Claude Code, Codex and OMP in their own terminal interfaces, and a second Claude Code session in Kelpie's chat view. Kelpie launches each installed CLI exactly as it is, and a plain PowerShell or Bash session belongs in the workspace just as naturally as an agent.

<details>
<summary><strong>Meet the sessions: Claude Code, Codex, OMP, Claude Code chat and PowerShell</strong></summary>

**Claude Code** keeps its native terminal interface and names the riskiest checkout boundaries.

![Claude Code's real answer in its own terminal interface inside Kelpie](docs/screenshots/claude.png)

**Codex** reads the smoke checks and suggests a missing assertion for the free-shipping boundary.

![Codex CLI's real answer inside Kelpie](docs/screenshots/codex.png)

**OMP** reads the checkout code and explains its shipping and rounding rules.

![OMP's real terminal answer and file-reading activity inside Kelpie](docs/screenshots/omp.png)

**Claude Code in chat view** answers the same kind of question as a conversation, with its tool rows and model controls; the [chat section](#read-the-result-in-the-form-that-makes-sense) shows it.

**PowerShell 7** runs the sample's checks and prints a real checkout quote.

![PowerShell 7 showing its version, passing checks and a checkout quote](docs/screenshots/powershell.png)

</details>

<details>
<summary><strong>Start from a clean welcome, choose the agent, choose the layout</strong></summary>

![Kelpie's welcome screen with Kip, the project and SSH buttons and the command palette](docs/screenshots/welcome.png)

![The new-session launcher with installed agents and placement options](docs/screenshots/new-session.png)

![Pane layout choices over a split workspace](docs/screenshots/layouts.png)

![The terminal context menu with transcript copying, splits and quick commands](docs/screenshots/terminal-menu.png)

![A floating terminal inspecting Git worktrees and running the sample checks](docs/screenshots/floating.png)

</details>

### Let the tasks take different paths

The checkout change turns into two questions: how should coupon input be normalized, and where should free shipping begin? Both questions belong to the same project, but they do not need to share a working copy.

You create a Git worktree for each task. Kelpie keeps the worktrees under their parent project, each with its own sessions and a comment that explains the work. The board turns those folders into one readable picture: **Todo → In progress → Review → Done**.

![The workspace board with eight sample projects and two worktrees across Todo, In progress, Review and Done](docs/screenshots/board.png)

<details>
<summary><strong>From a new worktree to a plan and a run</strong></summary>

The new-worktree dialog shows the repository's `.kelpie.yaml` setup and runs it only after you approve that exact file.

![The new-worktree dialog with branch, base reference, folder and setup options](docs/screenshots/worktree-dialog.png)

The Plan tab reads checklists from Markdown in your repository. When you tick a task, the file on disk changes by that one character.

![Completed and pending tasks in Kelpie's Plan tab](docs/screenshots/plan.png)

A run splits a larger job into agent tasks. In this run, two tasks wait for your approval and will each get a worktree of their own when they start, and a cross-review task waits for both.

![The Runs page with a three-task checkout run waiting for approval](docs/screenshots/runs.png)

</details>

### Know which session needs you

While the agents work, you look at something else. When a session asks a question, it moves to *Needs you* on the dashboard, its row turns amber in the sidebar, and the bell in the status bar gets a dot. You answer the prompt from the dashboard without opening its tab, and `Ctrl+Shift+.` takes you to the next session that is waiting.

![The session dashboard with a real shell confirmation prompt waiting for an answer](docs/screenshots/dashboard.png)

<details>
<summary><strong>Activity, automations and your phone</strong></summary>

![The activity inbox with clearly labelled sample notifications](docs/screenshots/inbox.png)

The automation examples below are disabled, so they cannot start paid model calls by surprise.

![Three disabled sample automations with their agents and schedules](docs/screenshots/automations.png)

You can also [pair a phone](docs/user-guide.md#phone-access). The phone lists your sessions, buzzes when one needs you, shows its screen and, if you allow it, sends replies.

</details>

### Review the result, not just a "done" message

An agent finishes. That moment starts your review; it is not a reason to skip it.

You open the diff, look at the changed line, and leave a note where the question belongs. Kelpie sends all notes to an agent as one focused request. You can also ask a second agent from another vendor to review the same worktree and leave its own notes on the diff.

![A checkout diff with a review note on coupon normalization and the Changes panel beside it](docs/screenshots/review-notes.png)

When the work is ready, **Finish** commits what is left, merges with the method you choose, runs the checks from `.kelpie.yaml` that you approved, and removes the worktree only after everything else succeeded. It never forces anything.

![The Finish dialog for the coupon-validation worktree with its merge method and approved check](docs/screenshots/finish.png)

<details>
<summary><strong>Inspect the change, draft the next request, try several agents</strong></summary>

![The Changes panel with the sample repository's uncommitted checkout change](docs/screenshots/changes.png)

![The floating composer with a drafted checkout review request](docs/screenshots/composer.png)

*Try with several agents* gives one prompt to two to five agents, each in its own worktree. The Compare view then shows time, cost, changed lines and checks side by side.

![The Try with several agents dialog with a shipping-boundary prompt](docs/screenshots/try.png)

</details>

### Read the result in the form that makes sense

Sometimes you want the agent's own terminal interface. Sometimes you want a conversation with rendered Markdown, tool activity and model controls. Supported agents can switch to Kelpie's chat view and back in the same conversation, and Kelpie makes sure that only one front end writes to it.

![A real Claude Code answer in Kelpie's chat view, with tool activity, the context ring and model controls](docs/screenshots/chat.png)

<details>
<summary><strong>Source, diagrams, data, notebooks and search</strong></summary>

![The checkout implementation in Kelpie's source editor](docs/screenshots/editor.png)

![Markdown architecture documentation with a rendered Mermaid diagram](docs/screenshots/markdown.png)

![The CSV viewer displaying synthetic demo measurements](docs/screenshots/csv.png)

![A notebook with a daily revenue summary rendered in Kelpie](docs/screenshots/notebook.png)

![Global search finding sample projects and sessions](docs/screenshots/search.png)

</details>

### Bring the browser and the servers along

The next question is on a web page. You open it beside the agent instead of burying the workspace behind another application. Here, ChatGPT's website and OMP's CLI are two separate panes in Kelpie. The website is the real, signed-out page; it is not a bundled model client.

![ChatGPT web and a real OMP checkout review side by side in native browser and terminal panes](docs/screenshots/chatgpt.png)

When an agent starts a dev server, the page gets a tab of its own in the background. The **Processes** panel shows what each session runs, with PIDs, memory, CPU and ports, and stops a program only after it tells you what will end.

![The Processes panel showing the sample catalog server and its port 8765](docs/screenshots/processes.png)

When the work moves to a server, the **SSH hosts** page keeps your machines, keys, port forwards and file transfers in one place. Agent panes on a server run inside `tmux`, so they survive a dropped connection.

![The SSH hosts page with three demo hosts grouped by environment](docs/screenshots/hosts.png)

### Update without stopping the work

Kelpie checks for a new release in the background. When you choose **Install now**, or **Install when idle**, only the window restarts. The daemon keeps every terminal and agent running, and the window reconnects to the same screens. You never have to wait for an agent to finish before you update. The one exception is the first update from an early 0.1 release whose daemon still ran from the install folder: that update stops sessions once, says so first, and agents resume.

When you quit Kelpie, it asks first if something is still running, and then stops its sessions. The next time you open it, agents resume the same conversation. If you want automations, phone access or file copies to continue with the window closed, you can [keep the daemon running in the background](docs/user-guide.md#install).

### Make it your workspace

A workbench is a place where you spend hours. You can choose a dark or light interface, tune terminal fonts and colours, change density and text size, and record shortcuts that fit your keyboard. The interface is available in English and Vietnamese.

![Appearance settings with language, theme, density, text-size and mascot controls](docs/screenshots/settings.png)

<details>
<summary><strong>Fonts, agents, plugins, shortcuts, light theme and Vietnamese</strong></summary>

![Terminal settings with font, shell and colour-palette controls](docs/screenshots/terminal-settings.png)

![Installed and available agent presets in Settings](docs/screenshots/agents.png)

![The demo plugin listed in Settings and waiting for review](docs/screenshots/plugins.png)

![Configurable keyboard shortcuts](docs/screenshots/shortcuts.png)

![The multi-pane workspace in light theme](docs/screenshots/light.png)

![The multi-pane workspace with Vietnamese interface labels](docs/screenshots/vietnamese.png)

</details>

## Everything in the box

<details>
<summary><strong>The full feature list, with links to the user guide</strong></summary>

**Workspace**
- [Projects and worktrees](docs/user-guide.md#everyday-use) appear in a sidebar that shows each session's status, task title, model and last activity.
- [Tabs, splits, grids and saved layouts](docs/user-guide.md#layouts) let you arrange as many sessions as you need, and *Broadcast input* types into every terminal of a tab.
- [Search everything](docs/user-guide.md#everyday-use) finds sessions, projects, files, commands, settings and earlier prompts from one box.
- [Past conversations](docs/user-guide.md#past-conversations) of Claude Code, Codex and OMP can be searched and resumed in a terminal or in chat view.

**Agents**
- Kelpie knows [23 agent CLIs](docs/user-guide.md#agents), including Claude Code, Codex, OMP, Gemini, Cursor, OpenCode and GitHub Copilot, and it can add agents from the Agent Client Protocol registry.
- [Chat view](docs/user-guide.md#everyday-use) continues the same conversation with tool cards, approvals, an agent map, saved replies and pictures.
- [Several accounts](docs/user-guide.md#several-claude-code--codex-accounts) of Claude Code or Codex can be used side by side, and a session can move to another account when it reaches its usage limit.
- [Sub-sessions](docs/user-guide.md#sub-sessions) let an agent open other agent sessions, with your approval and within limits you set.
- [Hand-off, hibernation, compact and restart](docs/user-guide.md#agents) keep long conversations healthy.

**Attention and cost**
- The [dashboard and quick answers](docs/user-guide.md#everyday-use) show every session that waits for you, the longest wait first.
- [Activity and notifications](docs/user-guide.md#activity-and-notifications) reach your desktop, ntfy, Telegram or a webhook.
- [Context, tokens and cost](docs/user-guide.md#context-tokens-and-cost) appear on every session, with a usage page and an optional cost cap.

**Review and Git**
- The [Changes panel](docs/user-guide.md#source-control-and-review) pushes, pulls, switches branches, shows history and stashes, drafts commit messages and opens pull requests.
- [Review notes, review queue and Finish](docs/user-guide.md#review-and-finish) take a worktree from "done" to merged.
- [Review with another agent and best of several](docs/user-guide.md#review-with-another-agent-and-best-of-several) compare independent attempts.
- [Turns and undo](docs/user-guide.md#turns-and-undo) restore what one agent turn changed, after a preview.

**Planning and automation**
- [Orchestration runs](docs/user-guide.md#orchestration-runs) start agent tasks in order of their dependencies.
- The [Plan tab](docs/user-guide.md#specs-and-tasks-the-plan-tab) reads, ticks and runs the checklists in your repository.
- [Automations](docs/user-guide.md#automations) start agents or approved commands on a schedule or when a file, commit, check or worktree changes.
- [Issue trackers](docs/user-guide.md#issue-trackers-and-gitlab) connect Linear, Jira and GitLab to new sessions and worktrees.

**Files, browser and processes**
- The [editor and viewers](docs/user-guide.md#files) open source, Markdown, Mermaid, CSV, PDF and Jupyter notebooks.
- [Browser panes](docs/user-guide.md#browser-panes-and-design-mode) include developer tools and a design mode that sends picked elements to an agent.
- The [Processes panel](docs/user-guide.md#processes-ports-and-preview) lists each session's programs and ports and stops them safely.

**Remote and mobile**
- [Remote projects](docs/user-guide.md#remote-projects-ssh), the [SSH hosts page](docs/user-guide.md#ssh-hosts) and [SSH panes in any project](docs/user-guide.md#ssh-panes-in-any-project) bring servers into the same workspace.
- [Keeping sessions running on a server](docs/user-guide.md#keeping-sessions-running-on-a-server) explains the `tmux` setup.
- [Phone access](docs/user-guide.md#phone-access) pairs a phone through Kelpie's relay, Tailscale or your local network.

**Personal setup**
- [Agent setup](docs/user-guide.md#agent-setup) manages MCP servers, instruction files and skills for every agent in one place.
- [Plugins](docs/user-guide.md#plugins), [quick commands](docs/user-guide.md#quick-commands) and the [composer](docs/user-guide.md#composer) extend what you can launch and send.
- [Voice dictation](docs/user-guide.md#voice-dictation) recognises speech on your computer; nothing is sent anywhere.
- [Export and import settings](docs/user-guide.md#export-and-import-settings) move your setup to another computer without its secrets.
- [Keyboard shortcuts](docs/user-guide.md#keyboard-shortcuts) can all be changed.

</details>

## Start in three steps

1. **[Download Kelpie](https://github.com/baopeak/kelpie/releases).** Run the Windows installer, drag the macOS app to Applications, or use the Linux AppImage or `.deb` package.
2. **Install and sign in to the agent CLIs you want to use.** Kelpie finds them on your `PATH` or through your login shell. Settings → Agents shows which agents it found.
3. **Add a project folder and start a session.** Put a shell beside the agent, give the agent a task, and keep the result in view.

Kelpie supports Windows 10 1809+ and Windows 11, macOS 12+, and Linux x86-64 with glibc 2.35 or newer. Windows receives the most hands-on testing. The GitHub CLI (`gh`) is optional and enables pull-request features. Remote sessions use your own OpenSSH setup and, for persistence on the server, `tmux`.

## Try the demo kit

The demo kit creates **eight small Git projects, two worktrees and a demo plugin** in a folder you choose. Every project runs with the Python standard library and includes smoke checks, architecture Markdown with a Mermaid diagram, a synthetic CSV dataset and a release checklist. The projects illustrate domains; they are not production applications.

| Project | Its part in the story |
|---|---|
| `atlas-commerce` | Checkout quotes, coupon normalization and the free-shipping boundary. It has a full `.kelpie.yaml`, a plan with two phases, a run file, two worktrees (one with a finished commit) and an uncommitted change to review. |
| `beacon-api` | A product catalog with category filtering and an optional local server on port 8765. |
| `orbit-mobile` | Offline note synchronization with conflict reporting. |
| `prism-design` | Colour tokens and contrast checks. |
| `harbor-infra` | Staging capacity and rollout planning. |
| `pulse-analytics` | Daily revenue aggregation, a CSV export and a notebook. |
| `relay-workers` | A deterministic job queue and retry-policy planning. |
| `field-notes` | A searchable Markdown research index. |

Generate the kit into a new, empty folder outside this repository:

```bash
python scripts/e2e/readme_demo.py ../kelpie-demo-projects
python ../kelpie-demo-projects/atlas-commerce/smoke.py
```

Then add the folders in Kelpie and follow the **[demo scripts](docs/demo-scripts.vi.md)** (in Vietnamese). They contain a three-minute pitch, a fifteen-minute tour and optional scenes, each with steps, narration and the expected result.

<details>
<summary><strong>Recapture the screenshots</strong></summary>

The [screenshot tour](scripts/e2e/readme_shots.py) drives the real app with WebDriver. It needs a built app and daemon, `tauri-driver`, and an `msedgedriver` that matches the installed WebView2 runtime:

```bash
cd app && pnpm tauri build --debug --no-bundle && cd .. && cargo build -p kelpied
python -X utf8 scripts/e2e/readme_shots.py docs/screenshots ../kelpie-demo-projects \
  --app target/debug/kelpie.exe --native-driver /path/to/msedgedriver.exe \
  --agents --browser
```

The tour uses its own data folder, daemon endpoint and empty SSH folder, so it never touches your sessions, keys or settings. The images are Full HD (1920 × 1080). `--agents` starts Claude Code, Codex and OMP in their terminals and Claude Code in chat view, each on one read-only task, and makes real model calls with your own sign-ins; leave out `--agents` and `--browser` for a tour without agents. Claude Code and Codex may ask once to trust the demo folder, and the tour answers yes. The agents keep their usual transcripts of these demo turns in their own folders, as any session of yours would. ChatGPT web is not signed in. The SSH hosts use documentation addresses and are never contacted. The tour writes [capture.json](docs/screenshots/capture.json) with the app version, the screenshots and every check it ran.

</details>

## How it works

Kelpie separates the window from the work. The SolidJS interface presents the workspace. The Rust daemon, `kelpied`, owns the terminals, agent processes, automations, runs and phone link, and the window talks to it over a local named pipe or Unix socket.

A terminal that is not on screen does not need to render. When you return to it, the window receives the missing output or a snapshot instead of asking the agent to start over. That separation lets an update restart the window while every agent keeps working, and it keeps the workspace responsive as sessions accumulate. It does not mean that more agents cost nothing; each agent still uses memory, CPU and tokens.

Read the [architecture](docs/architecture.md) for ownership, transport and recovery contracts, and the [performance procedure](scripts/perf/README.md) for measurement methods and budgets.

## Your tools, your data, your decisions

- **Your workspace stays on your computer.** Sessions, layouts and settings are stored locally, and Kelpie sends no telemetry. Your agent CLIs still contact their model providers under their own accounts and policies.
- **Every connection is one you turn on.** Remote hosts, issue trackers, notification channels and phone access all need explicit setup, and phone access is off by default.
- **The phone link is end-to-end encrypted.** You approve each phone on the computer by typing the code it shows and passing your operating system's own check, such as Windows Hello or Touch ID. Kelpie needs no account. The relay that connects a phone over the internet passes encrypted frames it cannot read, but it sees connection metadata, and the hosted phone app's code comes from that relay. The [phone access guide](docs/user-guide.md#phone-access) explains this trust model, the Tailscale alternative and how to check what the relay serves.
- **Secrets are handled with care, within limits.** Integration tokens, SSH passwords and notification tokens go to the operating system's credential store. Environment variables that you enter in agent settings are stored as plain text, so do not put secrets there or in demo projects.
- **Review remains your job.** Worktree setup, finishing checks, automation commands, plugins and agent permissions all ask for your approval. Kelpie organizes the work; it does not make an agent's output trustworthy by itself.

If you are upgrading from the former Erka name, quit Erka before launching Kelpie. Existing `.erka.yaml` files keep working.

## Build from source

You need Rust stable, Node.js 22+, pnpm 11 and your platform's native build tools: MSVC on Windows, Apple tooling on macOS, and the WebKitGTK packages listed in the [user guide](docs/user-guide.md#build-from-source) on Linux.

```bash
# Frontend dependencies
cd app && pnpm install --frozen-lockfile && cd ..

# Shell 1: an isolated development daemon
bash scripts/dev-daemon.sh run

# Shell 2: the desktop app connected to that daemon
cd app && KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev
```

In PowerShell, set `$env:KELPIE_ENDPOINT = 'kelpie-dev'` before the desktop command. Vite alone cannot exercise the native desktop APIs.

To build an installer, use the [release script](scripts/build-release.sh):

```bash
KELPIE_KEEP_VERSION=1 bash scripts/build-release.sh --bundles nsis
# macOS: --bundles app,dmg    Linux: --bundles appimage,deb
```

Without `KELPIE_KEEP_VERSION=1`, the release script bumps the patch version. It also builds native sidecars, so it is not a quick verification command.

### Checks and real-app verification

```bash
# Rust protocol, daemon and desktop backend
cargo test -p kelpie-proto -p kelpied -p kelpie

# Frontend, from app/
pnpm build
pnpm test
pnpm i18n:check
```

CI runs the Rust suite on Windows and macOS. A local pass on one platform does not replace the other platform's checks. `cargo test --workspace` also includes the optional native speech helper. Real-app scenarios live in [scripts/e2e](scripts/e2e); read their prerequisites first, because some use signed-in agents, remote fixtures or paid model calls. Keep test daemons isolated from your own sessions.

### Scripting the workbench

Agents and local scripts can inspect sessions and report progress through `kelpied`:

```bash
kelpied terminal list
kelpied terminal read 3 --lines 80
kelpied worktree set --comment "checkout ready for review" --status review
kelpied run create runs/checkout-hardening.json
kelpied sub spawn codex "check the catalog filter" --worktree
```

Session-scoped calls and user-only administration have different permissions. Read [automation and CLI usage](docs/user-guide.md#automation) before you script control of other sessions.

## Go deeper

- [User guide](docs/user-guide.md) and [hướng dẫn tiếng Việt](docs/user-guide.vi.md) cover everyday workflows, worktrees, chat, remote hosts, phone access, voice and integrations.
- [Demo scripts](docs/demo-scripts.vi.md) help you present Kelpie with the demo kit.
- [Architecture](docs/architecture.md) explains process ownership, security, transport, replay and recovery.
- [Plugins](docs/plugins.md) describes declarative extensions and their review.
- [Relay self-hosting](docs/relay-self-host.md) and the [link protocol](docs/link-protocol.md) document the phone connection.
- [Brand](docs/brand.md) presents the visual identity and Kip.
- [Performance](scripts/perf/README.md) documents reproducible measurements instead of marketing numbers.

## License and acknowledgements

Kelpie is [MIT licensed](LICENSE). The bundled Windows ConPTY binaries are Microsoft's and also MIT licensed; see the [ConPTY notes](docs/third-party-conpty.md).

Claude, Codex, Gemini, Cursor, OpenCode, GitHub Copilot and their logos are trademarks of their respective owners. Kelpie launches these tools but is not affiliated with or endorsed by them. See the [agent logo credits](docs/third-party-agent-logos.md).
