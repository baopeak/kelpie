// Searchable index of settings: one entry per row of the settings page. Drives the page's
// navigation, its search box, and "/" results in the global search.
import { msg, translated } from "./i18n";

export type SettingsSection = "agents" | "agent-setup" | "accounts" | "integrations" | "automations" | "terminal" | "layouts" | "remote" | "mobile" | "appearance" | "notifications" | "usage" | "commands" | "shortcuts" | "daemon" | "plugins" | "voice" | "about";

export const settingsSections: { id: SettingsSection; label: string; group: string }[] = [
  { id: "agents", label: msg("Agents"), group: msg("Workflow") },
  { id: "agent-setup", label: msg("Agent setup"), group: msg("Workflow") },
  { id: "accounts", label: msg("Accounts"), group: msg("Workflow") },
  { id: "integrations", label: msg("Integrations"), group: msg("Workflow") },
  { id: "remote", label: msg("Remote (SSH)"), group: msg("Workflow") },
  { id: "mobile", label: msg("Mobile"), group: msg("Workflow") },
  { id: "commands", label: msg("Quick Commands"), group: msg("Workflow") },
  { id: "automations", label: msg("Automations"), group: msg("Workflow") },
  { id: "voice", label: msg("Voice"), group: msg("Workflow") },
  { id: "terminal", label: msg("Terminal"), group: msg("Interface") },
  { id: "layouts", label: msg("Layouts"), group: msg("Interface") },
  { id: "appearance", label: msg("Appearance"), group: msg("Interface") },
  { id: "shortcuts", label: msg("Shortcuts"), group: msg("Interface") },
  { id: "notifications", label: msg("Notifications"), group: msg("Interface") },
  { id: "plugins", label: msg("Plugins"), group: msg("System") },
  { id: "usage", label: msg("Usage & Resources"), group: msg("System") },
  { id: "daemon", label: msg("Session Daemon"), group: msg("System") },
  { id: "about", label: msg("About"), group: msg("System") },
];

export interface SettingEntry {
  id: string;
  section: SettingsSection;
  label: string;
  description: string;
  keywords: string;
}

