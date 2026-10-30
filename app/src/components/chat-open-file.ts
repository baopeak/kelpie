// Opens what the transcript names. Agent output is untrusted: a file goes to Kelpie's own viewer
// only, by a path relative to the project, and nothing is ever handed to the OS to launch.
import { openUrl } from "@tauri-apps/plugin-opener";
import { type FileRef, parseChatLink, projectRelative } from "../chat/chat-links";
import { basename } from "../chat/tool-view";
import * as ipc from "../lib/ipc";
import { t } from "../lib/i18n";
import { store } from "../lib/store";
import type { Project } from "../lib/types";

/** Opens `ref` in the viewer, or says why not. A one byte read checks it exists (a large or binary
 *  file reads fine); failing that, it may be somewhere else in the project under the same name. */
export async function openChatFile(project: Project, ref: FileRef) {
  const rel = projectRelative(project, ref.path);
  if (rel === null) return store.notify(t("{path} is outside this project", { path: ref.path }));
  const open = (path: string) => store.openFile(project.id, path, { line: ref.line });
  const found = await ipc.readFile(project.path, rel, 1).then(() => true, () => false);
  if (found) return open(rel);
  const name = basename(rel);
  const hits = await ipc.quickOpen(project.path, name, 50).catch(() => []);
  const exact = hits.filter((h) => !h.is_dir && basename(h.path) === name).map((h) => projectRelative(project, h.path)).filter((p) => p !== null);
  if (exact.length === 1) return open(exact[0]);
  store.notify(exact.length ? t("{n} files named {name}", { n: exact.length, name }) : t("File not found: {path}", { path: ref.path }));
}

/** Follows a link from the transcript. Without a project, file links do nothing. */
export function openChatLink(href: string, project?: Project) {
  const link = parseChatLink(href);
  if (link.kind === "web") void openUrl(link.url);
  else if (link.kind === "file" && project) void openChatFile(project, link);
}
