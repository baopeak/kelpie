// `:5173` chips for the ports a session's programs listen on (see lib/processes.ts for when they are looked up).
// Click opens the port in a browser tab; the menu opens it beside the focused pane or copies the address.
import { For, Show, createMemo } from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { chips, openPort, ownerLabel, portUrl, uniquePorts, type Port } from "../lib/processes";
import { openContextMenu } from "./context-menu";
import { t } from "../lib/i18n";

export function PortChips(props: { id: string; max: number }) {
  const ports = createMemo<Port[]>(() => uniquePorts(chips[props.id]?.ports ?? []));
  // Off screen, the session is not looked at: its ports are as of when it was last on screen.
  const stale = () => !!chips[props.id]?.stale;
  const title = (p: Port) =>
    `${portUrl(p)}${ownerLabel(p) ? ` · ${ownerLabel(p)}` : ""}\n${stale() ? `${t("As of when this session was last on screen")}\n` : ""}${t("Click to open · right-click for more")}`;
  return (
    <Show when={ports().length}>
      <span class={`port-chips${stale() ? " stale" : ""}`}>
        <For each={ports().slice(0, props.max)}>
          {(p) => (
            <button
              class="port-chip"
              data-port={p.port}
              title={title(p)}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => { e.stopPropagation(); openPort(p, props.id, "tab"); }}
              onContextMenu={(e) =>
                openContextMenu(e, [
                  { label: t("Open preview beside"), run: () => openPort(p, props.id, "right") },
                  { label: t("Open in a browser tab"), run: () => openPort(p, props.id, "tab") },
                  { label: t("Copy address"), run: () => void writeText(portUrl(p)) },
                ])
              }
            >
              :{p.port}
            </button>
          )}
        </For>
        <Show when={ports().length > props.max}>
          <span class="port-more" title={ports().slice(props.max).map((p) => `:${p.port}`).join("  ")}>+{ports().length - props.max}</span>
        </Show>
      </span>
    </Show>
  );
}
