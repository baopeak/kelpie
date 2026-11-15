// A session's output reaches a view as chunks, each tagged with the offset its last byte ends at in everything the session ever
// printed (`seq`, the daemon's stream offset). A view that applies a chunk not starting where it left off draws the wrong thing
// for as long as the program redraws only what changed: a terminal keeps stale text, a chat's log loses the line a turn starts
// with. This is the pure check both views run on every live chunk.

export type ChunkFit =
  /** Apply `data` (the whole chunk, or the part after what the view already has). */
  | { kind: "apply"; data: Uint8Array }
  /** Everything in it was applied already. */
  | { kind: "old" }
  /** Bytes are missing before it: the view must attach again from where it is. */
  | { kind: "gap"; missing: number };

/** Where a chunk ending at offset `end` stands against a view that has applied the stream up to offset `have`. */
export function fitChunk(have: number, end: number, data: Uint8Array): ChunkFit {
  const start = end - data.length;
  if (start === have) return { kind: "apply", data };
  if (start > have) return { kind: "gap", missing: start - have };
  if (end <= have) return { kind: "old" };
  return { kind: "apply", data: data.subarray(have - start) };
}
