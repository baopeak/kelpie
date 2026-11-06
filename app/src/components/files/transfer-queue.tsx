// The transfer queue at the bottom of the Files tab: each job with its direction, progress, speed, ETA and state, and
// pause / resume / cancel / retry / show / clear. The daemon runs the jobs; closing the window does not stop them.
import { For, Show } from "solid-js";
import { t } from "../../lib/i18n";
import * as ipc from "../../lib/ipc";
import { formatSize } from "../../lib/file-pane";
import { directionText, etaText, isActive, isFinished, jobList, jobTitle, progress, routeText, speedText, stateText, transferNotice } from "../../lib/transfers";
import type { XferJob } from "../../lib/types";

export function TransferQueue(props: { label: (alias: string) => string }) {
  const act = (j: XferJob, action: "pause" | "resume" | "cancel" | "retry" | "clear") => void ipc.xferControl(j.id, action).catch(() => {});
  // A finished download to this computer: show it in Explorer / Finder.
  const reveal = (j: XferJob) => {
    const first = j.items[0]?.replace(/[\\/]+$/, "").split(/[\\/]/).pop();
    if (first) void ipc.revealEntry(j.dst_dir, first).catch(() => {});
  };
  return (
    <div class="transfer-queue" data-transfer-queue>
      <div class="transfer-queue-head">
        <span class="section-title">{t("Transfers")}</span>
        <span class="spacer" />
        <Show when={jobList().some(isFinished)}>
          <button class="btn" data-action="clear-finished" onClick={() => void ipc.xferClearFinished()}>{t("Clear finished")}</button>
        </Show>
      </div>
      <Show when={transferNotice()}><div class="form-note error">{transferNotice()}</div></Show>
      <For each={jobList()} fallback={<div class="muted">{t("No transfers. Drag files between the panes, or press F5 to copy the selection to the other pane.")}</div>}>
        {(j) => {
          const p = () => progress(j);
          return (
            <div class={`transfer-job ${j.state}`} data-job={j.id} data-state={j.state}>
              <div class="transfer-job-line">
                <span class="transfer-job-title" title={j.items.join("\n")}>{jobTitle(j)}</span>
                <span class="muted-inline">{directionText(j, props.label)}</span>
                <Show when={routeText(j, props.label)}>{(r) => <span class="muted-inline" data-route={j.route_used ?? ""}>{r()}</span>}</Show>
                <span class="spacer" />
                <Show when={j.kind === "copy" && j.totals.bytes}>
                  <span class="muted-inline">{formatSize(j.totals.done_bytes)} / {formatSize(j.totals.bytes)}</span>
                </Show>
                <Show when={isActive(j) && j.speed_bps}><span class="muted-inline">{speedText(j.speed_bps)}</span></Show>
                <Show when={isActive(j) && j.eta_s != null}><span class="muted-inline">{etaText(j.eta_s)}</span></Show>
              </div>
              <div class="transfer-bar"><div class="transfer-bar-fill" style={{ width: `${Math.round((p() ?? 0) * 100)}%` }} classList={{ unknown: p() == null && isActive(j) }} /></div>
              <div class="transfer-job-line">
                <span class={`transfer-state ${j.state}`} data-job-state>{stateText(j)}</span>
                <Show when={j.walk.folder_links}><span class="muted-inline">{t("{n} links to folders not followed", { n: j.walk.folder_links })}</span></Show>
                <Show when={j.walk.unreadable_names}><span class="muted-inline">{t("{n} names not readable, skipped", { n: j.walk.unreadable_names })}</span></Show>
                <Show when={j.renamed.length}><span class="muted-inline" title={j.renamed.map(([a, b]) => `${a} → ${b}`).join("\n")}>{t("{n} saved under another name", { n: j.renamed.length })}</span></Show>
                <span class="spacer" />
                <Show when={isActive(j) && j.state !== "queued"}><button class="btn" data-action="pause" onClick={() => act(j, "pause")}>{t("Pause")}</button></Show>
                <Show when={j.state === "paused" || j.state === "interrupted"}><button class="btn primary" data-action="resume" onClick={() => act(j, "resume")}>{t("Resume")}</button></Show>
                <Show when={j.state === "failed"}><button class="btn primary" data-action="retry" onClick={() => act(j, "retry")}>{t("Retry")}</button></Show>
                <Show when={!isFinished(j)}><button class="btn" data-action="cancel" onClick={() => act(j, "cancel")}>{t("Cancel")}</button></Show>
                <Show when={j.state === "done" && j.kind === "copy" && !j.dst.dest}><button class="btn" data-action="reveal" onClick={() => reveal(j)}>{t("Show")}</button></Show>
                <Show when={isFinished(j)}><button class="btn" data-action="clear" onClick={() => act(j, "clear")}>{t("Clear")}</button></Show>
              </div>
              <Show when={j.failures.length}>
                <details class="transfer-failures">
                  <summary>{t("{n} could not be copied", { n: j.totals.failed })}</summary>
                  <For each={j.failures}>{(f) => <div class="mono">{f.path}: {f.error}</div>}</For>
                </details>
              </Show>
            </div>
          );
        }}
      </For>
    </div>
  );
}
