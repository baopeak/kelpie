import { describe, expect, it } from "vitest";
import { ImportFailed, applyImport, exportConfig, exportableSettings, importableSettings, loadCurrent, planImport, readImportFile, type ConfigSection, type Current, type ImportRead, type ListedAutomation, type TransferContext } from "./config-transfer";
import type { Settings } from "./types";
import type { SshHost, Vault } from "./ssh-hosts";

const defaults = {
  fontSize: 13,
  theme: "dark",
  tipsSeen: [],
  integrations: { linear: false, jira: null, gitlabHosts: [] },
  voice: { model: null, language: "auto", device: null },
  quickCommands: [],
  layouts: [],
  profiles: [],
  accounts: [],
  activeAccounts: {},
  sshConfig: "",
} as unknown as Settings;

const host = (over: Partial<SshHost>): SshHost => ({
  id: "", label: "", alias: "", group: "", tags: [], hostname: "h.example", port: null, user: "", auth: "key", identityFile: "", jump: "",
  forwardAgent: false, keepalive: 30, env: {}, startup: "", folder: "", tmux: true, notes: "", color: "", terminalTheme: "", os: "", lastConnected: 0, ...over,
});

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
  } as unknown as Settings;
}

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
          { ...host({ id: "h-remote", label: "Box", alias: "box", hostname: "10.0.0.9" }) },
          { ...host({ id: "h-new", label: "New", alias: "new", hostname: "new.example" }) },
        ].map(({ os: _os, lastConnected: _lc, ...h }) => h),
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
    vault: { version: 1, hosts: [host({ id: "h1", label: "Box", alias: "box", hostname: "10.0.0.5", os: "ubuntu", lastConnected: 99 })], forwards: [] },
    automations: [automation({ id: "local-a", name: "Nightly", action: "shell", kind: "", shell: { program: "python", argv: ["old.py"], approved_hash: "h" }, command_hash: "h", command_approved: true, env: { TOKEN: "keep-me" }, last_run: 5 })],
    plugins: ["here"],
  };
}

const ALL: ConfigSection[] = ["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"];

describe("settings allowlist", () => {
  it("exports known, portable keys only", () => {
    const out = exportableSettings({ ...local(), sshConfig: "C:/x", rootYoloOk: ["h"] } as unknown as Settings, defaults);
    expect(Object.keys(out).sort()).toEqual(["fontSize", "integrations", "theme", "tipsSeen", "voice"]);
    expect(out.fontSize).toBe(14);
  });

  it("imports values of the right shape and names the rest", () => {
    const r = importableSettings({ fontSize: "big", theme: "light", accounts: [], unknown: 1, integrations: { linear: true, jira: { url: "u", email: "e" }, gitlabHosts: ["g"], extra: 1 }, voice: { model: 5 } }, defaults);
    expect(r.values).toEqual({ theme: "light", integrations: { linear: true, jira: { url: "u", email: "e" }, gitlabHosts: ["g"] } });
    expect(r.skipped.sort()).toEqual(["accounts", "fontSize", "unknown", "voice"]);
  });
});

