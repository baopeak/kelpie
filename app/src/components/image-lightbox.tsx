// A picture at full size, over everything: one instance rendered by <App>, opened from anywhere. Also the
// row of thumbnails a composer shows for what it holds, which is the same row in both composers.
import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { suspendKeymap } from "../lib/commands";
import { setCovering } from "../lib/overlays";
import { t } from "../lib/i18n";
import type { Attached } from "../lib/composer-pictures";
import { Icon } from "./icons";

export interface LightboxImage { url: string; name: string }

const [box, setBox] = createSignal<{ images: LightboxImage[]; index: number } | null>(null);
/** Where the keyboard goes back to: what had it, or else the message field of the chat clicked in. */
let returnFocus: { element: HTMLElement | null; chat: Element | null } = { element: null, chat: null };

/** `from`: what was clicked, for the chat it is in. */
export function openLightbox(images: LightboxImage[], index: number, from?: Element) {
  if (!images[index]) return;
  returnFocus = { element: document.activeElement as HTMLElement | null, chat: from?.closest(".chat") ?? null };
  setBox({ images, index });
}

function giveFocusBack() {
  const { element, chat } = returnFocus;
  returnFocus = { element: null, chat: null };
  const usable = element?.isConnected && element !== document.body;
  (usable ? element : chat?.querySelector<HTMLTextAreaElement>(".chat-composer textarea"))?.focus();
}

export function ImageLightbox() {
  let closeButton: HTMLButtonElement | undefined;
  const [actual, setActual] = createSignal(false);
  const isOpen = createMemo(() => !!box());

  const close = () => {
    setBox(null);
    setActual(false);
    giveFocusBack();
  };
  const step = (by: 1 | -1) => {
    const b = box()!;
    setActual(false);
    setBox({ ...b, index: (b.index + by + b.images.length) % b.images.length });
  };
  // Capture phase, and nothing goes on: Esc in the chat's message field interrupts the agent, and
  // that handler is delegated to the document, so a listener further down would come too late.
  const onKey = (e: KeyboardEvent) => {
    e.stopPropagation();
    const plain = !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey;
    if (e.key === "Escape") close();
    else if (e.key === "ArrowLeft" && plain) step(-1);
    else if (e.key === "ArrowRight" && plain) step(1);
    else if (e.key === "Tab") closeButton?.focus();
    else return;
    e.preventDefault();
  };
  createEffect(() => {
    setCovering("image-lightbox", isOpen());
    if (!isOpen()) return;
    window.addEventListener("keydown", onKey, true);
    // The app's shortcuts listen in the capture phase on the window too, and came first: a chord
    // pressed here would close a pane behind the picture.
    suspendKeymap(true);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey, true);
      suspendKeymap(false);
    });
  });
  /** The stage's own scrollbars are part of it: dragging one (a picture at actual size) is no click on the backdrop. */
  const onScrollbar = (stage: HTMLElement, e: MouseEvent) => {
    const r = stage.getBoundingClientRect();
    return e.clientX - r.left >= stage.clientWidth || e.clientY - r.top >= stage.clientHeight;
  };
  const onClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (target.tagName === "IMG") setActual((a) => !a);
    else if (target.classList.contains("image-lightbox-stage") && onScrollbar(target, e)) return;
    else if (!target.closest(".image-lightbox-caption")) close();
  };

  return (
    <Show when={box()}>
      {(b) => {
        const current = () => b().images[b().index];
        return (
          <Portal>
            <div class="overlay image-lightbox" role="dialog" aria-modal="true" aria-label={current().name} onClick={onClick}>
              <div class="image-lightbox-stage">
                <img class="image-lightbox-img" classList={{ actual: actual() }} src={current().url} alt={current().name} title={actual() ? t("Fit to the window") : t("Actual size")} />
              </div>
              <div class="image-lightbox-caption">
                <span class="name">{current().name}</span> · {b().index + 1} / {b().images.length}
              </div>
              <button class="icon-btn image-lightbox-close" title={t("Close")} aria-label={t("Close")} ref={(el) => { closeButton = el; queueMicrotask(() => el.focus()); }}>
                <Icon name="close" size={16} />
              </button>
            </div>
          </Portal>
        );
      }}
    </Show>
  );
}

/** What a composer holds, above its text. A click on a picture enlarges it, the caret staying in the
 *  message field for the lightbox to give the keyboard back to; a dropped one a terminal session was
 *  not given the bytes of shows an icon instead, and has nothing to enlarge. */
export function AttachedPictures(props: { images: Attached[]; onRemove: (id: number) => void }) {
  // A dropped file named like a picture may not be one: it shows the icon, never a broken image.
  const [broken, setBroken] = createSignal<ReadonlySet<number>>(new Set());
  const shows = (img: Attached) => !!img.url && !broken().has(img.id);
  const enlarge = (img: Attached, from: Element) => {
    const shown = props.images.filter(shows);
    openLightbox(shown.map((i) => ({ url: i.url!, name: i.name })), shown.indexOf(img), from);
  };
  return (
    <Show when={props.images.length}>
      <div class="composer-attached">
        <For each={props.images}>
          {(img) => (
            <span class="composer-image" title={img.path}>
              <Show when={shows(img)} fallback={<Icon name="files" size={14} />}>
                <button class="composer-image-open" title={t("Enlarge")} aria-label={t("Enlarge")} onMouseDown={(e) => e.preventDefault()} onClick={(e) => enlarge(img, e.currentTarget)}><img src={img.url} alt="" onError={() => setBroken((s) => new Set(s).add(img.id))} /></button>
              </Show>
              <span class="name">{img.name}</span>
              <button class="icon-btn mini" title={t("Remove")} onClick={() => props.onRemove(img.id)}><Icon name="close" size={11} /></button>
            </span>
          )}
        </For>
      </div>
    </Show>
  );
}
