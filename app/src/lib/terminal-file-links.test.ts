import { describe, expect, it } from "vitest";
import { type Cell, fileLinksAt, rowsText } from "./terminal-file-links";

const row = (text: string, cols = text.length): Cell[] => [...text.padEnd(cols)].map((chars) => ({ chars, width: 1 }));

describe("rowsText", () => {
  it("skips the second cell of a wide character and keeps columns", () => {
    const cells: Cell[] = [{ chars: "中", width: 2 }, { chars: "", width: 0 }, { chars: "a", width: 1 }, { chars: "", width: 1 }];
    const { text, at } = rowsText([cells]);
    expect(text).toBe("中a ");
    expect(at.map((p) => p.col)).toEqual([0, 2, 3]);
  });
});

describe("fileLinksAt", () => {
  it("places a link on its row with 1-based inclusive columns", () => {
    const [link] = fileLinksAt([row("see src/a.ts:42 ok")], 5, 5);
    expect(link.ref).toEqual({ path: "src/a.ts", line: 42 });
    expect(link.start).toEqual({ x: 5, y: 5 });
    expect(link.end).toEqual({ x: 15, y: 5 });
  });

  it("finds a path the terminal wrapped onto the next row, on both rows", () => {
    const rows = [row("open app/src/compo", 18), row("nents/x.tsx now", 18)];
    for (const y of [1, 2]) {
      const [link] = fileLinksAt(rows, 1, y);
      expect(link.ref.path).toBe("app/src/components/x.tsx");
      expect(link.start).toEqual({ x: 6, y: 1 });
      expect(link.end).toEqual({ x: 11, y: 2 });
    }
  });

  it("gives only the links that touch the asked row", () => {
    const rows = [row("a.py", 10), row("b.rs", 10)];
    expect(fileLinksAt(rows, 1, 2).map((l) => l.ref.path)).toEqual(["b.rs"]);
  });
});