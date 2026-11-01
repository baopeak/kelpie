// A browser pane: toolbar (back / forward / reload, address, design mode, screenshot, devtools)
// above a native child webview kept over the pane's body, and the elements picked in design
// mode with a note each, sent to an agent as one prompt.
import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "../lib/store";
import { uid } from "../lib/platform";
import { anyCovering } from "../lib/overlays";
import { chooseAgentAndSend } from "../lib/send-to-agent";
import {
  annotationPrompt, browserBounds, browserDesign, browserDevtools, browserGo, browserLabel, browserNavigate, browserOpen, browserScreenshot, browserVisible, browserState as state, markPageSeen, onBrowserEvent, setBrowserState as setState,
  type Annotation, type Rect,
} from "../lib/browser";
import { Icon } from "./icons";
import type { Pane, Project, Tab } from "../lib/types";
import { t, errorText } from "../lib/i18n";

export function BrowserPane(props: { pane: Pane; project: Project; tab: Tab; focused: boolean }) {
  const label = browserLabel(props.pane.id);
  markPageSeen(props.pane.id);
  if (!state[props.pane.id]) setState(props.pane.id, { title: "", url: props.pane.url ?? "", design: false, notes: [] });
  const s = () => state[props.pane.id];
  const [address, setAddress] = createSignal(props.pane.url ?? "");
  const [opened, setOpened] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let host!: HTMLDivElement;
  let addressInput!: HTMLInputElement;

  const rect = (): Rect => {
    const r = host.getBoundingClientRect();
    return { x: r.left, y: r.top, width: r.width, height: r.height };
  };
  const remember = (url: string) => {
    setState(props.pane.id, "url", url);
    // The pane keeps its page across restarts (layout state).
    if (props.pane.url !== url) store.updatePane(props.project.id, props.tab.id, props.pane.id, { url });
  };

  const unsubscribe = onBrowserEvent(label, (e) => {
    if (e.kind === "nav" && e.url) setAddress(e.url);
    if (e.kind === "load" && e.url) {
      setAddress(e.url);
      remember(e.url);
      // Design mode survives navigation.
      if (s().design) void browserDesign(label, true);
    }
    if (e.kind === "title") setState(props.pane.id, "title", e.title ?? "");
    if (e.kind === "popup" && e.url) void go(e.url);
    if (e.kind === "cancel") setState(props.pane.id, "design", false);
    if (e.kind === "pick" && e.data) void addPick(e.data);
  });

  const go = async (url: string) => {
    if (!url.trim()) return;
    setError(null);
    try {
      if (!opened()) {
        await browserOpen(label, url, rect());
        setOpened(true);
        setAddress(url);
      } else setAddress(await browserNavigate(label, url));
    } catch (e) {
      setError(String(e));
    }
  };

  const addPick = async (pick: NonNullable<import("../lib/browser").BrowserEvent["data"]>) => {
    const pad = 8;
    const clip = {
      x: pick.rect.x + pick.page.scrollX - pad,
      y: pick.rect.y + pick.page.scrollY - pad,
      width: Math.min(pick.rect.width + pad * 2, 4000),
      height: Math.min(pick.rect.height + pad * 2, 4000),
    };
    const shot = await browserScreenshot(label, clip).catch(() => null);
    const note: Annotation = { id: uid(), pick, pageUrl: pick.page.url, pageTitle: pick.page.title, shot, comment: "" };
    setState(props.pane.id, "notes", (n) => [...n, note]);
  };

  const screenshot = async () => {
    try {
      const shot = await browserScreenshot(label);
      setState(props.pane.id, "notes", (n) => [...n, { id: uid(), pick: null, pageUrl: s().url, pageTitle: s().title, shot, comment: "" }]);
    } catch (e) {
      store.notify(String(e));
    }
  };

  const toggleDesign = () => {
    const on = !s().design;
    setState(props.pane.id, "design", on);
    void browserDesign(label, on).catch((e) => store.notify(String(e)));
  };

  const removeNote = (id: string) => setState(props.pane.id, "notes", (n) => n.filter((x) => x.id !== id));
  const send = (e: MouseEvent, items: Annotation[]) =>
    chooseAgentAndSend(e, props.project.id, annotationPrompt(items), () => setState(props.pane.id, "notes", (n) => n.filter((x) => !items.includes(x))));

  // Keep the native webview exactly over the pane body (layout changes move it without a
  // resize event), and out of the way while something floats above.
  let last = "";
  let frame = 0;
  const track = () => {
    frame = requestAnimationFrame(track);
    if (!opened()) return;
    const r = rect();
    const key = `${Math.round(r.x)},${Math.round(r.y)},${Math.round(r.width)},${Math.round(r.height)}`;
    if (key !== last) {
      last = key;
      void browserBounds(label, r).catch(() => {});
    }
  };
  createEffect(() => {
    const hidden = anyCovering() || store.overlay() !== null;
    if (opened()) void browserVisible(label, !hidden).catch(() => {});
  });

  onMount(() => {
    frame = requestAnimationFrame(track);
    if (s().url || props.pane.url) void go(s().url || props.pane.url!);
    else queueMicrotask(() => addressInput.focus());
  });
  onCleanup(() => {
    cancelAnimationFrame(frame);
    unsubscribe();
    // Parked, not closed: the page keeps its state while another tab or project is shown.
    void browserVisible(label, false).catch(() => {});
  });

  return (
    <div class="browser-pane">
      <div class="browser-bar">
        <button class="icon-btn" title={t("Back")} disabled={!opened()} onClick={() => void browserGo(label, -1)}><Icon name="back" size={14} /></button>
        <button class="icon-btn" title={t("Forward")} disabled={!opened()} onClick={() => void browserGo(label, 1)}><Icon name="forward" size={14} /></button>
        <button class="icon-btn" title={t("Reload")} disabled={!opened()} onClick={() => void browserGo(label, 0)}><Icon name="refresh" size={14} /></button>
        <form class="browser-address" onSubmit={(e) => { e.preventDefault(); void go(address()); }}>
          <input ref={addressInput} value={address()} placeholder={t("Address, e.g. localhost:5173")} spellcheck={false} onInput={(e) => setAddress(e.currentTarget.value)} onFocus={(e) => e.currentTarget.select()} />
        </form>
        <button class={`icon-btn${s().design ? " active" : ""}`} title={s().design ? t("Design mode on: click elements to pick them (Esc in the page stops)") : t("Design mode: pick elements to send to an agent")} disabled={!opened()} onClick={toggleDesign}>
          <Icon name="target" size={14} />
        </button>
        <button class="icon-btn" title={t("Screenshot of the page")} disabled={!opened()} onClick={() => void screenshot()}><Icon name="camera" size={14} /></button>
        <button class="icon-btn" title={t("Developer tools")} disabled={!opened()} onClick={() => void browserDevtools(label)}><Icon name="code" size={14} /></button>
        <button class="icon-btn" title={t("Open in your browser")} disabled={!s().url} onClick={() => void openUrl(s().url)}><Icon name="external" size={13} /></button>
      </div>
      <div class="browser-host" ref={host}>
        <Show when={!opened()}>
          <div class="empty">
            <div class="welcome" style={{ "max-width": "420px" }}>
              <Icon name="globe" size={30} />
              <div style={{ color: "var(--text-dim)" }}>{t("Open a page — your dev server, docs, anything on http(s). Design mode lets you click an element and send it, with a screenshot, to an agent.")}</div>
              <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
            </div>
          </div>
        </Show>
        <Show when={opened() && (anyCovering() || store.overlay() !== null)}>
          <div class="empty muted">{t("Page hidden while a menu or dialog is open")}</div>
        </Show>
      </div>
      <Show when={s().notes.length}>
        <div class="browser-notes">
          <div class="browser-notes-head">
            <span>{t("{n} marked", { n: s().notes.length })}</span>
            <span style={{ flex: 1 }} />
            <button class="btn" onClick={() => void writeText(annotationPrompt(s().notes)).then(() => store.notify(t("Copied")))}>{t("Copy")}</button>
            <button class="btn primary" onClick={(e) => send(e, [...s().notes])}>{s().notes.length > 1 ? t("Send all to agent…") : t("Send to agent…")}</button>
          </div>
          <div class="browser-notes-list">
            <For each={s().notes}>
              {(n, i) => (
                <div class="browser-note" data-note={n.id}>
                  <Show when={n.shot}>{(shot) => <img class="browser-note-shot" src={`data:image/png;base64,${shot().data}`} alt="" />}</Show>
                  <div class="browser-note-main">
                    <div class="browser-note-title">{n.pick ? n.pick.element : t("Page screenshot")}</div>
                    <Show when={n.pick}><div class="mono browser-note-sel" title={n.pick!.selector}>{n.pick!.selector}</div></Show>
                    <textarea class="text-input" rows={2} placeholder={t("What should change here?")} value={n.comment}
                      onInput={(e) => setState(props.pane.id, "notes", i(), "comment", e.currentTarget.value)} />
                  </div>
                  <div class="browser-note-actions">
                    <button class="icon-btn" title={t("Send this one to an agent")} onClick={(e) => send(e, [n])}><Icon name="chat" size={13} /></button>
                    <button class="icon-btn" title={t("Remove")} onClick={() => removeNote(n.id)}><Icon name="close" size={12} /></button>
                  </div>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}
