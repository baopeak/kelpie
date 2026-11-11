// Opens what the transcript names. Agent output is untrusted: a file goes to Kelpie's own viewer
// only, by a path relative to the project, and nothing is ever handed to the OS to launch.
import { openUrl } from "@tauri-apps/plugin-opener";
import { type FileRef, parseChatLink, projectRelative } from "../chat/chat-links";
import { basename } from "../chat/tool-view";
import * as ipc from "../lib/ipc";
import { t } from "../lib/i18n";
import { store } from "../lib/store";
import type { Project } from "../lib/types";

const isRooted = (path: string) => /^([a-zA-Z]:|[\\/])/.test(path);

/** `rel` joined to the folder `cwd`, with `.` and `..` worked out (by name only; the result is still checked to lie in the project). */
function underFolder(cwd: string, rel: string): string {
  const parts: string[] = [];
  for (const part of `${cwd}/${rel}`.replace(/\\/g, "/").split("/")) {
    if (part === "..") parts.length > 1 && parts.pop();
    else if (part !== "." && (part || !parts.length)) parts.push(part);
  }
  return parts.join("/");
}

/** Where `ref` is in the project, relative to its folder, or why it cannot be named there. A one byte read checks it exists (a large or
 *  binary file reads fine); failing that, it may be somewhere else in the project under the same name. `cwd`: the folder the text was
 *  written in (a terminal's), tried before the project's own for a relative path. */
export async function resolveChatFile(project: Project, ref: FileRef, cwd?: string): Promise<{ rel: string } | { error: string }> {
  const fromCwd = cwd && !isRooted(ref.path) ? projectRelative(project, underFolder(cwd, ref.path)) : null;
  const rel = projectRelative(project, ref.path);
  const tries = [...new Set([fromCwd, rel].filter((p): p is string => p !== null))];
  if (!tries.length) return { error: t("{path} is outside this project", { path: ref.path }) };
  for (const path of tries) {
    if (await ipc.readFile(project.path, path, 1).then(() => true, () => false)) return { rel: path };
  }
  const name = basename(tries[0]);
  const hits = await ipc.quickOpen(project.path, name, 50).catch(() => []);
  const exact = hits.filter((h) => !h.is_dir && basename(h.path) === name).map((h) => projectRelative(project, h.path)).filter((p) => p !== null);
  if (exact.length === 1) return { rel: exact[0] };
  return { error: exact.length ? t("{n} files named {name}", { n: exact.length, name }) : t("File not found: {path}", { path: ref.path }) };
}

/** Opens `ref` in the viewer, or says why not. `onOpen`: it is about to open, so a dialog over the viewer can get out of the way and
 *  leave the keyboard to it. */
export async function openChatFile(project: Project, ref: FileRef, onOpen?: () => void, cwd?: string) {
  const found = await resolveChatFile(project, ref, cwd);
  if ("error" in found) return store.notify(found.error);
  onOpen?.();
  return store.openFile(project.id, found.rel, { line: ref.line });
}

/** Follows a link from the transcript. Without a project, file links do nothing. */
export function openChatLink(href: string, project?: Project, onOpenFile?: () => void) {
  const link = parseChatLink(href);
  if (link.kind === "web") void openUrl(link.url);
  else if (link.kind === "file" && project) void openChatFile(project, link, onOpenFile);
}
