// The desktop's way to a chat session: the Tauri IPC to the daemon (see transport.ts).
import { attachTerminal, b64ToBytes, daemon, sendInput } from "../lib/ipc";
import type { ChatTransport } from "./transport";

export function desktopTransport(): ChatTransport {
  let handle: number | null = null;
  return {
    async attach(h, onLive) {
      handle = h;
      const reply = await attachTerminal(h, null, (_seq, data) => onLive(data));
      return b64ToBytes(reply.data);
    },
    detach() {
      if (handle != null) void daemon("session.detach", { handle }).catch(() => {});
    },
    send(line) {
      if (handle != null) void sendInput(handle, line);
    },
    update(patch) {
      if (handle != null) void daemon("session.update", { handle, ...patch }).catch(() => {});
    },
  };
}
