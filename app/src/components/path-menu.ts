// The menu items every place that names a file offers (the file tree, a file's pane, a file named in a chat reply or a terminal): copy its
// path relative to the project, its full path, its name, and show it in the system's file manager.
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { revealEntry } from "../lib/ipc";
import { t } from "../lib/i18n";
import { isMac, joinPath } from "../lib/platform";
import { projectTarget, sessionTarget } from "../lib/remote";
import { store } from "../lib/store";
import type { Project } from "../lib/types";
import type { FileRef } from "../chat/chat-links";
import { basename } from "../chat/tool-view";
import { openChatFile, resolveChatFile } from "./chat-open-file";
import type { MenuItem } from "./context-menu";

const remoteRoot = /^ssh:\/\/[^/]+/;

/** `rel`'s full path: on this computer as the system writes it, on a remote project's host as that host does (`/abs/rel`). */
export function absolutePath(project: Pick<Project, "path">, rel: string): string {
  if (!remoteRoot.test(project.path)) return joinPath(project.path, rel);
  const root = project.path.replace(remoteRoot, "").replace(/\/+$/, "");
  return rel ? `${root}/${rel.replace(/\\/g, "/")}` : root || "/";
}

const copy = (text: string) => void writeText(text).then(() => store.notify(t("Copied")), (e) => store.notify(String(e)));

/** The copy and reveal items for a file whose place in the project `rel` gives (null: it could not be found, and the user was told). */
function pathItems(project: Pick<Project, "path">, rel: () => Promise<string | null>, name: string): MenuItem[] {
  const withRel = (fn: (rel: string) => unknown) => () => void rel().then((r) => r !== null && fn(r));
  const items: MenuItem[] = [
    { label: t("Copy Relative Path"), run: withRel(copy) },
    { label: t("Copy Path"), run: withRel((r) => copy(absolutePath(project, r))) },
    { label: t("Copy Name"), run: () => copy(name) },
  ];
  // The file manager is this computer's: a remote project's files are not in it.
  if (!remoteRoot.test(project.path)) {
    items.push({ label: isMac ? t("Reveal in Finder") : t("Reveal in Explorer"), run: withRel((r) => revealEntry(project.path, r).catch((e) => store.notify(String(e)))) });
  }
  return items;
}

/** Copy relative path, copy path, copy name and reveal, for `rel` (relative to the project folder, `/` or `\`). */
export const pathMenuItems = (project: Pick<Project, "path">, rel: string): MenuItem[] => pathItems(project, async () => rel, basename(rel));

/** The menu of a file named in a reply or a terminal line: open it, copy its paths, reveal it. The menu opens at once; the file is looked
 *  for in the project (a one byte read, then by its name) only when an item that needs it is picked, so a slow host never holds the menu
 *  back. `cwd`: the folder the text was written in, tried first. */
export function fileRefMenuItems(project: Project, ref: FileRef, onOpen?: () => void, cwd?: string): MenuItem[] {
  const rel = async () => {
    const found = await resolveChatFile(project, ref, cwd);
    if ("rel" in found) return found.rel;
    store.notify(found.error);
    return null;
  };
  return [{ label: t("Open"), run: () => void openChatFile(project, ref, onOpen, cwd) }, null, ...pathItems(project, rel, basename(ref.path))];
}

/** The project a terminal session's file links resolve in, with the folder its relative paths start from; none for a session with no
 *  project, or one running on another machine (or another linked host) than its project's files. */
export function sessionFiles(sessionId: string): { project: Project; cwd: string } | null {
  const s = store.sessions[sessionId];
  const project = s?.project_id ? store.state.projects.find((p) => p.id === s.project_id) : undefined;
  if (!s || !project) return null;
  const [at, files] = [sessionTarget(s), projectTarget(project)];
  if (at?.dest !== files?.dest || (at?.port ?? null) !== (files?.port ?? null)) return null;
  return { project, cwd: s.cwd };
}

/** Open link and copy link, for an `http(s)` address. */
export function linkMenuItems(url: string, open: () => void): MenuItem[] {
  return [
    { label: t("Open Link"), run: open },
    { label: t("Copy Link"), run: () => copy(url) },
  ];
}
