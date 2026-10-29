// `:5173` chips for the ports a session's programs listen on (see lib/processes.ts for when they are looked up).
// Click opens the port in a browser tab; the menu opens it beside the focused pane or copies the address.
import { For, Show, createMemo } from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { chips, openPort, ownerLabel, portUrl, type Port } from "../lib/processes";
import { openContextMenu } from "./context-menu";
import { t } from "../lib/i18n";

export function PortChips(props: { id: string; max: number }) {
  // A server that listens on both IPv4 and IPv6 is one port.
  const ports = createMemo<Port[]>(() => {
    const seen = new Set<number>();
    return (chips[props.id]?.ports ?? []).filter((p) => !seen.has(p.port) && !!seen.add(p.port));
  });
  const title = (p: Port) => `${portUrl(p)}${ownerLabel(p) ? ` · ${ownerLabel(p)}` : ""}\n${t("Click to open · right-click for more")}`;
  return (
    <Show when={ports().length}>
      <span class="port-chips">
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
