import { describe, expect, it } from "vitest";
import { CHAT_SANITIZE, chatMarkdownHtml, fileRefInCode, linkifyText, parseChatLink, projectRelative } from "./chat-links";

// DOMPurify needs a real DOM, so what it does with these is checked in a browser; this only
// stops an entry being dropped by accident.
describe("CHAT_SANITIZE", () => {
  it("refuses everything that could navigate the window or sit over the approval buttons", () => {
    expect(CHAT_SANITIZE.FORBID_TAGS).toEqual(expect.arrayContaining(["style", "form", "input", "button", "textarea", "select", "map", "area", "dialog"]));
    expect(CHAT_SANITIZE.FORBID_ATTR).toEqual(expect.arrayContaining(["style", "class"]));
  });
});

describe("parseChatLink", () => {
  it.each([
    ["src/a.ts", "src/a.ts", undefined],
    ["src/a.ts#L42", "src/a.ts", 42],
    ["src/a.ts#L42-L51", "src/a.ts", 42],
    ["src/a.ts#L42C7", "src/a.ts", 42],
    ["src/a.ts:42", "src/a.ts", 42],
    ["src/a.ts:42:7", "src/a.ts", 42],
    ["a.ts:42", "a.ts", 42],
    ["./a.ts", "./a.ts", undefined],
    ["foo.ts", "foo.ts", undefined],
    ["src/my%20file.ts", "src/my file.ts", undefined],
    ["src/d%E1%BB%B1.ts#L3", "src/dự.ts", 3],
    ["src/a%23b.ts", "src/a#b.ts", undefined],
    ["D:/proj/a.ts", "D:/proj/a.ts", undefined],
    ["D:/proj/a.ts:42:7", "D:/proj/a.ts", 42],
    ["D:\\proj\\a.ts", "D:\\proj\\a.ts", undefined],
    ["/home/u/p/a.ts#L9", "/home/u/p/a.ts", 9],
    ["file:///D:/proj/a.ts", "D:/proj/a.ts", undefined],
    ["FILE:///D:/proj/a.ts#L42", "D:/proj/a.ts", 42],
    ["file:///D%3A/proj/my%20a.ts", "D:/proj/my a.ts", undefined],
    ["file:///home/u/a.ts:12", "/home/u/a.ts", 12],
    ["README.md#installing", "README.md", undefined],
    ["a.ts:0", "a.ts", undefined],
    ["C:\\Windows\\system32\\calc.exe", "C:\\Windows\\system32\\calc.exe", undefined],
    ["C:foo.ts", "C:foo.ts", undefined],
  ])("%s names a file", (href, path, line) => {
    expect(parseChatLink(href)).toEqual({ kind: "file", path, line });
  });

  it.each([
    ["https://x.y/z", "https://x.y/z"],
    ["http://x.y", "http://x.y/"],
    ["HTTP://X", "http://x/"],
    ["HtTpS://X.y/A", "https://x.y/A"],
    ["https://x.y/a#L3", "https://x.y/a#L3"],
    ["https://x.y:8080/a", "https://x.y:8080/a"],
    ['https://x.y/" -foo', "https://x.y/%22%20-foo"],
    ['https://x.y/?a="b c"', "https://x.y/?a=%22b%20c%22"],
  ])("%s goes to the browser as %s", (href, url) => {
    expect(parseChatLink(href)).toEqual({ kind: "web", url });
  });

  it.each([
    "https://",
    "http://",
    "https://[bad",
    "https://x y/",
    "",
    "   ",
    "#top",
    "#L42",
    "javascript:alert(1)",
    "JavaScript:alert(1)",
    "javascript:1",
    "javascript%3Aalert(1)",
    "java\tscript:alert(1)",
    "data:text/html,x",
    "vbscript:x",
    "mailto:a@b.c",
    "tel:5551234",
    "ms-settings:privacy",
    "vscode://file/x.ts",
    "ssh://host/a",
    "https%3A//x.y",
    "http:x.y",
    "a.ts:stream",
    "\\\\host\\share\\a",
    "//host/a",
    "/\\host/a",
    "%5C%5Chost%5Cshare",
    "/%2Fhost/a",
    "file://host/a",
    "file://localhost/a",
    "file:////host/a",
    "file:///%5C%5Chost/a",
    "file:/a",
    "file:a",
    "\\\\?\\C:\\x",
  ])("%j is ignored", (href) => {
    expect(parseChatLink(href)).toEqual({ kind: "none" });
  });

  it("does not throw on a bad percent sequence", () => {
    expect(parseChatLink("src/100%.ts")).toEqual({ kind: "file", path: "src/100%.ts" });
    expect(parseChatLink("%E0%A4%A")).toEqual({ kind: "file", path: "%E0%A4%A" });
  });

  it("decodes before judging a path, so encoding cannot hide a parent folder", () => {
    expect(parseChatLink("%2e%2e/%2e%2e/secret")).toEqual({ kind: "file", path: "../../secret" });
    expect(projectRelative({ path: "D:\\proj" }, "../../secret")).toBeNull();
  });
});

