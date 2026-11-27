// A file outside the project, opened from a link in a terminal or the chat: shown read-only in Kelpie's viewer, by its absolute
// path. Only a local file: never a network share, which Windows would reach with the person's credentials (`\\host\…`, `//host/…`,
// and `\??\UNC\host\…` or `\\?\UNC\…`, which reach the same redirector).
import { isWindows } from "./platform";

/** `path` with `\` as `/` and `.` and `..` worked out, by name only. */
export function normalized(path: string): string {
  const parts: string[] = [];
  for (const part of path.replace(/\\/g, "/").split("/")) {
    if (part === "..") parts.length > 1 && parts.pop();
    else if (part !== "." && (part || !parts.length)) parts.push(part);
  }
  return parts.join("/");
}

/** A Git Bash path to a drive (`/d/Works/a.ts`) as Windows names it (`D:/Works/a.ts`); any other path as it is. */
export function drivePath(path: string, windows = isWindows): string {
  const m = windows ? /^\/([a-zA-Z])(\/.*)?$/.exec(path.replace(/\\/g, "/")) : null;
  return m ? `${m[1].toUpperCase()}:${m[2] ?? "/"}` : path;
}

/** A local absolute path: what a file pane outside the project holds (a pane of a file in the project holds a path relative to it).
 *  On Windows only a drive's (`C:\…`, `C:/…`): a path without one is read on whatever drive is current. Never one with `?` or `*`,
 *  which no file's name has, nor one that starts with two slashes. */
export function isOutsideFile(path: string, windows = isWindows): boolean {
  if (/[?*]/.test(path) || /^[\\/]{2}/.test(path)) return false;
  return /^[a-zA-Z]:[\\/]/.test(path) || (!windows && /^\//.test(path));
}

/** The folder and the name an outside file is read by: the viewer reads it as `name` in `root`, so the read stays in that folder. A
 *  drive's root has no name (`C:/` is not a file). */
export function outsideParts(path: string): { root: string; name: string } {
  const p = normalized(path);
  const at = p.lastIndexOf("/");
  if (at < 0) return { root: p, name: "" };
  const dir = p.slice(0, at);
  // A drive's root keeps its slash: `C:` alone is the drive's current folder.
  return { root: /^[a-zA-Z]:$/.test(dir) ? `${dir}/` : dir || "/", name: p.slice(at + 1) };
}
