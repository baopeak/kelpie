# AI coding IDE feature survey (as of 2026-10-23)

Researcher report (5 searches, no page fetches). Tags: **[V]** vendor docs/marketing · **[I]** independent (trade press, third-party) · **[K]** researcher prior knowledge, unverified (cutoff 06/2026).
Gaps in coverage: Windsurf, Cursor 2.x (Bugbot, memories, rules), cost/usage UIs, collaboration, perf claims.

## Feature matrix

| Feature | Cursor | Windsurf | Kiro | Antigravity | VS Code+Copilot | Zed | JetBrains | Kelpie fit |
|---|---|---|---|---|---|---|---|---|
| Dedicated multi-agent window | Agents Window (3.0) [V] | Cascade panel [K] | – | Agent Manager [V,I] | Agents window 1.120; editor panel 1.129 [I] | Agent panel [K] | Air [K] | Generic — Kelpie core |
| Unified session list local/cloud/other vendors | local, worktree, cloud, SSH [V] | – | – | multi-workspace [V] | Sessions view incl. Copilot coding agent, Copilot CLI, Codex [I] | – | – | Generic (Kelpie owns PTYs) |
| Best-of-N in worktrees | `/best-of-n` [V] | – | – | – | parallel subagents 1.109 [I] | – | – | Generic: 1 prompt → N CLIs × N worktrees |
| Plan mode | `/plan`, clarifying questions [I-blog] | – | spec flow | plan artifact [V,I] | Plan agent [K] | – | – | Vendor-specific; generic plan-first template |
| Spec-driven req → design → tasks | – | – | core [V,I] | task list/plan artifacts [I] | – | – | – | Generic: markdown spec folder fed to any CLI |
| Steering / rules files | Rules [K] | Rules [K] | `.kiro/steering` [V] | – | copilot-instructions, custom agents [K,I] | `.rules` [K] | Junie guidelines [K] | Generic: edit/sync AGENTS.md / CLAUDE.md / GEMINI.md |
| Agent hooks (event-driven) | Hooks [K] | – | `.kiro/hooks/*.json` file create/save/delete, pre/post task; fire only on agent edits [V,I] | – | Hooks [K] | – | – | Generic via daemon events (file watch, PTY idle/exit) |
| Memories | [K] | Cascade memories [K] | steering ≈ memory | – | – | – | – | Vendor-specific |
| Custom agents / chat modes | Modes [K] | – | – | – | custom agents, subagents, Agent Customizations panel [I] | – | – | Kelpie presets cover it |
| Artifacts with inline feedback | – | – | – | task lists, plans, walkthroughs, screenshots, recordings; comments reach agent without stopping it [V,I] | – | – | – | Partly generic: render files agent writes |
| Browser agent / preview | Browser + Design Mode [V] | Previews [K] | – | Chrome browse/test/record [V,I] | integrated browser [K] | – | – | Agent-driven = vendor; preview pane generic (Kelpie has one) |
| Checkpoints / rollback | [K] | reverts [K] | – | – | checkpoints [K] | edit review [K] | – | Generic via git snapshot per turn |
| Diff review accept/reject | [K] | [K] | – | artifact review [V] | diffs docked beside chat 1.129 [I] | multibuffer review [K] | – | Generic via worktree diff |
| Autonomy / approval levels | – | – | – | turbo vs strict, strict = sandbox w/ restricted network [V,I] | – | – | – | Vendor CLI flags |
| Approval inbox | – | – | – | inbox for interventions e.g. terminal approval [V,I] | – | – | – | Generic: detect permission prompt in PTY / hooks |
| Cloud / background agents | cloud agents moved to Agents Window 3.0 [V] | – | – | – | Copilot coding agent; agent host runs Copilot/Claude/Codex out-of-process 1.129 [I] | – | – | Remote daemon over SSH = generic equivalent |
| PR review bot | Bugbot [K] | – | – | – | Copilot review [K] | – | – | Vendor-specific |
| MCP | [K] | [K] | [K] | [V] | yes + BYOK in Agents window [I] | yes [K] | via ACP [I] | Generic: one MCP config → every CLI |
| ACP | – | – | – | – | community plugin [I] | native, created by Zed [I] | co-developed; ACP Registry with Zed Jan 2026 [I] | Generic & strategic |
| Next-edit / inline edit | Tab [K] | Supercomplete [K] | – | – | NES [K] | edit prediction [K] | – | Out of scope (not an editor) |
| Cost / usage | n/a | n/a | n/a | free preview [I weak] | BYOK | – | AI plan covers Junie/Claude Code/Codex/Gemini [I] | Generic: aggregate from CLI transcripts |

