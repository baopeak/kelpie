<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="app/src/assets/brand/kelpie-lockup-light.svg">
    <img src="app/src/assets/brand/kelpie-lockup.svg" alt="Kelpie" width="260">
  </picture>
</p>

<p align="center"><strong>Every coding agent you use, side by side on one screen.<br>Kelpie updates itself without interrupting a single one of them.</strong></p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><img alt="Latest release" src="https://img.shields.io/github/v/release/baopeak/kelpie?label=download" height="20"></a>
  <img alt="Windows, macOS and Linux" src="https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-4c7a8a" height="20">
  <img alt="Phone app" src="https://img.shields.io/badge/phone-iPhone%20%C2%B7%20Android-4c7a8a" height="20">
  <a href="LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-4c7a8a" height="20"></a>
</p>

<p align="center">
  <a href="https://github.com/baopeak/kelpie/releases"><strong>Download Kelpie</strong></a> ·
  <a href="#whats-inside">What's inside</a> ·
  <a href="#your-sessions-on-your-phone">Phone</a> ·
  <a href="#more-pictures-on-every-platform">Gallery</a> ·
  <a href="#try-the-demo-kit">Demo kit</a> ·
  <a href="docs/user-guide.md">User guide</a> ·
  <a href="README.vi.md">Tiếng Việt</a>
</p>

<p align="center">
  <a href="docs/screenshots/workspace.png"><img src="docs/screenshots/workspace.png" alt="Four agents at work in one Kelpie tab: Claude Code, Codex and OMP in their own terminals and Claude Code in chat view, all reviewing the same checkout, beside eight demo projects" width="900"></a>
  <br><sub>Four real agents in one tab: Claude Code, Codex and OMP in their own terminals, and Claude Code in chat view, all reviewing the same checkout.</sub>
</p>

Starting one coding agent is easy. Once four agents work across three Git branches and two terminal windows, though, it gets hard to remember which agent is waiting for your answer, which checkout belongs to which task, and what is actually ready to review. Kelpie brings all of it onto one screen. Claude Code, Codex, OMP and twenty more agent CLIs run next to your shells, a session that needs you stands out, and finished work moves into a review queue. Because the sessions live in a separate background process, **updating Kelpie never interrupts an agent in the middle of its work**.

## One workbench on every computer you use

