// A page the system suspended without saying so. An iPhone that locks may stop the page with no `visibilitychange` first, and Chrome
// can freeze a tab: the page then runs on after the unlock as though it had never left, its screen and its takeover still "open" on a
// connection that died meanwhile, and without the Face ID the screen asks again after every absence. Its own clock says it slept: a
// one-second tick that comes many seconds late.

/** A tick this late (ms) means the page did not run meanwhile (a busy page is never this late; a phone asleep is). */
export const SLEPT_MS = 5000;

export class PageSleep {
  private last: number | null = null;

  /** Called about every second while the page is shown, with the wall clock (`Date.now()`: a monotonic clock may stop while the phone
   *  sleeps). When the page slept since the previous call: the time it went to sleep (that call's); otherwise null. */
  tick(now: number): number | null {
    const last = this.last;
    this.last = now;
    return last !== null && now - last >= SLEPT_MS ? last : null;
  }

  /** The page was hidden, or its leaving and coming back were handled already: the gap that follows is not a sleep of its own. */
  reset() {
    this.last = null;
  }
}
