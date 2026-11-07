# pstack philosophy (research)

Date 2026-11-07. Read-only research. Verified = read from primary source via `gh api` on cursor/plugins repo.

## 1. What "pstack" is (ranked)

1. **pstack by Lauren Tan ("poteto")** - VERIFIED, best match. Skill/plugin pack for coding agents, authored by a Cursor engineer (ex-Meta, Netflix; React Compiler core). Canonical source: https://github.com/cursor/plugins/tree/main/pstack (MIT, per SourceForge mirror; unverified license in repo). Installs in Claude Code: `/plugin marketplace add cursor/plugins` then `/plugin install pstack@cursor-plugins` (commands become `/pstack:poteto-mode`). Contents (listed via API): ~29 skills, 24 `principle-*` skills, 23 playbooks, agents `poteto-agent`, `comment-sicko`, automations dir, `docs/guide`.
   - Ports: https://github.com/painhardcore/pstack (Codex/Claude Code/OpenCode adapter, syncs from pinned upstream), https://github.com/kashyab12/uni-pstack (portable Codex/CC port).
   - Explainers: https://www.theneuron.ai/explainer-articles/pstack-explained-lauren-tans-system-for-trustworthy-ai-agents/ (403 for me; summary via search only), https://youmind.com/tr-TR/landing/x-viral-articles/pstack-guide-part-two (X article mirror, read).
2. **BhaskarManam/pstack** - https://github.com/BhaskarManam/pstack. "Product-building" pack, 19 skills (vision, PRD, design brief), explicitly inspired by gstack. Small, not read in depth. Only matches if user means "product-thinking stack". UNVERIFIED depth.
3. Unix `pstack` (stack-trace printer) - disambiguation only; irrelevant.

No evidence pstack is a fork of gstack; Tan's pstack is independent (shared tagline "Fork it. Improve it. Make it yours." mirrors gstack's README, likely homage - inference). No evidence found of mrgoonie/agentkit.best "pstack" (searched indirectly only; UNVERIFIED, not exhaustively checked; Vietnamese community sources not checked).

Caveat: context phrase "cho phần mềm" fits either. Recommend treating Tan's pstack as primary, gstack as comparison.

## 2. pstack philosophy (verified from README + skills)

- Anti-slop: "if you want to go fast, go deep first." Goal is NOT max LOC; write less, higher-quality code.
- **"Fearless parallelism"**: go deep and trust one agent to produce verifiable code, then run many agents in parallel with confidence. Verification is what makes parallelism safe.
- Single entry point `/poteto-mode`: reads request, picks a playbook, invokes other skills. Others are situational. `/poteto-help` routes when unsure. `/setup-pstack` picks models/reasoning budget.
- Multi-model by design: code delegates to one model family, hardest changes/prose/judgment to another; cross-family judges so no model grades its own work.
- Two agent failure modes (guide): underspecified intent, missing context. Both fixed by better context, not longer prompts.
- "Don't believe in abstract planning": plans give illusion of progress; resolve unknowns by prototyping. Be willing to discard a design.
- Indirect prompting: make the agent restate the problem in its own words (`/in-your-own-words`) to expose misunderstanding early.
- Readme-driven design: write caller-side usage/API first, work backward.
- "Measure many times, cut once": don't accept first design; generate alternatives, test empirically.

## 3. Roles/personas

Not CEO/QA personas. Instead **capability skills + modes**: `how` (runtime mechanics), `why` (history: git, PRs, tickets, chat), `recall` (past conversations), `blast-radius`, `architect`, `interrogate` (multi-model adversarial), `teach`, `sharpen`, `correct`, `bro`, `unslop`, `technical-writing` (Diataxis), `no-comments`, `ketchup` (catch-up on changes), `show-me-your-work` (decision trail), `create/maintain-verification-skill` (per-product "how to drive and verify this app"), `statechart`, `benchmark-checklist`, `tdd`. Agents: `poteto-agent` (worker), `comment-sicko`.

## 4. Workflow stages (guide + poteto-mode)

1. Restate goal: first message of every task states goal, problem, smallest expected change; no waiting for confirmation. Restate goal in every subagent brief and PR `## Why`.
2. Understand: recall + `how` + `why` (parallel exploration agents on complex subsystems).
3. Name the data shape first (principle-model-the-domain), then design via `architect` (parallel candidate sketches from different model families, red-flag catalog, cross-model scoring, implement against sketch, discard if forced casts/`any` needed).
4. Prototype open "which approach" forks instead of asking the human: if the answer is observable by running something, it's not the human's to answer. Ask only for true product/preference calls.
5. Multi-phase plan -> small self-contained reviewable PRs, each with verification; plan validated by script; plans deleted when done.
6. Execute with verification, then babysit PR (conflicts, review threads, CI), then shipping.

