// How a picture in the transcript becomes something to show. A path can come from the agent's log, so
// what may be read is decided before any file-system call, and anything else stays a name.
import { describe, expect, it, vi } from "vitest";
import { createImageUrl, rememberAttached } from "./chat-images";

const TEMP = "C:\\Users\\me\\AppData\\Local\\Temp\\";
const IN_FOLDER = `${TEMP}kelpie-composer\\`;

function setup(temp: string | Promise<string> = TEMP) {
  const read = vi.fn(async (path: string) => ({ mime: "image/png", base64: `bytes-of-${path.split(/[\\/]/).pop()}` }));
  const tempDir = vi.fn(async () => temp);
  return { read, tempDir, imageUrl: createImageUrl({ read, tempDir }) };
}

describe("a picture with bytes", () => {
  it("becomes a data URL with nothing read", async () => {
    const { imageUrl, read, tempDir } = setup();
    expect(await imageUrl({ name: "a", mime: "image/png", data: "QUJD" })).toBe("data:image/png;base64,QUJD");
    expect(read).not.toHaveBeenCalled();
    expect(tempDir).not.toHaveBeenCalled();
  });

  it("is a placeholder when its type is not a picture type", async () => {
    const { imageUrl } = setup();
    for (const mime of ["image/svg+xml", "text/html", "constructor", "", undefined]) expect(await imageUrl({ name: "a", mime, data: "QUJD" })).toBeNull();
  });
});

describe("a picture with a path", () => {
  it("is read when it lies in the composer's temp folder", async () => {
    const { imageUrl, read } = setup();
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER}x.png` })).toBe("data:image/png;base64,bytes-of-x.png");
    expect(read).toHaveBeenCalledWith(`${IN_FOLDER}x.png`);
  });

  it("is read when it was attached in this window, wherever it lies", async () => {
    const { imageUrl, read } = setup();
    rememberAttached("D:\\shots\\mine.png");
    expect(await imageUrl({ name: "mine.png", path: "D:\\shots\\mine.png" })).toBe("data:image/png;base64,bytes-of-mine.png");
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("is never read from anywhere else", async () => {
    const { imageUrl, read } = setup();
    const elsewhere = [
      "D:\\secret\\x.png",
      "C:\\evil\\kelpie-composer\\x.png",
      `${TEMP}x.png`,
      `${TEMP}kelpie-composer-not\\x.png`,
      `${IN_FOLDER}..\\..\\x.png`,
      `${IN_FOLDER}.. \\x.png`,
      `${IN_FOLDER}sub\\x.png`,
      `${IN_FOLDER}x.png:hidden.png`,
      `${IN_FOLDER}`,
      "\\\\host\\share\\x.png",
      "/\\host\\x.png",
    ];
    for (const path of elsewhere) expect(await imageUrl({ name: "x.png", path }), path).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("is not read when its stated type is not a picture type", async () => {
    const { imageUrl, read } = setup();
    expect(await imageUrl({ name: "x.png", mime: "text/html", path: `${IN_FOLDER}x.png` })).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("is not trusted for what the native side says it is", async () => {
    const read = vi.fn(async () => ({ mime: "text/html", base64: "QUJD" }));
    const imageUrl = createImageUrl({ read, tempDir: async () => TEMP });
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER}x.png` })).toBeNull();
  });

  it("matches the folder whichever way the path is spelled on Windows", async () => {
    const { imageUrl, read } = setup();
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER.toUpperCase().replace(/\\/g, "/")}x.png` })).not.toBeNull();
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("tells case apart where the temp folder is a POSIX one", async () => {
    const { imageUrl, read } = setup("/tmp/");
    expect(await imageUrl({ name: "x.png", path: "/tmp/kelpie-composer/x.png" })).not.toBeNull();
    expect(await imageUrl({ name: "x.png", path: "/TMP/kelpie-composer/y.png" })).toBeNull();
    expect(read).toHaveBeenCalledTimes(1);
  });

  it("is a placeholder when the read fails, and tried again next time", async () => {
    const { imageUrl, read } = setup();
    read.mockRejectedValueOnce(new Error("swept"));
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER}x.png` })).toBeNull();
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER}x.png` })).not.toBeNull();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("is a placeholder when the temp folder cannot be known", async () => {
    const read = vi.fn();
    const imageUrl = createImageUrl({ read, tempDir: async () => { throw new Error("no"); } });
    expect(await imageUrl({ name: "x.png", path: `${IN_FOLDER}x.png` })).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });
});

describe("a picture with neither", () => {
  it("is a placeholder, with no call at all", async () => {
    const { imageUrl, read, tempDir } = setup();
    expect(await imageUrl({ name: "old.png" })).toBeNull();
    expect(read).not.toHaveBeenCalled();
    expect(tempDir).not.toHaveBeenCalled();
  });
});

describe("the cache", () => {
  const path = (n: number) => `${IN_FOLDER}${n}.png`;

  it("does not read a cached path again, also while the first read is on its way", async () => {
    const { imageUrl, read } = setup();
    const [a, b] = await Promise.all([imageUrl({ name: "x", path: path(1) }), imageUrl({ name: "x", path: path(1) })]);
    expect(a).toBe(b);
    await imageUrl({ name: "x", path: path(1) });
    expect(read).toHaveBeenCalledTimes(1);
  });

  /** Pictures of 10 MiB of base64 each, against a cache of 64 MiB. */
  function large() {
    const bytes = "A".repeat(10 * 1024 * 1024);
    const read = vi.fn(async () => ({ mime: "image/png", base64: bytes }));
    return { read, imageUrl: createImageUrl({ read, tempDir: async () => TEMP }) };
  }

  it("holds many small pictures: its limit is in bytes", async () => {
    const { imageUrl, read } = setup();
    for (let n = 0; n < 100; n++) await imageUrl({ name: "x", path: path(n) });
    await imageUrl({ name: "x", path: path(0) });
    expect(read).toHaveBeenCalledTimes(100);
  });

  it("evicts the oldest past its limit", async () => {
    const { imageUrl, read } = large();
    for (let n = 0; n < 7; n++) await imageUrl({ name: "x", path: path(n) });
    expect(read).toHaveBeenCalledTimes(7);
    await imageUrl({ name: "x", path: path(6) });
    await imageUrl({ name: "x", path: path(1) });
    expect(read).toHaveBeenCalledTimes(7);
    await imageUrl({ name: "x", path: path(0) });
    expect(read).toHaveBeenCalledTimes(8);
  });

  it("counts a use as recent", async () => {
    const { imageUrl, read } = large();
    for (let n = 0; n < 6; n++) await imageUrl({ name: "x", path: path(n) });
    await imageUrl({ name: "x", path: path(0) });
    await imageUrl({ name: "x", path: path(6) });
    expect(read).toHaveBeenCalledTimes(7);
    await imageUrl({ name: "x", path: path(0) });
    expect(read).toHaveBeenCalledTimes(7);
    await imageUrl({ name: "x", path: path(1) });
    expect(read).toHaveBeenCalledTimes(8);
  });
});
