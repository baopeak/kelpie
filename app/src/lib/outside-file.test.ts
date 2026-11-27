// Which paths outside a project Kelpie reads, and how: a local file only, never a network share, by its name in its folder.
import { describe, expect, it } from "vitest";
import { drivePath, isOutsideFile, outsideParts } from "./outside-file";

describe("isOutsideFile", () => {
  it("takes a drive's path, and on macOS and Linux a path from the root", () => {
    for (const p of ["C:\\Users\\u\\a.txt", "c:/x", "D:\\"]) expect(isOutsideFile(p, true)).toBe(true);
    expect(isOutsideFile("/home/u/a.txt", false)).toBe(true);
    // On Windows a path without a drive is read on whatever drive is current.
    expect(isOutsideFile("/home/u/a.txt", true)).toBe(false);
  });

  it.each([
    "\\\\host\\share\\a", "//host/share/a", "/\\host/share/a", "\\/host/share/a",
    "\\??\\UNC\\host\\share\\a.txt", "/??/UNC/host/share/a.txt", "\\??\\GLOBALROOT\\Device\\Mup\\host\\s\\a", "\\\\?\\UNC\\host\\s\\a",
    "\\\\.\\pipe\\x", "C:\\a\\*.txt", "D:a.ts", "a.ts", "../a.ts", "",
  ])("never takes %j", (p) => {
    expect(isOutsideFile(p, true)).toBe(false);
    expect(isOutsideFile(p, false)).toBe(false);
  });
});

describe("drivePath", () => {
  it("names a Git Bash drive path as Windows does, on Windows only", () => {
    expect(drivePath("/d/Works/a.ts", true)).toBe("D:/Works/a.ts");
    expect(drivePath("/c", true)).toBe("C:/");
    expect(drivePath("/d/Works/a.ts", false)).toBe("/d/Works/a.ts");
    for (const p of ["/dev/null", "/home/u", "D:\\a", "a/b"]) expect(drivePath(p, true)).toBe(p);
  });
});

describe("outsideParts", () => {
  it("splits a path into its folder and its name, keeping a drive root's slash", () => {
    expect(outsideParts("C:\\Users\\u\\a.txt")).toEqual({ root: "C:/Users/u", name: "a.txt" });
    expect(outsideParts("C:/a.txt")).toEqual({ root: "C:/", name: "a.txt" });
    expect(outsideParts("/a.txt")).toEqual({ root: "/", name: "a.txt" });
    expect(outsideParts("C:/x/../y/./b.md")).toEqual({ root: "C:/y", name: "b.md" });
  });

  it("gives no name for a drive's root", () => {
    expect(outsideParts("C:/").name).toBe("");
    expect(outsideParts("C:/a/../..").name).toBe("");
  });
});
