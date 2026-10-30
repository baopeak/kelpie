// The pictures both composers hold: what is attached, what is refused and with which message, what a
// terminal session is not given the bytes of, and what is left to the composer.
import { beforeEach, describe, expect, it, vi } from "vitest";

const { notify, readImage, writeTempFile } = vi.hoisted(() => ({
  notify: vi.fn(),
  readImage: vi.fn(),
  writeTempFile: vi.fn(),
}));
vi.mock("./store", () => ({ store: { notify } }));
vi.mock("./ipc", () => ({ readImage, writeTempFile }));
vi.mock("@tauri-apps/api/path", () => ({ tempDir: async () => "/tmp" }));

import { createPictures } from "./composer-pictures";
import type { SessionInfo } from "./types";

const session = (over: Partial<SessionInfo> = {}) => () => ({ chat: true, ...over }) as SessionInfo;
const png = (bytes = 3) => new File([new Uint8Array(bytes)], "shot.png", { type: "image/png" });
const clipboard = (files: File[], text = "") => ({ clipboardData: { files, getData: (type: string) => (type === "text/plain" ? text : "") }, preventDefault: vi.fn() }) as unknown as ClipboardEvent & { preventDefault: ReturnType<typeof vi.fn> };
const settled = async () => {
  for (let i = 0; i < 20; i++) await new Promise((r) => setTimeout(r));
};

beforeEach(() => {
  vi.clearAllMocks();
  readImage.mockResolvedValue({ mime: "image/png", base64: "QUJD" });
  writeTempFile.mockImplementation(async (_label: string, ext: string) => `C:\\Temp\\kelpie-composer\\image-${writeTempFile.mock.calls.length}.${ext}`);
  // Node has no FileReader; what the composer asks of it is this little.
  vi.stubGlobal("FileReader", class {
    result = "data:image/png;base64,QUJD";
    onload: (() => void) | null = null;
    readAsDataURL() {
      queueMicrotask(() => this.onload?.());
    }
  });
});

describe("a paste", () => {
  it("attaches its pictures to a temp file and keeps the field from taking them", async () => {
    const p = createPictures(session());
    const e = clipboard([png()]);
    p.paste(e);
    await settled();
    expect(e.preventDefault).toHaveBeenCalled();
    expect(p.images()).toMatchObject([{ name: "image-1.png", mime: "image/png", path: "C:\\Temp\\kelpie-composer\\image-1.png", data: "QUJD", url: "data:image/png;base64,QUJD" }]);
  });

  it("refuses a file that only claims to be a picture, and keeps a real one", async () => {
    vi.stubGlobal("createImageBitmap", async (f: File) => {
      if (f.name === "fake.png") throw new Error("InvalidStateError");
      return { close() {} };
    });
    const p = createPictures(session());
    p.paste(clipboard([new File([new Uint8Array(3)], "fake.png", { type: "image/png" }), png()]));
    await settled();
    vi.unstubAllGlobals();
    expect(p.images()).toHaveLength(1);
    expect(writeTempFile).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(String(notify.mock.calls[0][0])).toContain("fake.png");
  });

  it("leaves text to the field", () => {
    const p = createPictures(session());
    const e = clipboard([]);
    p.paste(e);
    expect(e.preventDefault).not.toHaveBeenCalled();
    expect(writeTempFile).not.toHaveBeenCalled();
  });

  it("leaves text with a rendered picture beside it to the field: Office copies both", () => {
    const p = createPictures(session());
    const e = clipboard([png()], "a table cell");
    p.paste(e);
    expect(e.preventDefault).not.toHaveBeenCalled();
    expect(writeTempFile).not.toHaveBeenCalled();
  });

  it("takes the picture when the text beside it is only white space", async () => {
    const p = createPictures(session());
    const e = clipboard([png()], " \n");
    p.paste(e);
    await settled();
    expect(e.preventDefault).toHaveBeenCalled();
    expect(p.images()).toHaveLength(1);
  });

  it("holds to the limit within one paste, keeping the order", async () => {
    const p = createPictures(session());
    p.paste(clipboard(Array.from({ length: 10 }, () => png())));
    await settled();
    expect(p.images().map((i) => i.name)).toEqual([1, 2, 3, 4, 5, 6, 7, 8].map((n) => `image-${n}.png`));
    expect(notify).toHaveBeenLastCalledWith("At most 8 pictures");
  });

  it("holds to the limit across pastes that come before the first is through", async () => {
    const p = createPictures(session());
    for (let i = 0; i < 10; i++) p.paste(clipboard([png()]));
    await settled();
    expect(p.images()).toHaveLength(8);
  });

  it("refuses what the composer cannot send, with the message for it", async () => {
    const big = createPictures(session());
    big.paste(clipboard([png(5 * 1024 * 1024 + 1)]));
    await settled();
    expect(big.images()).toEqual([]);
    expect(notify).toHaveBeenLastCalledWith("A picture sent to a chat can be at most 5 MB", undefined, 6000);

    const remote = createPictures(session({ remote: { dest: "host", tmux: false } }));
    remote.paste(clipboard([png()]));
    await settled();
    expect(remote.images()).toEqual([]);
    expect(notify).toHaveBeenLastCalledWith("Pictures cannot be sent to a session on another machine");

    const nine = createPictures(session());
    for (let i = 0; i < 9; i++) {
      nine.paste(clipboard([png()]));
      await settled();
    }
    expect(nine.images()).toHaveLength(8);
    expect(notify).toHaveBeenLastCalledWith("At most 8 pictures");
  });

  it("takes a large picture for a terminal: only a chat sends it to the model", async () => {
    const p = createPictures(session({ chat: false }));
    p.paste(clipboard([png(5 * 1024 * 1024 + 1)]));
    await settled();
    expect(p.images()).toHaveLength(1);
  });
});

