// The session export's golden files (app/src-tauri/tests/fixtures/session-export/*.md, written by
// session_export.rs and compared byte for byte by its Rust tests) rendered with the app's own
// Markdown renderer: every message heading and tool block survives, and nothing a transcript said
// reaches a viewer as raw HTML. Run with `pnpm test` (or `pnpm exec vitest run src/lib/session-export-goldens.test.ts`).
import { describe, expect, it } from "vitest";
import { marked } from "marked";

const files = import.meta.glob<string>("../../src-tauri/tests/fixtures/session-export/*.md", { query: "?raw", import: "default", eager: true });
const goldens = Object.keys(files).map((path) => path.split("/").pop() as string);
const read = (name: string) => files[Object.keys(files).find((p) => p.endsWith("/" + name)) as string];

/** Tags the export itself writes, or Markdown renders to. */
const ALLOWED = new Set(["h1", "h2", "p", "ul", "ol", "li", "blockquote", "details", "summary", "pre", "code", "hr", "strong", "em", "del", "a", "br", "table", "thead", "tbody", "tr", "th", "td", "input"]);

describe("session export golden files", () => {
  it("are present", () => expect(goldens.length).toBeGreaterThanOrEqual(3));

  for (const name of goldens) {
    it(`${name} keeps its structure when rendered`, () => {
      const md = read(name);
      const html = marked.parse(md, { async: false });
      // Headings and tool blocks written by the export (at line starts, outside code).
      const lines = md.split("\n");
      const wantHeadings = lines.filter((l) => /^## (User|Assistant)$/.test(l)).length;
      // A tool block: `<details>` then its summary line (a message may quote `<details>` as text or code).
      const wantDetails = (md.match(/^<details>\n<summary>(Tool call|Result)/gm) ?? []).length;
      expect((html.match(/<h2>(User|Assistant)<\/h2>/g) ?? []).length).toBe(wantHeadings);
      expect((html.match(/<details>/g) ?? []).length).toBe(wantDetails);
      expect((html.match(/<\/details>/g) ?? []).length).toBe(wantDetails);
      // Outside code, only the tags above: no comment, script, textarea… from a message.
      const outside = html.replace(/<pre><code[^>]*>[\s\S]*?<\/code><\/pre>/g, "").replace(/<code>[\s\S]*?<\/code>/g, "");
      expect(outside).not.toMatch(/<!--/);
      const tags = [...outside.matchAll(/<\/?([a-zA-Z][\w-]*)/g)].map((m) => m[1].toLowerCase());
      expect(tags.filter((t) => !ALLOWED.has(t))).toEqual([]);
      // The document ends with what the export wrote last, not inside a code block.
      expect(html.trimEnd().endsWith("</p>")).toBe(true);
    });
  }
});