<table>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/chat.png"><img src="docs/screenshots/chat.png" alt="Kelpie on Windows: a real Claude Code answer in chat view beside the project sidebar" width="230"></a><br><strong>Windows</strong><br><sub>Windows 10 1809+ and 11. A real Claude Code answer in chat view.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/mac-hero.png"><img src="docs/screenshots/mac-hero.png" alt="Kelpie on macOS: two shells and a chat side by side, with the demo projects in the sidebar (simulated agent)" width="230"></a><br><strong>macOS</strong><br><sub>macOS 12+. Two shells and a chat side by side (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/linux-hero.png"><img src="docs/screenshots/linux-hero.png" alt="Kelpie on Linux: two shells and a chat side by side, with the demo projects in the sidebar (simulated agent)" width="230"></a><br><strong>Linux</strong><br><sub>x86-64, glibc 2.35+, AppImage or .deb. The same workspace (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-inbox-control.png"><img src="docs/screenshots/phone-inbox-control.png" alt="The phone inbox: sessions that need you, working and idle, with See the computer's screen at the top (simulated agent)" width="92"></a><br><strong>Phone</strong><br><sub>iPhone and Android. Every session by status (simulated agent).</sub></td>
</tr>
</table>

## Updates never interrupt your agents

The `kelpied` daemon owns every terminal and agent process, and the window only presents them. When Kelpie finds a new release and you choose **Install now** or **Install when idle**, only the window restarts: the daemon keeps every terminal and agent running, and the window reconnects to the same screens. You never have to wait for an agent to finish before you update. The one exception is the first update from an early 0.1 release whose daemon still ran from the install folder: that update stops sessions once, says so first, and agents resume.

Kelpie is a personal tool. Your sessions run in `kelpied` on your own computer, there is no Kelpie account, and no Kelpie service holds your work. When you quit Kelpie, it asks first if something is still running and then stops its sessions; the next time you open it, agents resume the same conversation. If you want automations, phone access or file copies to continue with the window closed, you can [keep the daemon running in the background](docs/user-guide.md#install).

## Why people use it

- Kelpie launches the agent CLIs you already installed, with your own sign-ins, subscriptions and permission rules, so you change neither your tools nor your accounts.
- A dashboard sorts every session into *Needs you*, *Working*, *Done* and *Failed*, and you can answer a waiting agent without opening its tab.
- Each parallel task can get its own Git worktree, sessions and comment, while a board shows whether it sits in *Todo*, *In progress*, *Review* or *Done*.
- When an agent finishes, you review the diff and leave notes inside Kelpie, and **Finish** then commits, merges, runs the checks you approved and cleans up the worktree.
- An optional phone app tells you when a session needs you and lets you answer it from anywhere, end-to-end encrypted.

## Real agents, real answers

Kelpie launches each CLI exactly as it is, so every agent keeps its own interface. The three answers below, like the chat answer above, are real model answers made with real sign-ins on the demo checkout; [capture.json](docs/screenshots/capture.json) records which parts are sample data.

<table>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/claude.png"><img src="docs/screenshots/claude.png" alt="Claude Code's real answer in its own terminal interface inside Kelpie" width="280"></a><br><strong>Claude Code</strong><br><sub>Keeps its native terminal interface and names the riskiest checkout boundaries.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/codex.png"><img src="docs/screenshots/codex.png" alt="Codex CLI's real answer inside Kelpie" width="280"></a><br><strong>Codex</strong><br><sub>Reads the smoke checks and suggests a missing assertion for the free-shipping boundary.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/omp.png"><img src="docs/screenshots/omp.png" alt="OMP's real terminal answer and file-reading activity inside Kelpie" width="280"></a><br><strong>OMP</strong><br><sub>Reads the checkout code and explains its shipping and rounding rules.</sub></td>
</tr>
</table>

## What's inside

<table>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/dashboard.png"><img src="docs/screenshots/dashboard.png" alt="The Agent Dashboard with a shell waiting for an answer" width="280"></a><br><strong><a href="docs/user-guide.md#everyday-use">Know who needs you</a></strong><br><sub>The dashboard sorts sessions by status, and you answer a waiting prompt without opening its tab.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/board.png"><img src="docs/screenshots/board.png" alt="The workspace board: projects and worktrees by stage" width="280"></a><br><strong><a href="docs/user-guide.md#worktrees-and-the-workspace-board">Worktrees and the board</a></strong><br><sub>Each task gets its own Git worktree, and the board shows it in Todo, In progress, Review or Done.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/review-notes.png"><img src="docs/screenshots/review-notes.png" alt="A review note on a line of the diff, ready to send to an agent" width="280"></a><br><strong><a href="docs/user-guide.md#source-control-and-review">Review notes</a></strong><br><sub>Leave notes on the diff, and Kelpie sends them to an agent as one focused request.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/finish.png"><img src="docs/screenshots/finish.png" alt="The Finish dialog: approved checks, then merge into main" width="280"></a><br><strong><a href="docs/user-guide.md#review-and-finish">Finish</a></strong><br><sub>Commits, merges, runs the checks you approved, and removes the worktree only after everything else succeeded.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/review-with.png"><img src="docs/screenshots/review-with.png" alt="Review with another agent: pick the reviewer and the prompt; the diff goes with it" width="280"></a><br><strong><a href="docs/user-guide.md#review-with-another-agent-and-best-of-several">A second opinion</a></strong><br><sub>An agent from another vendor reviews the same worktree and leaves its own notes.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/sub-sessions.png"><img src="docs/screenshots/sub-sessions.png" alt="An agent with two sub-sessions: the sub-tab strip over its pane and the tree in the sidebar (simulated agent)" width="280"></a><br><strong><a href="docs/user-guide.md#sub-sessions">Sub-sessions</a></strong><br><sub>An agent opens other agent sessions that you can see, steer and close (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/runs.png"><img src="docs/screenshots/runs.png" alt="A run whose tasks wait for an approval or a dependency" width="280"></a><br><strong><a href="docs/user-guide.md#orchestration-runs">Orchestration runs</a></strong><br><sub>A run splits a job into agent tasks with dependencies and approvals.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/plan.png"><img src="docs/screenshots/plan.png" alt="The Plan tab lists the plans and specs of the project with their tasks" width="280"></a><br><strong><a href="docs/user-guide.md#specs-and-tasks-the-plan-tab">Plans in your repository</a></strong><br><sub>The Plan tab reads Markdown checklists, ticks them and starts an agent on any task.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/broadcast.png"><img src="docs/screenshots/broadcast.png" alt="Four shells tiled 2 × 2 with Broadcast input: one line typed once runs in all four" width="280"></a><br><strong><a href="docs/user-guide.md#layouts">Grids and broadcast</a></strong><br><sub>Tile sessions into any grid, and type one line into every terminal of a tab.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/chatgpt.png"><img src="docs/screenshots/chatgpt.png" alt="ChatGPT web and a real OMP checkout review side by side in native browser and terminal panes" width="280"></a><br><strong><a href="docs/user-guide.md#browser-panes-and-design-mode">A browser beside the agent</a></strong><br><sub>ChatGPT's real, signed-out website next to a real OMP review, in browser and terminal panes.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/design-mode.png"><img src="docs/screenshots/design-mode.png" alt="Design mode in a browser pane: pick an element of a local demo page and write what should change" width="280"></a><br><strong><a href="docs/user-guide.md#browser-panes-and-design-mode">Design mode</a></strong><br><sub>Pick an element of a local page and tell an agent what should change.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/processes.png"><img src="docs/screenshots/processes.png" alt="The Processes panel: the catalog server and the port it listens on" width="280"></a><br><strong><a href="docs/user-guide.md#processes-ports-and-preview">Processes and ports</a></strong><br><sub>Each session's programs, memory, CPU and ports, stopped only after Kelpie says what will end.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/hosts.png"><img src="docs/screenshots/hosts.png" alt="The SSH hosts page with demo hosts at documentation addresses, never connected" width="280"></a><br><strong><a href="docs/user-guide.md#ssh-hosts">SSH hosts</a></strong><br><sub>Machines, keys, port forwards and file transfers in one place; agents on a server run in <code>tmux</code>.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/powershell.png"><img src="docs/screenshots/powershell.png" alt="PowerShell 7 showing its version, passing checks and a checkout quote" width="280"></a><br><strong><a href="docs/user-guide.md#terminals">Shells beside agents</a></strong><br><sub>PowerShell, Bash or zsh fit in as naturally as an agent; here PowerShell 7 runs the sample's checks.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/file-link.png"><img src="docs/screenshots/file-link.png" alt="A path printed in a terminal is a link: Ctrl+click opens the file at that line" width="280"></a><br><strong><a href="docs/user-guide.md#terminals">File links</a></strong><br><sub>Ctrl+click a path an agent prints to open the file at that line.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/usage.png"><img src="docs/screenshots/usage.png" alt="Usage and cost by project, from the agents' transcripts (simulated agent)" width="280"></a><br><strong><a href="docs/user-guide.md#context-tokens-and-cost">Usage and cost</a></strong><br><sub>Cost and tokens by day, project, agent or model, from the agents' transcripts (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/agent-setup.png"><img src="docs/screenshots/agent-setup.png" alt="Agent setup: each agent's MCP servers, with the demo project's own server" width="280"></a><br><strong><a href="docs/user-guide.md#agent-setup">Agent setup</a></strong><br><sub>MCP servers, instruction files and skills for every agent in one place.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/theme-editor.png"><img src="docs/screenshots/theme-editor.png" alt="The theme editor: Dracula duplicated into a theme of your own, with a live preview" width="280"></a><br><strong><a href="docs/user-guide.md#terminals">Make it yours</a></strong><br><sub>Your own terminal themes with a live preview, shortcuts you record, and English or Vietnamese.</sub></td>
</tr>
</table>

The [full feature list](#everything-in-the-box) below links every feature to the user guide.

## Your sessions on your phone

Phone access is optional and off by default. Once you pair a phone, it lists your sessions by status, buzzes when one needs you, and lets you read, type, approve and start agents, with everything end-to-end encrypted. You approve each phone on the computer by typing the code it shows, and the phone's app lock asks for Face ID, a fingerprint or its passcode. If you allow it, the phone also sees and controls the computer's own screen on Windows and on macOS 12.3 or later, while an orange border and a stop hotkey stay on the computer. The [phone access guide](docs/user-guide.md#phone-access) explains the three ways to connect and what each one trusts.

<table>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-inbox.png"><img src="docs/screenshots/phone-safari-inbox.png" alt="The inbox by status in Safari on an iPhone: sessions that need you first (simulated agent)" width="170"></a><br><sub>The inbox in Safari on an iPhone, sessions that need you first (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-chat-approval.png"><img src="docs/screenshots/phone-chat-approval.png" alt="An agent asks to tag a release and push the tag: allow or deny it from the phone (simulated agent)" width="170"></a><br><sub>Allow or deny an agent's command from the phone (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-terminal-keys.png"><img src="docs/screenshots/phone-terminal-keys.png" alt="Typing into a terminal session, with the key bar above the keyboard" width="170"></a><br><sub>Type into a terminal session, with a key bar above the keyboard.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-screen-windows.png"><img src="docs/screenshots/phone-screen-windows.png" alt="The phone sees the Windows desktop live, with the trackpad, the Ctrl, Shift, Alt and Win keys and a field to type into" width="170"></a><br><sub>See and control the Windows desktop, with a trackpad and modifier keys.</sub></td>
</tr>
</table>

## Watch it work

<table>
<tr>
<td align="center" valign="top" width="50%"><!-- clip: mac-phone-control.mp4 --><a href="#watch-it-work"><img src="docs/screenshots/mac-phone-control-poster.png" alt="Clip poster: on the left the phone sees the Mac's screen, on the right the Mac with the line typed from the phone in TextEdit" width="420"></a><br><strong>The phone drives a Mac</strong><br><sub>From the phone: tap into TextEdit on the Mac, type a line, then ⌘S saves it.</sub></td>
<td align="center" valign="top" width="50%"><!-- clip: clip-worktree-finish.mp4 --><a href="#watch-it-work"><img src="docs/screenshots/clip-worktree-finish-poster.png" alt="Clip poster: Finish merged the worktree into main and removed it" width="420"></a><br><strong>From a worktree to main</strong><br><sub>New worktree, a shell commits a check in it, then Finish merges it into main and the worktree is gone.</sub></td>
</tr>
</table>

## More pictures on every platform

Every picture below comes from the real app with demo projects. A picture marked *simulated agent* shows small local demo programs speaking an agent's format; it makes no model call and shows no real model answer. [README-media.json](docs/screenshots/README-media.json) records each picture's platform, captions and how it was made.

<details>
<summary><strong>Windows</strong></summary>

<table>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/welcome.png"><img src="docs/screenshots/welcome.png" alt="The welcome screen before any project is added." width="260"></a><br><sub>The welcome screen before any project is added.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/search.png"><img src="docs/screenshots/search.png" alt="Search everything: sessions, projects, files and settings from one box." width="260"></a><br><sub>Search everything: sessions, projects, files and settings from one box.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/changes.png"><img src="docs/screenshots/changes.png" alt="The Changes panel beside two shells: the branch, the changed file and Commit all." width="260"></a><br><sub>The Changes panel beside two shells: the branch, the changed file and Commit all.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/editor.png"><img src="docs/screenshots/editor.png" alt="The editor on the demo checkout code, with the Changes panel open." width="260"></a><br><sub>The editor on the demo checkout code, with the Changes panel open.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/markdown.png"><img src="docs/screenshots/markdown.png" alt="Markdown with a Mermaid diagram, rendered in the file viewer." width="260"></a><br><sub>Markdown with a Mermaid diagram, rendered in the file viewer.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/csv.png"><img src="docs/screenshots/csv.png" alt="A CSV file as a sortable table." width="260"></a><br><sub>A CSV file as a sortable table.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/new-session.png"><img src="docs/screenshots/new-session.png" alt="The new-session launcher: agent, first prompt, worktree and where it opens." width="260"></a><br><sub>The new-session launcher: agent, first prompt, worktree and where it opens.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/layouts.png"><img src="docs/screenshots/layouts.png" alt="The layout menu: grid presets and saved layouts." width="260"></a><br><sub>The layout menu: grid presets and saved layouts.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/composer.png"><img src="docs/screenshots/composer.png" alt="The floating composer for a longer prompt." width="260"></a><br><sub>The floating composer for a longer prompt.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/terminal-menu.png"><img src="docs/screenshots/terminal-menu.png" alt="A terminal&#x27;s context menu: split, hand off, theme and more." width="260"></a><br><sub>A terminal's context menu: split, hand off, theme and more.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/floating.png"><img src="docs/screenshots/floating.png" alt="The floating terminal over the workspace." width="260"></a><br><sub>The floating terminal over the workspace.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/inbox.png"><img src="docs/screenshots/inbox.png" alt="The Activity inbox with labelled demo entries." width="260"></a><br><sub>The Activity inbox with labelled demo entries.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/worktree-dialog.png"><img src="docs/screenshots/worktree-dialog.png" alt="The new-worktree dialog, with the project&#x27;s .kelpie.yaml setup." width="260"></a><br><sub>The new-worktree dialog, with the project's .kelpie.yaml setup.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/automations.png"><img src="docs/screenshots/automations.png" alt="Scheduled automations (demo, switched off)." width="260"></a><br><sub>Scheduled automations (demo, switched off).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/settings.png"><img src="docs/screenshots/settings.png" alt="Settings → Appearance." width="260"></a><br><sub>Settings → Appearance.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/terminal-settings.png"><img src="docs/screenshots/terminal-settings.png" alt="Settings → Terminal: font, renderer, cursor and colour theme." width="260"></a><br><sub>Settings → Terminal: font, renderer, cursor and colour theme.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/shortcuts.png"><img src="docs/screenshots/shortcuts.png" alt="Settings → Shortcuts." width="260"></a><br><sub>Settings → Shortcuts.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/notebook.png"><img src="docs/screenshots/notebook.png" alt="A Jupyter notebook with its outputs." width="260"></a><br><sub>A Jupyter notebook with its outputs.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/try.png"><img src="docs/screenshots/try.png" alt="Try with several agents: one task, each agent in its own worktree." width="260"></a><br><sub>Try with several agents: one task, each agent in its own worktree.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/plugins.png"><img src="docs/screenshots/plugins.png" alt="Settings → Plugins: the demo plugin waiting for review." width="260"></a><br><sub>Settings → Plugins: the demo plugin waiting for review.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/sidebar.png"><img src="docs/screenshots/sidebar.png" alt="The sidebar: a session that needs you, one at work, and each project&#x27;s branch, comment and worktrees (simulated agent)." width="260"></a><br><sub>The sidebar: a session that needs you, one at work, and each project's branch, comment and worktrees (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/quick-answer.png"><img src="docs/screenshots/quick-answer.png" alt="Answer from the dashboard: the waiting session&#x27;s question and its keys, without leaving the board (simulated agent)." width="260"></a><br><sub>Answer from the dashboard: the waiting session's question and its keys, without leaving the board (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/split-picker.png"><img src="docs/screenshots/split-picker.png" alt="Ctrl+Shift+D splits a pane, and the empty pane asks what to run." width="260"></a><br><sub>Ctrl+Shift+D splits a pane, and the empty pane asks what to run.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/git-history.png"><img src="docs/screenshots/git-history.png" alt="The Changes panel with History unfolded: the last commits of the branch." width="260"></a><br><sub>The Changes panel with History unfolded: the last commits of the branch.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/git-stashes.png"><img src="docs/screenshots/git-stashes.png" alt="The Changes panel with Stashes unfolded: a demo stash with Apply and Drop." width="260"></a><br><sub>The Changes panel with Stashes unfolded: a demo stash with Apply and Drop.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/review-queue.png"><img src="docs/screenshots/review-queue.png" alt="The Review tab: a worktree waiting for review, its diff, Review with… and Approve → Finish." width="260"></a><br><sub>The Review tab: a worktree waiting for review, its diff, Review with… and Approve → Finish.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/automation-event.png"><img src="docs/screenshots/automation-event.png" alt="A new automation started by an event: a Markdown file under docs/ changes (not saved)." width="260"></a><br><sub>A new automation started by an event: a Markdown file under docs/ changes (not saved).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/host-edit.png"><img src="docs/screenshots/host-edit.png" alt="A host&#x27;s drawer with its appearance: label colour, theme, font, cursor and a live preview." width="260"></a><br><sub>A host's drawer with its appearance: label colour, theme, font, cursor and a live preview.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/notifications.png"><img src="docs/screenshots/notifications.png" alt="Settings → Notifications: desktop alerts, phone alerts, ntfy, Telegram and webhook." width="260"></a><br><sub>Settings → Notifications: desktop alerts, phone alerts, ntfy, Telegram and webhook.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mobile-settings.png"><img src="docs/screenshots/mobile-settings.png" alt="Settings → Mobile with phone access on (no pairing code shown)." width="260"></a><br><sub>Settings → Mobile with phone access on (no pairing code shown).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/voice.png"><img src="docs/screenshots/voice.png" alt="Settings → Voice: the on-device speech models (no microphone on the test machine)." width="260"></a><br><sub>Settings → Voice: the on-device speech models (no microphone on the test machine).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/settings-export.png"><img src="docs/screenshots/settings-export.png" alt="Export and import settings, with the parts to include." width="260"></a><br><sub>Export and import settings, with the parts to include.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/chat-approval.png"><img src="docs/screenshots/chat-approval.png" alt="A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent)." width="260"></a><br><sub>A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/sub-sessions-ask.png"><img src="docs/screenshots/sub-sessions-ask.png" alt="An agent asks to open a sub-session: Allow once, Allow for this session, Always allow or Deny (simulated agent)." width="260"></a><br><sub>An agent asks to open a sub-session: Allow once, Allow for this session, Always allow or Deny (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/owed-checks.png"><img src="docs/screenshots/owed-checks.png" alt="Activity: a sub-session&#x27;s report and the check it could not run itself (simulated agent)." width="260"></a><br><sub>Activity: a sub-session's report and the check it could not run itself (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/past-conversations.png"><img src="docs/screenshots/past-conversations.png" alt="The Sessions tab: past Claude Code and Codex conversations of this project (simulated agent)." width="260"></a><br><sub>The Sessions tab: past Claude Code and Codex conversations of this project (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><!-- clip: clip-phone-answer.mp4 --><a href="#watch-it-work"><img src="docs/screenshots/clip-phone-answer-poster.png" alt="Clip poster: a demo menu (not an agent) waits, the phone answers it with one tap, and the desktop pane carries on." width="260"></a><br><sub>Clip poster: a demo menu (not an agent) waits, the phone answers it with one tap, and the desktop pane carries on.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/light.png"><img src="docs/screenshots/light.png" alt="The multi-pane workspace in the light theme." width="260"></a><br><sub>The multi-pane workspace in the light theme.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/vietnamese.png"><img src="docs/screenshots/vietnamese.png" alt="The multi-pane workspace with the Vietnamese interface." width="260"></a><br><sub>The multi-pane workspace with the Vietnamese interface.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/agents.png"><img src="docs/screenshots/agents.png" alt="Settings → Agents: the installed and available agent presets." width="260"></a><br><sub>Settings → Agents: the installed and available agent presets.</sub></td>
</tr>
</table>

</details>

<details>
<summary><strong>macOS</strong></summary>

<table>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-workspace.png"><img src="docs/screenshots/mac-workspace.png" alt="One tab with two shell sessions and the Files panel." width="260"></a><br><sub>One tab with two shell sessions and the Files panel.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-welcome.png"><img src="docs/screenshots/mac-welcome.png" alt="The welcome screen before any project is added." width="260"></a><br><sub>The welcome screen before any project is added.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-chat.png"><img src="docs/screenshots/mac-chat.png" alt="Chat view with the agent&#x27;s tool calls, its edits and the files it changed (simulated agent)." width="260"></a><br><sub>Chat view with the agent's tool calls, its edits and the files it changed (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-chat-approval.png"><img src="docs/screenshots/mac-chat-approval.png" alt="A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent)." width="260"></a><br><sub>A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-dashboard-agents.png"><img src="docs/screenshots/mac-dashboard-agents.png" alt="The Agent Dashboard: sessions that need you, those working, and the idle ones (simulated agent)." width="260"></a><br><sub>The Agent Dashboard: sessions that need you, those working, and the idle ones (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-search.png"><img src="docs/screenshots/mac-search.png" alt="Search everything: sessions, projects, files and settings from one box." width="260"></a><br><sub>Search everything: sessions, projects, files and settings from one box.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-new-session.png"><img src="docs/screenshots/mac-new-session.png" alt="The new-session dialog with agents, quick commands and where to open it." width="260"></a><br><sub>The new-session dialog with agents, quick commands and where to open it.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-changes.png"><img src="docs/screenshots/mac-changes.png" alt="The Changes panel: branch, commit message and changed files." width="260"></a><br><sub>The Changes panel: branch, commit message and changed files.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-editor.png"><img src="docs/screenshots/mac-editor.png" alt="The built-in editor on a demo source file." width="260"></a><br><sub>The built-in editor on a demo source file.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-review-notes.png"><img src="docs/screenshots/mac-review-notes.png" alt="A review note on a line of the diff, ready to send to an agent." width="260"></a><br><sub>A review note on a line of the diff, ready to send to an agent.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-markdown.png"><img src="docs/screenshots/mac-markdown.png" alt="Markdown with a Mermaid diagram, rendered." width="260"></a><br><sub>Markdown with a Mermaid diagram, rendered.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-csv.png"><img src="docs/screenshots/mac-csv.png" alt="A CSV file as a sortable table." width="260"></a><br><sub>A CSV file as a sortable table.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-notebook.png"><img src="docs/screenshots/mac-notebook.png" alt="A Jupyter notebook with its outputs." width="260"></a><br><sub>A Jupyter notebook with its outputs.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-plan.png"><img src="docs/screenshots/mac-plan.png" alt="The Plan tab with the project&#x27;s specs and their tasks." width="260"></a><br><sub>The Plan tab with the project's specs and their tasks.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-board.png"><img src="docs/screenshots/mac-board.png" alt="The workspace board: projects and worktrees by stage." width="260"></a><br><sub>The workspace board: projects and worktrees by stage.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-layouts.png"><img src="docs/screenshots/mac-layouts.png" alt="The layout menu: grid presets, Tile all and Broadcast input." width="260"></a><br><sub>The layout menu: grid presets, Tile all and Broadcast input.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-composer.png"><img src="docs/screenshots/mac-composer.png" alt="The floating composer for a longer prompt." width="260"></a><br><sub>The floating composer for a longer prompt.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-terminal-menu.png"><img src="docs/screenshots/mac-terminal-menu.png" alt="A terminal&#x27;s context menu." width="260"></a><br><sub>A terminal's context menu.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-floating.png"><img src="docs/screenshots/mac-floating.png" alt="The floating terminal over the workspace." width="260"></a><br><sub>The floating terminal over the workspace.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-inbox.png"><img src="docs/screenshots/mac-inbox.png" alt="The activity inbox (labelled demo samples)." width="260"></a><br><sub>The activity inbox (labelled demo samples).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-worktree-dialog.png"><img src="docs/screenshots/mac-worktree-dialog.png" alt="The new-worktree dialog with the project&#x27;s .kelpie.yaml." width="260"></a><br><sub>The new-worktree dialog with the project's .kelpie.yaml.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-finish.png"><img src="docs/screenshots/mac-finish.png" alt="The Finish dialog: how to merge and the checks it runs." width="260"></a><br><sub>The Finish dialog: how to merge and the checks it runs.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-try.png"><img src="docs/screenshots/mac-try.png" alt="Try with several agents: one task, one worktree per agent." width="260"></a><br><sub>Try with several agents: one task, one worktree per agent.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-runs.png"><img src="docs/screenshots/mac-runs.png" alt="A run waiting for approval of its first tasks." width="260"></a><br><sub>A run waiting for approval of its first tasks.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-processes.png"><img src="docs/screenshots/mac-processes.png" alt="The Processes panel showing the demo server&#x27;s port." width="260"></a><br><sub>The Processes panel showing the demo server's port.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-hosts.png"><img src="docs/screenshots/mac-hosts.png" alt="SSH hosts (documentation addresses, never connected)." width="260"></a><br><sub>SSH hosts (documentation addresses, never connected).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-automations.png"><img src="docs/screenshots/mac-automations.png" alt="Scheduled automations (disabled samples)." width="260"></a><br><sub>Scheduled automations (disabled samples).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-plugins.png"><img src="docs/screenshots/mac-plugins.png" alt="Plugins: the demo plugin waiting for review." width="260"></a><br><sub>Plugins: the demo plugin waiting for review.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings.png"><img src="docs/screenshots/mac-settings.png" alt="Settings → Appearance." width="260"></a><br><sub>Settings → Appearance.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-terminal-settings.png"><img src="docs/screenshots/mac-terminal-settings.png" alt="Settings → Terminal." width="260"></a><br><sub>Settings → Terminal.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-shortcuts.png"><img src="docs/screenshots/mac-shortcuts.png" alt="Settings → Shortcuts with the Mac&#x27;s keys." width="260"></a><br><sub>Settings → Shortcuts with the Mac's keys.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings-mobile.png"><img src="docs/screenshots/mac-settings-mobile.png" alt="Settings → Mobile with phone access on (no pairing code shown)." width="260"></a><br><sub>Settings → Mobile with phone access on (no pairing code shown).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-mobile-permissions.png"><img src="docs/screenshots/mac-mobile-permissions.png" alt="The Mac&#x27;s Screen Recording and Accessibility rows in Settings → Mobile." width="260"></a><br><sub>The Mac's Screen Recording and Accessibility rows in Settings → Mobile.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings-notifications.png"><img src="docs/screenshots/mac-settings-notifications.png" alt="Settings → Notifications: desktop alerts and channels to the phone." width="260"></a><br><sub>Settings → Notifications: desktop alerts and channels to the phone.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings-commands.png"><img src="docs/screenshots/mac-settings-commands.png" alt="Settings → Quick Commands." width="260"></a><br><sub>Settings → Quick Commands.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings-daemon.png"><img src="docs/screenshots/mac-settings-daemon.png" alt="Settings → Session Daemon: what happens when Kelpie closes, export and import." width="260"></a><br><sub>Settings → Session Daemon: what happens when Kelpie closes, export and import.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-settings-about.png"><img src="docs/screenshots/mac-settings-about.png" alt="Settings → About on macOS." width="260"></a><br><sub>Settings → About on macOS.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-light.png"><img src="docs/screenshots/mac-light.png" alt="The light theme." width="260"></a><br><sub>The light theme.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-vietnamese.png"><img src="docs/screenshots/mac-vietnamese.png" alt="The app in Vietnamese." width="260"></a><br><sub>The app in Vietnamese.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/mac-screen-from-phone.png"><img src="docs/screenshots/mac-screen-from-phone.png" alt="The phone sees a TextEdit window on the Mac, with the ⌘ ⌥ ⌃ ⇧ key bar." width="130"></a><br><sub>The phone sees a TextEdit window on the Mac, with the ⌘ ⌥ ⌃ ⇧ key bar.</sub></td>
</tr>
</table>

</details>

<details>
<summary><strong>Linux (Ubuntu 22.04)</strong></summary>

<table>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-workspace.png"><img src="docs/screenshots/linux-workspace.png" alt="One tab with two shell sessions and the Files panel." width="260"></a><br><sub>One tab with two shell sessions and the Files panel.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-welcome.png"><img src="docs/screenshots/linux-welcome.png" alt="The welcome screen before any project is added." width="260"></a><br><sub>The welcome screen before any project is added.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-chat.png"><img src="docs/screenshots/linux-chat.png" alt="Chat view with the agent&#x27;s tool calls, its edits and the files it changed (simulated agent)." width="260"></a><br><sub>Chat view with the agent's tool calls, its edits and the files it changed (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-chat-approval.png"><img src="docs/screenshots/linux-chat-approval.png" alt="A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent)." width="260"></a><br><sub>A chat waiting for approval of a risky command: Allow, Allow for session or Deny (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-dashboard-agents.png"><img src="docs/screenshots/linux-dashboard-agents.png" alt="The Agent Dashboard: sessions that need you, those working, and the idle ones (simulated agent)." width="260"></a><br><sub>The Agent Dashboard: sessions that need you, those working, and the idle ones (simulated agent).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-search.png"><img src="docs/screenshots/linux-search.png" alt="Search everything: sessions, projects, files and settings from one box." width="260"></a><br><sub>Search everything: sessions, projects, files and settings from one box.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-new-session.png"><img src="docs/screenshots/linux-new-session.png" alt="The new-session dialog with agents, quick commands and where to open it." width="260"></a><br><sub>The new-session dialog with agents, quick commands and where to open it.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-changes.png"><img src="docs/screenshots/linux-changes.png" alt="The Changes panel: branch, commit message and changed files." width="260"></a><br><sub>The Changes panel: branch, commit message and changed files.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-editor.png"><img src="docs/screenshots/linux-editor.png" alt="The built-in editor on a demo source file." width="260"></a><br><sub>The built-in editor on a demo source file.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-review-notes.png"><img src="docs/screenshots/linux-review-notes.png" alt="A review note on a line of the diff, ready to send to an agent." width="260"></a><br><sub>A review note on a line of the diff, ready to send to an agent.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-markdown.png"><img src="docs/screenshots/linux-markdown.png" alt="Markdown with a Mermaid diagram, rendered." width="260"></a><br><sub>Markdown with a Mermaid diagram, rendered.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-notebook.png"><img src="docs/screenshots/linux-notebook.png" alt="A Jupyter notebook with its outputs." width="260"></a><br><sub>A Jupyter notebook with its outputs.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-plan.png"><img src="docs/screenshots/linux-plan.png" alt="The Plan tab with the project&#x27;s specs and their tasks." width="260"></a><br><sub>The Plan tab with the project's specs and their tasks.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-board.png"><img src="docs/screenshots/linux-board.png" alt="The workspace board: projects and worktrees by stage." width="260"></a><br><sub>The workspace board: projects and worktrees by stage.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-layouts.png"><img src="docs/screenshots/linux-layouts.png" alt="The layout menu: grid presets, Tile all and Broadcast input." width="260"></a><br><sub>The layout menu: grid presets, Tile all and Broadcast input.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-floating.png"><img src="docs/screenshots/linux-floating.png" alt="The floating terminal over the workspace." width="260"></a><br><sub>The floating terminal over the workspace.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-inbox.png"><img src="docs/screenshots/linux-inbox.png" alt="The activity inbox (labelled demo samples)." width="260"></a><br><sub>The activity inbox (labelled demo samples).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-worktree-dialog.png"><img src="docs/screenshots/linux-worktree-dialog.png" alt="The new-worktree dialog with the project&#x27;s .kelpie.yaml." width="260"></a><br><sub>The new-worktree dialog with the project's .kelpie.yaml.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-finish.png"><img src="docs/screenshots/linux-finish.png" alt="The Finish dialog: how to merge and the checks it runs." width="260"></a><br><sub>The Finish dialog: how to merge and the checks it runs.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-try.png"><img src="docs/screenshots/linux-try.png" alt="Try with several agents: one task, one worktree per agent." width="260"></a><br><sub>Try with several agents: one task, one worktree per agent.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-runs.png"><img src="docs/screenshots/linux-runs.png" alt="A run waiting for approval of its first tasks." width="260"></a><br><sub>A run waiting for approval of its first tasks.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-processes.png"><img src="docs/screenshots/linux-processes.png" alt="The Processes panel showing the demo server&#x27;s port." width="260"></a><br><sub>The Processes panel showing the demo server's port.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-hosts.png"><img src="docs/screenshots/linux-hosts.png" alt="SSH hosts (documentation addresses, never connected)." width="260"></a><br><sub>SSH hosts (documentation addresses, never connected).</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-automations.png"><img src="docs/screenshots/linux-automations.png" alt="Scheduled automations (disabled samples)." width="260"></a><br><sub>Scheduled automations (disabled samples).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-plugins.png"><img src="docs/screenshots/linux-plugins.png" alt="Plugins: the demo plugin waiting for review." width="260"></a><br><sub>Plugins: the demo plugin waiting for review.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings.png"><img src="docs/screenshots/linux-settings.png" alt="Settings → Appearance." width="260"></a><br><sub>Settings → Appearance.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings-mobile.png"><img src="docs/screenshots/linux-settings-mobile.png" alt="Settings → Mobile with phone access on (no pairing code shown)." width="260"></a><br><sub>Settings → Mobile with phone access on (no pairing code shown).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings-notifications.png"><img src="docs/screenshots/linux-settings-notifications.png" alt="Settings → Notifications: desktop alerts and channels to the phone." width="260"></a><br><sub>Settings → Notifications: desktop alerts and channels to the phone.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings-daemon.png"><img src="docs/screenshots/linux-settings-daemon.png" alt="Settings → Session Daemon: what happens when Kelpie closes, export and import." width="260"></a><br><sub>Settings → Session Daemon: what happens when Kelpie closes, export and import.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-light.png"><img src="docs/screenshots/linux-light.png" alt="The light theme." width="260"></a><br><sub>The light theme.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-vietnamese.png"><img src="docs/screenshots/linux-vietnamese.png" alt="The app in Vietnamese." width="260"></a><br><sub>The app in Vietnamese.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-agents.png"><img src="docs/screenshots/linux-agents.png" alt="Settings → Agents: the agent CLIs Kelpie knows and how each one starts." width="260"></a><br><sub>Settings → Agents: the agent CLIs Kelpie knows and how each one starts.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings-about.png"><img src="docs/screenshots/linux-settings-about.png" alt="Settings → About on Linux." width="260"></a><br><sub>Settings → About on Linux.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-terminal-settings.png"><img src="docs/screenshots/linux-terminal-settings.png" alt="Settings → Terminal." width="260"></a><br><sub>Settings → Terminal.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-settings-commands.png"><img src="docs/screenshots/linux-settings-commands.png" alt="Settings → Quick Commands." width="260"></a><br><sub>Settings → Quick Commands.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-composer.png"><img src="docs/screenshots/linux-composer.png" alt="The floating composer for a longer prompt." width="260"></a><br><sub>The floating composer for a longer prompt.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-csv.png"><img src="docs/screenshots/linux-csv.png" alt="A CSV file as a sortable table." width="260"></a><br><sub>A CSV file as a sortable table.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-terminal-menu.png"><img src="docs/screenshots/linux-terminal-menu.png" alt="A terminal&#x27;s context menu." width="260"></a><br><sub>A terminal's context menu.</sub></td>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-shortcuts.png"><img src="docs/screenshots/linux-shortcuts.png" alt="Settings → Shortcuts with the Linux keys." width="260"></a><br><sub>Settings → Shortcuts with the Linux keys.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="33%"><a href="docs/screenshots/linux-dashboard.png"><img src="docs/screenshots/linux-dashboard.png" alt="The Agent Dashboard with a shell waiting for an answer and Answer at hand." width="260"></a><br><sub>The Agent Dashboard with a shell waiting for an answer and Answer at hand.</sub></td>
</tr>
</table>

</details>

<details>
<summary><strong>Phone: Safari on an iPhone</strong></summary>

<table>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-first.png"><img src="docs/screenshots/phone-safari-first.png" alt="The phone page in Safari before pairing." width="170"></a><br><sub>The phone page in Safari before pairing.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-pair-code.png"><img src="docs/screenshots/phone-safari-pair-code.png" alt="The pairing code to type on the computer." width="170"></a><br><sub>The pairing code to type on the computer.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-lock-setup.png"><img src="docs/screenshots/phone-safari-lock-setup.png" alt="Setting up the app lock." width="170"></a><br><sub>Setting up the app lock.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-passkey-sheet.png"><img src="docs/screenshots/phone-safari-passkey-sheet.png" alt="Safari saves a passkey for the app lock (Face ID)." width="170"></a><br><sub>Safari saves a passkey for the app lock (Face ID).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-inbox-workspace.png"><img src="docs/screenshots/phone-safari-inbox-workspace.png" alt="The inbox by project (simulated agent)." width="170"></a><br><sub>The inbox by project (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-terminal.png"><img src="docs/screenshots/phone-safari-terminal.png" alt="A terminal session on the phone with its key bar (simulated agent)." width="170"></a><br><sub>A terminal session on the phone with its key bar (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-approval.png"><img src="docs/screenshots/phone-safari-approval.png" alt="Answering a menu prompt from the phone (simulated agent)." width="170"></a><br><sub>Answering a menu prompt from the phone (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-chat-approval.png"><img src="docs/screenshots/phone-safari-chat-approval.png" alt="Approving an agent&#x27;s command from the phone, with its risk (simulated agent)." width="170"></a><br><sub>Approving an agent's command from the phone, with its risk (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-chat.png"><img src="docs/screenshots/phone-safari-chat.png" alt="A finished chat on the phone (simulated agent)." width="170"></a><br><sub>A finished chat on the phone (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-new-session.png"><img src="docs/screenshots/phone-safari-new-session.png" alt="Starting a session from the phone (simulated agent)." width="170"></a><br><sub>Starting a session from the phone (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-settings.png"><img src="docs/screenshots/phone-safari-settings.png" alt="The phone&#x27;s settings: app lock and alerts." width="170"></a><br><sub>The phone's settings: app lock and alerts.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-locked.png"><img src="docs/screenshots/phone-safari-locked.png" alt="The page locked until Face ID." width="170"></a><br><sub>The page locked until Face ID.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-unlock-sheet.png"><img src="docs/screenshots/phone-safari-unlock-sheet.png" alt="Unlocking with the passkey." width="170"></a><br><sub>Unlocking with the passkey.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-inbox-dark.png"><img src="docs/screenshots/phone-safari-inbox-dark.png" alt="The inbox in dark mode (simulated agent)." width="170"></a><br><sub>The inbox in dark mode (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-chat-approval-dark.png"><img src="docs/screenshots/phone-safari-chat-approval-dark.png" alt="An approval in dark mode (simulated agent)." width="170"></a><br><sub>An approval in dark mode (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-terminal-dark.png"><img src="docs/screenshots/phone-safari-terminal-dark.png" alt="A terminal session in dark mode (simulated agent)." width="170"></a><br><sub>A terminal session in dark mode (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-first-vi.png"><img src="docs/screenshots/phone-safari-first-vi.png" alt="The phone page in Vietnamese before pairing." width="170"></a><br><sub>The phone page in Vietnamese before pairing.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-pair-code-vi.png"><img src="docs/screenshots/phone-safari-pair-code-vi.png" alt="The pairing code, in Vietnamese." width="170"></a><br><sub>The pairing code, in Vietnamese.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-lock-setup-vi.png"><img src="docs/screenshots/phone-safari-lock-setup-vi.png" alt="Setting up the app lock, in Vietnamese." width="170"></a><br><sub>Setting up the app lock, in Vietnamese.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-passkey-sheet-vi.png"><img src="docs/screenshots/phone-safari-passkey-sheet-vi.png" alt="Saving the passkey, in Vietnamese." width="170"></a><br><sub>Saving the passkey, in Vietnamese.</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-inbox-vi.png"><img src="docs/screenshots/phone-safari-inbox-vi.png" alt="The inbox in Vietnamese (simulated agent)." width="170"></a><br><sub>The inbox in Vietnamese (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-safari-settings-vi.png"><img src="docs/screenshots/phone-safari-settings-vi.png" alt="The phone&#x27;s settings in Vietnamese." width="170"></a><br><sub>The phone's settings in Vietnamese.</sub></td>
</tr>
</table>

</details>

<details>
<summary><strong>Phone: Chromium at iPhone and Android sizes</strong></summary>

<table>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-pair-scan.png"><img src="docs/screenshots/phone-pair-scan.png" alt="Pairing starts by scanning the code the computer shows, or pasting its link." width="170"></a><br><sub>Pairing starts by scanning the code the computer shows, or pasting its link.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-pair-name.png"><img src="docs/screenshots/phone-pair-name.png" alt="The phone names itself before the computer approves it." width="170"></a><br><sub>The phone names itself before the computer approves it.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-chat-tools.png"><img src="docs/screenshots/phone-chat-tools.png" alt="A chat on the phone lists each tool the agent ran and what it changed (simulated agent)." width="170"></a><br><sub>A chat on the phone lists each tool the agent ran and what it changed (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-chat-keyboard.png"><img src="docs/screenshots/phone-chat-keyboard.png" alt="Writing back to an agent while it waits for approval (simulated agent)." width="170"></a><br><sub>Writing back to an agent while it waits for approval (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-connection.png"><img src="docs/screenshots/phone-connection.png" alt="The connection sheet: how the phone reaches the computer and how fresh the link is." width="170"></a><br><sub>The connection sheet: how the phone reaches the computer and how fresh the link is.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-settings-2.png"><img src="docs/screenshots/phone-settings-2.png" alt="Phone settings: theme, language, text size and how the inbox sorts." width="170"></a><br><sub>Phone settings: theme, language, text size and how the inbox sorts.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-settings-3.png"><img src="docs/screenshots/phone-settings-3.png" alt="Choosing and ordering the keys of the key bar." width="170"></a><br><sub>Choosing and ordering the keys of the key bar.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-offline.png"><img src="docs/screenshots/phone-offline.png" alt="With the computer gone, the phone says so and keeps what it last saw (simulated agent)." width="170"></a><br><sub>With the computer gone, the phone says so and keeps what it last saw (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-chat-approval-vi.png"><img src="docs/screenshots/phone-chat-approval-vi.png" alt="An approval on the Vietnamese page (simulated agent)." width="170"></a><br><sub>An approval on the Vietnamese page (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-chat-tools-vi-light.png"><img src="docs/screenshots/phone-chat-tools-vi-light.png" alt="A chat on the Vietnamese page, light theme (simulated agent)." width="170"></a><br><sub>A chat on the Vietnamese page, light theme (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-connection-vi.png"><img src="docs/screenshots/phone-connection-vi.png" alt="The connection sheet in Vietnamese." width="170"></a><br><sub>The connection sheet in Vietnamese.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-offline-vi.png"><img src="docs/screenshots/phone-offline-vi.png" alt="The offline inbox in Vietnamese (simulated agent)." width="170"></a><br><sub>The offline inbox in Vietnamese (simulated agent).</sub></td>
</tr>
<tr>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-android-inbox.png"><img src="docs/screenshots/phone-android-inbox.png" alt="The inbox in Chrome on an Android-sized phone (Pixel 7) (simulated agent)." width="170"></a><br><sub>The inbox in Chrome on an Android-sized phone (Pixel 7) (simulated agent).</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-android-terminal.png"><img src="docs/screenshots/phone-android-terminal.png" alt="A terminal session on an Android-sized phone." width="170"></a><br><sub>A terminal session on an Android-sized phone.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-android-approval.png"><img src="docs/screenshots/phone-android-approval.png" alt="Answering a menu prompt on an Android-sized phone." width="170"></a><br><sub>Answering a menu prompt on an Android-sized phone.</sub></td>
<td align="center" valign="top" width="25%"><a href="docs/screenshots/phone-android-inbox-vi-light.png"><img src="docs/screenshots/phone-android-inbox-vi-light.png" alt="The inbox in Vietnamese, light theme, on an Android-sized phone (simulated agent)." width="170"></a><br><sub>The inbox in Vietnamese, light theme, on an Android-sized phone (simulated agent).</sub></td>
</tr>
</table>

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
- [Sub-sessions](docs/user-guide.md#sub-sessions) let an agent open other agent sessions, with your approval and within limits you set, and its report can list the checks it still owes you.
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
- [File links](docs/user-guide.md#terminals) turn the paths agents print, such as `src/a.ts:42`, into links: `Ctrl+click` opens the file at that line, and a right-click copies its path.
- [Browser panes](docs/user-guide.md#browser-panes-and-design-mode) include developer tools and a design mode that sends picked elements to an agent.
- The [Processes panel](docs/user-guide.md#processes-ports-and-preview) lists each session's programs and ports and stops them safely.

**Remote and mobile**
- [Remote projects](docs/user-guide.md#remote-projects-ssh), the [SSH hosts page](docs/user-guide.md#ssh-hosts) and [SSH panes in any project](docs/user-guide.md#ssh-panes-in-any-project) bring servers into the same workspace.
- [Keeping sessions running on a server](docs/user-guide.md#keeping-sessions-running-on-a-server) explains the `tmux` setup.
- [Phone access](docs/user-guide.md#phone-access) pairs a phone through Kelpie's relay, Tailscale or your local network, and explains seeing and controlling the computer's screen from it, with the two permissions a Mac asks for.

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

Kelpie supports Windows 10 1809+ and Windows 11, macOS 12+, and Linux x86-64 with glibc 2.35 or newer. Windows receives the most hands-on testing. Controlling the computer's screen from the phone works on Windows and on macOS 12.3 or later. The GitHub CLI (`gh`) is optional and enables pull-request features. Remote sessions use your own OpenSSH setup and, for persistence on the server, `tmux`.

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

Every picture comes from a script that drives the real app over the demo kit, in its own data folder and daemon endpoint, so it never touches your sessions, keys or settings. [README-media.json](docs/screenshots/README-media.json) names the script and machine behind each picture:

- Windows: [readme_shots.py](scripts/e2e/readme_shots.py) (with `--extra` for the newer scenes and `--clip-finish` for the worktree clip) and [readme_phone_win.py](scripts/e2e/readme_phone_win.py) for the phone's view of the screen.
- macOS: [mac_shots.py](scripts/e2e/mac_shots.py), and [screen_mac.py](scripts/e2e/screen_mac.py) for the Mac's screen seen from a phone.
- Linux and the phone in Chromium: [linux_shots.py](scripts/e2e/linux_shots.py) and [phone_tour.py](scripts/e2e/phone_tour.py), run on a hosted Ubuntu runner by the [readme-media workflow](.github/workflows/readme-media.yml).
- The phone in Safari: [phone_safari_sim.py](scripts/e2e/phone_safari_sim.py) `--tour` in the iOS Simulator.

The Windows tour drives the app with WebDriver. It needs a built app and daemon, `tauri-driver`, and an `msedgedriver` that matches the installed WebView2 runtime:

```bash
cd app && pnpm tauri build --debug --no-bundle && cd .. && cargo build -p kelpied
python -X utf8 scripts/e2e/readme_shots.py docs/screenshots ../kelpie-demo-projects \
  --app target/debug/kelpie.exe --native-driver /path/to/msedgedriver.exe \
  --agents --browser
```

The images are Full HD (1920 × 1080). `--agents` starts Claude Code, Codex and OMP in their terminals and Claude Code in chat view, each on one read-only task, and makes real model calls with your own sign-ins; leave out `--agents` and `--browser` for a tour without agents. Claude Code and Codex may ask once to trust the demo folder, and the tour answers yes. The agents keep their usual transcripts of these demo turns in their own folders, as any session of yours would. ChatGPT web is not signed in. The SSH hosts use documentation addresses and are never contacted. The tour writes [capture.json](docs/screenshots/capture.json) with the app version, the screenshots and every check it ran.

</details>

## How it works

Kelpie separates the window from the work. The SolidJS interface presents the workspace. The Rust daemon, `kelpied`, owns the terminals, agent processes, automations, runs and phone link, and the window talks to it over a local named pipe or Unix socket.

A terminal that is not on screen does not need to render. When you return to it, the window receives the missing output or a snapshot instead of asking the agent to start over. That separation lets an update restart the window while every agent keeps working, and it keeps the workspace responsive as sessions accumulate. It does not mean that more agents cost nothing; each agent still uses memory, CPU and tokens.

Read the [architecture](docs/architecture.md) for ownership, transport and recovery contracts, and the [performance procedure](scripts/perf/README.md) for measurement methods and budgets.

## Your tools, your data, your decisions

- Kelpie stores sessions, layouts and settings on your computer and sends no telemetry, while your agent CLIs still contact their model providers under their own accounts and policies.
- Every outside connection is one you set up yourself, including remote hosts, issue trackers, notification channels and phone access, and phone access is off by default.
- The phone link is end-to-end encrypted. You approve each phone on the computer by typing the code it shows and passing your operating system's own check, such as Windows Hello or Touch ID. Kelpie needs no account. The relay that connects a phone over the internet passes encrypted frames it cannot read, but it sees connection metadata, and the hosted phone app's code comes from that relay. The [phone access guide](docs/user-guide.md#phone-access) explains this trust model, the Tailscale alternative and how to check what the relay serves.
- Integration tokens, SSH passwords and notification tokens go to the operating system's credential store. Environment variables that you enter in agent settings are stored as plain text, so do not put secrets there or in demo projects.
- Worktree setup, finishing checks, automation commands, plugins and agent permissions all ask for your approval. Kelpie organizes the work, but judging an agent's output remains your job.

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

The phone page has scenarios of its own. The `mobile*.py`, `phone_*.py`, `takeover.py` and `screen*.py` scripts drive it in a phone-sized Chromium with a virtual passkey. [phone_safari_sim.py](scripts/e2e/phone_safari_sim.py) drives it in Mobile Safari on the iOS Simulator, with the passkey behind the simulator's Face ID. [screen_mac.py](scripts/e2e/screen_mac.py) checks a Mac's screen seen and controlled from a phone page.

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
- [Writing style](docs/writing-style.md) sets how documentation and interface text are written in English and Vietnamese.
- [Brand](docs/brand.md) presents the visual identity and Kip.
- [Performance](scripts/perf/README.md) documents reproducible measurements instead of marketing numbers.

## License and acknowledgements

Kelpie is [MIT licensed](LICENSE). The bundled Windows ConPTY binaries are Microsoft's and also MIT licensed; see the [ConPTY notes](docs/third-party-conpty.md).

Claude, Codex, Gemini, Cursor, OpenCode, GitHub Copilot and their logos are trademarks of their respective owners. Kelpie launches these tools but is not affiliated with or endorsed by them. See the [agent logo credits](docs/third-party-agent-logos.md).
