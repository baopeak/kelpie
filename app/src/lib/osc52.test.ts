import { describe, expect, it } from "vitest";
import { osc52Text } from "./osc52";

const b64 = (s: string) => btoa(String.fromCharCode(...new TextEncoder().encode(s)));

describe("osc52Text", () => {
  it("decodes what a program copies, UTF-8 included", () => {
    expect(osc52Text(`c;${b64("1. Ocean waves")}`)).toBe("1. Ocean waves");
    expect(osc52Text(`;${b64("Tiếng Việt ✓")}`)).toBe("Tiếng Việt ✓");
  });

  it("never answers a clipboard read, and ignores clears and garbage", () => {
    expect(osc52Text("c;?")).toBeNull();
    expect(osc52Text("c;")).toBeNull();
    expect(osc52Text("c;not base64!")).toBeNull();
    expect(osc52Text("no separator")).toBeNull();
  });
});
