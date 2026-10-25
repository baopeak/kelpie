// Parses big spec files off the main thread: the answer is `parseSpec`'s own, whole-file, as on the main thread.
import { parseSpec, type ParseReply, type ParseRequest } from "./spec-index";

export const handle = (m: ParseRequest): ParseReply => ({ id: m.id, doc: parseSpec(m.path, m.text, m.mtime, m.size) });

// Only inside a worker (the tests import `handle` into node or a page, where this does nothing).
const scope = globalThis as unknown as { WorkerGlobalScope?: unknown; onmessage: ((e: MessageEvent<ParseRequest>) => void) | null; postMessage(m: ParseReply): void };
if (scope.WorkerGlobalScope && globalThis instanceof (scope.WorkerGlobalScope as new () => object)) scope.onmessage = (e) => scope.postMessage(handle(e.data));
