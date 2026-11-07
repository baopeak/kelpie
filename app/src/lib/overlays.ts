// Native child webviews (browser panes) always paint above the page, so anything floating that
// could overlap one — menus, popovers, the floating terminal — registers here while open and
// browser panes hide their webview meanwhile.
import { createRoot, createSignal } from "solid-js";

const [open, setOpen] = createRoot(() => createSignal<ReadonlySet<string>>(new Set()));

export function setCovering(key: string, on: boolean) {
  setOpen((cur) => {
    if (on === cur.has(key)) return cur;
    const next = new Set(cur);
    if (on) next.add(key);
    else next.delete(key);
    return next;
  });
}

export const anyCovering = () => open().size > 0;
export const isCovering = (key: string) => open().has(key);
