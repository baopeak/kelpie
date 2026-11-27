// A file outside the project, opened from a link in a terminal or the chat: shown read-only in Kelpie's viewer, by its absolute
// path. Only a local file: never a network share (`\\host\…`, `//host/…`), which Windows would reach with the person's credentials.

/** `path` with `\` as `/` and `.` and `..` worked out, by name only. */
export function normalized(path: string): string {
  const parts: string[] = [];
  for (const part of path.replace(/\\/g, "/").split("/")) {
    if (part === "..") parts.length > 1 && parts.pop();
    else if (part !== "." && (part || !parts.length)) parts.push(part);
  }
  return parts.join("/");
}

/** A local absolute path (`C:\…`, `C:/…`, `/…`): what a file pane outside the project holds. A pane of a file in the project holds a
 *  path relative to it. */
export const isOutsideFile = (path: string) => /^[a-zA-Z]:[\\/]/.test(path) || /^[\\/](?![\\/])/.test(path);

/** The folder and the name an outside file is read by: the viewer reads it as `name` in `root`, so the read stays in that folder. */
export function outsideParts(path: string): { root: string; name: string } {
  const p = normalized(path);
  const at = p.lastIndexOf("/");
  const dir = p.slice(0, at);
  // A drive's root keeps its slash: `C:` alone is the drive's current folder.
  return { root: /^[a-zA-Z]:$/.test(dir) ? `${dir}/` : dir || "/", name: p.slice(at + 1) };
}
