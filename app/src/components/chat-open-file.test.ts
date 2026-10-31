// What a click on something the agent named does: the path it reaches the viewer with, the toast
// when it does not, and that nothing is handed to the operating system to launch.
import { beforeEach, describe, expect, it, vi } from "vitest";

const { notify, openFile, readFile, quickOpen, openUrl, openPath, revealItemInDir } = vi.hoisted(() => ({
  notify: vi.fn(),
  openFile: vi.fn(),
  readFile: vi.fn(),
  quickOpen: vi.fn(),
  openUrl: vi.fn(async () => {}),
  openPath: vi.fn(),
  revealItemInDir: vi.fn(),
}));
vi.mock("../lib/store", () => ({ store: { notify, openFile } }));
vi.mock("../lib/ipc", () => ({ readFile, quickOpen }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl, openPath, revealItemInDir }));

import { openChatFile, openChatLink } from "./chat-open-file";

const project = { id: "p1", name: "proj", path: "D:\\proj", color: "#fff" } as any;
const hit = (path: string) => ({ path, score: 1, indices: [], is_dir: false });

/** Lets the promise chains started by a click finish. */
const settled = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

beforeEach(() => {
  vi.clearAllMocks();
  readFile.mockResolvedValue({ size: 1 });
  quickOpen.mockResolvedValue([]);
});

