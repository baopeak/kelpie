// The desktop's way to a chat session: the Tauri IPC to the daemon (see transport.ts).
import { attachTerminal, b64ToBytes, daemon, sendInput } from "../lib/ipc";
import { errorText } from "../lib/i18n";
import { store } from "../lib/store";
import type { EnqueueResult } from "./chat-queue";
import type { ChatTransport } from "./transport";

export function desktopTransport(): ChatTransport {
  let handle: number | null = null;
  let logEnd: number | null = null;
  return {
    async attach(h, onLive) {
      handle = h;
      logEnd = null;
      const reply = await attachTerminal(h, null, (_seq, data) => onLive(data));
      logEnd = reply.seq;
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