describe("fileRefInCode", () => {
  it.each([
    ["src/a.ts", "src/a.ts", undefined],
    ["a.ts:12", "a.ts", 12],
    ["src/a.ts:12:3", "src/a.ts", 12],
    ["C:\\x\\a.rs", "C:\\x\\a.rs", undefined],
    ["C:\\x\\a.rs:7", "C:\\x\\a.rs", 7],
    ["README.md", "README.md", undefined],
    ["./a.ts", "./a.ts", undefined],
    ["app/(group)/[id]/page.tsx", "app/(group)/[id]/page.tsx", undefined],
    ["src/my%20file.ts", "src/my%20file.ts", undefined],
    [".gitignore", ".gitignore", undefined],
    [".dockerignore", ".dockerignore", undefined],
    [".editorconfig", ".editorconfig", undefined],
    [".gitattributes:3", ".gitattributes", 3],
    [".env", ".env", undefined],
  ])("%s is a file reference", (text, path, line) => {
    expect(fileRefInCode(text)).toEqual({ path, line });
  });

  it.each([
    ".tool-target",
    ".addEventListener",
    `.${"a".repeat(17)}`,
    "..",
    "",
    "Cargo",
    "npm install",
    "foo.bar()",
    "foo.bar(x/y)",
    "1.2.3",
    "--flag",
    "-x",
    "-x/y.ts",
    "https://x.y/z",
    "file:///D:/a.ts",
    "vscode://file/a.ts",
    "mailto:a@b.c",
    "javascript:alert(1)",
    "e.g.",
    "@types/node",
    "@scope/pkg.json",
    "\\\\host\\x",
    "//host/x",
    "\\n",
    "src/",
    "src\\",
    "src/*.ts",
    "owner/repo#12",
    "Array<string>",
    "std::vec::Vec",
    "'a.ts'",
    "\"a.ts\"",
    "a/b c.ts",
    "a/b\tc.ts",
    `${"a/".repeat(130)}b.ts`,
  ])("%j is not", (text) => {
    expect(fileRefInCode(text)).toBeNull();
  });

  it("takes a name up to the 260 character limit", () => {
    const longest = `${"a".repeat(257)}.ts`;
    expect(fileRefInCode(longest)).toEqual({ path: longest });
    expect(fileRefInCode(`a${longest}`)).toBeNull();
  });
});

