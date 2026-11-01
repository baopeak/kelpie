// The daemon's label first, then the full command: monospaced, nothing cut, what could hide something written out. A long one must be read to its end before it can be approved.
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { commandParts, levelClass, levelText, tagText, type Risk } from "./command-text";
import { t } from "./i18n";

export function RiskLabel(props: { risk: Risk | null | undefined }) {
  return (
    <div class={`risk ${levelClass(props.risk)}`} data-risk={levelClass(props.risk)}>
      <b>{levelText(props.risk)}</b>
      <For each={props.risk?.tags ?? []}>{(tag) => <span class="risk-tag" data-risk-tag={tag}>{tagText(tag)}</span>}</For>
    </div>
  );
}

export function CommandBlock(props: { command: string | null | undefined; risk: Risk | null | undefined; onSeen?: (seen: boolean) => void }) {
  let box: HTMLPreElement | undefined;
  const [unread, setUnread] = createSignal(false);
  const look = () => {
    if (!box) return;
    const more = box.scrollTop + box.clientHeight < box.scrollHeight - 4;
    setUnread(more);
    props.onSeen?.(!more);
  };
  createEffect(on(() => props.command, () => queueMicrotask(look)));
  return (
    <div class="command-block" data-command-block>
      <RiskLabel risk={props.risk} />
      <Show when={props.command}>
        <pre class="code command" ref={box} data-command onScroll={look}>
          <For each={commandParts(props.command!)}>{(p) => (p.escaped ? <span class="escaped">{p.text}</span> : <>{p.text}</>)}</For>
        </pre>
        <Show when={unread()}><div class="muted" role="status" data-command-unread>{t("Scroll to the end of the command to approve it.")}</div></Show>
      </Show>
    </div>
  );
}
