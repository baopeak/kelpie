// A phone's chat view reads the agent's log with the same adapters as the desktop, in observer mode. They are
// loaded when a chat of that agent is opened, so the bundle a terminal-only phone needs stays small.
import type { Adapter, AdapterContext } from "../../src/chat/chat-model";

export type MakeAdapter = (ctx: AdapterContext) => Adapter;

export async function adapterFor(kind: string): Promise<MakeAdapter> {
  if (kind === "claude") return (await import("../../src/chat/claude-adapter")).createClaudeAdapter;
  if (kind === "codex") return (await import("../../src/chat/codex-adapter")).createCodexAdapter;
  return (await import("../../src/chat/acp-adapter")).createAcpAdapter;
}
