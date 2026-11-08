// Editing a host's file. "Edit in Kelpie" opens it in Kelpie's editor through a remote project (which needs a shell on
// the host and compares before every save). "Open with external editor" downloads a private copy through the queue,
// opens it (src-tauri/src/sftp_edit.rs watches it) and uploads each save with the server copy's size and time as
// `expect`, so a change made on the server meanwhile is asked about, never overwritten. Tested in remote-edit.test.ts.
import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { onEvent, sftpStat, xferList, xferStart } from "./ipc";
import { t } from "./i18n";
import { store } from "./store";
import { confirmDialog } from "./confirm-dialog";
import { checkHost, hostProject, isHostKeyError, type SshHost } from "./ssh-hosts";
import { baseName, parentPath } from "./file-pane";
import type { XferJob, XferStart } from "./types";

export interface EditSession {
  id: string;
  dest: string;
  remotePath: string;
  localPath: string;
  name: string;
  /** The server copy as last downloaded or uploaded: an upload replaces only this. */
  base: { size: number; mtime: number };
  /** Upload on every save (else ask each time). */
  auto: boolean;
  state: "downloading" | "open" | "uploading" | "asking" | "pending" | "error";
  lastUpload: number | null;
  error: string | null;
  /** A save arrived while an upload ran or waited for an answer: the latest content is uploaded right after it. */
  dirty: boolean;
}

const [sessions, setSessions] = createSignal<Record<string, EditSession>>({});
export const editSessions = () => Object.values(sessions()).sort((a, b) => a.name.localeCompare(b.name));

const patch = (id: string, p: Partial<EditSession>) => setSessions((all) => (all[id] ? { ...all, [id]: { ...all[id], ...p } } : all));

/** The upload of a save: one file into its folder on the host, replacing only the copy last seen there. */
export function uploadJob(s: EditSession): XferStart {
  return { kind: "copy", src: {}, dst: { dest: s.dest }, items: [s.localPath], dst_dir: parentPath(s.dest, s.remotePath), conflict: "ask", preserve: false, expect: { ...s.base } };
}

/** What a finished upload means for the session. */
export function afterUpload(s: EditSession, job: XferJob, now: { size: number; mtime: number } | null): Partial<EditSession> {
  if (job.state !== "done" || job.totals.failed) return { state: "error", error: job.error ?? job.failures[0]?.error ?? t("The upload did not finish") };
  if (job.totals.skipped) return { state: "open" }; // "Discard my upload": the server copy stays as it was
  return { state: "open", base: now ?? s.base, lastUpload: Date.now(), error: null };
}

const openWithKey = (name: string) => `kelpie.files.openWith.${(name.split(".").pop() ?? "").toLowerCase()}`;
const remembered = (name: string): string | null => {
  try {
    return localStorage.getItem(openWithKey(name));
  } catch {
    return null;
  }
};

/** Waits for a job of the queue to finish (the queue tells the window; this asks it, so it also works before the events arrive). */
async function finished(id: string, onWaiting?: () => void): Promise<XferJob> {
  for (;;) {
    const job = (await xferList()).jobs.find((j) => j.id === id);
    if (!job) throw new Error(t("The transfer is gone"));
    if (job.state === "done" || job.state === "failed" || job.state === "cancelled") return job;
    if (job.questions.length) onWaiting?.();
    await new Promise((r) => setTimeout(r, 400));
  }
}

/** "Open with external editor" (`pick`: "Open with…", the person chooses the app, remembered per extension). */
export async function editExternal(dest: string, remotePath: string, pick = false) {
  const name = baseName(remotePath);
  let openWith = remembered(name);
  if (pick) {
    const chosen = await openDialog({ title: t("Open {name} with", { name }), multiple: false, directory: false });
    if (typeof chosen !== "string") return;
    openWith = chosen;
    try {
      localStorage.setItem(openWithKey(name), chosen);
    } catch {
      // Not remembered in a private window.
    }
  }
  const st = await sftpStat(dest, remotePath);
  const base = { size: st.stat?.size ?? 0, mtime: st.stat?.mtime ?? 0 };
  const { id, path } = await invoke<{ id: string; path: string }>("sftp_edit_prepare", { name });
  setSessions((all) => ({ ...all, [id]: { id, dest, remotePath, localPath: path, name, base, auto: true, state: "downloading", lastUpload: null, error: null, dirty: false } }));
  try {
    const job = await finished(await xferStart({ kind: "copy", src: { dest }, dst: {}, items: [remotePath], dst_dir: parentPath(null, path), conflict: "replace" }));
    if (job.state !== "done" || job.totals.failed) throw new Error(job.error ?? job.failures[0]?.error ?? t("The download did not finish"));
    await invoke("sftp_edit_watch", { id, name, openWith });
    patch(id, { state: "open" });
  } catch (e) {
    patch(id, { state: "error", error: String(e) });
  }
}