describe("planImport", () => {
  const plan = () => planImport(fileRead(), ALL, current(), { defaults, newId });

  it("counts what is added and what replaces an item with the same id or name", () => {
    const p = plan();
    expect(p.sections.settings).toEqual({ total: 5, add: 0, replace: 2, skipped: 3, changed: ["fontSize", "theme"] });
    expect(p.sections.quickCommands).toEqual({ total: 3, add: 1, replace: 2, skipped: 1 });
    expect(p.sections.layouts).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
    expect(p.sections.agents).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
    expect(p.sections.sshHosts).toEqual({ total: 2, add: 1, replace: 1, skipped: 0 });
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

  it("keeps what this computer recorded about a replaced host", () => {
    const hosts = plan().vault!.hosts;
    expect(hosts[0]).toMatchObject({ id: "h1", alias: "box", hostname: "10.0.0.9", os: "ubuntu", lastConnected: 99 });
    expect(hosts[1]).toMatchObject({ id: "h-new", os: "", lastConnected: 0 });
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

/** A computer in memory: app settings, the hosts file and the daemon's automations. */
function machine(opts: { failSave?: string; failHosts?: boolean; failSettingsSave?: boolean; canSave?: boolean } = {}) {
  const cur = current();
  const m = {
    settings: cur.settings,
    saved: JSON.stringify(cur.settings),
    vault: cur.vault as Vault,
    automations: new Map<string, ListedAutomation>(cur.automations!.map((a): [string, ListedAutomation] => [a.id, a])),
    approvals: [] as string[],
    calls: [] as string[],
    backups: 0,
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
      if (method === "automation.state") return { error: null } as T;
      if (method === "automation.list") return JSON.parse(JSON.stringify(list())) as T;
      if (method === "automation.save") {
        if (opts.failSave && params.name === opts.failSave) throw new Error("save refused");
        const a = { ...params, command_approved: false } as ListedAutomation;
        delete (a as any).approved_hash;
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
        return "C:/data/backups/config-1" as T;
      }
      if (cmd === "config_import_read") throw new Error("not a Kelpie settings file: expected value at line 1 column 1");
      throw new Error(`unexpected ${cmd}`);
    },
    newId,
  };
  const snapshot = () => JSON.stringify({ settings: m.settings, saved: m.saved, vault: m.vault, automations: list().map(({ command_approved, ...a }) => ({ ...a, command_approved })) });
  return { m, ctx, snapshot };
}

async function previewOn(ctx: TransferContext, chosen = ALL) {
  return planImport(fileRead(), chosen, await loadCurrent(chosen, ctx), { defaults, newId: () => "preview-id" });
}

describe("applyImport", () => {
  it("backs up first, then applies hosts, settings and automations", async () => {
    const { m, ctx } = machine();
    const r = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx);
    expect(r.backup).toBe("C:/data/backups/config-1");
    const firstWrite = m.calls.findIndex((c) => ["config_hosts_write", "saveNow", "automation.save"].includes(c));
    expect(m.calls.indexOf("config_backup")).toBeLessThan(firstWrite);
    expect(m.vault.hosts.map((h) => h.id)).toEqual(["h1", "h-new"]);
    expect(JSON.parse(m.saved).fontSize).toBe(16);
    expect([...m.automations.values()].map((a) => [a.id, a.enabled, a.command_approved])).toEqual([["local-a", false, false], ["zz", false, false]]);
  });

  it("undoes everything when a step fails, and restores the user's approval", async () => {
    const { m, ctx, snapshot } = machine({ failSave: "Report" });
    const before = snapshot();
    const err = await applyImport(fileRead(), ALL, await previewOn(ctx), ctx).catch((e) => e);
    expect(err).toBeInstanceOf(ImportFailed);
    expect(err.undoFailures).toEqual([]);
    expect(err.message).toContain("nothing was changed");
    expect(m.approvals).toEqual(["local-a:h"]);
    expect(snapshot()).toEqual(before);
  });

  it("undoes the hosts when the settings cannot be saved", async () => {
    const { ctx, snapshot } = machine({ failSettingsSave: true });
    const before = snapshot();
    await expect(applyImport(fileRead(), ["sshHosts", "settings"], await previewOn(ctx, ["sshHosts", "settings"]), ctx)).rejects.toBeInstanceOf(ImportFailed);
    expect(snapshot()).toEqual(before);
  });

  it("changes nothing when the current configuration cannot be read", async () => {
    const { m, ctx } = machine({ failHosts: true });
    const preview = planImport(fileRead(), ALL, current(), { defaults, newId });
    await expect(applyImport(fileRead(), ALL, preview, ctx)).rejects.toThrow("could not be read");
    expect(m.backups).toBe(0);
    expect(m.calls.filter((c) => /write|save|delete/.test(c))).toEqual([]);
  });

  it("refuses when the configuration moved since the preview", async () => {
    const { m, ctx } = machine();
    const preview = await previewOn(ctx);
    m.settings = { ...m.settings, theme: "light" } as Settings;
    await expect(applyImport(fileRead(), ALL, preview, ctx)).rejects.toThrow("changed since the preview");
    expect(m.backups).toBe(0);
  });

  it("refuses app sections when app state cannot be saved", async () => {
    const { m, ctx } = machine({ canSave: false });
    await expect(applyImport(fileRead(), ["layouts"], await previewOn(ctx, ["layouts"]), ctx)).rejects.toThrow("cannot be saved");
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
