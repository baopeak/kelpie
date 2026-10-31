// The desktop's way to a chat session: the Tauri IPC to the daemon (see transport.ts).
import { attachTerminal, b64ToBytes, daemon, sendInput } from "../lib/ipc";
import { errorText } from "../lib/i18n";
import { store } from "../lib/store";
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
      // Refused (too large for the connection, or no connection): the person has to know it did not go.
      if (handle != null) void sendInput(handle, line).catch((e) => store.notify(errorText(e), undefined, 7000));
    },
    update(patch) {
      if (handle != null) void daemon("session.update", { handle, ...patch }).catch(() => {});
    },
  };
}
