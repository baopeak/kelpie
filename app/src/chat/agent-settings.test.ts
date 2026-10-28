import { describe, expect, it } from "vitest";
import { claudeSettingsFrom, withSettings } from "./agent-settings";

const line = (o: unknown) => JSON.stringify(o);

describe("claudeSettingsFrom", () => {
  it("reads the latest mode, model and effort from the transcript", () => {
    const lines = [
      line({ type: "permission-mode", permissionMode: "default" }),
      line({ type: "assistant", requestedModel: "sonnet", effort: "low", message: { model: "claude-sonnet-5-5" } }),
      line({ type: "permission-mode", permissionMode: "bypassPermissions" }),
      line({ type: "assistant", isSidechain: true, requestedModel: "haiku", effort: "low" }),
      line({ type: "assistant", requestedModel: "opus", effort: "high", message: { model: "claude-opus-5-5" } }),
      line({ type: "user", message: { content: "hi" } }),
    ];
    expect(claudeSettingsFrom(lines)).toEqual({ mode: "bypassPermissions", model: "opus", effort: "high" });
  });

  it("falls back to the session's own flags", () => {
    expect(claudeSettingsFrom([], ["--dangerously-skip-permissions", "--model=haiku", "--effort", "max"])).toEqual({ mode: "bypassPermissions", model: "haiku", effort: "max" });
    expect(claudeSettingsFrom([], [])).toEqual({ mode: null, model: null, effort: null });
  });
});

describe("withSettings", () => {
  it("gives Claude its mode, model and effort, replacing the profile's", () => {
    expect(withSettings("claude", ["--dangerously-skip-permissions", "--model", "sonnet", "--verbose"], { mode: "acceptEdits", model: "opus", effort: "high" }))
      .toEqual(["--verbose", "--permission-mode", "acceptEdits", "--model", "opus", "--effort", "high"]);
    expect(withSettings("claude", ["--model", "sonnet"], { model: "default" })).toEqual(["--model", "sonnet"]);
  });

  it("gives Codex's TUI the chat view's preset, model and effort", () => {
    expect(withSettings("codex", ["--yolo", "-m", "o3", "-c", 'model_reasoning_effort="low"', "-c", "x=1"], { mode: "auto", model: "gpt-6-luna", effort: "high" }))
      .toEqual(["-c", "x=1", "-s", "workspace-write", "-a", "on-request", "-m", "gpt-6-luna", "-c", 'model_reasoning_effort="high"']);
    expect(withSettings("codex", ["-s", "read-only"], { mode: "full-access" })).toEqual(["--dangerously-bypass-approvals-and-sandbox"]);
  });

  it("leaves other agents and unknown settings alone", () => {
    expect(withSettings("omp", ["--model", "x"], { model: "y" })).toEqual(["--model", "x"]);
    expect(withSettings("claude", ["--verbose"], {})).toEqual(["--verbose"]);
  });
});
