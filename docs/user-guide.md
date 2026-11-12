# Kelpie — user guide

Kelpie runs many coding agents (Claude Code, Codex, OMP) and shells across many projects.
Sessions live in a background daemon (`kelpied`): switching projects, updating Kelpie or a
UI crash never stops them. Closing Kelpie stops them by default, after asking when something still
runs (see Install). Only the project you are looking at is rendered.

Architecture and protocol: [architecture.md](architecture.md).

## Install

- **Windows 10 1809+ / 11**: run the `Kelpie_x.y.z_x64-setup.exe` installer (per-user, no admin).
- **macOS 12+**: open the `.dmg` and drag Kelpie to Applications.
- **Linux (x86-64; built on Ubuntu 22.04, so glibc 2.35 or newer)**, either format:
  - **AppImage**: `chmod +x Kelpie_x.y.z_amd64.AppImage` and run it. It updates itself in place.
    Running an AppImage needs FUSE 2 (`sudo apt install libfuse2` on Ubuntu 22.04+, or start it
    with `--appimage-extract-and-run`).
  - **.deb** (Debian, Ubuntu and derivatives): `sudo apt install ./Kelpie_x.y.z_amd64.deb`; apt
    pulls in WebKitGTK. Kelpie is then `kelpie` on the PATH and in the applications menu.

  Data lives in `$XDG_DATA_HOME/Kelpie` (usually `~/.local/share/Kelpie`, readable by you only):
  settings, logs, the session daemon's socket and, for the AppImage, the copy of the session daemon
  that runs (an AppImage's files exist only while the app runs). Terminals and agents get your
  desktop's environment, not the AppImage's own GTK setup. Saved secrets (SSH passwords, notification and
  issue-tracker tokens) need a Secret Service — GNOME Keyring, KWallet or another
  `org.freedesktop.secrets` provider, unlocked; without one, saving a secret fails with a message
  that says so and nothing is stored anywhere else. **Keep computer awake** uses
  `systemd-inhibit` (systemd's logind) to block idle sleep and suspend while it is on; on a system without it the setting has no effect and the
  daemon log says why. Desktop notifications go through the desktop's notification service.

Agents are not bundled: install the CLIs you use (`claude`, `codex`, `omp`) as usual. Kelpie
finds them on your PATH (Windows) or through your login shell (macOS, Linux: apps started from
the desktop do not see the PATH your shell profile sets, so each agent starts through
`$SHELL -l`, or the account's shell when `SHELL` is not set).

**Updates.** Kelpie looks for a newer release on GitHub 30 s after it starts and every 6 hours
(Settings → About → **Check for updates**, or **Check now**). A notice says so once per release, and
a newer release stays as *Update x.y.z* in the status bar; its dialog shows the release notes, with **Install now**,
**Install when idle** (as soon as no session is working) and **Skip this version**. Updates are
signed; Kelpie installs nothing else. Installing restarts only the app: every session keeps
running and is there when Kelpie reopens (running an installer by hand over a running Kelpie works
the same way). The one exception is the first update from Kelpie 0.1.x, whose session daemon
runs from the install folder: that update stops sessions once (the dialog says so) and agents
resume. The sessions stay on the previous session daemon until they have all stopped; then
the new one takes over on its own. A notice offers **Switch now** if you want the new daemon's
features earlier (running sessions stop for a moment; agents that can resume continue).

**Closing Kelpie** stops its session daemon too, and with it every session, automation and file transfer. When
something still runs (sessions, run tasks, file transfers) Kelpie names it first and asks: **Close and stop** or
**Cancel**, with *Don't ask again*; with nothing running it just closes. Kelpie does not wait for the work: next time
sessions come back interrupted and agents resume their conversation (Settings → Agents → *Resume after a restart*),
and transfers show Interrupted, with Resume. To keep everything going with the window closed (automations, scheduled
runs, phone and Telegram access, file copies), set Settings → Session Daemon → *When Kelpie closes* to **Keep it
running in the background**. Updates never stop the daemon, whichever you choose.

## First start, Kip and tips

With no project open, Kelpie shows its welcome screen: Kip, the mascot, above *Add project folder*, *Add remote
project (SSH)*, *SSH hosts* and the command palette. Kip also sits over the sidebar footer and shows how your
agents are doing (working, needing you, finished); Settings → Appearance → *Kip, the mascot* hides it, and *Kip
animations* turns its small animations on, off or makes them follow the system's animation setting.

For the first launches Kelpie shows one short tip per launch, four seconds after it opens: search everything,
the agent dashboard, terminal or chat, and past conversations. *Turn off tips* on a tip, or Settings →
Notifications, stops them; *Show tips again* there brings them back. Icon-only buttons show a small hint with
their name and shortcut when you hover them.

## Everyday use

1. **Add a project**: sidebar `+` (or search → *Add Project Folder…*). Any folder works; for a
   git repository its other worktrees are added under it.
2. **Start a session**: click Claude / Codex / OMP / Terminal on the empty project screen, or
   `+` in the tab bar for options (extra args, name, split, *Chat view*) and your quick commands.
3. **Sidebar**: every project with its branch (`*` = uncommitted changes, ↑↓ ahead/behind) and
   its sessions — in the order their panes sit on screen (tab by tab, each read left to right,
   top to bottom; sessions in no pane last): status dot, task title, model and time since the
   last activity. Badges show
   sessions that need you (amber), are working (blue) or finished unread (green). The chevron
   folds a project; the toolbar filters (all / running / needs you) and sorts (manual /
   recent). Right-click a session to rename it inline, mark it unread, restart or resume it,
   continue it in chat view or stop it; right-click a project for worktrees, rename, reorder.
   Switch projects with a click or `Alt+1…9` (`⌘1…9`); `‹ ›` (or `Alt+Shift+←/→`,
   `⌘⌥←/→`) go back and forward through recently focused panes, across projects.
4. **Search everything** (`Ctrl+Shift+P` / `⌘⇧P`): recent items, sessions that need you,
   sessions, projects, files, commands and settings in one box. Prefix `@` sessions, `#` files,
   `>` commands, `/` settings, `!` prompts you sent earlier in this project, or press `Tab` to change
   scope. Some commands live only here, without a default key: *Answer Waiting Session…* (the quick
   answer of the dashboard for the session that waited longest) and *Clear Terminal Buffer*, among others.
5. **Dashboard** (`Ctrl+Shift+M` / `⌘⇧M`): every session of every project grouped by
   *Needs you / Failed / Working / Done / Idle* — click a card to jump there. *Needs you* lists
   the longest wait first ("waiting 3 min"); **Answer** opens a quick answer: the bottom of that
   session's screen, a button for each key its prompt offers (1 2 3, y n), Enter / Esc and a
   line of text — sent only if the session still waits on that prompt. `Ctrl+Shift+.` / `⌘⇧.`
   jumps to the session that has waited longest, then the next. Agents without hooks (Codex,
   Gemini, Aider, OpenCode…) count as *needs you* when their screen shows a menu or a yes/no
   question; the bell in the status bar gets a dot while any session needs you, and its
   *Needs you* list answers right there.
6. **Files**: the right panel has Files (filter by name, or search contents), Changes (git) and
   Sessions. Click a file to open it in the editor (`Ctrl+S` saves; external changes are
   detected); click a change to see its diff. Ignored files are dimmed.
   - **Switch branch**: click the branch name at the top of Changes for the local branches (filter by
     typing, then `↓`/`↑` and `Enter`; most recently committed first). Picking one switches this folder to
     that branch, only when nothing would be lost: nothing happens, and the panel says why, when
     - another worktree has the branch checked out (marked *worktree*; a branch cannot be checked out twice):
       the panel names that folder and, when it is in the sidebar, offers to go there;
     - there are uncommitted changes (untracked files included): it offers *Open in a new worktree…*, which
       checks that branch out in a new worktree and leaves this folder as it is;
     - a merge, rebase, cherry-pick or revert is in progress;
     - HEAD is detached on commits no branch, tag or remote has: switching would leave them behind, so
       create a branch for them first (`git switch -c <name>`).

     The switch runs your `post-checkout` hook like `git switch` in a terminal does (it is your own
     action; Finish, which works unattended, runs no hooks).
   - **History** (bottom of Changes, folded): the last 100 commits of this branch with short hash,
     subject, author and age; it is read when you open it and again when the branch changes (Refresh
     reads it again after new commits).
   - **Stashes** (folded): the repository's stashes, *Apply* (the stash is kept) and *Drop* (asks first).
     Git keeps one stash list per repository, so every worktree shows (and can drop) the same stashes. An
     action checks that the stash is still the one shown: Apply uses that exact stash, and a Drop that
     hit another one (the list moved at that instant) puts it back and says so; refresh and try again.
7. **Panes**: hover a pane for split right / down, maximize (`Ctrl+Shift+Enter`) and close. A
   split (button, terminal menu or shortcut) opens an empty pane asking what to run: click an
   agent or *Terminal*, or press `1`–`9`; *More…* opens the new-session dialog for that pane
   (arguments, account, chat view); `Esc` closes it.
8. **Chat view**: in an agent pane header click the chat icon to continue the *same
   conversation* in a chat UI (messages, tool cards with diffs, Allow/Deny prompts); the
   terminal icon switches back to the TUI. Kelpie stops one front-end before starting the other,
   so a conversation never has two writers. In the message field `/` lists the agent's own
   commands (skills and custom commands included), `@` the folder's files and, for Codex, `$` its
   skills (`↑`/`↓`, `Tab` or `Enter` to pick, `Esc` to close); the chips under the message field
   switch model and effort (one chip, left) and permission mode (right, amber when nothing asks
   for approval; `Shift+Tab` steps through the guarded modes). A Claude chat starts asking before
   edits, and its mode chip also offers *Bypass permissions* (confirmed first, unless *Permissions:
   Yolo* is set; a chat started before Kelpie could switch to it is started again in that mode,
   continuing the same conversation). A chat that comes back after a restart keeps the mode it
   last ran in. While the agent works with nothing
   typed, the round send button stops it. A command
   that only works in the terminal UI shows *Run it in the terminal*, which switches back and runs
   it there.
   - The transcript reads like the terminal UI: each tool call is one line (*Read app.ts · lines
     75-80*, *Edit main.rs +3 −1*, *Bash* with what it is doing; click a row for its command and output, which open by themselves when it fails), runs of reads and searches
     fold into *Explored 4 files, 2 searches*, and thinking folds into *Thought for 6s*. After a
     turn, *N files changed* lists each file with its line counts; click one to open its changes.
   - Every prompt and reply shows its date and time in the top right corner (hover for the full
     date), also after a restart or when you come back to the chat later. The icon beside it copies
     the message, and a tool's *IN* and *OUT* have one too, for the whole text; hover a code block to
     copy its code. A prompt whose turn changed files has *Undo this turn…* there: the same previewed
     undo as the Turns panel. A reply's dot pulses while it is being written, and a table appears as
     its rows arrive.
   - The bar over the transcript names the prompt of the turn you are reading; click it to go back to
     that prompt, also from far up a long conversation. It leaves the caret in the message field.
   - Click a file in a reply to open it: a link, a path in inline code or in the text (`src/a.ts:42` opens at that
     line; a bare `name.py` is found by its name), a tool's target (an edit's opens its changes) or the file on an approval card. Something
     outside the project only shows a message: nothing an agent names is ever launched. Addresses in
     your own messages open in the browser. Right-click a named file to copy its relative or full path or its name,
     or a link to copy its address. The same copy items are in the file tree and in a file pane's header.
   - Paste (`Ctrl+V`) or drop pictures into the message field, up to 8 and 5 MB each, also on their
     own and while the agent works. Click a thumbnail, or a picture in a reply, to enlarge it: click
     again for its real size, the arrows go through a message's pictures, `Esc` closes it without
     stopping the agent. A picture no longer available shows its name instead. An agent that cannot
     take pictures is given the path, and the field says so.
   - The bookmark next to a finished reply's copy button saves it to the *Saved* tab of the right
     panel, per project and newest first, also after its session is gone. Click one to go to the
     reply; if it is no longer in a chat, its saved text opens in a dialog to read and copy. A reply
     over 64 KiB is saved cut short, and a project holds up to 500.
   - Background work Claude left running (background commands, subagents) is listed above the
     message field as *In the background*; click it for each task's status.
   - *Agents N* next to the context ring, or *Show Agent Map* in the palette, opens the conversation's
     subagents as a tree with their status, time and tokens (Claude; Codex lists its agents without
     tokens, and ACP has no such thing to list). Click one for its prompt, what it did and its answer,
     and *Show in chat* for its row. `Esc` steps back, and never stops the agent.
   - A message sent while Claude or Codex works goes into the running turn, as in their terminal: it
     shows faded until the agent reads it at its next step, and the agent decides what to do with
     it. To change course at once, stop the agent first (`Esc`). A command (`/`, `!`), a message
     with a picture, a message for any other agent, and one sent while others wait, waits above the
     message field and goes when the turn ends. *Send now* gives such a message to the agent during
     its turn, or stops the turn first where it cannot go in; stopping the agent holds these until
     you send again.
   - The agent's task list (Claude's todos, Codex and OMP plans) stays pinned above the message
     field. The ring next to the pickers shows how full the context is (hover for tokens and cost;
     click to compact).
   - Approvals show the command or the edit they are about; press `1`–`9` (with an empty message
     field) to pick an option, or type what the agent should do instead and *Deny*. Questions the
     agent asks (Claude's multiple-choice questions) and plans it wants approved are answered in
     their card. *Waiting for your approval* appears when the card is scrolled out of view. An
     answered card keeps what was asked and chosen, any message sent with a denial, and who answered
     from another device, also after the window is closed; *No longer needed* is left for requests
     nobody answered.

### Past conversations

The **Sessions** tab of the right panel lists earlier Claude, Codex and OMP conversations read
from their own transcripts: *This folder*, *Project* (folders inside it) or *All*, grouped by
day, searchable by title and first prompt, with the number of prompts. Resume one in a
terminal or in chat view (it opens in its own folder), or copy the resume command. The list is
built from the start of each transcript and cached; prompt counts fill in from a background
pass.

**Export as Markdown…** (the download button on a row) saves the conversation as a readable
Markdown file: a header (agent, project folder, session id, start and end time, model) and then
the turns. Tool calls and their results are folded `<details>` blocks, each cut to 40 lines or
4,000 characters with a "… N lines omitted" marker. Thinking is left out; images and attachments
are named (`[image: image/png]`), never embedded. Every piece of text passes the same credential
filter as hand-offs (API keys, tokens, Bearer headers, passwords in URLs, `NAME=secret` lines
become `[redacted]`; it is a best-effort filter, read the file before sharing it). The file has LF
line endings; a very long conversation stops at 16 MiB with a note, and transcript lines that
cannot be read are left out and counted. For OMP, only the branch the session is on is exported
(turns left behind with `/tree` or a fork are not); Claude and Codex conversations are exported
in the order they were written. Only Claude, Codex and OMP conversations are listed and
exportable: other ACP agents keep their history where Kelpie cannot read it.

### Usage and memory

The status bar shows Claude and Codex plan usage (5 h and weekly windows, amber from 80 %, red
and a notice at 100 %) and Kelpie's memory (app + WebView, daemon, sessions). Usage is read with
the agents' own sign-in (`~/.claude/.credentials.json` or the macOS keychain, and
`~/.codex/auth.json`); tokens stay in Kelpie's Rust core and are sent only to the vendor's usage
endpoint, identifying as Kelpie. When a refresh fails (e.g. the endpoint's rate limit) the last
values stay shown, dimmed, with the reason in the tooltip. Both meters can be turned off in
Settings → Usage & Resources.

### Context, tokens and cost

Each Claude Code, Codex and OMP session shows a small **context ring** on its sidebar row, pane
header and tab: how much of the model's context window the conversation uses (grey, amber from
70 %, red from 85 %). Hover for the numbers: context, tokens in and out, cost and model. At 85 %
Kelpie says so once per session; click the ring (or the toast's button) for **Compact** (the agent's
own `/compact`), **Hand Off to another agent**, or the usage page. The numbers come from the agent's
own transcript and only for a session Kelpie can match to its conversation exactly; a session without
one shows nothing. Claude costs are **estimates at API prices** ("≈ $": a subscription is not billed
per token); OMP's and ACP agents' are their own numbers; Codex shows tokens only. OMP has no known
window, so it shows a cost chip instead of a ring.

**Usage & cost** (palette, right-click a usage meter, or the "Cost" item next to the meters) lists
cost and tokens by day, week, project, agent or model for the last 7 / 30 / 90 days or all time, from
every transcript on this computer, Claude's sub-agents included. The first count can take a
while; it continues in the background.

Settings → Usage & Resources has an optional **cost cap per session** (US dollars, off at 0): a
session over it (all its conversations together) gets a red cost chip and an Activity entry, and can
optionally be interrupted with Esc.

### Turns and undo

Settings -> Agents -> **Turn snapshots** (off by default; it costs a few hundred milliseconds per turn on a
large repository) keeps a snapshot of an agent's folder when each of its turns ends, for agents running on
this computer in a git folder. The **Turns** tab of the right panel (or "Agent Turns and Undo" in the palette)
lists them for a session: when, how many files and lines, and what changed; click a file for a before / after
diff. A turn marked *approx.* belongs to an agent that reports no turns: Kelpie guesses the end from the
output going quiet, so two turns may be merged. The **Files** view shows who touched which files in the
folder; a file changed by more than one session is marked shared. A turn's *Produced* row opens markdown,
images, Mermaid and CSV it wrote, rendered; a comment on a markdown block becomes a review note you can send
to an agent from any diff.

**Undo** (the arrow on a turn) previews every file it will restore or delete and changes nothing until you
confirm. It is available once the session is idle. Files another session also worked on in the same period,
files changed since the turn or between turns (by your editor or a terminal, even if the agent edited them too; Kelpie looks at the folder as each turn starts), start unticked and say why; tick one to allow overwriting it. Your index,
branch and commits are never touched. Snapshots live in the repository under `refs/kelpie` (the last 50 turns
per session) and are removed when the session is closed or its worktree removed. A repository where a
snapshot takes over a second twice in a row (on an otherwise idle machine) is switched off until Kelpie's
session daemon restarts, with a notice; turn it on again in the panel. Other git tools can see the
`refs/kelpie` refs (`git log --all`, graph tools), and a `git push --mirror` would send them. Line endings of
a restored file follow your git settings and can differ from before; Kelpie says so in the dialog.

### Review and Finish

The board's **Review** column is the review queue (so is `kelpied worktree set --status review`, which an
agent can run itself, and an automation set to *Move the folder's card to Review* when it finishes). A
worktree card in that column shows its files, +/- lines, commits, uncommitted files, the checks, what its
sessions cost and how many turns they took. The **Review** tab of the right panel lists the same cards
for the current repository with their files; a file opens a diff against the branch the worktree will be
merged into, committed changes included. Marking a worktree **Done** while it still has changes nobody
merged offers Review instead.

- **Open diff** shows that. **Request changes** sends the review notes you wrote on the diff (click the +
  beside a line) to an agent in the worktree and moves the card back to *In progress*.
- **Approve → Finish** (also the sidebar menu's *Finish…* and the palette's *Finish Worktree…*) opens the
  Finish dialog: the uncommitted files it will save as a "WIP from <agent>" commit (with their sizes; you tick
  to confirm exactly that list), how to merge (squash, merge commit or rebase) and the message, and the
  checks. Then Kelpie stops the worktree's sessions and reads the files again: if any appeared after you
  confirmed (the agent was still writing) nothing is committed and you confirm the new list. Only the
  confirmed files are committed; a folder with a git repository of its own inside the worktree is refused
  rather than committed as a pointer to nothing. Then it merges, runs the checks, moves the branch it merges
  into only if that branch has not changed meanwhile, updates the main checkout, deletes the branch only
  when its work is in the target, and removes the worktree last. It never forces anything. The branch is the
  one the worktree is really on: a worktree on a detached HEAD, whose commits would be lost with it, cannot be
  finished until you give them a branch (`git switch -c <name>`).
  - It merges into the branch the worktree was created from; if that is not the main checkout's current
    branch, or is unknown (worktrees added later), the dialog says so and asks you to confirm.
  - A conflict is announced in the dialog (*Resolve with agent* starts an agent in the worktree) and nothing
    is stopped for it; a failing check (*Fix with agent*), a target that moved (try again) and a main checkout
    with changes to tracked files (commit or stash them first) stop it before anything moves. Untracked notes
    or logs in the main checkout do not matter, except a file the merge would overwrite (including an ignored
    one such as `.env`): Finish names it and stops.
  - **Cancel Finish** works at every step: before the target branch moves it ends whatever runs (a merge, a
    signature that waits, a check) and changes nothing; once the work is merged it only skips the cleanup and
    says what is merged. A check that outlives its time (20 minutes) is stopped with everything it started.
    A Cancel never carries over to the next Finish.
  - If the worktree cannot be removed (a program still has a file open) the work is already merged; the branch
    is put back and the worktree stays, with the reason. If git deleted part of the folder before failing, the
    leftover folder is reported; it holds no links into your shared folders.
  - **Signing and hooks.** If the repository signs commits (`commit.gpgSign`, any config level), the commits
    Finish makes (squash, merge commit, every rebased commit, the WIP commit) are signed with git's own
    settings (`user.signingKey`, `gpg.format` openpgp / ssh / x509); a passphrase or security-key prompt that
    nobody answers within two minutes, or a failing signer, stops Finish before anything moves. Rebased commits
    are re-signed by you. **Commit hooks do not run** for Finish's commits (pre-commit, commit-msg,
    prepare-commit-msg, reference-transaction…): they are Kelpie's own bookkeeping, and with husky-style hooks
    the scripts would come from the branch being merged. So a Gerrit Change-Id or DCO sign-off added by a
    commit-msg hook is not added; add it to the message in the dialog. `merge.verifySignatures` is not
    applied either; the checks are the gate.