describe("projectRelative", () => {
  const win = { path: "D:\\proj" };
  const posix = { path: "/home/u/p" };

  it.each([
    [win, "src/a.ts", "src/a.ts"],
    [win, "src\\a.ts", "src/a.ts"],
    [win, "./a.ts", "a.ts"],
    [win, "D:\\proj\\src\\a.ts", "src/a.ts"],
    [win, "D:/proj/src/a.ts", "src/a.ts"],
    [win, "d:\\PROJ\\a.ts", "a.ts"],
    [{ path: "D:\\proj\\" }, "D:\\proj\\a.ts", "a.ts"],
    [posix, "/home/u/p/src/a.ts", "src/a.ts"],
    [{ path: "/home/u/p/" }, "/home/u/p/a.ts", "a.ts"],
    [{ path: "ssh://host/home/u/p" }, "/home/u/p/a.ts", "a.ts"],
    [{ path: "ssh://me@host:2222/home/u/p" }, "/home/u/p/src/a.ts", "src/a.ts"],
    [{ path: "ssh://host/home/u/p" }, "src/a.ts", "src/a.ts"],
  ])("%j: %s is %s", (project, path, rel) => {
    expect(projectRelative(project, path)).toBe(rel);
  });

  it.each([
    [win, "../a.ts"],
    [win, "src/../../a.ts"],
    [win, "D:\\proj\\..\\a.ts"],
    [win, "src/.. /a.ts"],
    [win, "D:\\other\\a.ts"],
    [win, "D:\\proj2\\a.ts"],
    [win, "D:\\proj"],
    [win, "C:\\Windows\\system32\\calc.exe"],
    [win, "D:a.ts"],
    [win, "C:windows/system32/calc.exe"],
    [win, "\\\\host\\share\\a.ts"],
    [win, "//host/share/a.ts"],
    [posix, "/Home/u/p/a.ts"],
    [posix, "/etc/hosts"],
    [{ path: "ssh://host/home/u/p" }, "/etc/hosts"],
    [{ path: "D:/proj" }, "D:/proj/C:/Windows/system32/calc.exe"],
    [{ path: "D:/proj" }, "D:/proj///host/s/a"],
    [{ path: "D:/proj" }, "D:/proj/\\\\?\\C:\\x"],
    [posix, "/home/u/p//etc/passwd"],
    [win, "/"],
    [win, "\\"],
    [posix, "/"],
    [win, ""],
  ])("%j: %s lies outside", (project, path) => {
    expect(projectRelative(project, path)).toBeNull();
  });

  it("calls `file:///` outside, not missing", () => {
    const link = parseChatLink("file:///");
    expect(link).toEqual({ kind: "file", path: "/" });
    expect(projectRelative(win, (link as { path: string }).path)).toBeNull();
  });
});

describe("projectRelative on POSIX", () => {
  it("takes a colon as part of a name", () => {
    expect(projectRelative({ path: "/home/u/p" }, "/home/u/p/a:b.txt")).toBe("a:b.txt");
    expect(projectRelative({ path: "D:\\proj" }, "D:/proj/C:/x")).toBeNull();
  });
});

describe("linkifyText", () => {
  it.each<[string, ({ text: string } | { url: string })[]]>([
    ["no link here", [{ text: "no link here" }]],
    ["https://x.y/z now", [{ url: "https://x.y/z" }, { text: " now" }]],
    ["see https://x.y/z now", [{ text: "see " }, { url: "https://x.y/z" }, { text: " now" }]],
    ["see http://x.y", [{ text: "see " }, { url: "http://x.y" }]],
    ["HTTPS://X.Y a", [{ url: "HTTPS://X.Y" }, { text: " a" }]],
    ["two https://a.b, https://c.d.", [{ text: "two " }, { url: "https://a.b" }, { text: ", " }, { url: "https://c.d" }, { text: "." }]],
    ["(https://x.y)", [{ text: "(" }, { url: "https://x.y" }, { text: ")" }]],
    ["(https://x.y/a_(b))", [{ text: "(" }, { url: "https://x.y/a_(b)" }, { text: ")" }]],
    ["https://x.y/a_(b)", [{ url: "https://x.y/a_(b)" }]],
    ["https://x.y/a?!;:,.", [{ url: "https://x.y/a" }, { text: "?!;:,." }]],
    ["https://x.y/a).", [{ url: "https://x.y/a" }, { text: ")." }]],
    ["a\nhttps://x.y\nb", [{ text: "a\n" }, { url: "https://x.y" }, { text: "\nb" }]],
    ["<https://x.y>", [{ text: "<" }, { url: "https://x.y" }, { text: ">" }]],
    ["javascript:alert(1)", [{ text: "javascript:alert(1)" }]],
    ["xhttps://x.y", [{ text: "xhttps://x.y" }]],
    ["https://x.y// z", [{ url: "https://x.y//" }, { text: " z" }]],
    ["https://", [{ text: "https://" }]],
    ["https://.", [{ text: "https://." }]],
    ["https://[bad end", [{ text: "https://[bad end" }]],
  ])("%j", (text, parts) => {
    expect(linkifyText(text)).toEqual(parts);
  });

  it.each<[string, string, string]>([
    ["[https://a.b]", "https://a.b", "]"],
    ["[https://a.b/c]", "https://a.b/c", "]"],
    ["{https://a.b/c}", "https://a.b/c", "}"],
    ["**https://a.b/c**", "https://a.b/c", "**"],
    ["“https://a.b/c”", "https://a.b/c", "”"],
    ["https://a.b/c。", "https://a.b/c", "。"],
    ["https://a.b/c，然后", "https://a.b/c", "，然后"],
    ["https://a.b/c\x1b[0m", "https://a.b/c", "\x1b[0m"],
  ])("ends the address before what follows it: %s", (input, url, after) => {
    const parts = linkifyText(input);
    expect(parts.find((p) => "url" in p)).toEqual({ url });
    expect(parts.map((p) => ("url" in p ? p.url : p.text)).join("")).toBe(input);
    expect(input.endsWith(after)).toBe(true);
  });

  it("keeps a bracket that belongs to the address", () => {
    expect(linkifyText("https://[::1]:8080/a")).toEqual([{ url: "https://[::1]:8080/a" }]);
    expect(linkifyText("https://a.b/c[1]")).toEqual([{ url: "https://a.b/c[1]" }]);
  });

  it("handles an address followed by a very long run of closing brackets", () => {
    const tail = ")".repeat(50_000);
    expect(linkifyText(`https://x.y/a${tail}`)).toEqual([{ url: "https://x.y/a" }, { text: tail }]);
    expect(linkifyText(`https://x.y/${"(".repeat(50_000)}${tail}`)).toEqual([{ url: `https://x.y/${"(".repeat(50_000)}${tail}` }]);
  });

  it("gives back every character it was given", () => {
    const text = "  a (https://x.y/(b)), then\n\thttps://c.d/e?f=1&g=2. end ";
    expect(linkifyText(text).map((p) => ("url" in p ? p.url : p.text)).join("")).toBe(text);
  });
});