describe("openChatFile", () => {
  it("opens a file that exists at its line, checking with a one byte read", async () => {
    await openChatFile(project, { path: "src\\a.ts", line: 42 });
    expect(readFile).toHaveBeenCalledWith("D:\\proj", "src/a.ts", 1);
    expect(openFile).toHaveBeenCalledWith("p1", "src/a.ts", { line: 42 });
    expect(notify).not.toHaveBeenCalled();
  });

  it("turns an absolute path inside the project into a relative one", async () => {
    await openChatFile(project, { path: "d:/PROJ/src/a.ts" });
    expect(openFile).toHaveBeenCalledWith("p1", "src/a.ts", { line: undefined });
  });

  it("does not touch the file system for a path outside, and says so", async () => {
    for (const path of ["C:\\Windows\\system32\\calc.exe", "D:\\other\\a.ts", "../a.ts", "\\\\host\\share\\a", "D:a.ts"]) {
      notify.mockClear();
      await openChatFile(project, { path });
      expect(notify).toHaveBeenCalledWith(`${path} is outside this project`);
    }
    expect(readFile).not.toHaveBeenCalled();
    expect(quickOpen).not.toHaveBeenCalled();
    expect(openFile).not.toHaveBeenCalled();
  });

  it("finds a file that moved by its name, when only one has that name", async () => {
    readFile.mockRejectedValue(new Error("not found"));
    quickOpen.mockResolvedValue([hit("lib/a.ts.map"), hit("lib/a.ts")]);
    await openChatFile(project, { path: "src/a.ts", line: 3 });
    expect(quickOpen).toHaveBeenCalledWith("D:\\proj", "a.ts", 50);
    expect(openFile).toHaveBeenCalledWith("p1", "lib/a.ts", { line: 3 });
  });

  it("opens a bare name in the project folder even when others share its name", async () => {
    await openChatFile(project, { path: "README.md" });
    expect(openFile).toHaveBeenCalledWith("p1", "README.md", { line: undefined });
    expect(quickOpen).not.toHaveBeenCalled();
  });

  it("finds a bare name in a subfolder", async () => {
    readFile.mockRejectedValue(new Error("not found"));
    quickOpen.mockResolvedValue([hit("src/foo.ts")]);
    await openChatFile(project, { path: "foo.ts" });
    expect(openFile).toHaveBeenCalledWith("p1", "src/foo.ts", { line: undefined });
  });

  it("says how many files share a name instead of guessing", async () => {
    readFile.mockRejectedValue(new Error("not found"));
    quickOpen.mockResolvedValue([hit("a/x.ts"), hit("b/x.ts")]);
    await openChatFile(project, { path: "x.ts" });
    expect(notify).toHaveBeenCalledWith("2 files named x.ts");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("says a file was not found, also when the search fails", async () => {
    readFile.mockRejectedValue(new Error("not found"));
    await openChatFile(project, { path: "src/x.ts" });
    expect(notify).toHaveBeenLastCalledWith("File not found: src/x.ts");
    quickOpen.mockRejectedValue(new Error("no host"));
    await openChatFile(project, { path: "src/y.ts" });
    expect(notify).toHaveBeenLastCalledWith("File not found: src/y.ts");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("does not take a folder for a file", async () => {
    readFile.mockRejectedValue(new Error("is a directory"));
    quickOpen.mockResolvedValue([{ ...hit("src/x.ts/"), is_dir: true }]);
    await openChatFile(project, { path: "x.ts" });
    expect(openFile).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledWith("File not found: x.ts");
  });

  it("works on a remote project", async () => {
    const remote = { ...project, path: "ssh://host/home/u/p" };
    await openChatFile(remote, { path: "/home/u/p/src/a.ts", line: 2 });
    expect(readFile).toHaveBeenCalledWith("ssh://host/home/u/p", "src/a.ts", 1);
    expect(openFile).toHaveBeenCalledWith("p1", "src/a.ts", { line: 2 });
  });
});

describe("what shows a file opening", () => {
  it("is told just before the file opens, so that the file ends up with the keyboard", async () => {
    const order: string[] = [];
    openFile.mockImplementationOnce(() => void order.push("open"));
    await openChatFile(project, { path: "src/a.ts" }, () => order.push("told"));
    expect(order).toEqual(["told", "open"]);
  });

  it("is told for a file found elsewhere under its name, and through a link", async () => {
    readFile.mockRejectedValue(new Error("missing"));
    quickOpen.mockResolvedValue([hit("D:\\proj\\src\\a.ts")]);
    const told = vi.fn();
    await openChatFile(project, { path: "a.ts" }, told);
    expect(told).toHaveBeenCalledTimes(1);
    readFile.mockResolvedValue({ size: 1 });
    openChatLink("src/a.ts", project, told);
    await settled();
    expect(told).toHaveBeenCalledTimes(2);
  });

  it("is not told when nothing opened, nor for a web link", async () => {
    const told = vi.fn();
    await openChatFile(project, { path: "D:\\other\\a.ts" }, told);
    readFile.mockRejectedValue(new Error("missing"));
    await openChatFile(project, { path: "gone.ts" }, told);
    openChatLink("https://x.y", project, told);
    await settled();
    expect(told).not.toHaveBeenCalled();
  });
});

describe("openChatLink", () => {
  it("sends only http(s) to the browser", async () => {
    openChatLink("HTTPS://x.y/z", project);
    expect(openUrl).toHaveBeenCalledWith("https://x.y/z");
    expect(openFile).not.toHaveBeenCalled();
  });

  it("hands the opener an encoded address, never one with quotes or spaces", () => {
    openChatLink('https://x.y/" -foo', project);
    expect(openUrl).toHaveBeenCalledWith("https://x.y/%22%20-foo");
  });

  it("does nothing with an address that does not parse", async () => {
    openChatLink("https://[bad", project);
    await settled();
    expect(openUrl).not.toHaveBeenCalled();
    expect(readFile).not.toHaveBeenCalled();
  });

  it("opens a file link in the viewer, at its line", async () => {
    openChatLink("file:///D:/proj/src/a.ts#L42-L51", project);
    await settled();
    expect(openFile).toHaveBeenCalledWith("p1", "src/a.ts", { line: 42 });
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("shows a toast for a local file outside the project and launches nothing", async () => {
    openChatLink("C:\\Windows\\system32\\calc.exe", project);
    await settled();
    expect(notify).toHaveBeenCalledWith("C:\\Windows\\system32\\calc.exe is outside this project");
    expect(readFile).not.toHaveBeenCalled();
    expect(openUrl).not.toHaveBeenCalled();
    expect(openPath).not.toHaveBeenCalled();
    expect(revealItemInDir).not.toHaveBeenCalled();
  });

  it.each(["\\\\host\\x", "//host/x", "file://host/x", "javascript:alert(1)", "data:text/html,x", "ms-settings:privacy", "#top", ""])("does nothing at all for %j", async (href) => {
    openChatLink(href, project);
    await settled();
    expect(readFile).not.toHaveBeenCalled();
    expect(quickOpen).not.toHaveBeenCalled();
    expect(openUrl).not.toHaveBeenCalled();
    expect(openPath).not.toHaveBeenCalled();
    expect(openFile).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
  });

  it("leaves file links inert without a project, web links still work", async () => {
    openChatLink("src/a.ts");
    await settled();
    expect(readFile).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
    openChatLink("https://x.y");
    expect(openUrl).toHaveBeenCalledWith("https://x.y/");
  });
});
