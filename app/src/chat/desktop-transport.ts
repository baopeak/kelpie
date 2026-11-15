// The desktop's way to a chat session: the Tauri IPC to the daemon (see transport.ts).
import { attachTerminal, b64ToBytes, daemon, sendInput } from "../lib/ipc";
import { errorText } from "../lib/i18n";
import { store } from "../lib/store";
import type { EnqueueResult } from "./chat-queue";
import type { ChatTransport } from "./transport";
import { fitChunk } from "../lib/stream-continuity";

export function desktopTransport(): ChatTransport {
  let handle: number | null = null;
  let logEnd: number | null = null;
  /** Counts attaches: chunks of an earlier one are dropped. */
  let attaches = 0;
  return {
    async attach(h, onLive) {
      handle = h;
      logEnd = null;
      // Live chunks are checked against the log's offset: one past a gap would lose the line a turn or a message starts with,
      // and the transcript would put the next turn's words in the last one. Until the reply says where the log ends, they wait.
      const attachId = ++attaches;
      let have: number | null = null;
      let broken = false;
      const waiting: { seq: number; data: Uint8Array }[] = [];
      const live = (seq: number, data: Uint8Array) => {
        if (broken || attachId !== attaches) return;
        if (have == null) return void waiting.push({ seq, data });
        const fit = fitChunk(have, seq, data);
        if (fit.kind === "old") return;
        if (fit.kind === "gap") {
          broken = true;
          const id = Object.values(store.sessions).find((s) => s.handle === h)?.id;
          console.warn(`chat ${id ?? h}: ${fit.missing} bytes of the log missing at ${have}; loading the conversation again`);
          // As when the daemon drops a window that fell behind: the chat pane attaches again and rebuilds the transcript.
          if (id) window.dispatchEvent(new CustomEvent("kelpie:chat-resync", { detail: id }));
          return;
        }
        have = seq;
        onLive(fit.data);
      };
      const reply = await attachTerminal(h, null, live);
      logEnd = reply.seq;
      have = reply.seq;
      for (const chunk of waiting.splice(0)) live(chunk.seq, chunk.data);
      return b64ToBytes(reply.data);
    },
    logEnd: () => logEnd,
    detach() {
      if (handle != null) void daemon("session.detach", { handle }).catch(() => {});
    },
    send(line) {
      // Refused (too large for the connection, or no connection): the person has to know it did not go.
      if (handle != null) void sendInput(handle, line).catch((e) => store.notify(errorText(e), undefined, 7000));
    },
    update(patch) {
      if (handle != null) void daemon("session.update", { handle, ...patch }).catch(() => {});
    },
    async enqueue(cid, text, rich, images) {
      if (handle == null) throw new Error("not attached");
      return daemon<EnqueueResult>("chat.send", { handle, cid, text, rich, images });
    },
    async writeTurn(qid, lines, turn, retry, tooLarge) {
      if (handle == null) throw new Error("not attached");
      try {
        await daemon("chat.write_turn", { handle, qid, lines, turn, ...(retry ? { retry: true } : {}), ...(tooLarge ? { too_large: true } : {}) });
      } catch (e) {
        // The daemon's reason: too_late, busy, switching, too_large.
        throw new Error(String(e).replace(/^Error:\s*/, ""));
      }
    },
    async queueOp(op, qid, opts) {
      if (handle == null) throw new Error("not attached");
      await daemon("chat.queue_op", { handle, op, qid, ...opts });
    },
  };
}
