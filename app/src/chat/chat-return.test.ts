// Coming back to a chat (a project or tab switch remounts its pane): the rows stay, and only what is new is read.
import { describe, expect, it, vi } from "vitest";
import { createEffect, createRoot } from "solid-js";
import { ChatController } from "./chat-model";
import { createClaudeAdapter } from "./claude-adapter";
import type { ChatTransport } from "./transport";

const encode = (s: string) => new TextEncoder().encode(s);
const assistant = (id: string, text: string) => `${JSON.stringify({ type: "assistant", message: { id, role: "assistant", content: [{ type: "text", text }] } })}\n`;
const texts = (c: ChatController) => c.state.items.flatMap((i) => (i.kind === "assistant" ? [(i as { text: string }).text] : []));

/** A daemon's log: `seq` counts everything ever printed, `pinned` + `kept` is what an attach hands over. */
function daemonLog(start = "") {
  const log = { kept: start, seq: encode(start).length, pinned: "", live: null as ((b: Uint8Array) => void) | null };
  const written: string[] = [];
  const transport: ChatTransport = {
    attach: async (_h, onLive) => {
      log.live = onLive;
      return encode(log.pinned + log.kept);
    },
    logEnd: () => log.seq,
    detach: () => { log.live = null; },
    send: (line) => void written.push(line),
  };
  const print = (text: string) => {
    log.kept += text;
    log.seq += encode(text).length;
    log.live?.(encode(text));
  };
  return { log, transport, print, written };
}

const controller = (transport: ChatTransport) => new ChatController("s", "/work", null, createClaudeAdapter, { transport });

describe("coming back to a chat", () => {
  it("does not rebuild a transcript that is still right, nor read its history again", async () => {
    const { transport } = daemonLog(assistant("a", "one"));
    const ctrl = controller(transport);
    const history = vi.fn(async () => () => {});
    ctrl.loadHistory = history;
    await ctrl.attach(1);
    const rows = ctrl.state.items.map((i) => i.id);
    const first = ctrl.state.items[0];
    ctrl.detach();
    await ctrl.attach(1);
    expect(history).toHaveBeenCalledTimes(1);
    expect(ctrl.state.items.map((i) => i.id)).toEqual(rows);
    expect(ctrl.state.items[0]).toBe(first);
  });

  it("adds what was printed while away, and what is printed from then on", async () => {
    const { transport, print } = daemonLog(assistant("a", "one"));
    const ctrl = controller(transport);
    await ctrl.attach(1);
    print(assistant("b", "two"));
    ctrl.detach();
    print(assistant("c", "three"));
    print(assistant("d", "four"));
    await ctrl.attach(1);
    print(assistant("e", "five"));
    expect(texts(ctrl)).toEqual(["one", "two", "three", "four", "five"]);
    ctrl.detach();
    await ctrl.attach(1);
    expect(texts(ctrl)).toEqual(["one", "two", "three", "four", "five"]);
  });

  it("treats what was printed while away as a replay: it answers nothing", async () => {
    const { transport, print, written } = daemonLog(assistant("a", "one"));
    const ctrl = controller(transport);
    await ctrl.attach(1);
    ctrl.detach();
    written.length = 0;
    print(`${JSON.stringify({ type: "control_request", request_id: "r1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" } } })}\n`);
    let replaying: boolean | null = null;
    const original = (ctrl as unknown as { feedText: (t: string) => void }).feedText;
    (ctrl as unknown as { feedText: (t: string) => void }).feedText = function (this: ChatController, t: string) {
      replaying ??= (this as unknown as { isReplaying: boolean }).isReplaying;
      original.call(this, t);
    };
    await ctrl.attach(1);
    expect(replaying).toBe(true);
    expect(written).toEqual([]);
  });

  it("carries on across a line that was cut in two", async () => {
    const { transport, print } = daemonLog();
    const ctrl = controller(transport);
    await ctrl.attach(1);
    const line = assistant("a", "whole");
    print(line.slice(0, 10));
    ctrl.detach();
    print(line.slice(10));
    await ctrl.attach(1);
    expect(texts(ctrl)).toEqual(["whole"]);
  });

  it("rebuilds when the log does not carry on from where it left off", async () => {
    const cases: [string, (l: ReturnType<typeof daemonLog>) => void][] = [
      ["the log started over", (d) => { d.log.kept = assistant("n", "new"); d.log.seq = d.log.kept.length; }],
      ["same offset, other contents", (d) => { d.log.kept = assistant("n", "xxx"); }],
      ["trimmed past where it left off", (d) => { d.log.pinned = '{"id":"kelpie-init"}\n'; d.print(assistant("x", "y".repeat(400))); d.log.kept = assistant("n", "new"); }],
    ];
    for (const [name, change] of cases) {
      const d = daemonLog(assistant("a", "one"));
      const ctrl = controller(d.transport);
      await ctrl.attach(1);
      ctrl.detach();
      change(d);
      await ctrl.attach(1);
      expect(texts(ctrl), name).toEqual([name === "same offset, other contents" ? "xxx" : "new"]);
    }
  });

  it("rebuilds when asked to, after the daemon dropped output on its way here", async () => {
    const d = daemonLog(assistant("a", "one"));
    const history = vi.fn(async () => () => {});
    const ctrl = controller(d.transport);
    ctrl.loadHistory = history;
    await ctrl.attach(1);
    await ctrl.attach(1, true);
    expect(history).toHaveBeenCalledTimes(2);
    expect(texts(ctrl)).toEqual(["one"]);
  });

  it("rebuilds every time for a transport that does not say where the log ends", async () => {
    const { transport } = daemonLog(assistant("a", "one"));
    const ctrl = controller({ ...transport, logEnd: undefined });
    const history = vi.fn(async () => () => {});
    ctrl.loadHistory = history;
    await ctrl.attach(1);
    await ctrl.attach(1);
    expect(history).toHaveBeenCalledTimes(2);
  });

  it("keeps the rows on screen during a rebuild, however long the history takes to read", async () => {
    const d = daemonLog(assistant("a", "one"));
    const ctrl = controller(d.transport);
    await ctrl.attach(1);
    let finishRead!: (apply: () => void) => void;
    ctrl.loadHistory = () => new Promise((r) => (finishRead = r));
    const seen: number[] = [];
    const dispose = createRoot((off) => {
      createEffect(() => seen.push(ctrl.state.items.length));
      return off;
    });
    const attaching = ctrl.attach(1, true);
    await new Promise((r) => setTimeout(r));
    expect(ctrl.state.items).toHaveLength(1);
    finishRead(() => {});
    await attaching;
    dispose();
    expect(texts(ctrl)).toEqual(["one"]);
    expect(seen).not.toContain(0);
  });

  it("applies what is printed while a rebuild reads its history, once", async () => {
    const d = daemonLog(assistant("a", "one"));
    const ctrl = controller(d.transport);
    let finishRead!: (apply: () => void) => void;
    ctrl.loadHistory = () => new Promise((r) => (finishRead = r));
    const attaching = ctrl.attach(1);
    await new Promise((r) => setTimeout(r));
    // Printed after the log was handed over: the daemon sends it live, and it is not in `kept` for this attach.
    d.log.live?.(encode(assistant("b", "two")));
    finishRead(() => {});
    await attaching;
    expect(texts(ctrl)).toEqual(["one", "two"]);
  });
});