/** What a save does: upload now, or (an upload is running or asking) mark that a newer save follows it. */
export function onSave(s: EditSession): "upload" | "follow" | "ask" {
  if (s.state === "uploading" || s.state === "asking") return "follow";
  return s.auto ? "upload" : "ask";
}

/** After an upload: send the newer save that came in meanwhile (never one that failed or was discarded). */
export function uploadAgain(s: EditSession): boolean {
  return s.dirty && s.state === "open";
}

/** Uploads the session's copy now; a save that arrives meanwhile follows as soon as this one is done. */
export async function uploadEdit(id: string) {
  const s = sessions()[id];
  if (!s) return;
  if (s.state === "uploading" || s.state === "asking") return patch(id, { dirty: true });
  patch(id, { state: "uploading", error: null, dirty: false });
  try {
    const job = await finished(await xferStart(uploadJob(s)), () => patch(id, { state: "asking" }));
    const now = job.state === "done" ? await sftpStat(s.dest, s.remotePath).then((r) => (r.stat ? { size: r.stat.size, mtime: r.stat.mtime } : null)).catch(() => null) : null;
    patch(id, afterUpload(sessions()[id] ?? s, job, now));
  } catch (e) {
    patch(id, { state: "error", error: String(e) });
  }
  const after = sessions()[id];
  if (after && uploadAgain(after)) return uploadEdit(id);
  // A newer save after a failed or discarded upload stays to be sent by hand.
  if (after?.dirty) patch(id, { state: after.state === "error" ? "error" : "pending" });
}

export function setAutoUpload(id: string, auto: boolean) {
  patch(id, { auto });
}

/** Whether Stop would lose edits not yet on the server. */
export const unsent = (s: EditSession) => s.dirty || s.state === "pending" || s.state === "error" || s.state === "uploading" || s.state === "asking";

/** Stops watching and deletes the local copy (asking first when it holds edits not yet on the server). */
export async function stopEdit(id: string) {
  const s = sessions()[id];
  if (s && unsent(s) && !(await confirmDialog(t("{name} has changes that are not on {host} yet. Stop anyway and delete the local copy?", { name: s.name, host: s.dest })))) return;
  await invoke("sftp_edit_stop", { id }).catch(() => {});
  setSessions((all) => {
    const next = { ...all };
    delete next[id];
    return next;
  });
}

let listening = false;
/** Saves in the external editor arrive as `sftp-edit-changed` (once per window). */
export function initRemoteEdit() {
  if (listening) return;
  listening = true;
  void onEvent<{ id: string }>("sftp-edit-changed", ({ id }) => {
    const s = sessions()[id];
    if (!s) return;
    const what = onSave(s);
    if (what === "follow") return patch(id, { dirty: true });
    if (what === "upload") return void uploadEdit(id);
    patch(id, { state: "pending" });
    store.notify(t("{name} changed: upload it to {host}?", { name: s.name, host: s.dest }), { label: t("Upload"), run: () => void uploadEdit(id) });
  });
}

/** "Edit in Kelpie": Kelpie's editor through the host's remote project, or a project for the file's folder when it is outside. */
export async function editInKelpie(host: SshHost, path: string) {
  let check;
  try {
    check = await checkHost(host);
  } catch (e) {
    if (isHostKeyError(e)) throw e;
    throw new Error(t("Kelpie's editor needs a shell on the host; use an external editor ({reason})", { reason: String(e) }));
  }
  const inside = (r: string) => r === "/" || path.startsWith(`${r.replace(/\/+$/, "")}/`);
  // A remote project of this host that holds the file (the deepest one), else the host's own, else one for its folder.
  const holding = store.state.projects
    .filter((p) => p.remote?.dest === host.alias && inside(p.remote.path))
    .sort((a, b) => (b.remote?.path.length ?? 0) - (a.remote?.path.length ?? 0))[0];
  let id = holding?.id ?? (await hostProject(host, check));
  let root = store.state.projects.find((p) => p.id === id)?.remote?.path ?? "/";
  if (!inside(root)) {
    root = parentPath(host.alias, path);
    // A project root keeps no backslash and no space at either end of a folder name: such a folder opens elsewhere.
    if (root.split("/").some((part) => part.includes("\\") || part !== part.trim())) throw new Error(t("Kelpie's editor cannot open a file in this folder; use an external editor"));
    id = await hostProject(host, check, root);
  }
  store.setOverlay(null);
  store.switchProject(id);
  store.openFile(id, path.slice(root.replace(/\/+$/, "").length).replace(/^\/+/, ""));
}
