// Asks open file views to read their file again. A local project also hears of changes from its watcher
// (`fs-changed`); a remote project has none (fs_watch skips `ssh://` roots), so the Explorer's Refresh, a viewer's
// Reload and a remote editor regaining focus go through here. Tested in file-refresh.test.ts.
import { createSignal } from "solid-js";

export interface RefreshAsk {
  root: string;
  /** One file of the project, or null for every open file of it. */
  path: string | null;
  n: number;
}

const [ask, setAsk] = createSignal<RefreshAsk>({ root: "", path: null, n: 0 });
/** The latest ask (a new object each time, so an effect runs for every ask). */
export const refreshAsk = ask;

/** Every open view of `root` (or only `path` in it) reads its file again. */
export function refreshFiles(root: string, path: string | null = null) {
  setAsk((a) => ({ root, path, n: a.n + 1 }));
}

/** The ask is about this view's file. */
export const asksFor = (a: RefreshAsk, root: string, path: string) => a.n > 0 && a.root === root && (a.path === null || a.path === path);

/** What an editor does with a fresh read of its file. */
export type Reread = "same" | "apply" | "changed";

/**
 * `same`: the text it holds is what the file has (only the time is taken over, so the next save compares with it);
 * `apply`: the file changed and there are no unsaved edits, so the new text replaces the old;
 * `changed`: the file changed under unsaved edits, which are kept and the person is told.
 * A host's times are whole seconds: two writes in one second have one time, so the text decides, not the time.
 */
export function reread(fresh: { text: string | null; truncated: boolean }, held: { text: string; truncated: boolean }, dirty: boolean): Reread {
  if (fresh.text !== null && fresh.text === held.text && fresh.truncated === held.truncated) return "same";
  return dirty ? "changed" : "apply";
}
