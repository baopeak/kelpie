// Waiting for file transfers before an older session daemon hands over to this release's (after an update). The wait
// belongs to the daemon it started for: a new connection ends it, so a stale timer never stops the next daemon. A
// daemon that refuses because a transfer started meanwhile is waited for again, never given up on silently.

/** What `daemon.shutdown` answered: `true` (it stops), `false` (a session runs), or the transfers that hold it. */
export type ShutdownAnswer = boolean | { waiting_for_transfers?: number } | null | undefined;

export interface HandOverDeps {
  /** File transfers the connected daemon is copying (0 when none, or unknown). */
  transfers: () => Promise<number>;
  /** `daemon.shutdown` without stopping sessions: `idle` (no session and no transfer) or `no_transfers`. */
  shutdown: (when: "idle" | "no_transfers") => Promise<ShutdownAnswer>;
  /** Transfers that hold the hand-over now (0: nothing holds it). */
  setHeld: (n: number) => void;
  /** The transfers finished on a hand-over that waits for idle: the caller checks the sessions again. */
  idleAgain: () => void;
  /** The daemon agreed to stop. */
  handedOver: () => void;
  every?: number;
  setTimer?: (run: () => void, ms: number) => unknown;
  clearTimer?: (timer: unknown) => void;
}

/** The transfers a refusal names; 0 when the daemon agreed (or said something else). */
export function heldBy(a: ShutdownAnswer): number {
  return a && typeof a === "object" ? a.waiting_for_transfers ?? 0 : 0;
}

export function createHandOver(d: HandOverDeps) {
  const every = d.every ?? 5_000;
  const setTimer = d.setTimer ?? ((run, ms) => setTimeout(run, ms));
  const clearTimer = d.clearTimer ?? ((t) => clearTimeout(t as ReturnType<typeof setTimeout>));
  // The daemon connection the wait is for: bumped on every new connection and on "switch anyway".
  let instance = 0;
  let timer: unknown;
  let switchWhenDone = false;

  const stopTimer = () => {
    if (timer !== undefined) clearTimer(timer);
    timer = undefined;
  };

  /** Nothing waits any more (a new daemon connection, or the person switched anyway). */
  function reset() {
    instance++;
    stopTimer();
    switchWhenDone = false;
    d.setHeld(0);
  }

  /** `held` transfers hold the hand-over: checks again every few seconds while they run. */
  function wait(held: number) {
    d.setHeld(held);
    stopTimer();
    const mine = instance;
    timer = setTimer(async () => {
      timer = undefined;
      const n = await d.transfers();
      if (mine !== instance) return;
      if (n) return wait(n);
      d.setHeld(0);
      if (switchWhenDone) void handOver("no_transfers");
      else d.idleAgain();
    }, every);
  }

  /** The person chose to wait for the transfers: the switch happens once they have finished. */
  function switchWhenFinished(held: number) {
    switchWhenDone = true;
    wait(held);
  }

  /** Asks the daemon to hand over. Refused for transfers: waits again. Refused for a session (`idle` only): the
   *  caller's next session change asks again. */
  async function handOver(when: "idle" | "no_transfers"): Promise<"done" | "waiting" | "busy"> {
    const mine = instance;
    // A daemon that stops while answering has handed over.
    const answer = await d.shutdown(when).catch((): ShutdownAnswer => true);
    if (mine !== instance) return "busy";
    const held = heldBy(answer);
    if (held) {
      wait(held);
      return "waiting";
    }
    if (answer === true) {
      switchWhenDone = false;
      stopTimer();
      d.setHeld(0);
      d.handedOver();
      return "done";
    }
    return "busy";
  }

  return { reset, wait, switchWhenFinished, handOver, waiting: () => timer !== undefined || switchWhenDone };
}
