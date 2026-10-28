// Floating terminal: a global shell (home folder, not tied to a project) one chord away.
import { For, Show } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { chordOf, commands } from "../lib/commands";
import { chordLabel } from "../lib/platform";
import { TerminalPane } from "./terminal-pane";
import { Icon } from "./icons";
import { StatusDot } from "./sidebar";
import { t } from "../lib/i18n";

export function FloatingTerminal() {
  const f = () => store.state.floating;
  const chord = () => {
    const c = commands.find((x) => x.id === "floating.toggle");
    return c && chordOf(c) ? ` (${chordLabel(chordOf(c)!)})` : "";
  };
  return (
    <div class="floating-term" onKeyDown={(e) => e.key === "Escape" && e.shiftKey && store.toggleFloating(false)}>
      <div class="floating-bar">
        <Icon name="float" size={14} />
        <div class="floating-tabs">
          <For each={f().sessions.filter((id) => store.sessions[id])}>
            {(id) => (
              <div class={`floating-tab${f().active === id ? " active" : ""}`} onClick={() => store.selectFloating(id)}>
                <StatusDot s={store.sessions[id]} />
                <span>{sessionLabel(store.sessions[id])}</span>
                <button class="icon-btn mini" title={t("Close")} onClick={(e) => { e.stopPropagation(); void store.closeFloating(id); }}><Icon name="close" size={10} /></button>
              </div>
            )}
          </For>
          <button class="icon-btn mini" title={t("New floating terminal")} onClick={() => void store.newFloating()}><Icon name="plus" size={13} /></button>
        </div>
        <span style={{ flex: 1 }} />
        <button class="icon-btn mini" title={t("Hide") + chord()} onClick={() => store.toggleFloating(false)}><Icon name="down" size={14} /></button>
      </div>
      <div class="floating-body">
        <Show when={f().active && store.sessions[f().active!] ? f().active : null} keyed fallback={<div class="muted">{t("Starting a terminal…")}</div>}>
          {(id) => <TerminalPane sessionId={id} focused={true} />}
        </Show>
      </div>
    </div>
  );
}