describe("a drop", () => {
  it("attaches a picture by its own path, with its bytes for a chat", async () => {
    const p = createPictures(session());
    await p.drop(["D:\\shots\\a.JPG"]);
    expect(p.images()).toMatchObject([{ name: "a.JPG", mime: "image/jpeg", path: "D:\\shots\\a.JPG", data: "QUJD", url: "data:image/jpeg;base64,QUJD" }]);
    expect(writeTempFile).not.toHaveBeenCalled();
  });

  it("does not read a picture for a terminal, which only gets its path", async () => {
    const p = createPictures(session({ chat: false }));
    await p.drop(["D:\\shots\\a.png"]);
    expect(readImage).not.toHaveBeenCalled();
    expect(p.images()).toMatchObject([{ path: "D:\\shots\\a.png" }]);
    expect(p.images()[0].url).toBeUndefined();
  });

  it("hands on everything else, once the pictures are in", async () => {
    const seen: unknown[] = [];
    const p = createPictures(session(), (others) => seen.push([others, p.images().length]));
    await p.drop(["D:\\a.txt", "D:\\b.png", "D:\\c"]);
    expect(seen).toEqual([[["D:\\a.txt", "D:\\c"], 1]]);
  });

  it("tells what could not be read and goes on with the rest", async () => {
    const p = createPictures(session());
    readImage.mockRejectedValueOnce("not a picture file");
    await p.drop(["D:\\a.png", "D:\\b.png"]);
    expect(notify).toHaveBeenCalledWith("not a picture file", undefined, 6000);
    expect(p.images().map((i) => i.name)).toEqual(["b.png"]);
  });

  it("refuses a picture too large for a chat", async () => {
    const p = createPictures(session());
    readImage.mockResolvedValueOnce({ mime: "image/png", base64: "A".repeat(7 * 1024 * 1024) });
    await p.drop(["D:\\a.png"]);
    expect(p.images()).toEqual([]);
    expect(notify).toHaveBeenCalledWith("A picture sent to a chat can be at most 5 MB", undefined, 6000);
  });
});

describe("sending to a chat", () => {
  it("gives the bytes it has, and reads those of a picture dropped while the session was a terminal", async () => {
    let chat = false;
    const p = createPictures(() => ({ chat }) as SessionInfo);
    await p.drop(["D:\\a.png"]);
    chat = true;
    await p.drop(["D:\\b.png"]);
    expect(readImage).toHaveBeenCalledTimes(1);
    expect(await p.forChat()).toEqual([
      { mime: "image/png", data: "QUJD", path: "D:\\a.png" },
      { mime: "image/png", data: "QUJD", path: "D:\\b.png" },
    ]);
    expect(readImage).toHaveBeenCalledTimes(2);
  });

  it("refuses at send what turned out too large", async () => {
    const p = createPictures(session({ chat: false }));
    await p.drop(["D:\\a.png"]);
    readImage.mockResolvedValueOnce({ mime: "image/png", base64: "A".repeat(7 * 1024 * 1024) });
    await expect(p.forChat()).rejects.toThrow("A picture sent to a chat can be at most 5 MB");
  });

  it("refuses a message whose pictures together would not fit the connection, and keeps them", async () => {
    const p = createPictures(session());
    // Each within the limit on one picture, 5 MB once decoded.
    readImage.mockResolvedValue({ mime: "image/png", base64: "A".repeat(6.5 * 1024 * 1024) });
    for (const n of [1, 2, 3, 4, 5]) await p.drop([`D:\\${n}.png`]);
    expect(p.images()).toHaveLength(5);
    await expect(p.forChat()).rejects.toThrow("Too many pictures for one message: send them in smaller groups");
    expect(p.images()).toHaveLength(5);
    p.remove(p.images()[0].id);
    await expect(p.forChat()).resolves.toHaveLength(4);
  });

  it("forgets a picture removed, and only those that were sent", async () => {
    const p = createPictures(session());
    await p.drop(["D:\\a.png", "D:\\b.png", "D:\\c.png"]);
    p.remove(p.images()[1].id);
    expect(p.images().map((i) => i.name)).toEqual(["a.png", "c.png"]);
    const sent = p.images();
    await p.drop(["D:\\later.png"]);
    p.forget(sent);
    expect(p.images().map((i) => i.name)).toEqual(["later.png"]);
  });
});

describe("what was attached", () => {
  it("may be read when the transcript shows it, wherever it lies", async () => {
    const { imageUrl } = await import("../chat/chat-images");
    const p = createPictures(session());
    const path = "D:\\anywhere\\attached-here.png";
    await p.drop([path]);
    readImage.mockClear();
    expect(await imageUrl({ name: "attached-here.png", path })).toBe("data:image/png;base64,QUJD");
    expect(readImage).toHaveBeenCalledWith(path);
    readImage.mockClear();
    expect(await imageUrl({ name: "x.png", path: "D:\\anywhere\\not-attached.png" })).toBeNull();
    expect(readImage).not.toHaveBeenCalled();
  });
});
