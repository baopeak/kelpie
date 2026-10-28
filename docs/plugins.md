# Kelpie plugins

A plugin is a folder `<data>/plugins/<id>/` holding `kelpie-plugin.json` (a plugin from before
the rename still works with its `erka-plugin.json`). It is declarative: it
adds agents, palette commands, quick commands, terminal themes, layouts and status-bar items.
Install, create, review and remove plugins in Settings → Plugins.

## Trust

Nothing a plugin declares is used until you **Review & enable** it. The review lists every
program it can run (agent launch commands, palette commands, quick commands, status-item
commands, with how often status items run). The approval is bound to the manifest's SHA-256
(kept in `<data>/plugins/approvals.json`): any change to `kelpie-plugin.json` turns the plugin off
until you review it again. Programs run with your permissions, like commands you type.

Git installs accept `https://`, `ssh://` and `git@` URLs (plain `http://` only for `localhost`
/ `127.0.0.1`); the clone is shallow and keeps no `.git` folder.

## Manifest

```json
{
  "id": "acme-tools",
  "name": "Acme Tools",
  "version": "1.2.0",
  "description": "Shown in Settings → Plugins",
  "contributes": {
    "agents": [
      { "id": "bot", "label": "Acme Bot", "bin": "acme", "launch": ["chat"],
        "yolo": ["--yes"], "resumeLatest": ["--continue"], "prompt": "flag:--ask", "acp": ["acp"] }
    ],
    "commands": [
      { "id": "test", "label": "Run tests", "run": "pnpm test -- ${file}", "in": "split" }
    ],
    "quickCommands": [
      { "label": "Review the diff", "text": "Review the uncommitted changes.", "action": "agent" }
    ],
    "terminalThemes": [
      { "id": "mono", "label": "Acme Mono", "dark": true, "colors": ["#101418", "#d0d0d0", "#ffcc00", "… 16 ANSI colours"] }
    ],
    "layouts": [
      { "name": "Two agents and a shell", "template": { "type": "split", "dir": "row", "sizes": [0.5, 0.5],
        "children": [{ "type": "leaf", "kind": "claude" },
                     { "type": "split", "dir": "col", "sizes": [0.5, 0.5],
                       "children": [{ "type": "leaf", "kind": "codex" }, { "type": "leaf", "kind": "shell" }] }] } }
    ],
    "statusItems": [
      { "id": "branch", "label": "Branch", "command": "git rev-parse --abbrev-ref HEAD", "intervalSeconds": 30 }
    ]
  }
}
```

- `id`: lowercase letters, digits and `-` (48 at most) and **equal to the folder name**. `name`
  is required. Unknown keys in `contributes` make the plugin invalid (it is listed with the
  error and cannot be enabled).
- **agents** — `id`, `label`, `bin` required. The session kind is `<plugin id>-<agent id>`;
  launchers show it when `bin` is found. `launch`: arguments always passed; `yolo`: added with
  Settings → Agents → Permissions: Yolo; `resumeLatest`: arguments that continue the folder's
  latest conversation (used to resume after a restart); `prompt`: how a first prompt is given —
  `"argv"` (last argument), `"flag:<flag>"` (`<flag> <prompt>`) or `"typed"` (default: typed
  once the program is ready); `acp`: arguments that start it as an Agent Client Protocol agent
  over stdio — it then opens in chat view too (daemon API 8 and later). The daemon keeps plugin agents in `custom-agents.json` so their
  sessions resume after a restart.
- **commands** — `id`, `label`, `run` required; shown in search as `<plugin name>: <label>` and
  bindable in Settings → Shortcuts. `in`: `"terminal"` (default: the focused shell, or a new
  one), `"split"` (a new shell to the right), `"tab"` (a new shell tab) or `"agent"` (sent as a
  prompt to the focused agent). `enter` (default `true`) presses Enter after typing.
  Variables: `${projectPath}`, `${projectName}`, `${branch}`, `${file}` (the focused file's full
  path); unknown variables stay as written.
- **quickCommands** — `label`, `text` required; `action` `"terminal"` (default) or `"agent"`,
  `enter` (default `true`). They appear with your own quick commands, for every project.
- **terminalThemes** — `id`, `label`, `colors` (19: background, foreground, cursor, ANSI 0–15);
  `dark` (default `true`). Offered in Settings → Terminal → Colour theme.
- **layouts** — `name` and a `template`: `{"type": "leaf", "kind": <agent kind | "shell" |
  null>}` or `{"type": "split", "dir": "row" | "col", "sizes": [...], "children": [...]}` (at most
  12 children per split; sizes are normalised when they do not match). Offered in the layout
  menu and on a project's start page.
- **statusItems** — `id`, `command` required; `label`, `intervalSeconds` (default 30, at least
  5), `timeoutSeconds` (default 15, 1–60). The command runs in the active project's folder
  (local projects only) and the first line of its output is shown in the status bar; a failing
  command shows *error* with its message in the tooltip. Click to refresh.

Settings → Plugins → *Create a plugin* writes a starter manifest with a command, a quick
command and a status item; edit it, then *Reload* and enable it.
