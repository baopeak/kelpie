---
phase: 4
title: "Persist question answers"
status: pending
priority: P1
effort: "10h"
dependencies: []
---

# Phase 4: Persist question answers

## Overview

An answered question card keeps showing its questions and what was chosen, as in Claude Code's "Answered · 3 questions", and still does after closing and reopening the window.

Today:

- `PermissionCard`'s resolved branch shows only `→ A · B · C`, and the questions disappear.
- `ChatController.resolved` is memory only, and the daemon's replay log holds only agent stdout. On replay an answered card meets `expireApprovals` at the turn's end and says "No longer needed", which is false.

## Requirements

- Functional:
  - A resolved question card lists each question, its options, the chosen ones marked, and any typed "Other" text.
  - Any resolved card shows the chosen label and the message sent with a denial.
  - An answer given on another device says who answered and when.
  - Reopening the window, or re-attaching, restores all of this for all three dialects (Claude, Codex, ACP).
  - "No longer needed" appears only for requests that really were never answered.
- Non-functional:
  - **Replay boundary:** applying a logged answer never writes to the agent. Covered by a test.
  - **Do not store `updatedInput`** except `answers`: it can hold whole file contents.
  - One answer line is at most 16 KiB; above that keep only the verdict.
  - An older daemon yields today's behaviour with no errors. An older frontend attached to a newer daemon must ignore the new line.

## Architecture

The daemon already sees every answer at two places in `crates/kelpied/src/chat.rs`: `answer()` (a phone) and `keep_unanswered()` (lines from a window). When an answer counts (`resolve` returned true), it appends to the log, using the existing `append`, which also broadcasts to subscribers:

```json
{"type":"kelpie_answer","request_id":"req-3","by":"desktop","answered_by":"desktop","at":1793000000000,
 "answer":{"behavior":"allow","answers":{"Q?":"A"},"lasting":false}}
```

- Claude `answer`: `behavior`, `message`, `answers`, `lasting` (true when `updatedPermissions` exists), `mode` (when a `setMode` permission is present).
- JSON-RPC `answer`: `{ "result": … }` or `{ "error": true }`.

This follows the `kelpie_time` precedent in `append`.

Frontend:

- `ChatController.feedText` intercepts `type === "kelpie_answer"` before `adapter.onMessage`, as it handles `kelpie_time`. Check where that is first and sit beside it.
- New optional `Adapter.describeAnswer(card, answer)` returns `{ label, answers?, message? }`.
- New field on the permission item:

```ts
answered?: { label: string; answers?: Record<string, string>; message?: string; by?: string; at?: number };
```

`resolved` stays a string (phone's `prompt-card.tsx` reads it).

Restart path: after a daemon restart, a conversation reloads from the agent's own history, which has no cards, only the `AskUserQuestion` tool row with its result text. Show that row open when no question card exists in the transcript (step 8).

## Related Code Files

- Modify:
  - `crates/kelpied/src/chat.rs`
  - `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`, next free level at commit time, in commit order across sessions; on 2026-10-30 levels 18–26 were planned for the phone work, so ask that session or the user before taking one)
  - `app/src/chat/chat-model.ts`, `app/src/chat/claude-adapter.ts`, `app/src/chat/codex-adapter.ts`, `app/src/chat/acp-adapter.ts` and their tests
  - `app/src/components/chat-rows.tsx`, `app/src/chat/tool-view.ts`, `app/src/styles.css`, `app/src/locales/vi/tools.json`
  - `docs/architecture.md`

## Implementation Steps