## Category notes
- **Orchestration**: Cursor 3 Agents Window (local/worktree/cloud/SSH, IDE stays alongside) [V]; VS Code Agent Sessions mixes local + background (Copilot coding agent, Copilot CLI, Codex), parallel subagents 1.109, Agents window 1.120, agent host out-of-process 1.129 [I]; Antigravity Agent Manager = mission control + inbox over editor & browser [V,I]. Convergence: sessions list + worktree isolation. Kelpie can be the vendor-neutral version.
- **Planning/specs**: Kiro requirements → design → tasks linked to requirements [V,I]; Cursor `/plan` [I]; Antigravity plan/task artifacts reviewed before execution [V,I]. Specs = markdown → generic.
- **Context/memory**: rules/steering files per vendor; memories & indexing vendor-internal → skip. Rules sync across 23 CLIs = real pain.
- **Review**: VS Code 1.129 docks files/diffs/follow-ups beside chat [I]; Antigravity comments on artifacts folded in without stopping [V,I]; per-turn git checkpoint is vendor-agnostic.
- **Browser**: Antigravity browser agent records artifacts; Cursor Design Mode annotates elements [V]. Agent-driven browser vendor-specific (or Playwright MCP); preview pane generic.
- **MCP/ACP**: ACP created by Zed, community-governed (github.com/agentclientprotocol), JetBrains co-develops (Oct 2025), ACP Registry Jan 2026, ~50 agents implement it per one source (06/2026): Gemini CLI, Copilot, Goose, Factory Droid… ACP gives structured events, diffs, permission requests instead of screen scraping → spike.
- **Cloud**: vendor-specific; SSH remote daemon = generic equivalent.
- **Cost, collaboration, perf claims**: no usable data found.

## Ranked recommendation (researcher)
1. Approval inbox + unified sessions · 2. per-turn checkpoint/diff review · 3. best-of-N across CLIs · 4. MCP + rules sync · 5. ACP spike.

## Unresolved questions
- Windsurf state after Cognition acquisition; Cursor 2.x Bugbot/memories/rules details; JetBrains Air & Junie ACP status; VS Code/Cursor checkpoint UX; pricing meters in Cursor/Copilot/Antigravity; which of Kelpie's 23 CLIs speak ACP natively vs adapter; collaboration features; perf numbers.

## Sources
- https://cursor.com/changelog/3-0
- https://pub.towardsai.net/cursor-3-just-launched-cloud-agents-i-just-tested-it-and-its-bigger-than-it-looks-613ba4a997ce
- https://en.wikipedia.org/wiki/Google_Antigravity
- https://developers.googleblog.com/build-with-google-antigravity-our-new-agentic-development-platform/
- https://www.altamira.ai/blog/antigravity-is-agent-first-coding-tool/
- https://code.visualstudio.com/updates/v1_106
- https://code.visualstudio.com/blogs/2026/02/05/multi-agent-development
- https://visualstudiomagazine.com/articles/2026/05/13/hands-on-with-the-new-agents-window-in-vs-code-1.120.aspx
- https://visualstudiomagazine.com/articles/2026/07/16/vs-code-1129-introduces-agent-host-and-experimental-agents-window-editor.aspx
- https://zed.dev/blog/jetbrains-on-acp · https://zed.dev/acp · https://zed.dev/acp/agent/junie
- https://blog.jetbrains.com/ja/blog/2026/02/19/acp-agent-client-protocol-jetbrains-ide/
- https://www.morphllm.com/agent-client-protocol
- https://kiro.dev/docs/crew/capabilities/steering/ · https://kiro.dev/docs/hooks/index · https://kiro.dev/docs/how-kiro-works/
- https://noqta.tn/en/blog/kiro-aws-agentic-ide-spec-driven-development-guide-2026
