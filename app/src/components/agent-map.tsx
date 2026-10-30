// The agent map: every subagent of a conversation as a tree, with live status, time and tokens, and the
// details of one on click. One dialog per chat, opened from its composer's chip or the palette. It only
// reads what the conversation already received: nothing here reaches the agent.
import { For, type JSX, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { type AgentRun, type RunNode, formatDuration, formatTokens, runDuration, runResult, runTree } from "../chat/agent-runs";
import type { ChatController, ChatItem } from "../chat/chat-model";
import { stripProjectRoot } from "../chat/tool-view";
import { suspendKeymap } from "../lib/commands";
import { t } from "../lib/i18n";
import { setCovering } from "../lib/overlays";
import type { Project } from "../lib/types";
import { Clamped } from "./chat-rows";
import { Icon } from "./icons";

const nameOf = (run: AgentRun) => run.description || run.type || t("Agent");

const statusLabel = (run: AgentRun) => {
  if (run.status === "running") return t("Running");
  if (run.statusText) return run.statusText;
  return run.status === "error" ? t("Failed") : run.status === "stopped" ? t("Stopped") : t("Done");
};

/** The dot's class: done is the plain green one. */
const dotClass = (run: AgentRun) => `tdot${run.status === "done" ? "" : ` ${run.status}`}`;

/** "3m 49s · 90.9k tokens", leaving out what is not known. */
function numbers(run: AgentRun, now: number): string {
  const ms = runDuration(run, now);
  return [ms == null ? "" : formatDuration(ms), run.tokens == null ? "" : t("{n} tokens", { n: formatTokens(run.tokens) })].filter(Boolean).join(" · ");
}

/** One level of the tree. Its runs are what `For` follows: they keep their cards, and any focus on one,
 *  when a status changes and the tree is worked out again. */
function AgentNodes(props: { nodes: RunNode[]; now: number; onOpen: (run: AgentRun) => void }) {
  return (
    <div class="agent-map-children">
      <For each={props.nodes.map((n) => n.run)}>
        {(run) => {
          const children = () => props.nodes.find((n) => n.run === run)?.children ?? [];
          return (
            <div class="agent-map-node">
              <button class="agent-map-card" data-run={run.id} data-status={run.status} onClick={() => props.onOpen(run)}>
                <span class="agent-map-card-main">
                  <span class={dotClass(run)} />
                  <span class="agent-map-card-name" title={nameOf(run)}>{nameOf(run)}</span>
                </span>
                <Show when={numbers(run, props.now)}>{(line) => <span class="agent-map-card-numbers">{line()}</span>}</Show>
              </button>
              <Show when={children().length}><AgentNodes nodes={children()} now={props.now} onOpen={props.onOpen} /></Show>
            </div>
          );
        }}
      </For>
    </div>
  );
}

function AgentDetails(props: { run: AgentRun; ctrl: ChatController; project: Project; now: number; markdown: (text: string) => JSX.Element; onShow: (id: string) => void }) {
  /** Its row in the transcript: a nested agent has none. */
  const row = createMemo(() => props.ctrl.state.items.find((i): i is Extract<ChatItem, { kind: "tool" }> => i.kind === "tool" && i.id === props.run.id));
  const result = () => runResult(props.run, row()?.output);
  const time = () => runDuration(props.run, props.now);
  return (
    <>
      <div class="agent-map-details">
        <dl class="agent-map-facts">
          <Show when={props.run.type}><dt>{t("Type")}</dt><dd>{props.run.type}</dd></Show>
          <dt>{t("Status")}</dt><dd class="agent-map-status"><span class={dotClass(props.run)} />{statusLabel(props.run)}</dd>
          <Show when={time() != null}><dt>{t("Time")}</dt><dd class="agent-map-time">{formatDuration(time()!)}</dd></Show>
          <Show when={props.run.tokens != null}><dt>{t("Tokens")}</dt><dd class="agent-map-tokens">{formatTokens(props.run.tokens!)}</dd></Show>
          <Show when={props.run.toolUses != null}><dt>{t("Tool uses")}</dt><dd class="agent-map-tool-uses">{props.run.toolUses}</dd></Show>
        </dl>
        <Show when={props.run.prompt}>
          <section>
            <div class="agent-map-section-label">{t("Prompt")}</div>
            <Clamped class="agent-map-prompt" text={props.run.prompt!} lines={8} />
          </section>
        </Show>
        <Show when={props.run.status === "running" && props.run.summary}>
          <section>
            <div class="agent-map-section-label">{t("Latest activity")}</div>
            <div class="agent-map-summary">{props.run.summary}</div>
          </section>
        </Show>
        <Show when={props.run.activity.length}>
          <section>
            <div class="agent-map-section-label">{t("Activity")}</div>
            <div class="agent-map-activity">
              <For each={[...props.run.activity].reverse()}>
                {(call) => (
                  <div class="agent-map-activity-row">
                    <span class={`tdot${call.status === "done" ? "" : ` ${call.status}`}`} />
                    <span class="agent-map-activity-name">{call.name}</span>
                    <span class="agent-map-activity-target" title={call.target}>{stripProjectRoot(call.target, props.project.path)}</span>
                  </div>
                )}
              </For>
            </div>
          </section>
        </Show>
        <Show when={result()} keyed>
          {(text) => (
            <section class="agent-map-result">
              <div class="agent-map-section-label">{t("Result")}</div>
              {props.markdown(text)}
            </section>
          )}
        </Show>
      </div>
      <Show when={row()}>
        <div class="footer">
          <button class="btn agent-map-show-in-chat" onClick={() => props.onShow(props.run.id)}>{t("Show in chat")}</button>
        </div>
      </Show>
    </>
  );
}

export function AgentMapDialog(props: {
  runs: AgentRun[];
  ctrl: ChatController;
  sessionName: string;
  project: Project;
  /** The conversation's sanitized markdown, for an agent's result. */
  markdown: (text: string) => JSX.Element;
  /** The message field, for the keyboard to go back to when what had it is gone. */
  composerField: () => HTMLElement | null | undefined;
  onClose: () => void;
  onShow: (id: string) => void;
}) {
  let modal!: HTMLDivElement;
  const [openId, setOpenId] = createSignal<string | null>(null);
  const opened = createMemo(() => props.runs.find((r) => r.id === openId()));
  const tree = createMemo(() => runTree(props.runs));

  // Exists only while the dialog is open, and ticks only while something runs.
  const [now, setNow] = createSignal(Date.now());
  const anyRunning = createMemo(() => props.runs.some((r) => r.status === "running"));
  createEffect(() => {
    if (!anyRunning()) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(timer));
  });

  const open = (run: AgentRun) => {
    setOpenId(run.id);
    modal.focus();
  };
  const back = () => {
    const id = openId();
    setOpenId(null);
    (id ? modal.querySelector<HTMLElement>(`[data-run="${CSS.escape(id)}"]`) : null)?.focus();
  };

  // Capture phase, and nothing goes on: the keyboard can be in the chat's message field, where Esc
  // interrupts the agent. A picture enlarged from a result has its own listener, which comes next.
  const onKey = (e: KeyboardEvent) => {
    if (document.querySelector(".image-lightbox")) return;
    e.stopPropagation();
    // The composer takes the keyboard when the palette closes: what is typed must not land there.
    if (!modal.contains(document.activeElement)) modal.focus();
    if (e.key === "Escape") {
      e.preventDefault();
      if (opened()) back();
      else props.onClose();
    } else if (e.key === "Tab") {
      const stops = [...modal.querySelectorAll<HTMLElement>("button, a[href]")];
      if (!stops.length) return e.preventDefault();
      const at = document.activeElement;
      const edge = e.shiftKey ? [stops[0], modal] : [stops[stops.length - 1]];
      if (!edge.includes(at as HTMLElement)) return;
      e.preventDefault();
      (e.shiftKey ? stops[stops.length - 1] : stops[0]).focus();
    }
  };
  const giveBack = document.activeElement as HTMLElement | null;
  onMount(() => {
    setCovering("agent-map", true);
    window.addEventListener("keydown", onKey, true);
    // The app's shortcuts listen in the capture phase on the window too, and came first.
    suspendKeymap(true, "agent-map");
    // A frame later: the palette, on its way out, gives the keyboard to the composer first.
    const frame = requestAnimationFrame(() => modal.focus());
    onCleanup(() => {
      cancelAnimationFrame(frame);
      setCovering("agent-map", false);
      window.removeEventListener("keydown", onKey, true);
      suspendKeymap(false, "agent-map");
      (giveBack?.isConnected && giveBack !== document.body ? giveBack : props.composerField())?.focus();
    });
  });

  const count = () => props.runs.length;
  const used = () => props.ctrl.state.usage?.used;

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="modal agent-map" role="dialog" aria-modal="true" aria-label={t("Agent map")} tabIndex={-1} ref={modal}>
        <div class="agent-map-head">
          <Show when={opened()}>
            <button class="icon-btn agent-map-back" title={t("Back to the list")} aria-label={t("Back to the list")} onClick={back}><Icon name="back" size={14} /></button>
          </Show>
          <h2 title={opened() ? nameOf(opened()!) : undefined}>{opened() ? nameOf(opened()!) : t("Agent map")}</h2>
          <Show when={!opened() && count()}>
            <span class="muted-inline">{count() === 1 ? t("1 agent · click it for details") : t("{n} agents · click an agent for details", { n: count() })}</span>
          </Show>
          <span class="spacer" />
          <button class="icon-btn agent-map-close" title={t("Close")} aria-label={t("Close")} onClick={() => props.onClose()}><Icon name="close" size={14} /></button>
        </div>
        <Show
          when={opened()}
          fallback={
            <Show when={count()} fallback={<div class="muted">{t("No agents in this conversation yet.")}</div>}>
              <div class="agent-map-tree">
                <div class="agent-map-root">
                  <div class="agent-map-root-name" title={props.sessionName}>{props.sessionName}</div>
                  <Show when={props.ctrl.state.model}><div class="agent-map-root-line" title={props.ctrl.state.model!}>{props.ctrl.state.model}</div></Show>
                  <Show when={used()}><div class="agent-map-root-line">{t("{n} tokens in context", { n: formatTokens(used()!) })}</div></Show>
                </div>
                <div class="agent-map-list"><AgentNodes nodes={tree()} now={now()} onOpen={open} /></div>
              </div>
            </Show>
          }
        >
          {(run) => <AgentDetails run={run()} ctrl={props.ctrl} project={props.project} now={now()} markdown={props.markdown} onShow={props.onShow} />}
        </Show>
      </div>
    </div>
  );
}
