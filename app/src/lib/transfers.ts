// The transfer queue as the window sees it: the daemon owns the jobs (`xfer.*`); this keeps the latest state of each,
// fed by `xfer.updated` / `xfer.removed`, and words it (speed, ETA, state, direction). Tested in transfers.test.ts.
import { createSignal } from "solid-js";
import { onEvent, xferList } from "./ipc";
import { t } from "./i18n";
import { formatSize } from "./file-pane";
import type { XferJob, XferQuestion } from "./types";

export type Jobs = Record<string, XferJob>;

/** A job's newer state replaces an older one; an update that arrives late (older `updated`) is ignored. */
export function applyJob(jobs: Jobs, job: XferJob): Jobs {
  const old = jobs[job.id];
  if (old && old.updated > job.updated) return jobs;
  return { ...jobs, [job.id]: job };
}

export function removeJob(jobs: Jobs, id: string): Jobs {
  if (!(id in jobs)) return jobs;
  const next = { ...jobs };
  delete next[id];
  return next;
}

const [jobs, setJobs] = createSignal<Jobs>({});
const [notice, setNotice] = createSignal<string | null>(null);
export { jobs as transferJobs, notice as transferNotice };

/** Newest first. */
export const jobList = (all: Jobs = jobs()): XferJob[] => Object.values(all).sort((a, b) => b.created - a.created);

let started = false;
/** Loads the queue and follows its events (once per window). */
export function initTransfers() {
  if (started) return;
  started = true;
  void onEvent<{ event: string; params: any }>("daemon-event", (msg) => {
    if (msg.event === "xfer.updated" && msg.params?.job) setJobs((j) => applyJob(j, msg.params.job as XferJob));
    else if (msg.event === "xfer.removed" && msg.params?.id) setJobs((j) => removeJob(j, msg.params.id as string));
  });
  void reloadTransfers();
}

export async function reloadTransfers() {
  try {
    const r = await xferList();
    setJobs(Object.fromEntries(r.jobs.map((j) => [j.id, j])));
    setNotice(r.notice);
  } catch {
    // An older daemon has no queue: the Files tab says so.
  }
}

const RUNNING = new Set(["queued", "scanning", "running", "waiting", "reconnecting"]);
export const isActive = (j: XferJob) => RUNNING.has(j.state);
export const isFinished = (j: XferJob) => j.state === "done" || j.state === "failed" || j.state === "cancelled";

// ---- words ----

export function speedText(bps: number): string {
  return bps > 0 ? `${formatSize(Math.round(bps))}/s` : "";
}

/** `45 s`, `12 min`, `3 h 05 min`, `> 1 day`; nothing when unknown. */
export function etaText(s: number | null | undefined): string {
  if (s == null || !Number.isFinite(s)) return "";
  if (s < 60) return t("{n} s", { n: Math.max(1, Math.round(s)) });
  if (s < 3600) return t("{n} min", { n: Math.round(s / 60) });
  if (s < 86400) return t("{h} h {m} min", { h: Math.floor(s / 3600), m: String(Math.round((s % 3600) / 60)).padStart(2, "0") });
  return t("> 1 day");
}

/** 0..1, or null while the size is not known yet. */
export function progress(j: XferJob): number | null {
  if (j.kind !== "copy") return j.totals.files ? j.totals.done_files / j.totals.files : null;
  if (!j.totals.bytes) return j.state === "done" ? 1 : null;
  return Math.min(1, j.totals.done_bytes / j.totals.bytes);
}

export function stateText(j: XferJob): string {
  switch (j.state) {
    case "queued":
      return t("Waiting to start");
    case "scanning":
      return t("Looking at the files…");
    case "running":
      return j.current ? t("Copying {name}", { name: j.current.split(/[\\/]/).pop() ?? "" }) : t("Working…");
    case "waiting":
      return t("Waiting for your answer");
    case "paused":
      return t("Paused");
    case "reconnecting":
      return t("Connection lost: trying again in {n} s (attempt {attempt})", { n: j.next_s, attempt: j.attempt });
    case "interrupted":
      return t("Interrupted when Kelpie's background service restarted");
    case "failed":
      return j.error ? t("Failed: {reason}", { reason: j.error }) : t("Failed");
    case "cancelled":
      return t("Cancelled");
    case "done":
      return j.totals.failed ? t("Done, {n} could not be copied", { n: j.totals.failed }) : j.totals.skipped ? t("Done, {n} skipped", { n: j.totals.skipped }) : t("Done");
  }
}

/** `report.pdf`, or `12 items`. */
export function jobTitle(j: XferJob): string {
  if (j.items.length === 1) return j.items[0].replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? j.items[0];
  return t("{n} items", { n: j.items.length });
}

const endName = (dest: string | undefined, label: (alias: string) => string) => (dest ? label(dest) : t("this computer"));

/** `this computer → prod`, `delete on prod`, `permissions on prod`. */
export function directionText(j: XferJob, label: (alias: string) => string = (a) => a): string {
  if (j.kind === "delete") return t("delete on {host}", { host: endName(j.src.dest, label) });
  if (j.kind === "chmod") return t("permissions on {host}", { host: endName(j.src.dest, label) });
  return `${endName(j.src.dest, label)} → ${endName(j.dst.dest, label)}`;
}

/** The question shown next: the oldest waiting job's first one (one at a time, never stacked). */
export function nextQuestion(all: Jobs = jobs()): { job: XferJob; q: XferQuestion } | null {
  const waiting = Object.values(all)
    .filter((j) => j.questions.length)
    .sort((a, b) => a.created - b.created);
  const job = waiting[0];
  return job ? { job, q: job.questions[0] } : null;
}

/** The status bar's chip while jobs run: `↑↓ 2 · 45 %`; null when nothing runs. */
export function chipText(all: Jobs = jobs()): string | null {
  const active = Object.values(all).filter(isActive);
  if (!active.length) return null;
  const sized = active.filter((j) => j.kind === "copy" && j.totals.bytes > 0);
  const total = sized.reduce((n, j) => n + j.totals.bytes, 0);
  const done = sized.reduce((n, j) => n + Math.min(j.totals.done_bytes, j.totals.bytes), 0);
  return total ? `↑↓ ${active.length} · ${Math.floor((done / total) * 100)} %` : `↑↓ ${active.length}`;
}

// ---- updates wait for transfers ----

/** Transfers running or waiting to run (paused and interrupted ones do not hold an update). */
export const activeCount = (all: Jobs = jobs()): number => Object.values(all).filter(isActive).length;

/** Whether an update may install now: one that stops the session daemon waits until no transfer runs. */
export function updateMayInstall(o: { daemonStops: boolean; transfers: number }): boolean {
  return !o.daemonStops || o.transfers === 0;
}

/** What holds an update or a daemon switch, in words; null when nothing does. */
export function heldText(transfers: number): string | null {
  return transfers > 0 ? (transfers === 1 ? t("Waiting for 1 file transfer to finish") : t("Waiting for {n} file transfers to finish", { n: transfers })) : null;
}