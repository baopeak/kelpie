# Feature inventory — Kelpie 0.1.15 (origin/master af17880)

Source: two read-only audits of `app/src`, `crates/kelpied`, `scripts/e2e`, docs. UG = `docs/user-guide.md`.

## Feature areas (all documented in UG unless marked)

| Area | Features |
|---|---|
| Workspace | projects + worktrees in sidebar, tabs, splits, grid presets, tile all, broadcast input, saved layouts, back/forward, search everything, dashboard, quick answer, floating terminal |
| Agents | 23 CLI presets (Claude Code, Codex, OMP + 20), ACP chat agents, custom agents from ACP registry, Yolo/Manual, hand-off, hibernation, keep awake, resume after restart, several accounts, usage-limit strip |
| Chat view | same conversation terminal ⇄ chat, tool rows, approvals, `/ @ $` pickers, model/mode chips, agent map, saved replies, pictures, queue/send-now |
| Sub-sessions | agents open other agent sessions (MCP tools / `kelpied sub`), approval cards, tree in sidebar, limits |
| Review & Git | Changes panel (push/pull/sync, branch switch, history, stashes), AI commit message, PRs (gh) / GitLab MRs, review notes, review queue, Review with another agent, Finish (WIP commit, merge, approved checks), turns + undo |
| Parallel work | worktrees + `.kelpie.yaml` (setup/tabs/copy/link/check), board, Runs (DAG tasks, approvals, worktrees), Try with several agents + Compare |
| Plan tab | plans/specs/.kiro/docs/plans checklists, tick, Run with…, New spec |
| Files | editor, CSV/TSV, PDF, notebooks, Mermaid, markdown, find in folder, open in external editor |
| Browser & processes | browser pane, design mode, agent web pages auto-tab, Processes panel (tree, ports, guarded kill) |
| Remote | SSH remote projects, SSH hosts page (Termius-like: keys, known hosts, forwards, files/SFTP queue, multi-host command), SSH panes in any project, tmux persistence, root-user setup |
| Phone | hosted app via relay, Tailscale, LAN browser; E2E encrypted, OS-verified pairing, app lock; read/type, chat from phone, risk-labelled approvals, browser takeover + screen (Windows) |
| Notifications | inbox, desktop notifications, ntfy/Telegram/webhook, Web Push |
| Usage | plan meters, context ring, cost page, per-session cost cap |
| Config | Agent setup (MCP, instructions, skills, ACP registry), plugins, quick commands, automations (schedule + events, approved commands), shortcuts, export/import settings, EN/VI, voice dictation (offline) |
| Platform | Windows, macOS, Linux (AppImage/.deb); updates never stop sessions; closing Kelpie stops the daemon by default (*Keep it running in the background* optional) |

## Gaps / inaccuracies found

- UG: Kip mascot + motion setting, tips (4 one-time popovers, show again), Welcome screen, launcher SSH card / Run-on picker, hover hints, status-bar switch-ms readout, daemon-older notice, palette-only commands (*Answer Waiting Session…*, *Clear Terminal Buffer*), `!` scope in search — undocumented or partial.
- UG: phone chat send undocumented in Phone access; CLI list omits `ping`, `stop`, `sub`, `review`, `mcp`; run JSON omits per-task `host`; sub-session branch is `kelpie/sub-<slug>-<tag>`, not `kelpie/sub-<name>`; open-in-editor menu shows at most 4 editors.
- README: claims sessions survive closing the window (false by default since close stops daemon); no Linux; no sub-sessions, processes, SFTP, hosted phone app, export/import.
- Demo kit: `capture.json` stale (0.1.5) with fields the script never writes; orphan `history.png`; fixtures lack `.kelpie.yaml` setup/tabs/copy, `plans/`, run JSON, notebook, plugin, event automation; no shots for welcome/Kip, runs, processes, finish, hosts, try, plugins, notebook.

## Not demoable safely in a scripted capture

Cost page and Agent setup read the real user's transcripts/agent configs; Mobile pairing needs a phone; sub-sessions and turns need paid agent turns. Covered in demo scripts as live steps instead.