1. **Daemon tests first:** an answer from a window is logged once, without `updatedInput`, but with `answers`; a dropped second answer logs nothing; a phone answer is logged with its device name; an oversized answer keeps only the verdict; an ordinary non-answer line logs nothing; a response to a request that was never pending logs nothing; a stop logs nothing.
2. **Sanitize:** add `answer_record(line: &Value) -> Value` next to `answer_lasts`, reusing its checks. Parse the line once in `keep_unanswered` (`answered_key` already parses; avoid parsing twice per line by returning the value).
3. **Append:** `resolve` and `keep_unanswered` are associated functions taking `g`; `append` takes `&self`. Return the record from those paths and have the callers, which hold `self`, call `self.append(&mut g, record)`. Append after writing to the agent's stdin, so the answer follows its request in the log. **Log only when `g.pending` held the key, checked before `resolve` removes it:** `answered_key` matches every response, including the ACP adapter's automatic "not implemented" errors, which must not be recorded. `stop()` is a third caller of `resolve`; it logs nothing. Check that `pinned_kind` and `ask_of` ignore the new line, that a `kelpie_answer` line never reaches an agent, and that the phone feed in `mobile.rs` passes it unchanged.
4. **Bump** `API_LEVEL` at commit time, and note it in `docs/architecture.md`.
5. **Controller.** Extract the id-key loop from `resolvedElsewhere` (`[id, approval-id, perm-id, elicit-id]`) into `findCard(requestId)`, and use it in the new `applyLoggedAnswer(msg)`:
   - Find the card; if absent, ignore. Skip while the card is `answering`, or a phone's own answer turns into "Already answered on Pixel".
   - Identify the sender by `answered_by`, not `by`.
   - `describeAnswer` from the adapter, falling back to a generic `Answered`.
   - Set `this.resolved`, and patch `resolved` and `answered`.
   - Label: for a window answer, the description; for another device, keep `answeredBeforeText(by, at)` as the header and add the description.

   Live `respond()` stores `answered` from its `label` and `reply` right away; the logged record then only confirms it. `forgetSentAnswers` still clears everything.
6. **Adapters, `describeAnswer`:**
   - Claude: `deny` matches the `deny` option; `answers` give a joined label; `mode` matches `plan-accept-edits`; `lasting` matches `allow-session`; otherwise the first plain `allow`.
   - Codex: map `result.decision`.
   - ACP: `result.outcome.optionId` to the card's option.
   - Unknown: `null`, so the fallback applies.
7. **Card UI.** Resolved `ask` cards show a header `Answered · {n} questions` with a chevron, open by default. For each question: tag, text, options with the chosen marked (reuse `.ask-opt.sel`, disabled, no buttons), and remainder text as `Other: …`. Match chosen by option labels first, because labels can contain `, ` (the answers are joined with that): an option is chosen when the answer equals it, or contains it as a `, `-separated part; what is left over is the other text. Other cards keep `→ label`, plus the message. Show by/at when known. Unanswered cards are untouched.
8. **Restart path.** In `describe`, add `AskUserQuestion` with verb `t("Questions")`. `ToolRow` opens it by default when its output exists and no `ask` card is in `ctrl.state.items` (pass as a prop). The output text is shown verbatim.
9. Run the Rust tests: `CARGO_INCREMENTAL=0 cargo test -p kelpied chat` then `-p kelpie-proto`. Run frontend tests, then the real-app check below.

## Tests

- TS: replaying a log with a request and `kelpie_answer` gives a resolved card with `answered`, **and writes nothing** (assert on the harness's `written`), and `expireApprovals` at turn end does not overwrite it. A request with no answer still expires. The same line through each adapter's harness is ignored by the adapter itself. `describeAnswer` for each dialect. A label containing `, ` marks the right option.
- Rust: step 1 list.

## Success Criteria

- [ ] Real app, isolated daemon: answer a question card with two questions and an "Other". The card lists them all with the choices marked.
- [ ] Close and reopen the window: the same card is unchanged, no "No longer needed".
- [ ] Deny with a message, close and reopen: the message is still shown.
- [ ] Repeat one approval on Codex and one on an ACP agent.
- [ ] Answer from a phone (or a test client through `mobile`): the desktop card names the device.
- [ ] A never-answered card from a turn that ended still says "No longer needed".
- [ ] Log stays free of edit content: grep the record of an approved `Write` for its file content.

## Risk Assessment

- **Answer text in the log.** It is what the user typed; the log already holds the conversation and has the same owner and same access. Only sanitized fields go in. Phones already see the session.
- **Log trim removes a request but not its answer.** `findCard` misses and the line is ignored.
- **Id reuse after an agent restart.** The log's order puts an answer after its request; a later request with the same id arrives unresolved, unless the id is in `this.resolved`. Check against the existing `cardIdentity`/`forgetSentAnswers` handling with a test. If a reused id shows a stale answer, key `resolved` by id plus `cardIdentity`.
- **Claude's `control_response` shape changes.** Signal: `describeAnswer` falling to the fallback in the real app. Response: update the adapter; the fallback stays truthful.
- **`resolve` is called for a late dropped answer.** It returns false and nothing is logged; the existing `chat.resolved` event still corrects views.
