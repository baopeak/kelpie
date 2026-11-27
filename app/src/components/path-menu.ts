// The menu items every place that names a file offers (the file tree, a file's pane, a file named in a chat reply or a terminal): copy its
// path relative to the project, its full path, its name, and show it in the system's file manager.
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { revealEntry } from "../lib/ipc";
import { t } from "../lib/i18n";
import { isMac, joinPath } from "../lib/platform";
import { isOutsideFile, outsideParts } from "../lib/outside-file";
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

/** Where a file is: `rel` in the folder `root` (the project's; for a file outside the project, its own folder, and then it has no path
 *  relative to the project). */
type Place = { root: string; rel: string; outside?: boolean };

/** The copy and reveal items for a file whose place `where` gives (null: it could not be found, and the user was told). */
function pathItems(project: Pick<Project, "path">, where: () => Promise<Place | null>, name: string): MenuItem[] {
  const at = (fn: (p: Place) => unknown) => () => void where().then((p) => p !== null && fn(p));
  // An outside file's folder is held with `/`: on a drive it is written back with `\`, as the system writes it.
  const full = (p: Place) => absolutePath({ path: p.outside && /^[a-zA-Z]:/.test(p.root) ? p.root.replace(/\//g, "\\") : p.root }, p.rel);
  const items: MenuItem[] = [
    { label: t("Copy Relative Path"), run: at((p) => copy(p.outside ? full(p) : p.rel)) },
    { label: t("Copy Path"), run: at((p) => copy(full(p))) },
    { label: t("Copy Name"), run: () => copy(name) },
  ];
  // The file manager is this computer's: a remote project's files are not in it.
  if (!remoteRoot.test(project.path)) {
    items.push({ label: isMac ? t("Reveal in Finder") : t("Reveal in Explorer"), run: at((p) => revealEntry(p.root, p.rel).catch((e) => store.notify(String(e)))) });
  }
  return items;
}

/** Copy relative path, copy path, copy name and reveal, for `rel` (relative to the project folder, `/` or `\`; or a file pane's absolute
 *  path, for a file outside the project). */
export function pathMenuItems(project: Pick<Project, "path">, rel: string): MenuItem[] {
  let place: Place = { root: project.path, rel };
  if (isOutsideFile(rel) && !remoteRoot.test(project.path)) {
    const { root, name } = outsideParts(rel);
    place = { root, rel: name, outside: true };
  }
  return pathItems(project, async () => place, basename(rel));
}

/** The menu of a file named in a reply or a terminal line: open it, copy its paths, reveal it. The menu opens at once; the file is looked
 *  for in the project (a one byte read, then by its name) only when an item that needs it is picked, so a slow host never holds the menu
 *  back. `cwd`: the folder the text was written in, tried first. */
export function fileRefMenuItems(project: Project, ref: FileRef, onOpen?: () => void, cwd?: string): MenuItem[] {
  const where = async (): Promise<Place | null> => {
    const found = await resolveChatFile(project, ref, cwd);
    if ("rel" in found) return { root: project.path, rel: found.rel };
    if ("abs" in found) {
      const { root, name } = outsideParts(found.abs);
      return { root, rel: name, outside: true };
    }
    store.notify(found.error);
    return null;
  };
  return [{ label: t("Open"), run: () => void openChatFile(project, ref, onOpen, cwd) }, null, ...pathItems(project, where, basename(ref.path))];
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
