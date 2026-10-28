// The page's way to parse a big spec file: in a worker (`spec-worker.ts`), so no turn of the main thread is long. The
// answer is `parseSpec`'s own; where there is no worker, or it fails, the file is parsed here, as it would have been.
import { WORKER_BYTES, parseSpec, problemDoc, stats, type ParseReply, type ParseRequest, type SpecDoc } from "./spec-index";

/** A worker that has not answered after this long is stopped (a file that makes the lexer hang must not stall the index). */
export const WORKER_TIMEOUT_MS = 15_000;
/** The size up to which such a file is then parsed here, which costs under a second: a larger one is shown as too slow to read. */
const MAIN_THREAD_BYTES = 64 * 1024;

let worker: Worker | null | undefined;
let requests = 0;
const waiting = new Map<number, { path: string; text: string; mtime: number; size: number; resolve: (d: SpecDoc) => void; timer?: ReturnType<typeof setTimeout> }>();

/** The worker cannot start or fails: what waits is parsed here, and the worker is not tried again. */
function giveUp(failed: Worker) {
  failed.terminate();
  if (worker === failed) worker = null;
  for (const r of [...waiting.values()]) {
    clearTimeout(r.timer);
    r.resolve(parseSpec(r.path, r.text, r.mtime, r.size));
  }
  waiting.clear();
}

/** One file took too long: the worker is stopped (the next big file starts a new one), the file is parsed here if that is
 *  cheap and shown as too slow to read if not, and what else waited for the worker is parsed here. */
function timedOut(id: number) {
  const r = waiting.get(id);
  if (!r) return;
  waiting.delete(id);
  const stuck = worker;
  worker = undefined;
  stuck?.terminate();
  r.resolve(r.text.length <= MAIN_THREAD_BYTES ? parseSpec(r.path, r.text, r.mtime, r.size) : problemDoc(r.path, r.mtime, r.size, "slow"));
  for (const [other, o] of [...waiting]) {
    waiting.delete(other);
    clearTimeout(o.timer);
    o.resolve(parseSpec(o.path, o.text, o.mtime, o.size));
  }
}

function startWorker(): Worker | null {
  if (worker !== undefined) return worker;
  if (typeof Worker === "undefined") return (worker = null);
  try {
    const w = new Worker(new URL("./spec-worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (e: MessageEvent<ParseReply>) => {
      const r = waiting.get(e.data.id);
      waiting.delete(e.data.id);
      clearTimeout(r?.timer);
      r?.resolve(e.data.doc);
    };
    w.onerror = () => giveUp(w);
    w.onmessageerror = () => giveUp(w);
    return (worker = w);
  } catch {
    return (worker = null);
  }
}

/** For the tests: forget the worker and what waits for it, so the next big file starts one again. */
export function resetWorker() {
  worker?.terminate();
  worker = undefined;
  for (const r of waiting.values()) clearTimeout(r.timer);
  waiting.clear();
  requests = 0;
}

export function parseSpecAsync(path: string, text: string, mtime: number, size = text.length): Promise<SpecDoc> {
  const w = text.length < WORKER_BYTES ? null : startWorker();
  if (!w) return Promise.resolve(parseSpec(path, text, mtime, size));
  stats.parses++;
  return new Promise((resolve) => {
    const id = ++requests;
    waiting.set(id, { path, text, mtime, size, resolve, timer: setTimeout(() => timedOut(id), WORKER_TIMEOUT_MS) });
    try {
      w.postMessage({ id, path, text, mtime, size } satisfies ParseRequest);
    } catch {
      giveUp(w);
    }
  });
}