- **Checks**: add `check:` to the repository's `.kelpie.yaml`, one command per line (a program and its
  arguments; no `&&`, pipes or variables):

  ```yaml
  check:
    - pnpm test
    - cargo clippy -- -D warnings
  ```

  They are read from the file committed on the branch you merge into, never from the branch being merged,
  and run only after you approved that exact list (the Finish dialog shows them and asks; if the list changes
  it shows what was added or removed and asks again). The approval covers the checks only: it is kept apart
  from the approval of `setup:`, `tabs:`, `copy:` and `link:` that the New worktree dialog asks for (which now
  lists the `check:` commands too), so approving a check never approves a changed setup command. They run
  without a shell in a temporary worktree of the merged result, where the `copy:` and `link:` entries are
  available. **They run the branch's own code**: `pnpm test` or `cargo test` execute the branch's scripts and
  build files, so a check is only as safe as the branch. If the branch itself edits `.kelpie.yaml`, its card
  says so, its checks are off, and Finish makes you choose between finishing without them and running the
  target branch's list; the same holds when the file cannot be read. Finish waits until the checks have been read before it can start.
- **Also changed in …**: when two or more worktrees of a repository have an agent running, a chip on each row
  (and in the Changes tab) says which other worktree changed the same files (checked every 30 s and
  after a turn ends).

### Review with another agent, and best of several

**Review with…** (the button on a review card, or the palette's *Review Worktree with…*) asks another
agent — by default one that did not write the changes, so it can be another vendor — to review a worktree.
You choose the agent and can edit the prompt it starts with; to change the default, save a quick command
called **Review prompt** (Settings → Quick commands). The session starts in the same worktree with the diff
against the branch it will be merged into (at most 48 KB, credentials masked). The reviewer can leave
comments on lines of the diff with `kelpied review add <file>:<line> "<comment>"` (`--old` for a removed
line, `--base <branch>` to name the base; Kelpie puts the command in the prompt): they appear in the normal
diff view like your own notes, marked with the agent's name, and **Request changes** sends them on. A
reviewer can only write notes in its own worktree. (An agent whose launcher strips `*TOKEN*` variables from the
programs it runs cannot prove which session it is: `kelpied review add` is refused there; paste its comments as notes yourself.)

**Try with several agents…** (the link in the New session dialog, or the palette) sends one prompt to 2 to
5 agents, each in a worktree of its own, started from where the folder is now. It is an ordinary run, so
the Runs page follows it. When the repository's `.kelpie.yaml` has been approved (New worktree shows it),
each worktree also gets its `link:` folders (shared, nothing copied) and `copy:` files, and the dialog shows
what that is and how large the copies are before you start; without approval they start as clean checkouts.
The Runs page's *New run* does the same for its tasks that have their own worktree.

**Compare** (the tab of a run in Runs, when two or more of its tasks have a worktree) puts the candidates in
columns: status, time, cost, files, +/- lines, the base's checks and turns, with a star on the quickest and
the smallest change among those that finished. Click a file to see two candidates' versions of it side by
side (pick which two). **Use this one** asks, listing the other candidates, stops their agents and opens Finish
for the winner. Once it has merged, each other candidate that is still exactly as it was when you confirmed
is cleaned up: what it had not committed is saved in a commit on its branch, its worktree is removed (never
forced) and its branch is deleted only if it is merged, otherwise it stays with the candidate's work. A
candidate that changed meanwhile, or that cannot be removed without force, is left alone and Activity says
why. **Discard…** does the same for a single candidate after asking.

### New agent sessions in a worktree

Settings → Agents → *Worktree for new agent sessions*: in the New session dialog an agent started in a git
folder can run in its own new worktree, with the branch named from the first prompt
(`Fix the login redirect` → `fix-the-login-redirect`). *Ask* shows an unticked box, *Always* ticks it, *Never*
hides it. The repository's `.kelpie.yaml` setup runs there only if you approved that exact file before.

### Composer

`Ctrl+Shift+I` (`⌘⇧I`), or the pencil in a pane's toolbar, opens a multi-line composer over the
focused session, whether it is a shell, an agent's terminal UI or a chat view. Enter sends,
Shift+Enter adds a line, Esc closes it and keeps the draft, and Up / Down bring back the
project's earlier prompts (the search palette finds them too: type `!` and a word).

Type `@` to mention:

- **files and folders** (`@src/app.ts`, `@"my docs/read me.md"`);
- **`@selection`**: what is selected in an editor, a diff or a terminal (also *Send Selection to
  Agent…* in their right-click menus, which opens the composer of the agent you pick);
- **`@diff`** (uncommitted changes), **`@note`** (your review notes);
- **`@terminal:<session>`**: the recent output of another session, **`@issue:<key>`**: an issue
  from Linear, Jira or GitLab (a bare `@issue` is the issue the session was started on). Secrets in
  both are masked before they are sent.

Claude Code, Codex and Gemini read `@path` themselves, so they get the path. Other programs get
the content, fenced and labelled with where it came from; a snippet of more than 200 lines, or
more than 64 KB in all, becomes a path instead. A shell never gets content: it gets paths, quoted for it,
left at the prompt for you to read and run (a file name can be a script, so nothing that holds a
mention or a picture is sent with Enter). A terminal that did not ask for bracketed paste (an agent
that is still starting, an older shell) takes no multi-line text, since every line would run.

Paste or drop a picture: a terminal UI gets its path (the picture is saved to a temporary file), a
chat view gets the picture itself.

### Quick commands

Settings → Quick Commands saves commands and prompts. *Terminal* types into the focused shell
(or a new one), *Agent* sends a prompt to the focused agent session; optionally press Enter;
for all projects or one. Run them from search (`>` *Run: …*) or the new-session menu
(`Ctrl+1…9`).

### Worktrees and the workspace board

Right-click a repository → **New worktree…**: branch (a name is suggested), base (current
branch) and folder (`<repo>-worktrees/<branch>` next to the repository). The worktree appears
under the repository with its own sessions and layout. **Remove worktree…** first says what
would be lost (running sessions, uncommitted files, ignored files such as `.env`), then stops
its sessions and runs `git worktree remove`; the branch is always kept. A worktree on a
detached HEAD with commits no branch contains is not removed — create a branch first.

A repository may contain `.kelpie.yaml` (or `.erka.yaml`, its name before the rename). The new-worktree dialog shows its commands as they are
in the base commit and runs them only if you tick the box (it stays ticked for the same file
next time):

```yaml
setup:            # typed, one per line, into a "setup" terminal
  - pnpm install
tabs:             # sessions to open: claude, codex, omp, shell, or a command
  - claude
  - command: pnpm dev
```

The **workspace board** (`Ctrl+Shift+J` / `⌘⇧J`, or the board icon in the sidebar) shows every
project and worktree as a card in *Todo / In progress / Review / Done* with the agents running
in it; drag cards between columns, pin important ones, click to open.

`.kelpie.yaml` can also bring ignored files into new worktrees (a fresh checkout lacks them):

```yaml
copy:             # private copies (also read from .worktreeinclude, one path per line)
  - .env
link:             # shared with the repository (symlink; a junction on Windows)
  - node_modules
```

Only literal paths that exist in the repository and are ignored by git qualify; copies are
capped at 2 GiB / 50,000 files per worktree. The *From* field suggests local and remote
branches and tags (a commit id works too). After **Remove worktree…** a notice offers to
delete its branch (`git branch -d`; when it has unmerged commits Kelpie asks before `-D`).
In search, typing a name that matches nothing offers **Create worktree "…"**.
Folder names remain literal when linked, including spaces, Unicode, `&` and `%`; creating a
Windows junction does not run the name through a shell.

A worktree (or any folder) can carry a **comment** — what it is doing right now — and a board
status. Set them from the sidebar menu (*Add comment…*) or let an agent set them:

```
kelpied worktree set --comment "reproduced the bug; testing a fix" --status progress
kelpied worktree get
```

The comment shows under the sidebar row and on its board card.

When an agent exits, the pane offers **Restart** / **Resume** (agents resume their
conversation). A desktop notification fires when a session you are not looking at needs input
or fails (see Activity and notifications).

### Agents

Besides Claude Code, Codex and OMP, Kelpie knows 20 more agent CLIs (Gemini, Cursor CLI,
OpenCode, GitHub Copilot, Grok, Qwen Code, Droid, Aider, Amp, Goose, Kimi, Crush, Cline, Kiro,
Continue, Devin, Auggie, CodeBuddy, Mistral Vibe, Pi). Installed ones appear in the launchers;
Settings → Agents shows them all, hides ones you do not want and edits their arguments and
environment. **Permissions: Yolo** starts agents with their skip-permission flags (agents
with their own permission mode set keep it). The new-session dialog takes an optional **first
prompt**; for a terminal it is a startup command.

- **Hand off**: right-click an agent terminal → *Hand Off to …* starts another agent next to
  it with the end of this session's transcript as its brief.
- **Hibernation** (Settings → Agents): agents that finished and sat unseen in a background
  project for N minutes are stopped; they resume their conversation when you open them
  again. *Sleep* in a project's menu does the same at once.
- **Keep computer awake**: off, while an agent works, or always (status bar coffee icon). The
  display may still turn off. Linux: through `systemd-inhibit`. While Kelpie keeps the computer awake, a suspend chosen from the system menu waits until it lets go; closing the lid still follows your system's setting; switch keep-awake off (or to *while an agent works*) when you want the computer to sleep.
- **Prioritize visible terminals** (Settings → Agents, on by default, Windows): the terminal on
  screen gets the CPU first, so typing stays quick while other agents build and test or other
  programs keep the machine busy. Nothing else is slowed down.
- **Compact / Clear / Restart, keeping the conversation**: in a session's menu (and the command
  palette, *Agent: …*). Compact and Clear send the agent's own commands; Restart starts a fresh
  process on the same conversation, which helps when a long-running TUI has become sluggish.