## 5. Decide what to build / product taste

- `principle-experience-first`: delight over implementation convenience; every feature/control justified; ship fewer polished features; prototype in throwaway HTML before production; sweat transitions, alignment, error states; tighten the core loop; "user" includes next maintainer.
- `principle-laziness-protocol` / `subtract-before-you-add`: bias to deletion, smallest change, remove dead weight first.
- `attack-the-premise`: two fixes sharing one premise failed same gate -> write the premise down, test it directly.
- `never-block-on-the-human`: reversible -> proceed then present; irreversible (force-push, delete prod data, external messages) -> confirm. Product direction from human, execution doesn't block.
- `outcome-oriented-execution`: for planned migrations, converge on end-state; planned scoped intermediate breakage OK, no throwaway compat code.
- `minimize-reader-load`, `foundational-thinking`, `redesign-from-first-principles`, `migrate-callers-then-delete-legacy-apis`, `type-system-discipline`, `make-operations-idempotent`, `explain-the-number`, `encode-lessons-in-structure` (a lint/script beats a repeated instruction), `build-the-lever` (codemod/script/rerunnable check instead of hand work).

## 6. Review / QA

- `principle-prove-it-works`: verify the real artifact (run feature, read actual value), not proxy/self-report/"it compiles"; when verification fails suspect the observation method first; prefer a deterministic script whose output a reviewer can rerun.
- Per-product verification skill (`create-verification-skill`, `control-ui`/`control-cli` from cursor-team-kit): drive app, screenshots/video, timing/layout measurements. Bug fix = reproduce first on same surface, fix, show runtime evidence.
- Independent verifier per PR on a different model family; verdict tied to one head SHA; green CI is input not verdict.
- Bugbot/security-review comments: skeptical triage fix/dismiss/ask; dismiss noise with a concrete reason.
- `benchmark-checklist` before reporting any perf number; `hillclimb` playbook = hypothesis loop, before/after, one commit per accepted win.
- Release gate in the port: `make check` (frontmatter, links, manifests) - repo hygiene, not product QA.

## 7. Shipping

- Shipping playbook: "green is not safe." Independent per-PR verdict first; land only the contiguous verified run bottom-up from the stack root. Playbooks also: babysit, opening-a-pr, pause-safely, session-pickup, worktree-cleanup, autonomous-run ("I'm going to bed, land the stack").
- Long/autonomous runs leave a decision trail (`show-me-your-work`), local by default, committed when stakes need audit.

## 8. Multi-agent structure (most relevant)

- `swarm`: Frame (done predicate + report shape) -> Fan out (all workers in one message, background, standalone briefs) -> Aggregate (drop results lacking required SHAs/method, respawn once; a gap is not a pass) -> Report one table (PASS/ISSUES/BLOCKED + evidence). Shapes: partition slices, race N identical, mixed.
- `arena`: N candidates same prompt, rubric (3-6 criteria) hidden from candidates, each in own worktree, cross-family judge, pick maintainable base, graft 1-2 best parts from losers, verify. Convergence = strong signal; wild divergence = under-specified frame, reframe not average.
- `orchestrate` playbook (standing multi-day project, dozens-hundreds of subagents, few human turns):
  - Coordinator never writes code; state in a tracker (`notes.md`).
  - One owner per PR/branch = sole writer. Fresh worker per independent task; kickoff self-contained (upstream reports pasted in full; workers cannot see siblings).
  - Max 3 levels (coordinator, sub-coordinators, workers); ~10 workers in flight per coordinator, start next as each finishes.
  - Start with ONE unit end-to-end before fanning out.
  - Each kickoff names evidence of done (command + expected output), writable paths, forbidden actions. If you can't name evidence, unit isn't scoped.
  - Read diff/artifact not summary; check live PR state not worker's last report; account for every worker as done/replaced/absorbed.
  - Stall detection by pushes/PR activity not status text; retry by cause (smaller scope after context limit, same kickoff after disconnect, different model after tool error); drop unit after 2 retries.
  - Never message a worker just to check on it (restarts idle agent). Fork when new work needs a worker's built state.
  - Ask the human only for real decision/blocker; mechanics, flakes, triage never reach them.
- `principle-separate-before-serializing-shared-state`: give each actor own file/branch/worktree; merge at read/report boundary; serialize structurally (locks, exclusive ownership), not by instruction.
- `principle-guard-the-context-window`: route bulk to subagents, keep summaries in main thread.
- `principle-sequence-verifiable-units`: each unit ends verifiable, check before next; stack PRs so sequence proves itself (failing test first).

## 9. gstack (Garry Tan) summary, for comparison

