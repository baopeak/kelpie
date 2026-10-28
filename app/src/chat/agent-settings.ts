// What a conversation runs with — model, reasoning effort, permission mode — carried across a
// switch between an agent's terminal UI and chat view, so the other front-end continues the same
// way (a TUI in bypass mode does not come back as "ask before edits").
//
// Claude Code restores none of these on `--resume`; its transcript records them (`permission-mode`
// entries, each reply's `effort` and requested model), and both front-ends take them as flags.
// Codex's app-server restores the thread's own settings; its TUI is told them as flags. OMP keeps
// its model and thinking level in the session file both front-ends read.

export interface AgentSettings {
  model?: string | null;
  effort?: string | null;
  /** Claude: its permission mode. Codex: chat view's preset (read-only / auto / full-access). */
  mode?: string | null;
}

/** Claude's settings as its transcript last recorded them; the session's own flags fill what it
 *  never recorded (a session started with `--dangerously-skip-permissions` and no turn yet). */
export function claudeSettingsFrom(lines: string[], args: string[] = []): AgentSettings {
  const out: AgentSettings = {};
  for (let i = lines.length - 1; i >= 0 && (out.mode == null || out.model == null || out.effort == null); i--) {
    const line = lines[i];
    if (!line.includes('"permission-mode"') && !line.includes('"assistant"')) continue;
    let m: any;
    try {
      m = JSON.parse(line);
    } catch {
      continue;
    }
    if (m.type === "permission-mode" && out.mode == null && typeof m.permissionMode === "string") out.mode = m.permissionMode;
    if (m.type === "assistant" && !m.isSidechain) {
      if (out.model == null) out.model = m.requestedModel ?? m.message?.model ?? null;
      if (out.effort == null && typeof m.effort === "string") out.effort = m.effort;
    }
  }
  const flag = (name: string) => {
    const i = args.findIndex((a) => a === name || a.startsWith(name + "="));
    return i < 0 ? null : args[i].includes("=") ? args[i].split("=").slice(1).join("=") : args[i + 1] ?? null;
  };
  out.mode ??= args.includes("--dangerously-skip-permissions") ? "bypassPermissions" : flag("--permission-mode");
  out.model ??= flag("--model");
  out.effort ??= flag("--effort");
  return out;
}

/** Drops `names` (and the value after each, when `valued`) from `args`. */
function drop(args: string[], names: string[], valued: boolean): string[] {
  const out: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    if (names.includes(a)) {
      if (valued) i++;
      continue;
    }
    if (valued && names.some((n) => n.startsWith("--") && a.startsWith(n + "="))) continue;
    out.push(a);
  }
  return out;
}

/** Drops Codex `-c key=…` overrides for `keys`. */
function dropConfig(args: string[], keys: string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < args.length; i++) {
    if ((args[i] === "-c" || args[i] === "--config") && keys.some((k) => args[i + 1]?.startsWith(k + "="))) {
      i++;
      continue;
    }
    out.push(args[i]);
  }
  return out;
}

/** Codex presets (chat view's mode picker) as its TUI's flags. */
const CODEX_MODE_FLAGS: Record<string, string[]> = {
  "read-only": ["-s", "read-only", "-a", "on-request"],
  auto: ["-s", "workspace-write", "-a", "on-request"],
  "full-access": ["--dangerously-bypass-approvals-and-sandbox"],
};

/** `args` (a profile's flags) with flags that set `s`, replacing the profile's own for the same
 *  setting (duplicates are an error for Codex, and the last one would win for Claude). */
export function withSettings(kind: string, args: string[], s: AgentSettings): string[] {
  let out = [...args];
  if (kind === "claude") {
    if (s.mode) {
      out = drop(out, ["--permission-mode"], true);
      out = drop(out, ["--dangerously-skip-permissions", "--allow-dangerously-skip-permissions"], false);
      out.push("--permission-mode", s.mode);
    }
    if (s.model && s.model !== "default") out = [...drop(out, ["--model"], true), "--model", s.model];
    if (s.effort) out = [...drop(out, ["--effort"], true), "--effort", s.effort];
  } else if (kind === "codex") {
    if (s.mode && CODEX_MODE_FLAGS[s.mode]) {
      out = drop(out, ["-s", "--sandbox", "-a", "--ask-for-approval"], true);
      out = drop(out, ["--full-auto", "--yolo", "--dangerously-bypass-approvals-and-sandbox"], false);
      out = dropConfig(out, ["approval_policy", "sandbox_mode"]);
      out.push(...CODEX_MODE_FLAGS[s.mode]);
    }
    if (s.model) out = [...dropConfig(drop(out, ["-m", "--model"], true), ["model"]), "-m", s.model];
    if (s.effort) out = [...dropConfig(out, ["model_reasoning_effort"]), "-c", `model_reasoning_effort="${s.effort}"`];
  }
  return out;
}