describe("chatMarkdownHtml", () => {
  it.each([
    ["[a](src/a.ts)", "src/a.ts"],
    ["[a](src/a.ts#L42-L51)", "src/a.ts#L42-L51"],
    ["[a](src/a.ts:42:7)", "src/a.ts:42:7"],
    ["[a](file:///D:/proj/a.ts)", "file:///D:/proj/a.ts"],
    ["[a](D:/proj/a.ts)", "D:/proj/a.ts"],
    ["[a](my%20file.ts)", "my%20file.ts"],
    ["[a](https://x.y/z)", "https://x.y/z"],
  ])("%s keeps its target for the click handler", (md, href) => {
    expect(chatMarkdownHtml(md)).toContain(`<a href="#" data-href="${href}" title="${href}">a</a>`);
  });

  it("keeps the link's own title, or shows its target", () => {
    expect(chatMarkdownHtml('[a](b.ts "why")')).toContain('title="why"');
    expect(chatMarkdownHtml("[a](b.ts)")).toContain('title="b.ts"');
    expect(chatMarkdownHtml('[a](<x"y.ts>)')).toContain('title="x&quot;y.ts"');
  });

  it("leaves table alignment as an attribute, which survives without `style`", () => {
    const html = chatMarkdownHtml("| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |");
    for (const side of ["left", "center", "right"]) {
      expect(html).toContain(`<th align="${side}">`);
      expect(html).toContain(`<td align="${side}">`);
    }
    expect(html).not.toContain("style=");
  });

  it("shows a task list's state without a form control", () => {
    const html = chatMarkdownHtml("- [x] done\n- [ ] open");
    expect(html).not.toContain("<input");
    expect(html).toContain("☑ done");
    expect(html).toContain("☐ open");
  });

  it("makes bare addresses links too", () => {
    expect(chatMarkdownHtml("see https://x.y/z now")).toContain('data-href="https://x.y/z"');
  });

  it("escapes the target and the title in their attributes", () => {
    const html = chatMarkdownHtml('[a](<x"y&z.ts> "t\\"<b>")');
    expect(html).toContain('data-href="x&quot;y&amp;z.ts"');
    expect(html).toContain('title="t&quot;&lt;b&gt;"');
  });

  it("renders the link's own markup", () => {
    expect(chatMarkdownHtml("[**a**](b.ts)")).toContain("<strong>a</strong></a>");
  });

  it("still renders tables and code", () => {
    const html = chatMarkdownHtml("| a |\n| - |\n| 1 |\n\n`x`");
    expect(html).toContain("<table>");
    expect(html).toContain("<code>x</code>");
  });
});
