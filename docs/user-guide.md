# Kelpie — user guide

Kelpie runs many coding agents (Claude Code, Codex, OMP) and shells across many projects.
Sessions live in a background daemon (`kelpied`): switching projects, closing the window or a
UI crash never stops them. Only the project you are looking at is rendered.

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
(Settings → About → **Check for updates**, or **Check now**). A newer release shows as
*Update x.y.z* in the status bar; its dialog shows the release notes, with **Install now**,
**Install when idle** (as soon as no session is working) and **Skip this version**. Updates are
signed; Kelpie installs nothing else. Installing restarts only the app: every session keeps
running and is there when Kelpie reopens (running an installer by hand over a running Kelpie works
the same way). The one exception is the first update from Kelpie 0.1.x, whose session daemon
runs from the install folder: that update stops sessions once (the dialog says so) and agents
resume. The sessions stay on the previous session daemon until they have all stopped; then
the new one takes over on its own. A notice offers **Switch now** if you want the new daemon's
features earlier (running sessions stop for a moment; agents that can resume continue).

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
   `>` commands, `/` settings, or press `Tab` to change scope.
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
   for approval; `Shift+Tab` steps through the guarded modes). While the agent works with nothing
   typed, the round send button stops it. A command
   that only works in the terminal UI shows *Run it in the terminal*, which switches back and runs
   it there.
   - The transcript reads like the terminal UI: each tool call is one line (*Read app.ts · lines
     75-80*, *Edit main.rs +3 −1*, *Bash* with its command and output), runs of reads and searches
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
   - Click a file in a reply to open it: a link, a path in inline code (`src/a.ts:42` opens at that
     line), a tool's target (an edit's opens its changes) or the file on an approval card. Something
     outside the project only shows a message: nothing an agent names is ever launched. Addresses in
     your own messages open in the browser.
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
   - Messages sent while the agent works wait above the message field and go when its turn ends.
     *Send now* gives one to the agent during its turn (Codex and Claude; for other agents it stops
     the turn first); stopping the agent holds the queue until you send again.
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
conversation). Desktop notifications fire when a session you are not looking at finishes or
needs input.

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
lines as text), splits, your quick commands and hand-offs. Settings → Terminal → *Colour
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
top. Click an entry to jump there; right-click toggles read. Settings → Notifications can add
a short chime to desktop notifications. A session you are looking at (the window has focus and
shows it) raises nothing.

#### On your phone, with the window closed

Settings → Notifications → *Off this computer* sends the same events from the session daemon,
which keeps running when you close the window. Nothing is sent until you switch a channel on.

- **ntfy**: switch it on, install the ntfy app and subscribe to the topic Kelpie shows (*Show topic*).
  The topic is 128 random bits only you know; use *New topic* if you shared it by accident. An
  access token is optional, and the server can be your own.
- **Telegram**: make a bot with @BotFather, paste its token and your chat id, press *Send a test*.
- **Webhook**: a JSON `POST` to an address you choose.

Choose which events go out (needs you, finished, failed, automation, run approval, cost cap), skip
turns shorter than a number of seconds (30 by default), and optionally send only while Kelpie's
window is not in focus or the computer has been idle for 5 minutes (the system's own clock on Windows; elsewhere, no key or pointer use in Kelpie's window). A session sends at most one message per
30 seconds. Messages carry the project, the session's name, what happened and how long the turn took,
with credentials masked; the end of the terminal screen is added only if you switch that on (it leaves
the computer, through the server you chose). With phone access on, the message links to the session in
the phone app; the link holds no pairing code, so it opens only on a phone that is already paired. Tokens
and the topic are kept in the system credential store, never in a file, and are never shown again.

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
Cursor, Windsurf, Zed, Sublime Text or IntelliJ IDEA when installed.

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
kelpied run create run.json          # {"name", "objective", "tasks": [{"id", "title", "prompt",
kelpied run list                     #   "kind", "deps": [...], "approve", "worktree"}]}
kelpied run show <run> | approve|retry|done|skip <run> <task> | cancel|delete <run>
kelpied run wait <run>
kelpied review add <file>:<line> "<comment>" [--old] [--base <ref>]   # a note on this worktree's diff
kelpied review list [--json]
```

The same prompt to several agents, with a compare view to pick the best, is *Try with several agents…*
(see Review and Finish).

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

### Processes, ports and preview

When a program in a session listens on a port (a dev server, `python -m http.server 8000`…), a
`:8000` chip appears on the session's tab and its sidebar row. Click it to open the address in a
browser pane; right-click for *Open preview beside* (split next to the focused pane), *Open in a
browser tab* or *Copy address*. Chips of a session that is not on screen are dimmed: they show the
ports as of when it was last on screen. The **Processes** tab of the right panel (palette:
*Processes and Ports*) lists what each running session of the project runs, as a tree with PID,
memory, CPU and ports. **Kill** stops a program and what it started. **Kill children** stops what a
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
- **Files**: browse the host, upload files or folders (or drop them from Explorer / Finder onto
  the list), download, new folder, rename, delete (`scp` / SFTP).
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

Settings → Mobile turns on a small web app for your phone on your network (off by default;
port 4817; choose a private or Tailscale address — never every interface; HTTPS with your own
certificate, e.g. from `tailscale cert`). **Pair a phone** shows a QR code valid for two
minutes; scan it, name the phone and approve it on the computer. The phone lists every session
grouped by *Needs you / Working / Idle / Stopped*, buzzes when one starts needing you, shows a
session's screen and — if you allow that device to type — sends replies and keys (Enter, Esc,
Ctrl+C, arrows, y / n, 1–3). The list and the screen update as soon as something changes. A reply typed
while a menu or permission prompt is waiting is refused (answer with the keys instead). Unpairing a
device disconnects it at once.

Turning Mobile off also closes existing phone connections, not just the listening port.
Paired devices stay saved, but must reconnect after access is enabled again; an old connection
cannot keep reading or answering prompts across that switch.

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
  for AI actions; hibernation; turn snapshots; prioritize visible terminals; keep computer awake.
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
- **Session Daemon**: status, data folder and log, *Export and import settings*, *Stop daemon &
  all sessions*.

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

Besides `kelpied terminal …`, `kelpied worktree …` and `kelpied run …` (above), `kelpied call <method> [json]` talks
to the running daemon directly, e.g.

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
