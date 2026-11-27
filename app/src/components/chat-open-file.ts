// Opens what the transcript names. Agent output is untrusted: a file goes to Kelpie's own viewer only, by a path relative to the
// project, or read-only by its absolute path for a local file outside it, and nothing is ever handed to the OS to launch.
import { openUrl } from "@tauri-apps/plugin-opener";
import { type FileRef, parseChatLink, projectRelative } from "../chat/chat-links";
import { basename } from "../chat/tool-view";
import * as ipc from "../lib/ipc";
import { drivePath, isOutsideFile, normalized, outsideParts } from "../lib/outside-file";
import { isRemote } from "../lib/remote";
import { t } from "../lib/i18n";
import { store } from "../lib/store";
import type { Project } from "../lib/types";

const isRooted = (path: string) => /^([a-zA-Z]:|[\\/])/.test(path);

/** `rel` joined to the folder `cwd`, with `.` and `..` worked out (by name only; the result is then placed in or out of the project). */
const underFolder = (cwd: string, rel: string) => normalized(`${cwd}/${rel}`);

/** Where `ref` is in the project, relative to its folder; or, for a local file outside a local project, its absolute path (shown
 *  read-only); or why it cannot be named. A one byte read checks it exists (a large or binary file reads fine); failing that, a file of
 *  the project may be somewhere else in it under the same name. `cwd`: the folder the text was written in (a terminal's), tried before
 *  the project's own for a relative path. */
export async function resolveChatFile(project: Project, ref: FileRef, cwd?: string): Promise<{ rel: string } | { abs: string } | { error: string }> {
  // A Git Bash path (`/d/w/a.ts`, as agents running in it print) names a drive on this computer.
  const path = isRemote(project) ? ref.path : drivePath(ref.path);
  const fromCwd = cwd && !isRooted(path) ? projectRelative(project, underFolder(cwd, path)) : null;
  const rel = projectRelative(project, path);
  const tries = [...new Set([fromCwd, rel].filter((p): p is string => p !== null))];
  if (!tries.length) return outside(project, path, ref.path, cwd);
  for (const path of tries) {
    if (await ipc.readFile(project.path, path, 1).then(() => true, () => false)) return { rel: path };
  }
  const name = basename(tries[0]);
  const hits = await ipc.quickOpen(project.path, name, 50).catch(() => []);
  const exact = hits.filter((h) => !h.is_dir && basename(h.path) === name).map((h) => projectRelative(project, h.path)).filter((p) => p !== null);
  if (exact.length === 1) return { rel: exact[0] };
  return { error: exact.length ? t("{n} files named {name}", { n: exact.length, name }) : t("File not found: {path}", { path: ref.path }) };
}

/** `path` outside the project: its absolute path when it is a local file that exists, read by its folder and name. A network share, a
 *  drive-relative path (`D:a.ts`) and anything of a project on a host stay refused (`isOutsideFile`). `shown`: the path as it was named. */
async function outside(project: Project, path: string, shown: string, cwd?: string): Promise<{ abs: string } | { error: string }> {
  const refused = { error: t("{path} is outside this project", { path: shown }) };
  if (isRemote(project) || (isRooted(path) && !isOutsideFile(path))) return refused;
  const abs = isRooted(path) ? normalized(path) : underFolder(cwd ?? project.path, path);
  const { root, name } = outsideParts(abs);
  if (!isOutsideFile(abs) || !name) return refused;
  const exists = await ipc.readFile(root, name, 1).then(() => true, () => false);
  return exists ? { abs } : { error: t("File not found: {path}", { path: shown }) };
}

/** Opens `ref` in the viewer, or says why not. `onOpen`: it is about to open, so a dialog over the viewer can get out of the way and
 *  leave the keyboard to it. */
export async function openChatFile(project: Project, ref: FileRef, onOpen?: () => void, cwd?: string) {
  const found = await resolveChatFile(project, ref, cwd);
  if ("error" in found) return store.notify(found.error);
  onOpen?.();
  return store.openFile(project.id, "abs" in found ? found.abs : found.rel, { line: ref.line });
}

/** Follows a link from the transcript. Without a project, file links do nothing. */
export function openChatLink(href: string, project?: Project, onOpenFile?: () => void) {
  const link = parseChatLink(href);
  if (link.kind === "web") void openUrl(link.url);
  else if (link.kind === "file" && project) void openChatFile(project, link, onOpenFile);
}