### Terminals

Right-click a terminal for copy / paste / select all / clear, **Copy Context** (the last 400
lines as text), splits, your quick commands and hand-offs. `Ctrl/⌘+click` on an address opens it; on a file path
(`src/a.ts:42`, `D:\w\x.py`, or a bare `name.py`) it opens the file in Kelpie's viewer at that line, looked for from session's folder, then the project's, then by its name in the project. Right-click a path for *Open*, *Copy Relative*, *Copy Path*, *Copy Name* and *Reveal in Explorer*, or an address for *Open Link* and *Copy Link*. Settings → Terminal → *Colour
theme* offers Dracula, Nord, One Dark, Tokyo Night, Catppuccin, Gruvbox, Solarized and GitHub
Light (agents are told the same colours when they ask).

Programs that take over the mouse — Claude Code with `"tui": "fullscreen"` (`/tui fullscreen`),
OMP — scroll their own view with the wheel and copy what you select in them to the clipboard
(OSC 52; only from the terminal you are in). Hold `Shift` while dragging to select terminal text
instead. They draw on the alternate screen, so the terminal itself keeps no scrollback for
them: `/tui classic` brings Claude Code's plain scrollback back.

The **floating terminal** (`Ctrl+Shift+\`` / `⌘⌥A`, or the status bar) is a shell in your home
folder, above any project; hide it and it keeps running.

Scripts and agents can drive sessions:

```
kelpied terminal list
kelpied terminal read <session> --lines 80
kelpied terminal send <session> "run the tests" --enter
kelpied terminal wait <session>          # until it stops working
```

`<session>` is a handle, a session id (prefix) or a session name.

Inside a Kelpie session `kelpied` speaks for that session: it can drive other sessions while
Settings → Agents → **Agents can control other sessions** is on (the default) and only its own
session when it is off. Kelpie's settings, automations, custom agents and phone access are never
reachable from inside a session; run `kelpied` from a terminal outside Kelpie for those.
Prompts Kelpie types for you (quick commands, review notes, hand-offs) drop control characters,
and hand-offs, Copy Context and notes or issues sent to an agent have recognisable credentials
(API keys, tokens, private keys, `PASSWORD=` values) replaced by `[redacted]`.

**Long output.** Scrolled up in a long conversation, a pill in the corner counts the lines that
arrived meanwhile and takes you back to the bottom. Every line you submit (Enter) is a mark:
`Ctrl+Shift+↑ / ↓` (`⌘↑ / ↓`) jumps to the previous / next prompt you sent, so a Claude Code
conversation of thousands of lines reads turn by turn; `Ctrl+Shift+End` (`⌘End`) goes back to
the bottom. Settings → Terminal → *Scrollback* sets how many lines each terminal keeps.

### Activity and notifications

The bell in the status bar lists what happened while you looked elsewhere: agents that
finished or need you, automation runs, new worktrees — with the agents working right now on
top. Click an entry to jump there; right-click toggles read. A session you are looking at (the
window has focus and shows it) raises nothing.

Desktop notifications are kept for what needs you: an agent waiting for input, a run waiting
for approval, a failure, the cost cap. Finished turns and automation starts stay in the bell's
list; Settings → Notifications → **Also when a turn finishes** brings finished turns back, and
can add a short chime. The same notification shows at most once a minute. The card icon next to
the bell mutes desktop notifications (it reads *muted* in amber) and turns them on again; the
list keeps everything either way.

#### On your phone, with the window closed

Settings → Notifications → *Off this computer* sends the same events from the session daemon,
which keeps running with the window closed when Settings → Session Daemon → *When Kelpie closes* is *Keep it running in the background*. Nothing is sent until you switch a channel on or a phone
turns its alerts on. A computer that is asleep sends nothing; *Keep computer awake* (Settings → Agents)
keeps it up while agents work.

- **Alerts on the phone app**: on a phone paired on the hosted app, over HTTPS or Tailscale, open the connection sheet (tap
  the connection indicator) and press *Turn on alerts*. On an iPhone the app must be installed to the Home
  Screen first. *Send a test alert* checks it. The computer signs and encrypts each alert itself and hands it
  to the phone's own push service (Apple's, Google's, Mozilla's or Microsoft's); no account is needed and no
  server of ours is involved. An alert says only "A session needs you" or the like, in the phone's language,
  and opens the session when tapped; no project, session or command text is in it. Plain `http://` pages
  cannot get alerts: the browser does not offer them there, and the sheet says so. Removing the phone in
  Settings → Mobile ends its alerts.
- **ntfy**: switch it on, install the ntfy app and subscribe to the topic Kelpie shows (*Show topic*).
  The topic is 128 random bits only you know; use *New topic* if you shared it by accident. An
  access token is optional, and the server can be your own.
