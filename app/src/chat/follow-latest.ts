/** How close to the bottom still counts as being there, in pixels. */
const NEAR_BOTTOM = 40;

/** Whether the transcript keeps following its latest output, after a scroll. Distance alone cannot
 *  tell: a row growing, or first painted at its real height, moves the bottom away with nobody
 *  scrolling. Only moving up leaves the bottom; reaching it again comes back. */
export function followsLatest(following: boolean, gap: number, top: number, lastTop: number): boolean {
  if (gap < NEAR_BOTTOM) return true;
  return following && top >= lastTop;
}
