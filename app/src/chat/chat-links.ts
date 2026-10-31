// What the transcript's links and inline code point at. All of it is agent output, so untrusted:
// these functions only read text, and only `web` and project-relative paths lead anywhere.
import { Marked } from "marked";
import type { Project } from "../lib/types";

export interface FileRef { path: string; line?: number }

export type ChatLink =
  | { kind: "web"; url: string }
  | { kind: "file"; path: string; line?: number }
  | { kind: "none" };

const NONE: ChatLink = { kind: "none" };

const lineNumber = (digits: string | undefined) => {
  const line = Number(digits);
  return line > 0 && Number.isSafeInteger(line) ? line : undefined;
};

/** `path:12` and `path:12:7` as the path and the line; anything else as it is. */
function splitLine(text: string): FileRef {
  const m = /^(.+?):(\d+)(?::\d+)?$/.exec(text);
  return m ? { path: m[1], line: lineNumber(m[2]) } : { path: text };
}

/** An `http(s)` address as the OS opener should get it, or null: percent-encoded, so a quote or a
 *  space cannot end it inside a command line, and with a lowercase scheme, which is all the
 *  opener's allowlist matches. */
function webUrl(raw: string): string | null {
  if (!/^https?:\/\//i.test(raw)) return null;
  try {
    return new URL(raw).href;
  } catch {
    return null;
  }
}

/** What a link leads to. Only `http(s)` is for the browser; a `\\host` or `//host` path is refused
 *  here, before anything could touch it (Windows would sign in to that host). */
export function parseChatLink(href: string): ChatLink {
  const raw = href.trim();
  if (!raw || raw.startsWith("#") || /[\x00-\x1f\x7f]/.test(raw)) return NONE;
  const url = webUrl(raw);
  if (url) return { kind: "web", url };
  const hash = raw.indexOf("#");
  let rest = hash < 0 ? raw : raw.slice(0, hash);
  const fragmentLine = hash < 0 ? undefined : lineNumber(/^L(\d+)/i.exec(raw.slice(hash + 1))?.[1]);
  try {
    rest = decodeURIComponent(rest);
  } catch {
    // A name with a stray `%` is still a name.
  }
  if (/^file:/i.test(rest)) {
    const after = /^file:\/\/\/(.*)$/i.exec(rest)?.[1];
    if (after === undefined) return NONE;
    rest = /^[a-zA-Z]:/.test(after) ? after : `/${after}`;
  }
  // Not a scheme: a single letter before the colon (a drive), or `a.ts:42` (a name with a dot and a line).
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]+):(.*)$/.exec(rest);
  if (scheme && !(scheme[1].includes(".") && /^\d+(:\d+)?$/.test(scheme[2]))) return NONE;
  const { path, line } = splitLine(rest);
  if (!path || /^[\\/]{2}/.test(path)) return NONE;
  return { kind: "file", path, line: fragmentLine ?? line };
}

/** The file inline code names, if it looks like one: a single word with a folder in it, an
 *  extension or the shape of a dotfile (`.gitignore`), plus `:line[:col]`. It is taken literally (no
 *  percent decoding). A false positive costs one toast on click, since nothing is checked until then. */