- **Telegram**: make a bot with @BotFather, paste its token and your chat id, press *Send a test*. What
  it gets is chosen in Settings → Mobile (see [Telegram's choices](#telegram-alerts-links-and-approvals)).
- **Webhook**: a JSON `POST` to an address you choose.

Choose which events go out (needs you, finished, failed, automation, run approval, cost cap), skip
turns shorter than a number of seconds (30 by default), and optionally send only while Kelpie's
window is not in focus or the computer has been idle for 5 minutes (the system's own clock on Windows; elsewhere, no key or pointer use in Kelpie's window). A session sends at most one message per
30 seconds. Messages carry the project, the session's name, what happened and how long the turn took,
with credentials masked; the end of the terminal screen is added only if you switch that on (it leaves
the computer, through the server you chose). With phone access on, the message links to the session in
the phone app; the link holds no pairing code, so it opens only on a phone that is already paired. Tokens
and the topic are kept in the system credential store, never in a file, and are never shown again.

A service that does not answer costs only itself: each destination has its own queue. Kelpie tries again for
up to 10 minutes (a message that says a session waits for you, for as long as it still does) and drops what you
have answered meanwhile. A server that takes a message and never answers may have acted on it, so ntfy, Telegram and a webhook
get such a message twice at most. So does a webhook that answers with a server error other than 502 or 503, since a flow can act before it fails, also when a session keeps waiting. No connection, no name, no certificate, 502 and 503 mean nothing arrived, and those keep being tried. Under load a phone's alerts about things that wait for nobody may arrive as one; ntfy, Telegram and a webhook keep each message as it is. After a real outage several messages arrive as one ("3 sessions need you, and 2 more updates"). What is still to be
sent stays in `notify-outbox` in the data folder and goes out after the daemon restarts, if it is still true: a session that came back no longer waiting is not told. Text for ntfy, Telegram and a webhook
is kept there until it is sent, and a phone's entries hold no text.

#### When an agent hits the usage limit

A Claude Code session stopped by its subscription's usage limit shows a strip above its pane:
**Continue at reset** (the time comes from the usage meter; when the meter cannot say, type the time
and confirm it), **Switch account…** and **Hand off to…**. The daemon keeps the timer, so it works with
the window closed. When the time comes it types `continue` only if the session is still stopped by
that limit, in the same conversation and the same run, and nothing was typed since its last prompt was sent (a half-written message would otherwise be submitted); otherwise it
cancels and leaves a note in the inbox saying why. Other agents do not report their limits, so they
have no strip.

### Source control and review

The Changes panel pushes (publishing the branch the first time), pulls (fast-forward only)
and syncs; when the branch and its upstream diverged it offers an explicit **force push with
lease**. During a merge, rebase, cherry-pick or revert it shows the conflicts with **Abort**
and **Resolve with AI**. The ✦ button drafts a commit message from the diff (with the agent
chosen in Settings → Agents → *Agent for AI actions*, run once without a terminal); a failed
commit offers **Fix with AI**; *Amend* rewrites the last commit.

**Pull requests** (GitHub, through your `gh`): the branch's PR shows with its state and
checks (also as a chip in the sidebar); *Create pull request…* pushes and opens one, with
**Generate with AI** for the title and description. Failing checks offer *Fix with AI*.

**Review notes**: in a diff, click the gutter (`+`) to note a line; notes on all files of
the project collect in a bar above the diff. **Send to agent ▾** sends them as one prompt to
a running agent or a new one; resolve notes once addressed. `F7` / `Shift+F7` move between
changes. The session daemon keeps the notes (so another agent can write them too, see *Review with…*
below); notes you wrote with an older daemon move over by themselves the first time, after a copy of
the app state is made.

### Files

Besides the editor and markdown preview, CSV / TSV files open as a sortable, filterable
table, PDFs in a viewer, Jupyter notebooks with their outputs, and Mermaid diagrams (`.mmd`
files and ```` ```mermaid ```` blocks in markdown). `Alt+Z` toggles word wrap. Right-click a
folder → **Find in Folder…** searches only there. A project's menu opens it in VS Code,
Cursor, Windsurf, Zed, Sublime Text or IntelliJ IDEA when installed (the first four Kelpie finds on
this computer are listed).

### Automations

Settings → Automations runs an agent with a prompt every hour, day, weekday or week (local
time), in a project. They are stored and started by the session daemon, so they run while
the window is closed; *Run now* tries one immediately. Runs missed by more than 30 minutes
(computer asleep) are skipped, not caught up. *When it finishes* can move the project's card to
the board's Review column once the run's first turn ends (a daemon restart in the middle forgets it).

**When something happens.** *Starts → When something happens* runs an automation on an event instead of the clock:

| Event | Seen by | Also with the window closed |
|---|---|---|
| A file changes (a pattern such as `**/*.md`; a pattern without `/` matches the file name anywhere) | the window's file watcher | no |
| An agent finishes a turn in the project | the daemon | yes |
| A commit is made (HEAD moves on its branch: by hand, by an agent, a rebase or an amend; not a checkout) | the daemon, which asks git every ten seconds and when an agent's turn ends | yes |
| Pull request checks fail (`gh`, looked at when the Git panel refreshes and every five minutes for projects that have such an automation) | the window | no |
| A worktree is finished (Finish merged it) | the window | no |

What it does: start an agent session with the prompt, **type a saved prompt** (a Quick command for agents) into an
session that automation started earlier (a new session the first time, and never one you opened yourself, nor one
you typed into in the last 30 seconds), or **run a command**. The prompt gets a short note of what happened (the
changed files, the commit, listed as names, not instructions). A command is a program and its arguments, started
**without a shell**: nothing is parsed, expanded or interpolated, so a file named `$(rm -rf ~)` is just a name. The
program is looked up in the automation's folder (a path such as `scripts\lint.cmd`) or on `PATH`, with the usual
Windows extensions (`npm` finds `npm.cmd`), and the approval covers the file that was found. An
argument that is only `{file}`, `{files}`, `{commit}` or `{branch}` is replaced by that value; the same data is in
`KELPIE_EVENT`, `KELPIE_ROOT`, `KELPIE_FILE`, `KELPIE_FILES` (one per line), `KELPIE_COMMIT`, `KELPIE_BRANCH` and
`KELPIE_PR_NUMBER`. After saving a command, the list shows its hash and **Approve command**; changing the program,
its arguments or the folder takes the approval away. Commands can only be created, edited and approved from the
Kelpie window; programs running in a session cannot touch automations at all.

Kelpie gives event data only to commands that cannot turn it into code. A program that is not a command interpreter
(`git`, a real `.exe`, a `.cmd` file called directly) gets it as whole arguments. A **command interpreter**
(PowerShell, bash and other shells, python, node, ruby, perl, php, deno…) gets it only when it runs a **script file
inside the project**: `powershell -File scripts\lint.ps1 {file}`, `bash lint.sh {file}`, `python lint.py {file}`; the
script reads the name from its arguments or from `KELPIE_FILE` (in a batch file, quote it). Anything that hands one
line to another shell or machine (`cmd`, `ssh`, `wsl`, `sudo`, `env`, `start`, `xargs`…), and every package manager and build tool (`npm`, `pnpm`, `yarn`, `make`, `cmake`, `gradle`, `mvn`, `msbuild`, `just`, `rake`, and wrappers that run another program such as `uv run`, `poetry run`, `bundle exec`, `direnv exec`, `mise exec`, `nix-shell`, `docker run`, `doppler run`…: they run targets and scripts through a shell, and `npm run` expands `%VAR%` in a name), is not given event data at all,
and neither is `powershell` without `-File`, `bash -c`, `python -c`, `node -e` and the like. Kelpie refuses such a
command when you save it and says what to write instead. The approval covers the **contents** of the script (and of the
program, when it lives in the project): if an agent edits the script, the automation stops with a note in the inbox
until you approve it again. A program Kelpie has no rule for (`git`, a plain `.exe`) is given the names as arguments, and the approval dialog says so in a warning: approve it only if it never runs them as commands. An agent kind of `shell` or `custom` counts as a command too (approval, and no event data
typed into it).

Approval checks read each covered file in full, not just its size or a prefix. If a covered
file cannot be read, the command cannot be approved or run; restore read access first.

Guards: an event automation runs **at most once a minute**, **at most N times a day** (default 10; you are told
once when the limit is reached), **not while a session or command it started is working** (or in the five seconds
after), and never while automations are **paused** (the bolt in the status bar, shown once any automation exists).
The agent an automation starts therefore does not set the same automation off again with its own edits, but it can
set off a different automation. *Dry run* (a checkbox, and a button on every automation) matches events and notes
what would happen without starting anything. *Run now* and a manual dry run use an example event. Automations that
one automation's work sets off in turn are cut off after three links (the inbox says so). Patterns match
case-insensitively on Windows only, and `[abc]` classes are taken literally; a file watched through a folder inside
the automation's folder is matched by its path from the watched folder.

Automations are kept in `automations.json` with a version. An older file is upgraded after a copy to
`automations.json.bak`. A file that cannot be read is renamed `automations.json.corrupt-<time>`, you are told in the
activity inbox, and nothing is written until you press **Dismiss** in Settings → Automations, also after a restart.
Every save keeps a copy as `automations.json.v2`. Downgrading Kelpie is not supported (an older version shows an
empty list and replaces the file with its own); if it happens, the next start of this version tells you and offers
**Restore the newer list** (which also keeps what the older version added) or **Keep the current list**. The daemon
must be restarted once after an update (Settings → Session Daemon) before these options appear; an older daemon would
treat an event automation as a daily one, so the editor does not offer them.

### Layouts

**Tabs and panes.** A tab is one piece of work; panes are what you look at side by side inside
it. A new terminal or session opens in a new tab; *Split right / down* halves the focused pane
and asks what to run there. A tab with one pane has no pane header — the tab shows its agent,
title and host, and the pane's buttons (chat view, dictation, split, close) appear when you hover
its top-right corner. As soon as a tab holds two or more panes, each gets a header and the tab
shows one coloured dot per pane.

**Tile all** (layout menu, or the palette) puts the tab's panes and every tab that holds a
single CLI (terminal or chat) into one even grid: 4 CLIs become 2 × 2, a 5th makes it 3 + 2,
7 make 3 + 2 + 2. It also evens out a grid that splitting made lopsided. Tabs that already hold
a split, and file, diff or browser tabs, are left alone; *Broadcast input* turns off when other
tabs join. **Spread into tabs** does the opposite: each pane of the tab gets its own tab.
Sessions keep running through both.

The grid button in the tab bar (or `Ctrl+Shift+L` / `⌘⇧L`) arranges the tab: presets from
2 columns to 3×3, any number of rows or columns, or per-row counts such as `3,2` (three panes
on top, two below). Existing panes keep their place; new cells are empty panes with a launcher.
The new-session dialog's *Grid…* placement starts one agent per cell of a new grid tab. Drag a pane by its header onto another pane to
swap them or onto its edge to move it there, onto another tab to move it into that tab, or onto
empty tab-bar space to give it its own tab. Drag a tab along the tab bar to reorder it, or onto
an edge of a pane of the open tab to merge its panes in there (palette: *Move Tab Left / Right*,
*Move Pane to New Tab*). Drag gutters to resize, double-click a gutter (or
*Make all panes equal*) to even them out. `Ctrl+Alt+←/→/↑/↓` (`⌃⌥` arrows) focus the pane in
that direction. **Broadcast input** sends what you type to every terminal of the tab (a badge
marks them). Double-click a tab to rename it.

*Save this layout* stores the arrangement with what each pane runs; saved layouts open from
the layout menu and a project's start page, and are renamed or deleted in Settings → Layouts.

### Resuming after a restart

When the session daemon stops while sessions run (computer restart, update, crash), its
sessions come back *interrupted* with their last screen. Settings → Agents → *Resume after a
restart* decides what happens next: resume each one when you open it (default), all at once,
or only with its **Resume** button. Agents continue the same conversation (Claude Code
`--resume <id>`, Codex `resume <id>`, others with their "continue latest" flag); shells start
again below their old output. Kelpie names each new Claude Code conversation itself, so even two
Claude sessions in one folder — or on an SSH host — each come back to their own conversation.

### Several Claude Code / Codex accounts

Settings → Accounts adds sign-ins: each account gets its own config folder
(`<data>/accounts/<agent>/<id>`, used as `CLAUDE_CONFIG_DIR` / `CODEX_HOME`) and you sign in
once in a terminal Kelpie opens there. The *active* account per agent is used by new sessions,
chat view, automations and AI actions; running sessions keep theirs. Right-click a usage meter
to switch; each account has its own meter. The new-session dialog can pick an account, and the
Sessions panel lists the conversations of every account and resumes each with its own.

A running Claude Code or Codex session can move to another account (*Switch account…* in the
usage-limit strip): Kelpie copies the conversation into that account's folder, stops the agent and
starts it again there on the same conversation. It refuses, with the reason and before stopping
anything, when the target is not signed in, already holds a different or newer conversation with the same id (an older copy from an earlier switch is replaced, so you can switch back and forth),
or the conversation is over 1 GB; sessions in chat view or on another machine do not switch.

### Orchestration runs

**Runs** (`Ctrl+Shift+R` / `⌘⇧R`) split a larger job into agent tasks: each task has a prompt,
an agent, the tasks it waits for, optionally an **approval** before it starts and its own
**worktree** (a new branch, so parallel tasks never edit the same checkout). *Plan tasks with
AI* drafts the tasks from an objective for you to review. The session daemon runs them —
also with the window closed — starting each task when its dependencies are done. A task ends
when its agent reports (`kelpied run report done "summary"` / `failed "why"`, which Kelpie adds to
the prompt), exits, or worked and then stayed idle for 5 minutes. Failed tasks can be retried,
skipped or marked done; the status bar shows active runs and tasks waiting for approval.

```
kelpied run create run.json          # {"name", "objective", "cwd"?, "tasks": [{"id", "title", "prompt",
kelpied run list                     #   "kind", "args"?, "env"?, "deps": [...], "approve", "worktree", "host"?}]}
kelpied run show <run> | approve|retry|done|skip <run> <task> | cancel|delete <run>
kelpied run wait <run>
kelpied review add <file>:<line> "<comment>" [--old] [--base <ref>]   # a note on this worktree's diff
kelpied review list [--json]
```

The same prompt to several agents, with a compare view to pick the best, is *Try with several agents…*
(see Review and Finish).

### Sub-sessions

An agent can open other agent sessions to share a job. Unlike the subagents some agents run
inside their own process, a **sub-session** is a real Kelpie session (Claude, Codex, OMP, ...
whatever is installed): it shows in the sidebar, you can open it, type to it or close it, and it
keeps running with the window closed. Nothing to install: agents Kelpie starts get the tools.

**Ask in plain words**, in a session that is Claude, Codex or another agent Kelpie started: *"open
sub-sessions to fix #1 #2 #3 with Codex"*. The agent opens one child per task, ends its turn, and
each child's result arrives as a new message in the parent when the parent is idle: done, blocked,
needs you, or exited. It does not have to poll. A child that stops at a prompt is reported anyway. One that
stops without reporting gets a hint from what Kelpie sees (the end of its screen, redacted), at the
agent's turn end (Claude, Codex) or after a stretch of quiet (about 30 s; 2 min for a Codex child without hooks); the child may still be
working, and only its own *done* report means done. The parent can
also message a child, or a session in another project (delivered when that session is idle, never
into a prompt waiting on you). A message from another session is never you: it cannot approve
anything.

**Approvals.** By default each sub-session asks you first: a card at the bottom right names the
asking session, the agent, folder, task and model, and says in red when the child will run without
asking before it acts. *Allow once*, *Allow for this session* (until the session daemon restarts),
*Always allow* (changes the Settings mode to Allow, with a confirm) or *Deny*. Buttons switch on
half a second after a card appears. The card also lives in Activity (the bell), and Kelpie raises
a notification when the window is not focused. *Later* hides a card; `Ctrl+Shift+A` / `⌘⇧A`
brings the first open request back and focuses it (the key is Kelpie's only while a request is
open). A request not answered in 5 minutes is refused, and one withdrawn when its agent gave up.

**The tree.** A child sits under its parent in the sidebar (if both are in the same project); the
parent row has a fold arrow and a badge coloured by the worst state below it (needs you, failed,
working, finished, idle). Over a session with children, a strip lists them as chips with their
last report; click one to open it, `+N` shows the rest. A child's strip starts with *Sub-session of
<parent>*. *Close finished* on the strip, or in the right-click menu of the strip or the parent's
row, closes the children that are done or exited after listing them; folders and worktrees are
kept. Closing a parent leaves its children running.

**Limits on this computer.** At most 3 levels deep, 10 running children per session, and a limit
across the computer: Auto (about half the cores, or half the memory above 4 GB, between 2 and 8; the
number is shown in Settings) or a number you set. A child that has reported done does not count
while it only sits idle. Spawns are refused, with what is running, when the limit is reached or the
computer is under heavy load. Each child is another agent using tokens and memory; ask for a few.

**Permissions.** A sub-session never gets more than the session that opened it. It starts with
the agent's skip-permission flags only if its opener runs with permission checks off (Settings →
Agents → Permissions: Yolo, or a session started that way); a request from such a session says
so on its card. A session that asks before it acts cannot message one that does not. Agents can only
choose a model for a child, not other arguments, and not start plain terminals this way.

**Agents without the tools.** Claude and Codex (terminal and chat) and ACP chats get the tools
automatically. Other agents see a short brief in the child's prompt and `$KELPIE_CLI` in their
environment; the same operations are a command, in bash `"$KELPIE_CLI" sub spawn codex "fix #1"
--worktree` and in PowerShell `& $env:KELPIE_CLI sub spawn codex "fix #1" --worktree`.
`kelpied sub --help` lists them (`agents`, `spawn`, `list`, `send`, `read`, `wait`, `report`,
`inbox`, `close`, and `open`, which shows you a web page: see "Processes, ports and preview"). `--worktree` gives the child its own branch `kelpie/sub-<name>-<tag>` (`<tag>` is four
random characters, so two children with the same task never share a branch), which closing
never removes (Finish and Review do). Switching a parent between terminal and chat keeps its
children.

**Turning it off.** Settings → Agents → **Sub-sessions**: Off, Ask (default) or Allow, and how many
run at once. Off refuses every request; sessions already running keep their tools until restarted,
and new ones start without them. With Allow no card appears, so every request is a spend you did not
see. If the settings say the session daemon is older, restart it there (conversations are kept).

### Specs and tasks (the Plan tab)

The **Plan** tab of the right panel (palette: *Plan (specs and tasks)*) shows the plans and specs your
repository already has, written by any agent or by you: `plans/<name>/plan.md` with its `phase-*.md`,
`specs/**`, `.kiro/specs/**` (Kiro's requirements, design and tasks) and `docs/plans/**`. There is no
format of Kelpie's own, only markdown with `- [ ]` / `- [x]` task lists. Each plan or spec is a group with
its progress; a file is a phase; unfinished groups are open and listed first. A file with no checkboxes
shows its list items as they are, so nothing is hidden. Links (symlinks, junctions) are not followed, and
nothing outside the project is read or written.

- **Live**: when a file under those folders changes (an agent ticked a box, you edited it, or an agent wrote
  the first `specs/`, `docs/plans/` or `.kiro/specs/` of the repository) the panel follows after a third of a second. With the tab closed nothing is read or parsed.
- **Tick a box** in the panel and the file changes by that one character; line endings and everything
  else stay byte for byte. If the file changed after the panel read it, nothing is written: it is read
  again and you tick once more. Files that are not UTF-8 text, or larger than 256 KB, are listed but
  cannot be ticked from here.
- **Run with…** (the bolt at the end of a task row) starts an agent on that task: choose the agent, and
  whether it works *here* (the folder as it is), in a *worktree of its own* (started from the last commit,
  with the repository's approved `link:` / `copy:` files like any run worktree; a spec file with uncommitted
  changes is flagged because the agent would not see them) or *on a host*: any saved SSH host linked to the
  project is offered, and the agent starts in the project's folder there, the way a pane on that host does
  (for a remote project, *here* is its own host). A task on a host has no worktree, and its agent works on
  the host's own copy of the file, so its tick is made there. It does **not** run in tmux and cannot run Kelpie's
  report command: the task is *done* when the agent exits with code 0, and *failed* when the agent stops with another
  code or the connection drops (the task says so). The agent ends with its connection once the host notices that
  it is gone, which can take a few minutes, so a failed task may still have its agent running there for a while: *Retry*
  and a new *Run with…* ask for your confirmation first. If Kelpie itself restarts while such an agent runs, the task
  is marked failed as the daemon starts, before any window can resume sessions, and its session is not resumed (a resumed agent would be one no task follows). There is no idle
  timeout for such a task; *Mark done* ends it by hand. Needs a daemon of API level 14. It is an ordinary run with one task, so the Runs
  page follows it; the row shows the run's status (click it to open the run). The prompt names the file
  and the section, points the agent at the file the way its own kind reads it (`@path`, or the file's
  content for other agents) and asks it to tick the box when done; agents can also report with
  `kelpied run report`. Headings and task text, and the text of the file when it is included in the prompt, are cleaned
  of escape sequences and of control, bidirectional and invisible characters first. A row follows its run by the
  task's words: if you change them (or add an identical task above), the row loses its chip and the run stays on
  the Runs page.
- **Remote projects** have the Plan tab too: their files are listed, read, ticked and created over ssh. A
  host is not watched, so use the refresh button to read it again (a refused tick reads the file again by itself). A
  tick on a host is checked against the file's content (not only its time, which is whole seconds there) and written
  through a temporary file in the file's folder (made with the host's `mktemp`, so exclusively and private; a host without
  it is not written to) that replaces the old one, after its size and checksum are checked: a
  dropped connection leaves the old file, never a truncated one, and a link stays a link (the target is written; the
  plan refuses a target outside the project). The folder must be writable for that; where it is not, the file is
  written in place, but only after the whole content has arrived. A file deleted meanwhile is not recreated. Saving
  a file from the editor on a host uses the same write and keeps the file's mode; a file that belongs to someone else is
  written in place too, so that its owner does not change.
- **New spec** (the button, or the palette) asks for a name and writes `specs/<name>/requirements.md`,
  `design.md` and `tasks.md` (what is wanted, how it will be built, then the tasks). The name becomes
  letters, digits and dashes. It never overwrites: if any of the three files exists nothing is written.

### Issue trackers and GitLab

Settings → Integrations connects **Linear** (personal API key), **Jira** (Cloud: site, e-mail
and API token; Server / Data Center: personal access token) and **GitLab** (gitlab.com and
self-hosted hosts; public projects work without a token). Tokens go to the OS credential store
(Windows Credential Manager, macOS Keychain), never to Kelpie's files. The **Issues** panel lists
your open issues; start an agent on one, create a worktree for it, or move it to another state.
For a GitLab repository the Changes panel shows the branch's merge request with its pipeline,
creates merge requests (with AI-written description) and offers *Fix with AI* on a failing
pipeline.

### Browser panes and design mode

`Ctrl+Shift+U` (or the globe in the tab bar) opens a web page in a pane: address bar, back /
forward / reload, developer tools and a page screenshot. **Design mode** lets you click
elements on the page: each pick records its selector, text, HTML, attributes (passwords and
token-like values removed) and size, with a screenshot of the element; add a note and **send
to an agent** as one prompt. With Settings → Terminal → *Open localhost links in a browser
pane*, `Ctrl/⌘+click` on a dev-server address in a terminal opens it next to the terminal.
A page that opens a sign-in window of a set size (an OAuth "Continue with…" window) gets it as a
window of its own, so the sign-in can hand back to the page; other links that open a new window
open in the pane.

### Processes, ports and preview

With Settings → Terminal → *Open an agent's web pages in a tab* turned on (it is off on a new install), when an agent on screen starts a web page (a dev server, a studio, a dashboard), the page gets a tab
of its own in the background and a short message says so: the agent stays in view and keeps the
keyboard, and the highlighted tab is one click away. Only a port that answers with a page counts;
one that answers *Not Found* or is not a website gets no tab. A tab you never opened goes when the
page stops, and a page you closed does not come back until the app restarts. Servers you start yourself in a shell
get no tab. Whatever that setting says, an agent can also open a page when you ask it to (*"show me
the page"*) or when it started a server you should look at: it uses the `kelpie_open_page` tool (or
`"$KELPIE_CLI" sub open <url>`), and the page opens the same way, as a background tab of the
session's project with a short message and *View*. Only `http://` and `https://` addresses without a
user name or password are opened; a tab of the project already on the same address is not opened
twice, the message says so instead (another page of the same server gets its own tab, and an
automatic tab the agent asked for this way stays when its server stops). When the window cannot open a
page it was sent (its project was removed, say), the message says what was asked and why. One session
can open at most 10 pages a minute. The tool is offered only to sessions on this computer (an SSH
session's agent does not have it), so `localhost` in the address is this computer. If Kelpie's window is
not open, the agent is told so and gives you the address. The **Processes** tab of the right panel (palette: *Processes and Ports*) lists what
each running session of the project runs, as a tree with PID, memory, CPU and ports. Click a port
to show it in a browser tab, the one already on it if there is one. **Kill** stops a program and what it started. **Kill children** stops what a
session started while the session itself keeps running; in an agent session the agent also keeps
the programs it runs directly (its MCP servers, the shell it runs a tool in), and what those started
(a dev server a tool ran) is stopped. Any of them can still be stopped on its own with **Kill**. Both
ask first and name what would end and which ports it frees. On macOS and Linux a program is asked to
end and forced after 3 seconds; on Windows a program with a window is asked to close it and forced
after 3 seconds, while a console program (most dev servers) is ended at once. Kelpie only stops a
process that is still the one it listed (same PID, start time and program): a program that already
ended, or a number now used by another program, is never touched. After a session is restarted, its
new process is the session's own and is never stopped by either button. A program that left its
session but still runs is listed under *Left the session*. If a port is still in use after a stop,
Kelpie says which program holds it. Kelpie looks at processes only while this tab is open, or for a
visible session: when it appears, shortly after its terminal prints something (at most every 2
seconds), and every 3 seconds while programs run under it. Needs a daemon of API level 16.

### Remote projects (SSH)

*Add Remote Project (SSH)…* (search, or the server icon in the sidebar's project toolbar) picks a host
(a saved one, or one from your SSH config) and a folder on it. Terminals and agents run there over
`ssh -t` — inside `tmux` when the host has it, so they survive a dropped connection — and the file
tree, editor, search and git work through ssh as well. Kelpie uses your OpenSSH client and config
(Settings → Remote picks another config file): keys, agents, users, ports and jump hosts apply.
The launchers show the agents installed on the host; *Open in VS Code* uses its Remote-SSH
extension. Chat view is not available for remote sessions.

### SSH hosts

The **SSH hosts** page (the server icon at the bottom of the sidebar, `Ctrl+Alt+H` / `⌘⌥H`, or
*SSH Hosts…* in search) keeps your servers the way Termius does:

- **Hosts** with a name, group, tags, colour and notes; address, user and port; log in with a key
  (or the SSH agent) or a **password** saved in the system credential store (Windows Credential
  Manager / macOS keychain) — never in Kelpie's files. Jump host, agent forwarding, keepalive,
  environment variables, a start folder and a **startup command** typed into every new shell.
  *New host* and *Edit* open a drawer on the right: address and credentials first, the rest as
  collapsed options, *Save* and **Connect** at the bottom. *Import* adds the hosts of your SSH
  config. The box at the top filters by name, address, group or tag; type `user@host:port` and
  press Enter to add that address as a new host. Each host shows a badge of its OS (seen on the
  last connection).
- **Appearance**: a colour and a **terminal theme** per host — every pane on that host uses it, so
  production never looks like your laptop.
- **One click on a host**: *Terminal*, **Claude Code here** or any other agent the host has, in
  its folder and inside tmux. *Open in* chooses the host's own project or the local project you
  have open. *Install Claude Code* types the official installer into a terminal when the host
  lacks it. *Open as project* adds the host's folder to the sidebar (files, git, search). Sessions
  on a host resume like local ones.
- **Used in projects**: the projects this host is linked to and the folder each uses; link it to
  the open project or unlink it (the host stays saved).
- **First connection**: Kelpie shows the host's key fingerprints before trusting it, and warns
  loudly when a known host's key changed.
- **Files**: two panes, by default this computer (Downloads) on the left and the host on the right. Each pane can
  show this computer or any saved host, so a copy can also go from one host to another.
  - **Panes**: a path bar, up, home, drives (Windows), hidden files, a filter, new folder, and columns you can sort
    (name, size, modified, permissions, owner). Owner and group show as uid / gid numbers (SFTP does not send
    names).
  - **Keys**: Ctrl/Cmd+click and Shift+click select several, Enter opens, Backspace goes up, F2 renames, Delete
    deletes, Tab switches panes, F5 copies to the other pane.
  - **Copy**: drag between the panes, use the arrow buttons or the context menu, or drop files from Explorer /
    Finder onto a host's pane. Copies on this computer itself are for Explorer / Finder.
  - **Transfers** run in Kelpie's background service, so closing the window does not stop them. The queue below the
    panes shows progress, speed and time left, with pause, resume, cancel and retry; a chip in the status bar shows
    them from any page.
    - A dropped connection is retried for up to 5 minutes and continues where it stopped.
    - After the background service restarts, a transfer shows *Interrupted* with *Resume*.
    - A name already there is never overwritten unasked: *Replace*, *Skip*, *Keep both*, *Replace if newer* or
      *Skip identical*, with *Apply to all in this transfer*. Two files of one transfer with the same name (from two
      folders) are both kept, as `name (2)`, and a retry writes the same names again.
    - Names Windows cannot hold are saved with `_` and listed.
    - Links to folders inside a copied folder are not followed.
    - An upload keeps the source file's permissions. From Windows, which has none to keep, a replaced file keeps its
      own and a new one gets the host's usual ones (often `rw-r--r--`). A file edited from Kelpie keeps the server
      copy's permissions.
    - A full disk on the target fails the files it refuses instead of retrying. A transfer that needs more room than
      the target has fails with *not enough free space*; *Retry* continues it once there is room.
    - An update or a switch to a newer background service waits for running transfers and says so (*Waiting for N
      file transfers to finish*; click it to switch now). A transfer that only waits for your answer does not hold
      them: it shows *Interrupted* afterwards, to answer and resume. *Install anyway* / *Switch anyway* stops running
      transfers; they show *Interrupted* with *Resume*.
  - **Host to host**: with a host in each pane, *Copy (Auto)* goes **direct** when the first host can log in to
    the second by itself, as the same user Kelpie uses for it and with its own known hosts. Otherwise the data
    streams **through this computer**, with nothing written to its disk. The queue says which, and why
    (*through this computer: the first host has no login of its own on the second host*). *Copy through this
    computer* always relays.
    - Kelpie never turns on agent forwarding for this. With forwarding, anyone with root (or your user) on the
      first host could use your loaded keys for as long as the connection lasts.
  - **Permissions…** (a host) sets rwx or octal, optionally for everything inside a folder; *x for folders only*
    keeps folders openable and leaves each file's own x as it was, where the file stays readable. Delete on a host asks
    first: there is no trash there. Delete on this computer moves to the trash.
  - **Edit** a host's file *in Kelpie* (through the host's remote project; needs a shell on the host) or *with an
    external editor*: a private copy is opened and uploaded on every save. A change made on the server meanwhile is
    asked about, never overwritten. *Stop* deletes the copy, and copies older than 7 days are removed when Kelpie
    starts.
- **Run a command on several hosts**: tick hosts in the list, type the command (or pick a quick
  command), one terminal each.
- **Keychain**: the keys in `~/.ssh` with type and fingerprint; generate (Ed25519, RSA 4096,
  ECDSA), import, copy the public key, **install it on a host** (`authorized_keys`), add it to the
  SSH agent, save its passphrase, move it to the trash.
- **Known hosts**: every entry with its fingerprint; forget one.
- **Port forwarding**: local (`-L`), remote (`-R`) and SOCKS (`-D`) rules per host, started and
  stopped from the page; rules marked *Start when Kelpie opens* come back with the app.

Saved hosts become entries of a config Kelpie generates (`ssh/config` in its data folder) that
ends by including your own SSH config, so `ssh <alias>` aliases keep working and every remote
project can use a saved host.

### SSH panes in any project

A local project can hold terminals and agents on any saved host, next to its local panes — in the
same tab, split or grid. An empty pane (a split, a grid cell) and *New session…* have a **Run on**
row: *This computer*, the hosts linked to the project, or *Other host…* for any saved one; `Tab`
moves between them in an empty pane. Pick a host, then Claude Code or a shell: it starts in the
host's folder for this project, inside tmux, with the host's saved password or key. A shell with no
folder chosen starts in the host's start folder or login folder; an agent never does — type the
project's folder on the host first (the first one typed is linked to the project). Splitting a
pane on a host offers that host first, and *Hand off* from an agent on a host starts the next
agent there too.

- **Links**: *Link to this project* remembers a host and a folder for the project, so its
  launchers offer it first. Hosts are shared: several projects can link the same host with
  different folders, and one project can link several hosts. A project can also run panes on a
  host it is not linked to.
- **Telling machines apart**: a pane on another machine has a stripe in the host's colour, the
  host and folder in its header, the host on its tab, and the host's terminal theme. Activity
  items name the host.
- **Shared folders**: when an agent of another project already runs in the same folder of that
  host, the launcher says so before you start a second one.
- **Saved layouts** remember which panes ran on which host and start them there again — in the
  folder the project you open the layout in uses on that host. A host that cannot be reached (a
  new host key, offline), or an agent pane on a host where the project has no folder yet, leaves
  an empty pane offering it.
- **Logged in as root**: Claude Code refuses to skip permission prompts as root (it would quit at
  once), so with *Permissions: Yolo* Kelpie starts it on such a host without its skip-permission
  flags and in the default permission mode (a `defaultMode: bypassPermissions` in its settings is
  refused too): it asks for permissions. Before the first such start a dialog explains why
  (*Don't explain this again* turns it off for that host; *Explain again before Claude starts* on
  the host's page turns it back on); the host's check on the SSH hosts page shows *logs in as
  root*. On a host that is only a sandbox, `IS_SANDBOX=1` in its login shell lets Claude skip
  prompts as root, and Kelpie then treats it as a regular login.
- **Set up a regular user** (from that dialog, or the host's page) gives Yolo mode on a root host
  in four steps:
  1. *Create the user* — Kelpie runs `useradd` as root: no password (it logs in with the SSH keys
     root has, yours included, added to its `~/.ssh/authorized_keys`) and no sudo. Running it
     again only adds the keys; an existing system account, one without a login shell, or one
     whose `~/.ssh` is a symbolic link is refused. It warns when sshd's `AllowUsers` /
     `AllowGroups` would keep the user out.
  2. *Add it to Kelpie* — a saved host with the same address and key logging in as the user, then
     a check that it logs in as that user and has Claude Code (*Install Claude Code* if not).
  3. *Give it your project* — folders under `/root` are private to root, so the user cannot open
     them: *Copy in a terminal* copies the project into the user's home in a terminal on the
     root host, which says when it is done (the original stays; an existing copy is never
     overwritten). *Use … for* links the copy to the project.
  4. *Sign in* — each user has its own Claude Code sign-in; its first start asks for it.

  Do not give that user passwordless sudo: agents in Yolo mode could act as root again.

The file tree, search and source control of the project stay those of this computer; use the
host's *Files* or *Open as project* to work with its files.

### Keeping sessions running on a server

Install **tmux** on the server, and Kelpie runs every terminal and agent there inside it. A session
then keeps running on the server when the connection drops, when you close Kelpie or when your
computer sleeps or restarts; opening it again (or *Resume*) attaches to it as it was — an agent in
the middle of a task carries on meanwhile. Without tmux, a session ends with its connection.

The host's check (SSH hosts page, *Check connection*, or *Add Remote Project*) shows `tmux ✓` or
*tmux not installed*; on a known system, **Install tmux** types the command into a terminal there:

| Server | Command |
|---|---|
| Ubuntu, Debian | `sudo apt-get update && sudo apt-get install -y tmux` |
| Fedora, RHEL, CentOS, Rocky, Alma, Amazon Linux | `sudo dnf install -y tmux` (older: `sudo yum install -y tmux`) |
| Arch, Manjaro | `sudo pacman -S tmux` |
| openSUSE | `sudo zypper install -y tmux` |
| Alpine | `sudo apk add tmux` |
| macOS | `brew install tmux` |

Logged in as root, leave out `sudo`; without admin rights, ask the server's administrator. Then
check the host again — *Check connection* on its page, or add a remote project of it again — and
terminals and agents opened from then on run in tmux. *Keep sessions in tmux* on a host (on by
default) turns it off for that host. Kelpie's sessions are named `kelpie-…` on the server
(`tmux ls` lists them, `tmux attach -t <name>` opens one from any SSH client).

### Phone access

Phone access is off by default. Settings → Mobile → *Mobile access* turns it on, and a phone can then reach this computer in three ways:
with the Kelpie app from Kelpie's relay, which needs no Tailscale (the next subsection, and what a new setup uses); with an app served only
by this computer, over Tailscale; or as a browser on your network. Each phone is paired with a one-time code and approved on this
computer, and what it sees and sends is end-to-end encrypted.

**Pair a phone with the Kelpie app (no Tailscale).**

1. In Settings → Mobile, turn on *Mobile access*. In a new setup (one that never had phone access) this also turns on the relay, Kelpie's
   own on Cloudflare, and leaves this network's listeners off, so nothing on this computer is opened to your network. Kelpie asks you to
   confirm once, as described below, for the relay. There is no account and no registration key. In a setup from before, turn on *Reach this
   computer through the relay* yourself; *Pair a phone* under *Hosted phone app* then appears once Kelpie has found the relay serving the app.
2. **Step 1: Install** opens by itself after the switch in a new setup; otherwise press *Pair a phone*. Scan its code with the phone's camera; it holds no
   secret. The page that opens shows how to install Kelpie on the phone (Share → Add to Home Screen on an iPhone, the Install button or the
   browser's menu on Android). Open Kelpie from the Home Screen, then press *Already installed: Step 2* on the computer. A phone that
   cannot install can press *Continue in this browser*, but a browser tab and the installed app keep their pairings apart, on an iPhone
   especially: pair in the place you will use.
3. **Step 2: Pair** shows the pairing code, which works once and expires in two minutes. In the Kelpie app press *Scan the code* (or
   paste the link), then type the six digits it shows into Kelpie. Kelpie's dialog says *It pairs on: hosted app* and offers *Allow reading*,
   the first choice, and *Allow reading and typing*; confirming comes next, as below.
4. The phone then asks to set up its app lock with Face ID, a fingerprint or its passcode. Do it right away (see *The app lock* below).

**What a phone on the hosted app may do.** It can read your sessions and, if you allow it, type into them. It is never given *May take
control* (Settings does not offer it, and the computer refuses it), so it cannot take over a browser pane or see and drive the computer's
screen; those stay with Tailscale or your own HTTPS address. Its app lock is always on, even with *Lock Kelpie on phones* turned off. A
phone gets alerts like any other (the hosted app's connection sheet, *Turn on alerts*), and the link in an alert opens the hosted app.

**The relay is always in the path.** The hosted app has no direct route to this computer, even on the same Wi-Fi: every message goes
through the relay, end-to-end encrypted, so the phone needs the internet and the relay must be up. If it is not, the app says
*Relay unreachable* or *Computer offline*, whichever does not answer. A hosted app belongs to one computer: pairing it with another asks
first, and the first computer loses that phone, including its alerts. A phone that is not seen for the time in *Remove phones not seen for*
under Advanced (90 days unless you change it) is removed, as every phone is; with *Never remove*, a hosted phone unused for a year has to be paired
again. **Moving the relay to another address cuts these phones off**, since it is their only way here: Settings says how many before it does.

**What the hosted app trusts.** The app's code comes from Kelpie's relay on Cloudflare, not from this computer, so you trust whoever can
change what that relay serves. Whoever controls that Cloudflare account, or Kelpie's release deploy, could send your phone different code
that reads your sessions and types into them while the app is unlocked. What narrows this, and what it cannot do:

- **The app is deployed only from a release, by Kelpie's CI, after the maintainer approves.** Each release publishes the app's
  fingerprint (a hash of every file) and the list of file hashes, so anyone can compare them with what the relay serves, after the fact, and
  rebuild the release to get the same fingerprint. That finds a broad swap. It cannot find code served to one phone only or between two
  checks, and the account's owner, or someone who steals the account or a session, can still deploy from Cloudflare directly.
  **Nobody can check what one phone received**, and the version and fingerprint the app shows come from the same relay, so changed code
  could claim any.
- **The app lock is checked by your computer.** A key copied by changed code reaches nothing, and changed code acts only while you have the
  app unlocked; while it is unlocked it can see and send whatever that phone may.
- **Typing is the ceiling.** Changed code cannot drive the computer's screen or a browser pane, but it can type into the terminals and chats the
  phone may write to, and so run commands through a session. Allow typing only on phones you would trust at your keyboard.
- **The relay itself** sees that a phone and this computer are connected, their internet addresses, when and how much they send, and the
  room's number, never what is said. It keeps no logs of what passes.

If that is more trust than you want, use the Tailscale option below: the app then comes from this computer, and Kelpie's relay serves nothing
a phone runs. A relay of your own on a server serves no app either; one on your own Cloudflare account serves the app you deploy there and makes
you the one to trust ([relay-self-host.md](relay-self-host.md)).

**Checking what the relay serves.** From a clone of this repository, `python -I scripts/hosted/verify_live.py` asks the relay which
release it says it runs, fetches every file that release's manifest lists, and compares each file's hash and size, the response headers
and `/version.json` with it. It exits 0 when everything matches, 1 on a difference and 2 when it could not finish. It can show that the relay serves, to your
computer at that moment, the files of a Kelpie release. It cannot show what a phone received at another time, or code a phone already
cached. Options (a given release, a rebuild, the deployed scripts) are in [relay-self-host.md](relay-self-host.md#checking-what-the-relay-serves).

**Approving a phone, and a browser on your network.** With *Pair browsers on this network* on (under Advanced; on for a setup from before,
off for a new one), Settings → Mobile also serves a small web app for your phone on your network (port 4817; choose a private or Tailscale
address — never every interface; HTTPS with your own certificate, e.g. from `tailscale cert`, or from Tailscale itself as described below).
Off, nothing on this computer listens on your network, and phones come through the relay or Tailscale; turning it off closes the port at
once, and phones paired on this network then reach this computer only when it is on again. Settings does not let you turn it off while
neither the relay nor Tailscale is on, since nothing else would reach this computer. **Pair a browser on this network** (offered only while
it is on) shows a QR code valid
for two minutes; scan it, name the phone, and type the six digits it shows into Kelpie. Windows then
asks you to confirm with Windows Hello or your password (Touch ID or your password on a Mac). Read
that prompt: it names the phone and what it would be allowed to do. While it is up Kelpie's window
does not respond; your sessions keep running, and the window comes back as soon as the prompt ends. On
Linux the confirmation comes from polkit, your desktop's password prompt, and it asks for an
administrator's password. **Its text is generic and does not say which phone this is about**, so read
Kelpie's own dialog before it: that one names the phone, its key and what it asks for. Without polkit or
an authentication agent (a headless login) Kelpie refuses and says what to do. Scanning a new code on a phone
that is already paired to this computer replaces its old entry in the list; if you cancel, the old
pairing keeps working. Raising what a phone may do,
resetting phone access and keeping unused phones paired for longer ask the same way; declining a phone,
lowering a permission and removing a phone do not. If you refuse three prompts in a few minutes,
or leave three unanswered in 20, Kelpie stops asking for five minutes: if you did not start them, use
*Turn off phone access and revoke all*, near the top of the page. A prompt you leave open for nearly five minutes stops counting, and
while its dialog is still on screen Kelpie asks you to close it before showing another.

**How the page is laid out.** The top of Settings → Mobile says in a few lines how a phone reaches this computer now, whether the app lock is on, where alerts go and how many phones are paired. Below it is one row per option, each with the same lines: *What it does*, *Trade-off*, *How safe*, and *Which phones* where that differs. A row that cannot be changed right now says why and what to do first, and one that will ask you to verify says so. Your network address, port, certificate, the list of paired phones and resetting are under **Advanced** at the bottom. *Turn off phone access and revoke all* stays outside it, near the top, so it is in sight when a phone is lost. *Show them* in the summary opens it, and so does a phone that must be paired again.

| Option | Starts | Changing it asks you to verify |
| --- | --- | --- |
| *Mobile access* | off | in a new setup, turning it on (it turns the relay on too); otherwise no |
| *Reach this computer with Tailscale* | off | turning it on |
| *Reach this computer through the relay* | off, until a new setup's *Mobile access* turns it on | turning it on |
| *Alerts on your phone* | off, until a phone turns them on | no. Each phone turns them on for itself, in the phone app, so here it only shows how many have |
| *Lock Kelpie on phones* | on | turning it off |
| *Lock after idle* | 5 minutes | a longer time |
| *Ask again for dangerous commands* | off | turning it off |
| *Allow reading over plain HTTP* | on | turning it on |
| *Pair browsers on this network* | on, but off in a new setup | no |
| *ntfy*, *Telegram* | off | no |
| *Telegram gets* | *Alerts with a link* (a setting from before keeps *Alerts only*) | choosing approvals |

The *ntfy* and *Telegram* switches here are the same ones as in Settings → Notifications, which keeps their server, topic and tokens. *Allow reading over plain HTTP* only counts while the lock is on: with the lock off such a page reads anyway.

<a id="telegram-alerts-links-and-approvals"></a>**Telegram: alerts, links and approvals.** With Telegram on, *Telegram gets* has three choices:

- **Alerts only**: what Kelpie sent before there was a choice. A setting from an earlier version stays here, so nothing changes until you choose; the link to the session comes only with *Include a link to the phone page* in Settings → Notifications.
- **Alerts with a link** (what Telegram gets when you switch it on now): each message ends with a link to the session in the phone app, at the Tailscale address when there is one. Telegram sees the link, with this computer's name or address in it, but does not open it, and the link holds no code or token.
- **Alerts with a link, and approvals of low-risk requests**: off unless you choose it. A message about a request Kelpie labels **low risk** gets two buttons, *Approve once* and *Deny*; everything else (risk unknown, medium or dangerous) gets the alert and the link only, and you answer it on the computer or a paired phone.

Read what approving from Telegram gives up before choosing it: **Telegram can read these messages** (a chat with a bot is not end-to-end encrypted, and it shows the command), **a tap asks for no Face ID**, and **anyone who has this Telegram account can approve** (a stolen phone, a SIM swap, a session left open elsewhere). Kelpie asks you to verify first (Windows Hello or your password), then shows a code: from the Telegram account that will approve, send `/bind <code>` within five minutes in the chat set in Settings → Notifications (your chat with the bot, or the group). A code sent from any other chat, or by a group's anonymous admin, binds nothing, and the bot says why (once a minute at most in each chat). That account in that chat is then the only one whose presses count; Settings shows its name next to its number, so you can tell it is yours. Only the installed Kelpie app can turn this on, and only on the computer.

How the buttons behave:

- Each pair answers one request, once, for five minutes. It stops working when the request is answered anywhere else (the message then says so), when Kelpie restarts, and when you turn approvals off. The first answer wins: a late press is told who answered.
- Only the one-time choice is offered. "Always", "don't ask again" and "for this session" stay on the computer, and so does a request whose one-time yes Kelpie cannot tell for certain (two requests at once, a terminal menu it does not recognise, or a menu that does not show exactly the command the agent reported).
- A file edit, write or patch gets no buttons, even inside the project: Telegram would show only the file's name, not what goes in it. Answer it on the computer or a paired phone, where you can see the change.
- A command that takes more than one line in the agent's permission dialog gets no buttons either, only the alert and its link: on screen, a long line that wrapped looks just like two commands.
- At most six approvals a minute from Telegram; past that, messages come without buttons until the minute is over.
- Every approval, denial and refused press is in *What phones did* under Advanced, as *telegram*, with the command as a length and a hash. Presses from any other account or chat do nothing, and their count is noted there once a minute, as is the count of presses of buttons that no longer work.
- **Turning it off**: choose *Alerts with a link* or *Alerts only*, or switch Telegram off. Another chat id or bot token turns approvals off too, and so does *Turn off phone access and revoke all*, turning phone access off, and resetting it. The messages already sent lose their buttons, and turning approvals on again needs a new code. While a code is shown, choosing *Alerts with a link* or *Alerts only*, switching Telegram off, or changing its chat id or bot token ends that code; changing other settings leaves it waiting.
- If Telegram refuses the polling because the bot has a webhook or another program reads its messages, Settings says so; remove the webhook (`deleteWebhook`) or stop that program. A token Telegram rejects stops approvals, and Settings says that too. Kelpie only ever connects out to Telegram: nothing listens on your computer for it.

**Reaching this computer from anywhere, with Tailscale.** This is the option for a phone app that only this computer serves, and for
seeing the computer's screen or taking over a browser pane from a phone, which the hosted app never does. If you use [Tailscale](https://tailscale.com) on
both the computer and the phone, turn on *Reach this computer with Tailscale* in Settings → Mobile. Kelpie then
publishes the phone page at `https://<machine>.<tailnet>.ts.net:8443` with `tailscale serve`, which only your
own Tailscale devices can reach: there is no Kelpie server and no Kelpie account. It asks you to confirm first, as above. Settings says
exactly what to do if Tailscale is missing, stopped, not signed in, or your tailnet has no HTTPS certificates
(turn on MagicDNS and HTTPS in the DNS page of the Tailscale admin console). If something else already uses port 8443
of `tailscale serve`, Kelpie says so and leaves it alone; turning this off removes only Kelpie's own entry.

Pairing then takes two steps. *Pair a phone over Tailscale* shows **Step 1: Install**, a code with no secret: scan
it with the phone's camera and install the app from the page that opens (Share → Add to Home Screen on an
iPhone, the Install button or the browser's menu on Android). The very first visit can take up to a minute while Tailscale gets its
certificate. **Step 2: Pair** shows the pairing code: open Kelpie from the
Home Screen, press *Scan the code* (or paste the link, if the camera cannot be used), and type the six digits on
the computer. A browser keeps its pairing per address, so a phone paired over plain HTTP stays paired there, read-only, and must
be paired again to type over Tailscale. If the phone says *Turn on Tailscale*, it failed to reach this address at once. Tailscale being off on the phone is the likely reason (an iPhone allows one
VPN at a time), but it may also show when this mode or phone access is off on the computer, so check both; the app still opens and shows what it last saw. While this is on, links in notifications open at the Tailscale address, so a phone paired only over plain HTTP
cannot use them.

What to know: **use this only on a computer with one user account.** The Tailscale entry stays while Kelpie is not running, so another user account on the same computer
could take its place, answer your phone at the trusted address, read this phone's Kelpie key and later use it. A program on your computer, or a device on your tailnet, can also keep
phones from connecting for as long as it keeps trying, with no key and no access to anything. **Your computer's name goes into public certificate logs** when Tailscale gets its certificate, and that
cannot be undone. Tailscale sees which devices connect and when, not what they say, which is end-to-end encrypted
inside the connection. [Tailnet Lock](https://tailscale.com/kb/1226/tailnet-lock) protects you from a device key handed out without your approval. The free
Tailscale plan is for non-commercial use. Turning phone access off, the kill switch and a reset all end this mode and
remove the entry; turning phone access on again does not bring it back until you turn it on and confirm. If Tailscale is stopped at that moment, Settings says the entry is
still there. Kelpie tries again when you open Settings → Mobile or press *Check again*, and when it next starts; nothing runs in the background to retry while phone access is off.

**The relay.** With *Reach this computer through the relay* on, phones reach this computer through a relay on Cloudflare that passes on
what they say without being able to read it: the hosted app always, and an app installed over Tailscale when Tailscale is off on the
phone, or a network blocks it. This computer connects out to the relay; nothing on it is opened to the internet. Turning it on asks you to
confirm, and makes this computer's room there once. Kelpie's relay needs no registration key; a relay you run may ask for one, and Settings
shows *Registration key* only then (it makes the room and stays in the credential store). *Relay address* is Kelpie's relay unless you run
your own ([relay-self-host.md](relay-self-host.md)). Each phone has a token it gets when it pairs on the hosted app, or the next time it
connects over Tailscale, and that changes every week; removing a phone shuts it out of the relay too, even while this computer is offline.
A phone installed over Tailscale tries the Tailscale address first and the relay a moment later, and goes back to the Tailscale address by
itself once it answers again.

What to know: **Kelpie's relay serves the phone app; a relay you run on a server serves none.** With such a relay, *Hosted phone app* says
*This relay does not serve the phone app: pair phones over Tailscale*, and installing the app on a phone needs Tailscale the first time.
**A phone installed over Tailscale is let in through the relay only from this computer's Tailscale address**, so Tailscale mode must have
been on at least once for those phones, and its row says so; phones on the hosted app need no Tailscale. If this computer's Tailscale name
changes (renaming it, or MagicDNS giving it a new name), phones installed from the old name are cut off from the relay as well as from the old
address, until you install and pair them again at the new name. With the relay down
and Tailscale off an app installed over Tailscale has no way to this computer, and says *No way to your computer*; *Relay unreachable* and
*Computer offline* say which of the two does not answer. The relay sees that a phone and this computer are connected, their internet
addresses, when and how much they send, and the room's number, never what is said. It serves the phone app too (see *What the hosted app
trusts*), and keeps no logs of what passes; *What the relay can see* under its row says the same. If the relay stays slow from
this computer for a day, its row offers *Make a new room*, placed near where the computer is now; phones move to it by themselves. When the relay is turned on or moved while the app is open, the app reloads itself the next time you come back to it, or at once if it cannot reach the computer any other way: a page can only reach the relay its own page allows. When you unpair a phone while the relay is on, and making a room needs nothing typed (Kelpie's relay, or a relay whose registration key this computer keeps), Settings asks whether to make a new room too, and the answer is *Not now* unless you choose otherwise: the removed phone still knows the old room's ticket, though the relay no longer lets it in. Phones connected at that moment move to the new room by themselves, over their encrypted link; a phone that is not connected cannot use the relay until it connects once over Tailscale, which a phone on the hosted app cannot do, so it has to be paired again. Making a room asks you to verify.

**The app lock.** A phone paired at a name over Tailscale or HTTPS, or on the hosted app (which is locked even with *Lock Kelpie on phones* turned off), opens only after Face ID, a fingerprint or its passcode, and your computer checks that. Someone who copied the phone's key cannot get in while it is unlocked, cannot keep it unlocked, and cannot set up a passkey of their own, with one gap: in the five minutes after you approve a phone or press *Reset this phone's lock*, someone with its key could set one up first. Your computer tells you when a phone sets up its lock: if that was not you, unpair it. Set it up right after pairing, when it asks. If those five minutes have passed, it says to press *Reset this phone's lock* on the computer first. Leaving the app covers it at once. Coming back within the time chosen in the phone's Settings asks nothing (5 minutes unless you choose *Immediately*, 1 or 15 minutes); after that time, after a reload or an app update, or once the idle time in Settings → Mobile runs out (5 minutes unless you change it), it asks again. Seeing or controlling the computer's screen or a browser pane asks again after any time away, and leaving the app ends them. On an iPhone the app switcher's picture may still show the last screen. Unlocking is one tap and then Face ID: it never starts by itself. A network that drops and comes back while you are using it does not ask again; restarting Kelpie on the computer does.

**Phones paired with Kelpie 0.1.11 or earlier** are not covered by this lock, and can only read until you pair them again through Tailscale mode, however they connect. That includes a phone that could type over a Tailscale address or your own certificate before. To keep typing on the same pairing instead, turn off *Lock Kelpie on phones* in Settings → Mobile. Reading without a lock can be closed with *Allow reading over plain HTTP*.

If Face ID was reset or you moved to a new phone, press *Reset this phone's lock* next to it in Settings → Mobile: for the next five minutes that phone may set up a new passkey. There is no other way past it. A plain `http://` page has no way to do any of this. **So by default it can read everything with no lock at all: your sessions, screens, chat logs and the commands waiting for approval.** While the lock is on it can never type. That includes a plain `http://` address on your tailnet, and an HTTPS address that is a number rather than a name, since a passkey needs a name. Turn off *Allow reading over plain HTTP* to close it; to type, use the Tailscale HTTPS address. What the phone last showed stays in its browser's storage, not encrypted, for as long as it is paired. Turning the lock off or lengthening the idle time asks you to confirm on the computer.

**Settings on the phone.** The gear at the top of the phone app, or *Settings* in the sheet the connection indicator opens, holds what the phone keeps for itself; like everything else it stays behind the app lock. *App lock* sets how long the app may be out of sight before it asks again (*Immediately*, 1, 5 or 15 minutes, and a choice longer than the computer's idle time says it is limited to that time), with *Lock now* and the lock's own settings shown read-only, since only Settings → Mobile on the computer changes them. *Alerts* turns this phone's alerts on and off. *Display* has the theme, the language, the text size of terminals and chats, and whether long terminal lines wrap or scroll sideways. *Keys* picks which keys the key bar shows and in what order, with Ctrl always first; *Ask before sending Ctrl+C*, on unless you turn it off, asks before Ctrl+C goes from the key bar or from Ctrl with C in the reply box, because it stops what the agent is doing. Where the phone can vibrate (not on an iPhone), *Vibration* turns off the buzz when a session starts needing you. *Connection* names the computer, how this phone was paired, the way it connects now (Tailscale, the relay, this network or this computer), the round trip, what this phone may do and, when the computer's screen is not offered, why not. *Clear what this phone keeps* removes the saved session list and screens, and drafts too if you tick that box, while the pairing stays; *Forget this computer* removes the pairing from the phone. The connection indicator itself reads *Live · through …* with the way it takes.

**The inbox on the phone.** A switch above the session list shows it *By status* (the default) or *By workspace*, with one section per project, named after it; a session with no project is listed under the folder it runs in. Sections with a session that needs you come first, then the most recently active, and each row says its state in a word. A stopped session has a grey dot and says when it ended. *Inbox* in the phone's Settings has the same choice, *Order inside a group* (the most recent activity first, or by name from A to Z) and *Show stopped sessions*, on unless you turn it off. While it is off, a line under the list says how many stopped sessions are hidden, and tapping it shows them again. Whatever the order, sessions that need you come first in their group, the longest wait first.

**What a phone is asked to approve.** Every approval card leads with Kelpie's own label (*Low risk*, *Medium risk*, *Dangerous*, *Risk unknown*) and what it is based on, then the whole command, with anything that could hide part of it written out. A long command has to be scrolled to its end before you can approve it. The label is Kelpie's reading of the command and not the agent's description. It is information and not a guarantee: it reads words, it is not a shell, and a command built to fool it can. A terminal prompt has a command and a label only when the agent's hook said what would run; otherwise the label says the risk is unknown, and the session's screen above it is what there is to read. *Ask again for dangerous commands* (off unless you turn it on) makes approving something labelled dangerous ask for Face ID again. While such a request is open in a terminal, typing and keys from the phone are refused, apart from Esc and Ctrl+C: answer it on the computer, or on its card if one is showing. Once it has been answered that way, or you pressed Esc, you can type again. *Risk unknown* does not ask again, so a command Kelpie could not read gets no second look. Only Claude Code's hooks say what a terminal prompt would run: with any other agent in a terminal, every prompt is *Risk unknown*, so this does not cover it. Chat sessions label their requests themselves.

The phone lists every session grouped by *Needs you / Working / Idle / Stopped*, buzzes when one
starts needing you, shows a session's screen and — if you allow that device to type — sends replies
and keys (Enter, Esc, Ctrl+C, arrows, y / n, 1–3). A browser connected over plain HTTP
can only read, because someone on the network could swap the page; with the app lock off, one on a
Tailscale address can type. The list and the screen
update as soon as something changes. A reply typed while a menu or permission prompt is waiting is
refused (answer with the keys instead). The phone offers no button for a choice that lasts ("always",
"don't ask again"); a device allowed to type can still reach one with the arrow keys, Shift+Tab or a
command, so allow typing only on phones you would trust at your keyboard. Recognising such a choice is best
effort: a new agent's wording may still get a button, or an ordinary option lose one. Unpairing a device
disconnects it at once.

A session in chat view opens as a chat on the phone: the conversation, its tool rows and its approval
cards. A phone allowed to type sends messages into it (plain text), answers approvals and questions, and
stops the agent or cancels a message it queued. The computer keeps one queue per chat, so messages from
the window and from phones go to the agent one at a time, in the order they arrived. A message only the
window can build (with a picture or a command) waits for its chat to be open there, and the phone says so.

**Taking over a browser pane from the phone** (Windows). When an agent is stuck on a sign-in, a consent screen, a 2FA code or a check in one of Kelpie's
browser panes, a phone may drive that pane: *Take over a browser pane* in a session lists the panes with a page open, those of the session's project first.
The phone shows the page; a tap clicks, a swipe scrolls, holding still for a moment (0.4 s) and then moving drags, pinching does not zoom the page, the key row has Enter, Tab, Backspace, Esc and the arrows,
and the text field types into the page (a password goes as one message, and the phone says *Sent to the page* or *Not sent*; it is never sent twice by
itself). A sign-in window the page opens comes to the phone too, and closing it goes back to the page. On the computer the pane's tab comes to the front and
stays there (switching away brings it back once, since the phone sees nothing of a hidden page; switch away again and it stays where you put it; the phone then says the pane is hidden on the computer, as it does while Kelpie is minimised, and sends nothing until the pane shows again), with *Controlled from <phone> · Stop* above it; the sign-in window's title says the same. Stop on either side ends it, and so do 10 minutes with
nothing typed, the phone locking or leaving the app, and removing the phone or lowering what it may do, within a second or two.

Only a phone given *May take control* can do this, and Settings → Mobile offers that only for a phone paired over Tailscale (or your own HTTPS name) whose app
lock is set up, while *Lock Kelpie on phones* is on; giving it asks you to verify, and that is the only confirmation a takeover asks for. Turning the lock
off takes control away from every such phone, which can then only type: after turning the lock back on, give it *May take control* again. It works over Tailscale or that HTTPS address, not through the relay, so never for a phone on the hosted app. What is typed
is not recorded anywhere: *What phones did* says only when a takeover started and why it ended. If the computer is very busy a new takeover waits; if the
picture stops for a few seconds the phone dims it and sends nothing until it is back, so a tap never lands on a page that changed meanwhile. A program on the
computer that tries to stand in for Kelpie's window here is refused, and Kelpie says so.

**Seeing the computer's screen from the phone** (Windows). When an agent waits on something outside Kelpie — a dialog, an installer, the app it is building,
an IDE — a phone given *May take control* can see one of the computer's screens or one window and use it: *See the computer's screen* in the inbox or in a session lists the
screens, then Kelpie and the code editors that are open, then the other windows. On the phone one finger works as a trackpad: it moves the computer's pointer,
a tap clicks where the pointer is, holding still then moving drags, and a long press right-clicks; *Direct touch* clicks under your finger instead. Two fingers
scroll, pinching zooms the phone's view (not the computer's), and two fingers tapped together right-click. The key row has Ctrl, Shift, Alt and Win (tap one,
then a key, or type one letter with it held for a shortcut such as Ctrl+S or Win+E), Esc, Tab, Enter, the arrows, ⊞ (the Windows key alone, for Start) and a few more; the text field types on the computer and
the phone says *Typed on the computer* or *Not sent*. Under the picture the phone says how it is going (`720p · 15 fps · reduced: computer busy`); tap that
line for details. When text stands still it is sent once more as a sharp picture (*Crisp text* in Settings → Mobile → Advanced, on by default; it uses more
data, so it turns itself off while the connection is slow, mobile data included, and the phone says *crisp text off: slow connection*).

On the computer an orange border goes around what is shared, with a badge naming the phone and the stop hotkey, and the status bar says *Shared with <phone>*
with *Stop*. **Ctrl+Alt+Shift+K stops it from anywhere**; it is always on (you can pick another key in Settings → Mobile → Advanced, not turn it off, and Kelpie
tells you if another program holds it). Stop on the phone ends it too, and so do 10 minutes with nothing done, the phone locking or leaving the app (at once,
with no grace), removing the phone or lowering what it may do (within a second), and closing the shared window. It needs the Kelpie app open on the computer (the phone says so otherwise), works over Tailscale or your HTTPS
address and never through the relay, and one phone at a time. If the computer is very busy a new screen waits, and after five very busy minutes the picture
pauses until you tap it. The phone says when the computer is locked, when a Windows security prompt (UAC) is up, when an app running as administrator is in
front, and when the window is minimized: those cannot be seen or answered from the phone. What is typed or clicked is not recorded anywhere: *What phones
did* says which program was shared (such as `Code.exe`, never the window's title) or that it was a whole screen, when, and why it ended.

Keys and text go only into the shared window: if another window has taken the focus on the computer, Kelpie brings the shared one to the front once,
and if that does not work it types nothing and the phone says *Not typed*, so a password never lands in a chat that popped up. Sharing does not start
if the orange border cannot be shown or another program holds the stop hotkey (Kelpie says so: pick another hotkey), nor while Kelpie itself runs as
administrator. A 4K or larger screen is shown at 1080p, with crisp text at its full sharpness; a rotated screen cannot be shown yet (share one of its
windows). If the picture stops while the computer says it is sending, the phone says *The picture is stuck* and asks for a fresh one.

**What *May take control* means for the screen.** A phone given it can do anything you can do at this computer while signed in: for example Win+R and a
command runs any program as you. Win+L locks the computer, and the phone cannot unlock it again: someone has to sign in at the computer. Ctrl+Alt+Del
cannot be sent from the phone, and Windows security prompts (UAC), the lock screen and sign-in cannot be seen or answered from it. Give it only to your own
phone, with its lock on. The phone needs a browser that decodes H.264 video with WebCodecs: Safari on iOS 16.4 or later (iOS 26 works
best), or Chrome on Android; an older one says so.

**"Not paired here" on the phone** means the computer did not recognise this phone, or keeps not
answering it. It may have been removed there, phone access may have been reset, something on the
network may be interfering, or the answer may not have come from your computer. The phone keeps its pairing and tries again by itself. If it was
removed on purpose, tap *Forget this device*, then pair again. A phone only forgets its pairing by
itself when your computer tells it so over the encrypted connection. *Turn off phone access and
revoke all* removes every phone and cuts access at once, connected or not. A phone connected at that moment
is told, and wipes its key and everything it kept. One that is not connected cannot be told: its key no longer opens anything, but it
keeps what it already shows, for anyone who can open the app on it, until you tap *Forget this device* there. Once phone access is back on it says *Not paired here*.

Kelpie's check stops a program that merely holds your account's Kelpie token from adding a phone. It
does not stop a program determined to get around it: your account's programs can read the keys in
the credential store and change Kelpie's files, including the list of phones.

Turning Mobile off also closes existing phone connections, not just the listening port.
Paired devices stay saved, but must reconnect after access is enabled again; an old connection
cannot keep reading or answering prompts across that switch. Turning it off is not how to cut off a
phone: any program holding your account's Kelpie token can turn it back on, which is recorded in
*What phones did*, and every paired phone then has its access back. To cut a phone off, use *Unpair*,
under **Advanced**, or *Turn off phone access and revoke all* near the top of the page for every phone.

### Voice dictation

Settings → Voice downloads a speech model once (checked against a pinned checksum): a fast
Vietnamese model (26 MB) or Whisper tiny / base / small for about 100 languages (104–375 MB).
Then `Ctrl+Shift+Space` (`⌘⇧Space`) or a microphone button (chat composer, terminal panes,
Settings) starts listening; press it again (or **Done**) and the words are inserted where you
were typing — at the cursor of a text field, or pasted into the focused terminal without
pressing Enter. `Esc` cancels. Speech is recognised on this computer by a separate helper
process (`kelpie-stt`) that exits after ten idle minutes; nothing is sent anywhere.

### Agent setup

Settings → Agent setup (or the link on a project's start page, or the palette) is one page for
what your agents are told and can use. It reads their files once when you open it (*Refresh*
reads again); nothing is watched in the background.

- **MCP servers**: Claude Code (`~/.claude.json`, the project's `.mcp.json`), Codex
  (`config.toml`), Gemini (`settings.json`), OpenCode (`opencode.json`) and Cursor
  (`.cursor/mcp.json`), each with the user's file and the project's. Every server is tagged
  **user** or **project · untrusted** (a project's file comes with the repository). Switches turn a
  server off the way each agent does it (Codex and OpenCode `enabled`, Gemini `mcp.excluded`, Claude
  for a project in `.claude/settings.local.json`); Claude's user file and Cursor have no switch.
  **Copy to…** converts a server to another agent's format and writes it to the files you tick; *Add an MCP
  server* does the same for a new one, so one entry serves Claude, Codex and Gemini. Environment
  and header values are shown by name only and are never written into a project file (they become
  references to a variable of the same name); writing the values to your own user-level file is an
  opt-in. Copying a **project** server to a user-level file skips the agent's own "trust this
  server?" prompt, so the preview lists everything it would run (the program and each argument,
  the working folder, every environment and header entry as the repository wrote it, a reference
  to one of your own variables named, never expanded) and the button stays off until you confirm.
  A path relative to the project (`./scripts/mcp.sh`) is made absolute, so it keeps running that
  project's file, and the preview says so. None of the project's environment is copied unless you
  tick the box, and Kelpie never invents references to your own variables for it. Codex gets
  variables through its own `env_vars`, `env_http_headers` and `bearer_token_env_var`; `Bearer ${TOKEN}`
  and other references inside a value are rewritten into each agent's syntax.
- **Files of a project that lead elsewhere** (a link or junction out of the repository, on
  `CLAUDE.md`, `AGENTS.md`, `.mcp.json`, `.cursor/`, `.gemini/`…) are neither read nor written; the
  page names where the link goes. `CLAUDE.md` linked to `AGENTS.md` counts as already one file.
- **While Claude Code runs**: `~/.claude.json` is changed under Claude's own lock
  (`.claude.json.lock`) and only if it still matches the preview; if another program saves it
  right after Kelpie's write, Kelpie leaves that save alone and says so instead of reporting a
  success.
- **Instructions**: the CLAUDE.md, AGENTS.md, GEMINI.md, `.kiro/steering` and
  `.github/copilot-instructions.md` of the project and the user-level ones. **One file for all
  agents** makes AGENTS.md the source: CLAUDE.md imports it with `@AGENTS.md`, Gemini lists it in
  its settings (`context.fileName`) or gets a copy. Nothing is deleted.
- **Skills & commands**: a read-only list of the skills, commands and subagents the agents find
  (`~/.claude/skills`, `.claude/commands`, Codex prompts, Gemini commands, OpenCode agents…), with
  a button to show the file.
- **Find more agents**: *Search the registry* makes one request to the Agent Client Protocol
  registry, only when pressed. Each entry shows the full command with its version pinned;
  *Save as custom agent…* asks for confirmation and then adds it to the agents Kelpie can start
  (`npx` and `uvx` entries; downloaded programs are not supported). The package is pinned to an
  exact version (a tag such as `latest` or `beta`, a range, or a URL / git spec is replaced by the
  registry's own version or refused) and the page shows it. Such an agent has no terminal
  interface: it starts as a chat, and its launcher card shows the real command. Saving never runs it.

Every change goes through the same steps: a **preview** (a diff, secrets masked), a **backup** of
each file (in Kelpie's data folder, `agent-config-backups`, newest 20 per file; *Backups* at the
bottom restores one, again with a preview), the write, and a check that the file still parses; if
it does not, the files go back as they were at once. Comments, key order and keys Kelpie does not
know stay as you wrote them.

### Plugins

A plugin is a folder with an `kelpie-plugin.json` manifest that adds agents, palette commands,
quick commands, terminal themes, layouts and status-bar items. Settings → Plugins installs one
from a folder or a git URL, or creates a starter to edit. A plugin does nothing until you
**Review & enable** it: the review lists every program it would run. Editing the manifest turns
the plugin off until you review it again. Format: [plugins.md](plugins.md).

### Language

Settings → Appearance → *Language*: English or Tiếng Việt (Vietnamese); *System* follows your
computer. Dates and times follow the language. The phone app follows the phone's language.

## Keyboard shortcuts

App shortcuts use `Ctrl+Shift` on Windows so plain `Ctrl+letter` keys reach the agent TUIs.

| Action | Windows / Linux | macOS |
|---|---|---|
| Search everything | Ctrl+Shift+P | ⌘⇧P |
| Go to file | Ctrl+Shift+O | ⌘P |
| Jump to session / project | Ctrl+Shift+K | ⌘K |
| Agent dashboard | Ctrl+Shift+M | ⌘⇧M |
| Next session waiting for you | Ctrl+Shift+. | ⌘⇧. |
| Workspace board | Ctrl+Shift+J | ⌘⇧J |
| Back / forward | Alt+Shift+← / → | ⌘⌥← / → |
| New terminal | Ctrl+Shift+T | ⌘T |
| New session… | Ctrl+Shift+N | ⌘⇧N |
| Split right / down | Ctrl+Shift+D / Ctrl+Shift+S | ⌘D / ⌘⇧D |
| Close pane | Ctrl+Shift+W | ⌘W |
| Maximize / restore pane | Ctrl+Shift+Enter | ⌘⇧↩ |
| Next / previous pane | Alt+] / Alt+[ | ⌘] / ⌘[ |
| Next / previous tab | Ctrl+Tab / Ctrl+Shift+Tab | ⌃Tab / ⌃⇧Tab |
| Project 1…9 | Alt+1…9 | ⌘1…9 |
| Find in terminal | Ctrl+Shift+F | ⌘F |
| Explorer / Changes / Search panel | Ctrl+Shift+E / G / H | ⌘⇧E / G / F |
| Toggle sidebar | Ctrl+Shift+B | ⌘B |
| Font size + / − / reset | Ctrl+= / Ctrl+- / Ctrl+0 | ⌘= / ⌘- / ⌘0 |
| Settings | Ctrl+, | ⌘, |
| Floating terminal | Ctrl+Shift+` | ⌘⌥A |
| Word wrap (editor focused) | Alt+Z | ⌥Z |
| Layout menu | Ctrl+Shift+L | ⌘⇧L |
| Focus pane left / right / up / down | Ctrl+Alt+← / → / ↑ / ↓ | ⌃⌥← / → / ↑ / ↓ |
| Runs (orchestration) | Ctrl+Shift+R | ⌘⇧R |
| Review a sub-session request (only while one is open) | Ctrl+Shift+A | ⌘⇧A |
| Open browser | Ctrl+Shift+U | ⌘⇧U |
| Dictate (start / stop) | Ctrl+Shift+Space | ⌘⇧Space |
| Composer (write a prompt with @ mentions) | Ctrl+Shift+I | ⌘⇧I |
| Close all editor tabs | Ctrl+Alt+W | ⌘⌥W |
| Next / previous change (diff) | F7 / Shift+F7 | F7 / ⇧F7 |
| Previous / next prompt sent (terminal) | Ctrl+Shift+↑ / ↓ | ⌘↑ / ↓ |
| Terminal: back to the bottom | Ctrl+Shift+End | ⌘End |
| SSH hosts | Ctrl+Alt+H | ⌘⌥H |

Every shortcut can be changed in Settings → Shortcuts (record a key; Kelpie warns when another
command or terminal programs use it). When Kelpie keeps a key from the focused terminal it says
so once (Settings → Notifications turns these notes off).

In terminals: `Ctrl+C` copies when text is selected (otherwise it is sent to the program),
`Ctrl+V` pastes, `Shift+Enter` inserts a newline in agent prompts, `Ctrl/⌘+click` opens links.
Drop files from Explorer/Finder onto a terminal to paste their paths. Programs may set the
clipboard (OSC 52) only from the terminal you are looking at, and Kelpie shows a notice when
they do.

## Settings

Settings (`Ctrl+,`) is a page with a search box (`Ctrl+F`); `/` in search everything jumps to
a setting.

- **Agents**: permissions (Manual / Yolo); per agent extra arguments (e.g. `--model opus`),
  permission mode (Claude) or approval policy (Codex) and environment variables (stored in
  plain text) — used by terminal and chat sessions; which agent CLIs launchers show; the agent
  for AI actions; hibernation; turn snapshots; prioritize visible terminals; keep computer awake;
  *Sub-sessions* (Off / Ask / Allow and how many run at once, see "Sub-sessions").
- **Agent setup**: MCP servers, instruction files, skills and the ACP registry (see above).
- **Accounts**, **Integrations**, **Remote (SSH)**, **Mobile**, **Voice**, **Plugins**,
  **Layouts**, **Quick Commands**, **Automations**, **Shortcuts**: see above.
- **Terminal**: font, size, line height, scrollback, renderer (WebGL / DOM), cursor blink,
  default shell, colour theme, floating terminal.
- **Appearance**: language, dark or light theme, compact density, interface text size, editor
  word wrap.
- **Notifications**: desktop notifications and their sound, ntfy / Telegram / webhook messages
  with the window closed (events, turn length, away-only, screen text), notes about shortcuts kept
  from the terminal, first-run tips.
- **Usage & Resources**: usage and memory meters, the per-session cost cap, the Usage & cost page.
- **Session Daemon**: status, *When Kelpie closes* (stop the session daemon and its sessions, the default, or
  keep it running in the background; *Ask first when something still runs* undoes *Don't ask again*), data folder
  and log, *Export and import settings*, *Stop daemon & all sessions*.

Settings and layouts live in `app-state.json` in the data folder; when its format changes the
old file is first copied to `app-state.v<N>.bak.json`.

### Export and import settings

Settings → Session Daemon → **Export and import settings** moves your setup to another computer
as one JSON file. Tick the parts you want: settings, quick commands, saved layouts, automations,
SSH hosts, the plugin list and agent arguments and environment. **Export…** asks where to save it.

What stays on this computer: SSH passwords, key passphrases and key files (a key's *path* is
kept), integration, notification, phone-pairing, daemon and updater tokens, anything in the OS
credential store, your Claude Code / Codex sign-ins, and settings that only make sense here (the
SSH config file, accounts). Text that looks like a credential is masked as `[redacted]`: known token
formats, `NAME=value` and `--flag value` where a part of the name is PASS, PWD, KEY, AUTH, COOKIE or PAT
or ends in PASSWORD, SECRET, TOKEN, CREDENTIAL, PASSPHRASE, SSHPASS, DBPASS, PRIVATEKEY, SECRETKEY,
ACCESSKEY, APIKEY, AUTHORIZATION or WEBHOOK (so `--api-key`, `SMTP_PASS`, `SSHPASS`, `HTTPAuth` and
`SESSION_TOKEN`, but not `--dangerously-bypass-…`, `--session-id` or Codex's `env_key=NAME`), secret
headers (`x-api-key: …`, `Authorization: Basic …`), `Bearer` tokens, `mysql -p…`, `sshpass -p …` and
`curl -u user:…`. Agent environment variables are listed by name; their values come along only when
you tick *Include values of agent environment variables* (off by default: values are plain text in
the file), and never for a variable with such a name. This is a filter, not a guarantee: open the
file and check it before you share it.

**Import…** opens a file (at most 2 MB) and shows, per part, how many items are new and how many
replace one with the same id or name; untick what you do not want. Settings with a value the
settings page does not allow (a font size of 0, an unknown theme) are left out and named. A value
masked in the file is never written here: it keeps this computer's value, and the preview says so (for
an argument, the value after the same occurrence of the same flag, or of the same header after it, or
at the same place when both argument lists have the same shape). When this computer has none, it is
left empty and listed for you to fill in. An argument list (an agent's, an agent automation's, the
default shell's) is then not imported at all: this computer's list stays as it is (none for a new
agent), and the preview names it so you can enter the hidden value again. A new agent automation
whose arguments cannot be filled in is left out. The same holds for a list with more than 256
arguments or with one longer than 64 KB, not text or containing a NUL character: it is never taken in
part, on import or on export (the export leaves that list out and names it). An item that cannot work without it (a quick
command's text, an automation's prompt, a command that would then run differently) is left out and
named.
Network and device paths (`\\server\share`, `//server/…`, `\\?\UNC\…`, `\??\…`) for a key file, an
automation's folder, program or arguments, or the terminal's program and arguments are refused and the
items named, on export and on import; the session daemon never sees them. Everything that decides what programs run on this
computer is listed with its current and new value: the terminal's program and arguments, agent
permissions, whether agents may control other sessions, each agent's arguments, permission mode and
variables, the text of new and replaced quick commands, saved layouts with panes on another machine,
automation agents, commands, arguments, prompts, folders, triggers, schedules and what happens when
they finish, and SSH hosts' targets (new hosts), key files and startup commands. When the list is
not empty, **Import** stays off until you tick *I've checked these commands*. Before anything
changes, the current configuration is copied to `backups/config-<time>` in the data folder; if a
step fails, what was already changed is put back. An automation this computer cannot take (an agent
that is not installed here, a script that is not there) is skipped and named; the rest is imported.

Imported automations come in switched off. Their commands need approving on this computer, unless
this computer had already approved exactly the same command for the automation it replaces.
Imported SSH hosts carry no passwords: enter them again on the Hosts page. When an imported host
replaces one here but connects somewhere else (another host name, user, port or jump host), the
saved password of the old one is forgotten first, before anything is saved, agent forwarding is
turned off and its port forwards are removed (they are in the backup); the preview says so. That
password stays forgotten even if the import then fails, and the error says so. Agent forwarding is
never switched on by a file. Agent variables the file names without a value keep this computer's
value, or are listed for you to fill in. The plugin list only tells you which plugins to install;
nothing is installed. Automations need a session daemon of the same release (after an update,
restart the daemon to export or import them); a part that cannot be read here is shown as
unavailable, with the reason.

## Automation

The `kelpied` command groups, described in their sections above:

| Command | What it does |
|---|---|
| `kelpied terminal list\|read\|send\|wait` | Lists sessions, reads a screen, types into a session, waits until it stops working ([Terminals](#terminals)). |
| `kelpied worktree get\|set` | Reads or sets a folder's comment and board status ([Worktrees](#worktrees-and-the-workspace-board)). |
| `kelpied review add\|list` | Leaves or lists notes on the worktree's diff ([Review with another agent](#review-with-another-agent-and-best-of-several)). |
| `kelpied run create\|list\|show\|approve\|retry\|done\|skip\|cancel\|delete\|report\|wait` | Creates and drives orchestration runs ([Orchestration runs](#orchestration-runs)). |
| `kelpied sub agents\|spawn\|list\|send\|read\|wait\|report\|inbox\|close\|open` | Opens and steers sub-sessions ([Sub-sessions](#sub-sessions)); `open` shows you a web page ([Processes, ports and preview](#processes-ports-and-preview)). |
| `kelpied ping` | Prints the running daemon's version and API level. |
| `kelpied stop [--kill-sessions] [--wait [--timeout-ms N]]` | Asks the daemon to exit; with `--kill-sessions` its sessions end too, and `--wait` waits until the process is gone. |
| `kelpied mcp` | The MCP server Kelpie gives Claude Code and Codex for sub-sessions and web pages (started by the agents, not by hand). |
| `kelpied call <method> [json]` | Calls any daemon method directly. |

`kelpied call` talks to the running daemon directly, e.g.

```
kelpied call session.list
kelpied call session.create '{"kind":"claude","cwd":"D:/proj"}'
kelpied call session.input '{"handle":3,"data":"run the tests\r"}'
kelpied call session.read '{"handle":3}'
```

## Build from source

Requirements: Rust (stable), Node 22+, pnpm, and on Windows the MSVC build tools. On Linux
(Debian/Ubuntu names): `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev
libdbus-1-dev libasound2-dev patchelf xdg-utils`, and `libfuse2` (or
`APPIMAGE_EXTRACT_AND_RUN=1`) to build the AppImage.

```
cd app && pnpm install && cd ..
scripts/build-release.sh --bundles nsis           # Windows installer
scripts/build-release.sh --bundles app,dmg        # macOS
scripts/build-release.sh --bundles appimage,deb   # Linux
```

Each run bumps the patch version (in `Cargo.toml`, `app/package.json` and
`app/src-tauri/tauri.conf.json`) before building, so every installer has its own number.

Voice dictation needs the speech helper: `cargo build --release -p kelpie-stt` (the build script
does it; it downloads the speech engine's prebuilt library, ~120 MB, at build time). A
development build finds it in `target/release`.

Development: `scripts/dev-daemon.sh run` (isolated endpoint/data dir), then in `app/`
`KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev`. Tests: `cargo test`, `pnpm typecheck`,
`pnpm i18n:check` (every interface text has a Vietnamese translation), `kelpied bench --sessions
20 --mb 2`, `kelpied bench-echo` (keystroke latency; `--cmd claude --ready ❯`, `--load N` busy
sessions, `--external N` busy processes), and the WebDriver end-to-end scripts in `scripts/e2e/`
(require `tauri-driver` + a matching `msedgedriver`; on Linux `WebKitWebDriver` from
`webkit2gtk-driver`, under Xvfb when headless — `scripts/e2e/boot_smoke.py` is the minimal one).
