import { describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({ daemon: vi.fn() }));

const { buildLaunchCatalogue, profileLaunchOf } = await import("./phone-launch");
type Preset = Parameters<typeof buildLaunchCatalogue>[1][number];

const preset = (id: string, label: string, yolo: string[], installed = true): Preset => ({ id, label, bin: id, yolo, resume_latest: true, installed });
const PRESETS: Preset[] = [
  preset("claude", "Claude Code", ["--dangerously-skip-permissions"]),
  preset("codex", "Codex", ["--dangerously-bypass-approvals-and-sandbox"]),
  preset("omp", "OMP", ["--auto-approve"], false),
  preset("gemini", "Gemini", ["--yolo"]),
  preset("goose", "Goose", []),
  preset("cursor", "Cursor CLI", ["--yolo"], false),
  preset("kimi", "Kimi", ["--yolo"]),
];

const ACCOUNT = { CLAUDE_CONFIG_DIR: "C:/accounts/work" };

function state(settings: Partial<Parameters<typeof profileLaunchOf>[0]> & { hiddenAgents?: string[]; registryAgents?: { id: string }[] } = {}) {
  return {
    projects: [
      { id: "p1", name: "Shop", path: "C:/work/shop" },
      { id: "r1", name: "Server", path: "ssh://box/srv", remote: { dest: "me@box", path: "/srv", tmux: true } },
      { id: "p2", name: "Docs", path: "C:/work/docs" },
    ],
    settings: {
      profiles: [
        { kind: "claude", label: "Claude Code", args: ["--model", "opus"], env: { CLAUDE_LOG: "1" } },
        { kind: "codex", label: "Codex", args: [], permission: "on-request" },
      ],
      agentPermissions: "yolo" as "manual" | "yolo",
      hiddenAgents: ["gemini"],
      registryAgents: [{ id: "kimi" }],
      ...settings,
    },
  };
}

/** The store's own resolution, as `profileLaunch` does it, with the active account of Claude. */
const launchOf = (s: ReturnType<typeof state>) => (kind: string) => profileLaunchOf(s.settings, kind, PRESETS.find((p) => p.id === kind)?.yolo ?? [], kind === "claude" ? ACCOUNT : undefined);

describe("the launch catalogue a phone starts from", () => {
  it("offers this computer's projects, never one over SSH, and the launchers' agents without the hidden and the chat-only ones", () => {
    const s = state();
    const c = buildLaunchCatalogue(s, PRESETS, launchOf(s));
    expect(c.projects).toEqual([{ id: "p1", name: "Shop", path: "C:/work/shop" }, { id: "p2", name: "Docs", path: "C:/work/docs" }]);
    // The core three always (the daemon keeps only the installed ones), then installed presets; gemini is hidden, kimi chat-only.
    expect(c.agents.map((a) => a.kind)).toEqual(["claude", "codex", "omp", "goose"]);
    expect(c.agents.map((a) => a.label)).toEqual(["Claude Code", "Codex", "OMP", "Goose"]);
  });

  it("gives each agent exactly what the desktop starts it with: profile arguments, permission mode or Yolo, and the account's environment", () => {
    const s = state();
    const launch = launchOf(s);
    const c = buildLaunchCatalogue(s, PRESETS, launch);
    for (const a of c.agents) expect({ args: a.args, env: a.env }, a.kind).toEqual({ args: launch(a.kind).args, env: launch(a.kind).env ?? {} });
    const byKind = Object.fromEntries(c.agents.map((a) => [a.kind, a]));
    expect(byKind.claude).toEqual({ kind: "claude", label: "Claude Code", args: ["--model", "opus", "--dangerously-skip-permissions"], env: { CLAUDE_LOG: "1", ...ACCOUNT } });
    expect(byKind.codex.args, "a permission mode of its own: no Yolo").toEqual(["-c", "approval_policy=on-request"]);
    expect(byKind.omp.args).toEqual(["--auto-approve"]);
    expect(byKind.goose).toEqual({ kind: "goose", label: "Goose", args: [], env: {} });
  });

  it("follows the settings as they change: Yolo off, a permission mode, an agent shown again", () => {
    const manual = state({ agentPermissions: "manual", hiddenAgents: [] });
    const c = buildLaunchCatalogue(manual, PRESETS, launchOf(manual));
    expect(c.agents.find((a) => a.kind === "claude")!.args).toEqual(["--model", "opus"]);
    expect(c.agents.map((a) => a.kind)).toContain("gemini");
    const bypass = state({ profiles: [{ kind: "claude", label: "Claude Code", args: [], permission: "bypassPermissions" }] });
    expect(buildLaunchCatalogue(bypass, PRESETS, launchOf(bypass)).agents[0].args).toEqual(["--permission-mode", "bypassPermissions"]);
  });

  it("leaves the profile's flags alone when they already hold Yolo's, and the chat view's launch without them", () => {
    const s = state({ profiles: [{ kind: "claude", label: "Claude Code", args: ["--dangerously-skip-permissions"] }] });
    expect(profileLaunchOf(s.settings, "claude", ["--dangerously-skip-permissions"], undefined).args).toEqual(["--dangerously-skip-permissions"]);
    expect(profileLaunchOf(state().settings, "omp", ["--auto-approve"], undefined, false)).toEqual({ args: [], env: undefined });
  });
});