export function fileRefInCode(text: string): FileRef | null {
  if (text.length > 260 || /[\s*?"'<>|#]/.test(text) || /^(-|@|\\|file:)/i.test(text) || /[\\/)]$/.test(text)) return null;
  if (parseChatLink(text).kind !== "file") return null;
  const ref = splitLine(text);
  return /[\\/]/.test(ref.path) || /\.[a-zA-Z][a-zA-Z0-9]{0,7}$/.test(ref.path) || /^\.[a-z][a-z0-9_]{0,15}$/.test(ref.path) ? ref : null;
}

/** `path` relative to the project folder, or null when it lies outside. A remote project's root
 *  is `ssh://host/abs`, its paths plain `/abs/…`. What comes back is never rooted: `D:foo` and
 *  `D:/proj//etc/x` would otherwise leave the project as soon as they are joined to its folder. */
export function projectRelative(project: Pick<Project, "path">, path: string): string | null {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const root = norm(project.path.replace(/^ssh:\/\/[^/]+/, ""));
  const p = norm(path).replace(/^\.\//, "");
  // Stricter than the file system: a name of dots and spaces is no real file, and reads like `..`.
  if (p.split("/").some((part) => /^\.\.[. ]*$/.test(part))) return null;
  const win = /^[a-zA-Z]:/.test(root);
  const same = (a: string, b: string) => (win ? a.toLowerCase() === b.toLowerCase() : a === b);
  const inside = same(p.slice(0, root.length), root) && p[root.length] === "/";
  const rel = !/^([a-zA-Z]:)?\//.test(p) ? p : inside ? p.slice(root.length + 1) : null;
  // On POSIX `a:b.txt` is an ordinary name.
  return rel && !(win ? /^([a-zA-Z]:|\/)/ : /^\//).test(rel) ? rel : null;
}

const BRACKETS: Record<string, string> = { ")": "(", "]": "[", "}": "{" };

/** `url` without the punctuation that follows an address in a sentence; a closing bracket stays when
 *  it closes one opened inside the address. */
function trimUrl(url: string): string {
  const count = (c: string) => url.split(c).length - 1;
  const extra: Record<string, number> = {};
  for (const close in BRACKETS) extra[close] = count(close) - count(BRACKETS[close]);
  let end = url.length;
  for (; end > 0; end--) {
    const last = url[end - 1];
    if (last in BRACKETS ? extra[last]-- <= 0 : !".,;:!?*".includes(last)) break;
  }
  return url.slice(0, end);
}

/** `text` split into plain parts and valid `http(s)` addresses. */
export function linkifyText(text: string): ({ text: string } | { url: string })[] {
  const parts: ({ text: string } | { url: string })[] = [];
  let at = 0;
  // Control characters and CJK or curly punctuation end an address: none is valid in one unencoded.
  for (const m of text.matchAll(/\bhttps?:\/\/[^\s<>"'`\x00-\x1f\x7f“”‘’«»。，、；：！？（）【】《》「」『』]+/gi)) {
    const url = trimUrl(m[0]);
    if (!webUrl(url)) continue;
    if (m.index > at) parts.push({ text: text.slice(at, m.index) });
    parts.push({ url });
    at = m.index + url.length;
  }
  if (at < text.length) parts.push({ text: text.slice(at) });
  return parts;
}

const escapeAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

// DOMPurify's URI filter empties `file:///…`, `D:/x` and `a.ts:42` hrefs, and the app's other
// markdown must keep its normal links. So a chat link's target is data, read by `parseChatLink`
// alone on click, and its `href` is only there to make it a focusable link. A task list's state is
// text: chat refuses form controls.
const chatMarked = new Marked({
  gfm: true,
  renderer: {
    link({ href, title, tokens }) {
      const label = this.parser.parseInline(tokens);
      return `<a href="#" data-href="${escapeAttr(href)}" title="${escapeAttr(title || href)}">${label}</a>`;
    },
    checkbox({ checked }) {
      return checked ? "☑ " : "☐ ";
    },
  },
});

/** A reply as HTML, not yet sanitized. */
export const chatMarkdownHtml = (text: string) => chatMarked.parse(text, { async: false }) as string;

/** DOMPurify's settings for a reply. A form or an image map navigates the window on click, and
 *  `style`, `class` (any of the app's own, like the fixed `.toast`) or an open `<dialog>` could lay a reply
 *  over the approval buttons. Table cells keep their alignment: marked writes it as `align=`. */
export const CHAT_SANITIZE = {
  FORBID_TAGS: ["style", "form", "input", "button", "textarea", "select", "map", "area", "dialog"],
  FORBID_ATTR: ["style", "class"],
};
