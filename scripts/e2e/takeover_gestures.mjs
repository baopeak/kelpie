// The phone page's own gesture and coordinate code (app/phone/src/takeover.ts), run on pointer events for scripts/e2e/takeover.py, the way
// pane-takeover.tsx drives it: a touch that lands outside the picture starts nothing, a release outside it is passed on as such.
//
//   node takeover_gestures.mjs < {"el": {left, top, width, height}, "natural": {w, h}, "meta": {...}, "events": [{type, id, x, y, t}]}
//
// Prints the ops, merged as the page sends them. Node strips the types of the .ts it imports (23.6 on; 22.6 to 23.5 with --experimental-strip-types).
import { readFileSync } from "node:fs";
import { coalesce, drawnBox, Gesture, toPage } from "../../app/phone/src/takeover.ts";

const spec = JSON.parse(readFileSync(0, "utf8"));
const box = drawnBox(spec.el, spec.natural.w, spec.natural.h);
const gesture = new Gesture();
const ops = [];
for (const e of spec.events) {
  const p = toPage(e.x, e.y, box, spec.meta);
  if (e.type === "down" && p) ops.push(...gesture.down(e.id, p, e.t));
  else if (e.type === "move" && p) ops.push(...gesture.move(e.id, p, e.t));
  else if (e.type === "up") ops.push(...gesture.up(e.id, p));
  else if (e.type === "cancel") ops.push(...gesture.cancel(e.id));
}
process.stdout.write(JSON.stringify(coalesce(ops)));
