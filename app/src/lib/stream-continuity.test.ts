import { describe, expect, it } from "vitest";
import { fitChunk } from "./stream-continuity";

const bytes = (s: string) => new TextEncoder().encode(s);
const text = (b: Uint8Array) => new TextDecoder().decode(b);

describe("a live chunk against what a view has applied", () => {
  it("applies a chunk that starts where the view left off", () => {
    const fit = fitChunk(10, 15, bytes("hello"));
    expect(fit.kind).toBe("apply");
    expect(fit.kind === "apply" && text(fit.data)).toBe("hello");
  });

  it("reports the bytes missing before a chunk that starts later", () => {
    expect(fitChunk(10, 20, bytes("hello"))).toEqual({ kind: "gap", missing: 5 });
  });

  it("drops a chunk the view already has, and applies only the new part of one that overlaps", () => {
    expect(fitChunk(20, 15, bytes("hello"))).toEqual({ kind: "old" });
    expect(fitChunk(15, 15, bytes("hello"))).toEqual({ kind: "old" });
    const fit = fitChunk(13, 15, bytes("hello"));
    expect(fit.kind === "apply" && text(fit.data)).toBe("lo");
  });

  it("treats an empty chunk at the view's offset as nothing new", () => {
    expect(fitChunk(10, 10, bytes(""))).toEqual({ kind: "apply", data: bytes("") });
  });
});
