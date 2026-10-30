// The chats the phone is following: one observer controller each, and where in each log the phone is.
// A chat line that arrives for a chat nobody follows any more (the person went Back while the daemon had
// one in flight) is ignored: it must not move the position, or the next open would ask for "what came
// after" and show only the tail.
import type { ChatController } from "../../src/chat/chat-model";

export interface ChatLine {
  id: string;
  mode: "reset" | "delta" | "more" | "live";
  seq: number;
  lines: string[];
}

export class ChatFeed {
  private controllers = new Map<string, ChatController>();
  private seq = new Map<string, number>();

  controller(id: string): ChatController | undefined {
    return this.controllers.get(id);
  }

  open(id: string, ctrl: ChatController) {
    this.controllers.set(id, ctrl);
    this.seq.delete(id);
  }

  close(id: string) {
    this.controllers.delete(id);
    this.seq.delete(id);
  }

  /** Where in the log the phone is, for `chat_sub {since_seq}`: null when it holds nothing. */
  since(id: string): number | null {
    return this.controllers.has(id) ? this.seq.get(id) ?? null : null;
  }

  /** The daemon restarted: every position means nothing now. */
  forgetPositions() {
    this.seq.clear();
    // ...and so does an answer saved for a retry: a new request may reuse the id of an old one.
    for (const c of this.controllers.values()) c.forgetSentAnswers();
  }

  /** Applies a chat message; false when nobody follows that chat. */
  apply(m: ChatLine): boolean {
    const ctrl = this.controllers.get(m.id);
    if (!ctrl) return false;
    this.seq.set(m.id, m.seq);
    ctrl.applyLog(m.mode, m.lines);
    return true;
  }
}
