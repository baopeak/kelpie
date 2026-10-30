// A short message about the open session that must outlive the thing it is about: "Already answered on the
// desktop 2 min ago" arrives after the prompt card was already removed by the next screen frame. It stays
// at least 5 s (here 8) or until the person dismisses it.

export const NOTICE_SHOWN_MS = 8000;

export interface Notice {
  id: number;
  session: string;
  text: string;
  at: number;
}

export class NoticeBoard {
  private seq = 0;
  private current: Notice | null = null;

  show(session: string, text: string, now: number): Notice {
    this.current = { id: ++this.seq, session, text, at: now };
    return this.current;
  }

  dismiss(id?: number) {
    if (this.current && (id === undefined || this.current.id === id)) this.current = null;
  }

  /** The notice for `session` that is still due to be shown. */
  visible(session: string, now: number): Notice | null {
    const n = this.current;
    return n && n.session === session && now - n.at < NOTICE_SHOWN_MS ? n : null;
  }
}

/** The notice an answer's reply calls for, as data: none while the card that asked is still on screen and shows the
 *  same outcome itself (it would say it twice). */
export function noticeFor(o: Parameters<typeof answerNotice>[0], cardStillShown: boolean) {
  return cardStillShown ? null : answerNotice(o);
}

/** What an answer's reply says that the person should still see after the card is gone, as data for a sentence. */
export function answerNotice(o: { t: string; result?: string; answered_by?: string; at?: number }): { kind: "stale"; by?: string; at?: number } | { kind: "dup" } | null {
  if (o.t === "error" && o.result === "stale") return { kind: "stale", by: o.answered_by, at: o.at };
  if (o.t === "error" && o.result === "dup") return { kind: "dup" };
  return null;
}
