// A small sample terminal drawn with an appearance (theme, font, cursor), for the host editor and
// the theme editor: a prompt, ls-style colours, an error line and a selection. Plain HTML, so it
// costs nothing while it is on screen and needs no terminal session.
import { For, type JSX } from "solid-js";
import type { ITheme } from "@xterm/xterm";
import { t } from "../lib/i18n";
import type { CursorStyle } from "../lib/terminal-appearance";

export interface PreviewLook {
  theme: ITheme;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
}

type Slot = keyof ITheme;
const ANSI: Slot[] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite"];

export function TerminalPreview(props: { look: PreviewLook; host?: string; class?: string }) {
  const c = (slot: Slot) => (props.look.theme[slot] as string | undefined) ?? (props.look.theme.foreground as string);
  /** A run of text in a palette colour. */
  const s = (slot: Slot, text: string, extra: JSX.CSSProperties = {}) => <span style={{ color: c(slot), ...extra }}>{text}</span>;
  const host = () => props.host || "prod-api"; // i18n-ignore
  const prompt = () => (
    <>
      {s("green", `deploy@${host()}`, { "font-weight": 700 })}
      {s("foreground", ":")}
      {s("blue", "~/app", { "font-weight": 700 })}
      {s("foreground", "$ ")}
    </>
  );
  const cursor = (): JSX.CSSProperties => {
    const col = c("cursor");
    const st = props.look.cursorStyle;
    if (st === "block") return { background: col, color: c("cursorAccent") || c("background") };
    if (st === "underline") return { "box-shadow": `inset 0 -2px 0 ${col}` };
    return { "box-shadow": `inset 2px 0 0 ${col}` };
  };
  return (
    <div
      class={`term-preview${props.class ? ` ${props.class}` : ""}`}
      role="img"
      aria-label={t("Terminal preview")}
      data-terminal-preview
      style={{
        background: c("background"),
        color: c("foreground"),
        "font-family": props.look.fontFamily,
        "font-size": `${props.look.fontSize}px`,
        "line-height": String(props.look.lineHeight),
      }}
    >
      {/* Sample shell text: the same in every language, like a real terminal. i18n-ignore */}
      <div>{prompt()}{s("foreground", "ls")}</div>
      <div>
        {s("blue", "src", { "font-weight": 700 })}{"  "}
        {s("cyan", "docs")}{"  "}
        {s("green", "deploy.sh", { "font-weight": 700 })}{"  "}
        <span style={{ background: c("selectionBackground"), color: c("selectionForeground") || c("foreground") }}>README.md</span>{"  "}
        {s("magenta", "logo.png")}{"  "}
        {s("brightBlack", ".env")}
      </div>
      <div>{prompt()}{s("foreground", "./deploy.sh")}</div>
      <div>{s("yellow", "warning:")} {s("foreground", "using cached image")}</div>
      <div>{s("red", "error:", { "font-weight": 700 })} {s("brightRed", "connection refused (port 5432)")}</div>
      <div>{prompt()}<span class={`term-preview-cursor${props.look.cursorBlink ? " blink" : ""}`} data-cursor={props.look.cursorStyle} style={cursor()}>{" "}</span></div>
      <div class="term-preview-ansi" aria-hidden="true">
        <For each={ANSI}>{(slot) => <span style={{ background: c(slot) }} />}</For>
      </div>
    </div>
  );
}