export const settingsCatalog: SettingEntry[] = [
  { id: "agent-permissions", section: "agents", label: msg("Permissions"), description: msg("Manual: agents ask before acting, as they normally do. Yolo: start them with their skip-permission flags (agents with their own permission choice below keep it)."), keywords: "yolo dangerously skip bypass approval permission auto" },
  { id: "agent-list", section: "agents", label: msg("Agent CLIs"), description: msg("Agents found on this computer. Shown ones appear in launchers and the new-session menu."), keywords: "gemini cursor opencode copilot aider grok amp qwen droid kimi crush cline kiro devin goose installed enable hide" },
  { id: "ai-agent", section: "agents", label: msg("Agent for AI actions"), description: msg("Writes commit messages and pull request descriptions (runs once, without a terminal)."), keywords: "generate commit message pull request ai" },
  { id: "auto-worktree", section: "agents", label: msg("Worktree for new agent sessions"), description: msg("In a git folder, new agent sessions in the New session dialog can start in their own worktree, named from the first prompt. Ask: a box you tick per session. Always: ticked unless you untick it. Never: not offered."), keywords: "worktree isolate branch new session agent automatic auto parallel" },
  { id: "hibernate", section: "agents", label: msg("Hibernate idle agents"), description: msg("Stop agents that finished and sat unseen in a background project; they resume their conversation when you open it again."), keywords: "sleep memory idle pause resume hibernation" },
  { id: "resume-sessions", section: "agents", label: msg("Resume after a restart"), description: msg("When the session daemon stops while sessions run (computer restart, update, crash), bring them back: when you open them, all at once, or only with their Resume button. Agents continue the same conversation."), keywords: "restore reopen reboot continue session resume startup" },
  { id: "agent-setup", section: "agent-setup", label: msg("MCP servers, instructions and skills"), description: msg("One page for what your agents are told and can use: their MCP servers (switch on and off, copy between agents), instruction files such as AGENTS.md, and their skills, commands and subagents. Every change shows a preview and keeps a backup."), keywords: "mcp server tools claude.md agents.md gemini.md steering copilot instructions skills commands subagents registry acp find agents context" },
  { id: "accounts", section: "accounts", label: msg("Claude Code and Codex accounts"), description: msg("Sign in with several accounts. New sessions use the active account; running ones keep theirs. Each account has its own sign-in, settings and history."), keywords: "account login sign in profile switch subscription team personal work CLAUDE_CONFIG_DIR CODEX_HOME" },
  { id: "integration-linear", section: "integrations", label: msg("Linear"), description: msg("A personal API key: your assigned issues in the Issues panel, start agents or worktrees from them, move them between states."), keywords: "linear issues tracker api key ticket" },
  { id: "integration-jira", section: "integrations", label: msg("Jira"), description: msg("Jira Cloud (site, e-mail and API token) or Server / Data Center (personal access token): your open issues, transitions."), keywords: "jira atlassian issues tracker ticket jql" },
  { id: "integration-gitlab", section: "integrations", label: msg("GitLab"), description: msg("Merge requests and pipelines for projects on GitLab (public ones work without a token), creating merge requests and your issues. Add self-hosted hosts here."), keywords: "gitlab merge request mr pipeline ci self-hosted token" },
  { id: "ssh-config", section: "remote", label: msg("SSH config file"), description: msg("Remote projects use your OpenSSH client with this config file (empty: ssh's default, ~/.ssh/config). Hosts, users, ports, keys and jump hosts come from it."), keywords: "ssh remote server host config identity key proxyjump" },
  { id: "remote-projects", section: "remote", label: msg("Remote projects"), description: msg("Projects on other machines: terminals and agents run there over SSH (kept alive in tmux when it is installed); files, search and git work through ssh too."), keywords: "ssh remote server cloud vm tmux host" },
  { id: "mobile-access", section: "mobile", label: msg("Mobile access"), description: msg("A web app on your network for your phone: every session, what needs you, a session's screen and (if allowed) replies. Off until you turn it on; each phone is paired and approved here."), keywords: "phone mobile companion remote lan wifi pwa tablet" },
  { id: "mobile-network", section: "mobile", label: msg("Network address"), description: msg("The address and port phones connect to (a private or Tailscale address; never every interface)."), keywords: "ip port interface lan tailscale" },
  { id: "mobile-tls", section: "mobile", label: msg("HTTPS certificate"), description: msg("Serve HTTPS with your certificate and key (PEM), e.g. from `tailscale cert`: encrypted, and the phone can install the app."), keywords: "https tls ssl certificate tailscale secure" },
  { id: "mobile-tailscale", section: "mobile", label: msg("Reach this computer with Tailscale"), description: msg("Publishes the phone page at an HTTPS address only your own Tailscale devices can reach, with `tailscale serve`, so that the phone works away from home and can install the app. Asks you to verify first."), keywords: "tailscale https serve remote anywhere install pwa secure" },
  { id: "mobile-pair", section: "mobile", label: msg("Pair a phone"), description: msg("Shows a one-time QR code (2 minutes). After scanning, approve the phone here."), keywords: "qr pair connect device phone" },
  { id: "mobile-devices", section: "mobile", label: msg("Paired devices"), description: msg("Phones that may connect. Typing is allowed per device; unpairing disconnects it at once."), keywords: "device revoke unpair permission" },
  { id: "saved-layouts", section: "layouts", label: msg("Saved layouts"), description: msg("Pane arrangements saved from a tab (Layout menu → Save this layout), with what each pane runs. Open one from the Layout menu or a project's start page."), keywords: "grid rows columns split arrangement template workspace" },
  { id: "plugins", section: "plugins", label: msg("Installed plugins"), description: msg("Plugins add agents, palette commands, quick commands, terminal themes, layouts and status-bar items. A plugin does nothing until you review what it runs and enable it; editing it turns it off until you review it again."), keywords: "plugin extension addon extend manifest kelpie-plugin.json" },
  { id: "plugin-install-git", section: "plugins", label: msg("Install from a Git repository"), description: msg("Clones the repository (https, ssh or git@) into the plugins folder; it stays off until you enable it."), keywords: "git clone url github install plugin" },
  { id: "plugin-create", section: "plugins", label: msg("Create a plugin"), description: msg("A starter kelpie-plugin.json with a command, a quick command and a status item to edit."), keywords: "new scaffold author develop plugin template" },
  { id: "voice-models", section: "voice", label: msg("Speech models"), description: msg("Dictation runs on this computer: download a model once, then speak into any text field or terminal. Vietnamese has its own fast model; Whisper models understand many languages."), keywords: "voice dictation speech to text microphone stt whisper vietnamese offline transcribe" },
  { id: "voice-language", section: "voice", label: msg("Language"), description: msg("What you speak, for Whisper models (Detect automatically works but is slower and can guess wrong on short phrases)."), keywords: "language english vietnamese dictation" },
  { id: "voice-mic", section: "voice", label: msg("Microphone"), description: msg("Input device for dictation."), keywords: "microphone input device audio headset" },
  { id: "voice-try", section: "voice", label: msg("Try it"), description: msg("Click the microphone (or press the Dictate shortcut), speak, then click it again: the words appear at the cursor. Esc cancels."), keywords: "test dictation shortcut" },
  { id: "turn-checkpoints", section: "agents", label: msg("Turn snapshots"), description: msg("When an agent finishes a turn, keep a snapshot of its folder (git folders on this computer) so you can see what each turn changed and undo one. Snapshots stay in the repository under refs/kelpie and never touch your index, branch or files (other git tools can see that ref, and a mirror push would send it); a repository where they are slow turns them off by itself."), keywords: "checkpoint undo revert rewind turn snapshot diff history timeline artifacts" },
  { id: "agents-scripting", section: "agents", label: msg("Agents can control other sessions"), description: msg("Programs running in a session may list, read, type into and start other sessions through `kelpied` (terminal send, run create…). Off: each session can only report on and act on itself. Settings, automations and phone access are never available to them."), keywords: "security scripting kelpied cli terminal send permissions sandbox token" },
  { id: "prioritize-visible", section: "agents", label: msg("Prioritize visible terminals"), description: msg("When the computer is busy, terminals on screen get the CPU first, so typing into an agent stays quick while agents in the background build and test. Windows only."), keywords: "cpu priority slow lag typing input latency performance background" },
  { id: "keep-awake", section: "agents", label: msg("Keep computer awake"), description: msg("Prevent idle sleep while an agent is working, or always (the display may still turn off)."), keywords: "caffeinate sleep power awake" },
  { id: "automations", section: "automations", label: msg("Automations"), description: msg("Start an agent, type a saved prompt or run a command on a schedule or when a file changes, an agent finishes a turn, a commit is made, pull request checks fail or a worktree is finished. They run from the session daemon; file, pull request and finish triggers need this window open."), keywords: "schedule cron daily hourly weekly recurring automation trigger event file glob commit hook shell command dry run" },
  { id: "terminal-theme", section: "terminal", label: msg("Colour theme"), description: msg("Terminal colours; Default follows the app theme."), keywords: "dracula nord solarized gruvbox one dark tokyo catppuccin github palette colors" },
  { id: "localhost-browser", section: "terminal", label: msg("Open localhost links in a browser pane"), description: msg("Ctrl/Cmd+click on a local dev-server address (localhost, 127.0.0.1) in a terminal opens it in a browser pane next to the terminal instead of your browser."), keywords: "link url dev server preview browser pane localhost" },
  { id: "floating-terminal", section: "terminal", label: msg("Floating terminal"), description: msg("A global terminal in your home folder, one shortcut away from any project."), keywords: "quake dropdown global scratch float" },
  { id: "language", section: "appearance", label: msg("Language"), description: msg("Language of Kelpie's interface. System follows your computer."), keywords: "language english vietnamese tiếng việt ngôn ngữ locale translation" },
  { id: "mascot", section: "appearance", label: msg("Kip, the mascot"), description: msg("Kip peeks over the sidebar footer and shows how your agents are doing; it also greets you on empty screens."), keywords: "mascot dog kip kelpie companion character" },
  { id: "mascot-motion", section: "appearance", label: msg("Kip animations"), description: msg("Small, light animations when agents work, need you or finish. Follow system: only when the operating system's animation effects are on."), keywords: "mascot animation motion effects reduce" },
  { id: "word-wrap", section: "appearance", label: msg("Editor word wrap"), description: msg("Wrap long lines in the file editor (Alt+Z toggles it)."), keywords: "soft wrap lines editor" },
  { id: "notification-sound", section: "notifications", label: msg("Notification sound"), description: msg("Play a short chime with desktop notifications."), keywords: "sound chime audio beep" },
  { id: "push-ntfy", section: "notifications", label: msg("ntfy"), description: msg("Send notifications to the ntfy app on your phone, also with Kelpie's window closed, under a random topic only you know."), keywords: "phone push mobile notification ntfy topic away" },
  { id: "push-telegram", section: "notifications", label: msg("Telegram"), description: msg("Send notifications to a Telegram chat through a bot of your own."), keywords: "phone push bot chat botfather notification" },
  { id: "push-webhook", section: "notifications", label: msg("Webhook"), description: msg("Post each notification as JSON to an address you choose (Slack, Home Assistant, your own server)."), keywords: "http post json slack integration notification" },
  { id: "push-events", section: "notifications", label: msg("Events sent off this computer"), description: msg("Which events reach your phone or webhook."), keywords: "needs you done failed automation approval cost cap filter" },
  { id: "push-filters", section: "notifications", label: msg("Turn length and presence"), description: msg("Skip short turns, and send only while Kelpie's window is not in focus or the computer is idle."), keywords: "away idle focus duration seconds filter quiet" },
  { id: "push-link", section: "notifications", label: msg("Include a link to the phone page"), description: msg("Adds a link to the session in the phone app (needs phone access on). It contains this computer's address on your network."), keywords: "link url address lan ip phone mobile privacy" },
  { id: "push-screen", section: "notifications", label: msg("Include the end of the screen"), description: msg("Adds the last lines of the terminal to each message. Off by default: that text leaves this computer."), keywords: "screen text terminal output privacy" },
  { id: "agent-claude", section: "agents", label: msg("Claude Code"), description: msg("Arguments, permission mode and environment for new Claude Code sessions."), keywords: "model args flags profile permission env claude" },
  { id: "agent-codex", section: "agents", label: msg("Codex"), description: msg("Arguments, approval policy and environment for new Codex sessions."), keywords: "model args flags profile approval env codex" },
  { id: "agent-omp", section: "agents", label: msg("OMP"), description: msg("Arguments and environment for new OMP sessions."), keywords: "model args flags profile env omp pi" },
  { id: "quick-commands", section: "commands", label: msg("Quick commands"), description: msg("Saved commands and prompts, run from search or the new session menu."), keywords: "snippet saved command run prompt" },
  { id: "font-family", section: "terminal", label: msg("Terminal font"), description: msg("Font family for terminals; include a Nerd Font for agent icons."), keywords: "font family nerd jetbrains" },
  { id: "font-size", section: "terminal", label: msg("Font size and line height"), description: msg("Terminal text size and line spacing."), keywords: "zoom size spacing" },
  { id: "scrollback", section: "terminal", label: msg("Scrollback lines"), description: msg("Lines each terminal keeps above the screen."), keywords: "history buffer" },
  { id: "renderer", section: "terminal", label: msg("Renderer"), description: msg("GPU (WebGL) is fastest; DOM is a fallback for unusual GPUs."), keywords: "gpu webgl canvas dom" },
  { id: "cursor-blink", section: "terminal", label: msg("Cursor blink"), description: msg("Blink the terminal cursor."), keywords: "caret" },
  { id: "shell", section: "terminal", label: msg("Default shell"), description: msg("Command for new terminals; empty uses the system shell."), keywords: "pwsh bash zsh shell" },
  { id: "theme", section: "appearance", label: msg("Theme"), description: msg("Dark or light interface, terminals and editors."), keywords: "dark light color colour" },
  { id: "density", section: "appearance", label: msg("Density"), description: msg("Compact fits more sessions in the sidebar."), keywords: "spacing compact comfortable" },
  { id: "ui-scale", section: "appearance", label: msg("Interface text size"), description: msg("Size of sidebar, panels and menus (terminals have their own size)."), keywords: "ui zoom text scale" },
  { id: "shortcuts", section: "shortcuts", label: msg("Keyboard shortcuts"), description: msg("Change or remove the key for any command."), keywords: "keybinding hotkey chord key" },
  { id: "notifications", section: "notifications", label: msg("Desktop notifications"), description: msg("Notify when an agent finishes or needs you while Kelpie is in the background."), keywords: "alert toast done" },
  { id: "shortcut-hints", section: "notifications", label: msg("Explain shortcuts kept from the terminal"), description: msg("Show a one-time note when Kelpie takes a key the terminal would have received."), keywords: "hint toast intercept" },
  { id: "tips", section: "notifications", label: msg("Tips"), description: msg("Occasional tips about Kelpie's features, one at a time."), keywords: "onboarding help popover" },
  { id: "usage-meters", section: "usage", label: msg("Claude / Codex usage meters"), description: msg("Plan usage (5 h and weekly windows) in the status bar, read with the agents' own sign-in."), keywords: "rate limit quota tokens subscription" },
  { id: "cost-cap", section: "usage", label: msg("Cost cap per session"), description: msg("A session whose cost passes this many US dollars gets a red chip and an inbox entry. Claude costs are estimates at API prices; 0 turns the cap off."), keywords: "budget limit spend money dollars price tokens usd" },
  { id: "cost-cap-esc", section: "usage", label: msg("Press Esc when a session passes the cap"), description: msg("Interrupts a working session the first time it passes the cap. Only for sessions whose numbers come from their own exact transcript."), keywords: "interrupt stop budget escape" },
  { id: "memory-meter", section: "usage", label: msg("Memory meter"), description: msg("Memory used by Kelpie, its session daemon and the sessions."), keywords: "ram resources" },
  { id: "daemon-info", section: "daemon", label: msg("Daemon status"), description: msg("The process that keeps sessions running when the window closes."), keywords: "kelpied pid endpoint version" },
  { id: "daemon-data", section: "daemon", label: msg("Data directory and logs"), description: msg("Where sessions, snapshots and logs are stored."), keywords: "path folder log" },
  { id: "config-transfer", section: "daemon", label: msg("Export and import settings"), description: msg("Settings, quick commands, layouts, automations, SSH hosts, the plugin list and agent arguments in a JSON file, to move to another computer. Saved passwords and keys stay here, and values that look like credentials are left out or masked; check the file before you share it."), keywords: "export import backup restore migrate transfer move json config configuration settings" },
  { id: "daemon-stop", section: "daemon", label: msg("Stop daemon and all sessions"), description: msg("Ends every running session."), keywords: "kill quit kelpied shutdown" },
  { id: "update-checks", section: "about", label: msg("Check for updates"), description: msg("Look for a newer Kelpie on GitHub Releases 30 s after start and every 6 hours. Installing restarts the app only: running sessions keep going."), keywords: "update upgrade release version auto-update new" },
  { id: "version", section: "about", label: msg("Version"), description: msg("Kelpie build and platform."), keywords: "about build" },
];

export const catalogEntry = (id: string) => settingsCatalog.find((e) => e.id === id);

/** Every word of `query` appears in the entry's label, description or keywords. */
export function entryMatches(e: SettingEntry, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hay = `${e.label} ${e.description} ${e.keywords} ${translated(e.label) ?? ""} ${translated(e.description) ?? ""}`.toLowerCase();
  return words.every((w) => hay.includes(w));
}