Source: https://github.com/garrytan/gstack (read via fetch, summarised by tool; MIT).
- Metaphor: virtual **company** - slash commands are roles: `/office-hours` (reframe problem, design doc), `/plan-ceo-review` (challenge scope), `/plan-eng-review`, `/plan-design-review`, `/autoplan`, `/review`, `/qa` (real browser; reproduce+fix) / `/qa-only`, `/cso` (security), `/ship`, `/land-and-deploy`, `/canary`, `/retro`, `/careful` `/freeze` `/guard`.
- Sprint: Think -> Plan -> Build -> Review -> Test -> Ship -> Reflect; each skill's output feeds the next (design doc -> plan -> test plan -> ship).
- Principles: structured roles beat blank prompt; push back on framing before code; narrow wedge (smallest useful version); review targets prod failures not CI status; safe-by-default for destructive ops; opt-in telemetry; forkable.
- Parallelism: author claims "ten sprints at once"; `/captain` = one controller, worker "lanes" report ready and never merge, controller merges via trains, relays approvals, digest; `/pair-agent` shares browser with scoped tokens/attribution; `/runaway-stopper` halts runaway agents.

### pstack vs gstack
| Axis | pstack | gstack |
|---|---|---|
| Frame | engineering rigor, anti-slop, verification | product company roles, sprint |
| Entry | one `/poteto-mode` router + playbooks | many role commands in a pipeline |
| Product thinking | experience-first principle, prototype over debate | CEO/office-hours scope challenge up front |
| Parallelism | owner-per-PR, independent cross-model verifier, swarm/arena | captain controller, lanes, merge trains |
| Human role | async supervisor; asks only for irreversible/product calls | approves at role gates |
Shared: worker never merges own work (lane / owner+verifier), controller digests, destructive-action guards, cross-model second opinions.

## 10. Limitations

- Did not read all 24 principles / 23 playbooks / guide in full; read poteto-mode, swarm, arena, orchestrate, and 9 principles.
- Neuron article 403; Lauren Tan's X posts not read; Vietnamese community/mrgoonie/agentkit.best not checked; BhaskarManam repo not read.
- Model names in README (opus 5.5, grok) are Cursor-specific defaults; not applicable to Kelpie.

## 11. Principles applicable to designing a multi-session orchestration feature in a desktop coding-agent workbench

- One writer per branch/worktree: UI binds each session to an owned worktree+branch; show owner badge; block second writer (matches Kelpie's "no two writers per conversation" rule).
- Separate before serializing: per-session isolated state/worktree by default; shared state only via structural locks, not conventions.
- Coordinator never edits code: orchestrator session = read/spawn/verify only; keep program state in a visible tracker (goal, units, status) rather than chat history.
- Brief = contract: spawn UI should require goal, writable paths, forbidden actions, and "evidence of done" (command + expected output); refuse to launch a unit with no evidence.
- Start with one unit: offer "run first unit, then fan out" as the default for batch spawns.
- Fan-out in one action, aggregate to one table: PASS/ISSUES/BLOCKED with evidence per session; missing evidence = gap, never pass; account for every spawned session (done/replaced/absorbed).
- Independent verifier per unit, different agent/model, verdict pinned to a head SHA; invalidate verdict on new push; green CI is input not verdict.
- Show live truth, not worker self-report: derive state from git/PR/process activity (pushes, diff, CI) rather than status text; stall detection by activity.
- Retry by cause (smaller scope vs same kickoff vs different agent), cap at 2, then surface "drop and replan".
- Don't poke idle sessions to ask status (restarts them); read artifacts instead. Fork-from-session when new work needs built state.
- Bound the hierarchy: max 3 levels, ~10 live workers per coordinator, start next when one finishes (queue/concurrency cap in UI).
- Never block on the human: auto-handle reversible mechanics (rebase, flaky CI, retries); interrupt only for irreversible actions or real product calls; batch asynchronous "needs you" inbox with a reason for each.
- Pause-safely / session-pickup / worktree-cleanup as first-class: resume summary on reopen, clean worktrees and kill processes on session end.
- Arena/race mode: N sessions on same brief in separate worktrees, hidden rubric, cross-agent judge, pick base + graft; flag divergence as under-specified brief.
- Guard context: orchestrator receives summaries/digests, raw transcripts stay in worker sessions (link to open).
- Restate goal first: each session's first message states goal + smallest expected change; surface it as the session title/summary in the list.
- Fewer polished controls over many rough ones (experience-first): core loop = spawn, watch, verify, merge; everything else out of the way.

## Unresolved questions
- Does user mean Lauren Tan's pstack, or a Vietnamese-community/agentkit variant? Confirm.
- Did not confirm whether mrgoonie/ClaudeKit has a "pstack" analogue.
