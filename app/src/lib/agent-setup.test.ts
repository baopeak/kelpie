import { describe, expect, it } from "vitest";
import {
  MASK, commandText, copyNeedsConfirm, copyTargets, diffCounts, diffLines, envOptIn, foldUnchanged, maskArg, maskArgs, maskedEnv, provenance,
  quoteArg, registryCommandLine, registryProblem, savedFromRegistry, warningText, sizeText, splitArgs, type McpFile, type RegistryAgent,
} from "./agent-setup";

describe("masking", () => {
  it("shows environment names and never values", () => {
    expect(maskedEnv(["API_KEY", "MODE"])).toEqual([`API_KEY=${MASK}`, `MODE=${MASK}`]);
  });

  it("hides secrets in arguments but leaves ordinary ones", () => {
    expect(maskArg("--token=abc123")).toBe(`--token=${MASK}`);
    expect(maskArg("API_KEY=sk-live-1")).toBe(`API_KEY=${MASK}`);
    expect(maskArg("--root=/home/me/docs")).toBe("--root=/home/me/docs");
    expect(maskArg("-y")).toBe("-y");
    expect(maskArg("@acme/files")).toBe("@acme/files");
    expect(maskArg("https://user:hunter2@host.example/mcp")).toBe(`https://user:${MASK}@host.example/mcp`);
    expect(maskArg("https://host.example/mcp?token=abc&x=1")).toBe(`https://host.example/mcp?token=${MASK}&x=1`);
    expect(maskArg("https://host.example/mcp?page=2")).toBe("https://host.example/mcp?page=2");
  });

  it("hides credentials by their shape, wherever they sit", () => {
    expect(maskArg("ghp_0123456789abcdefghijABCDEFGHIJ012345")).toBe(MASK);
    expect(maskArg("Bearer sk-ant-api03-abcdefghijklmnopqrstuvwxyz")).toBe(`Bearer ${MASK}`);
    expect(maskArg("https://x.example/?q=1&t=xoxb-1234567890-abcdefghij")).toBe(`https://x.example/?q=1&t=${MASK}`);
    expect(maskArg("sk-learn")).toBe("sk-learn");
  });

  it("hides the value after a bare secret flag", () => {
    expect(maskArgs(["--api-key", "sk-1", "--verbose", "run"])).toEqual(["--api-key", MASK, "--verbose", "run"]);
    expect(maskArgs(["--token", "--other"])).toEqual(["--token", "--other"]);
    expect(maskArgs(["--name", "bob"])).toEqual(["--name", "bob"]);
  });

  it("writes a server's command the way it will run, masked", () => {
    expect(commandText({ transport: "stdio", command: "node", args: ["srv.js", "--token=x"], url: "" })).toBe(`node srv.js --token=${MASK}`);
    expect(commandText({ transport: "http", command: "", args: [], url: "https://a.example/mcp?key=1" })).toBe(`https://a.example/mcp?key=${MASK}`);
  });
});

describe("provenance and trust", () => {
  it("treats a project's file as untrusted and the user's as their own", () => {
    expect(provenance("project")).toEqual({ tag: "project", untrusted: true });
    expect(provenance("user")).toEqual({ tag: "user", untrusted: false });
  });

  it("asks for confirmation only when a project server goes to a user file", () => {
    expect(copyNeedsConfirm("project", ["user"])).toBe(true);
    expect(copyNeedsConfirm("project", ["project", "user"])).toBe(true);
    expect(copyNeedsConfirm("project", ["project"])).toBe(false);
    expect(copyNeedsConfirm("user", ["user", "project"])).toBe(false);
  });

  it("offers environment values only for user-level destinations", () => {
    expect(envOptIn(["project"])).toBe(false);
    expect(envOptIn(["project", "user"])).toBe(true);
  });

  it("lists every other file as a destination", () => {
    const file = (agent: string, scope: "user" | "project"): McpFile => ({ agent, scope, path: "", exists: true, error: null, can_toggle: false, servers: [] });
    const all = [file("claude", "user"), file("claude", "project"), file("codex", "user")];
    expect(copyTargets(all, { agent: "claude", scope: "user" }).map((f) => `${f.agent}/${f.scope}`)).toEqual(["claude/project", "codex/user"]);
  });
});

describe("diff", () => {
  const ops = (a: string[], b: string[]) => diffLines(a, b).map((l) => l.op + l.text);

  it("marks added and removed lines and keeps the rest", () => {
    expect(ops(["a", "b", "c"], ["a", "b", "x", "c"])).toEqual([" a", " b", "+x", " c"]);
    expect(ops(["a", "b", "c"], ["a", "c"])).toEqual([" a", "-b", " c"]);
    expect(ops(["a", "b"], ["a", "z"])).toEqual([" a", "-b", "+z"]);
    expect(ops([], ["a"])).toEqual(["+a"]);
    expect(ops(["a"], [])).toEqual(["-a"]);
    expect(ops(["a"], ["a"])).toEqual([" a"]);
  });

  it("aligns a change in the middle by what lines have in common", () => {
    expect(ops(["{", "a", "b", "c", "d", "}"], ["{", "a", "c", "e", "d", "}"])).toEqual([" {", " a", "-b", " c", "+e", " d", " }"]);
  });

  it("copes with a very large change without aligning it", () => {
    const a = Array.from({ length: 2500 }, (_, i) => `a${i}`);
    const b = Array.from({ length: 2500 }, (_, i) => `b${i}`);
    expect(diffCounts(diffLines(a, b))).toEqual({ added: 2500, removed: 2500 });
  });

  it("counts changes and folds long unchanged runs", () => {
    const before = Array.from({ length: 30 }, (_, i) => `l${i}`);
    const after = before.slice();
    after[15] = "changed";
    const d = diffLines(before, after);
    expect(diffCounts(d)).toEqual({ added: 1, removed: 1 });
    const folded = foldUnchanged(d, 2);
    expect(folded.filter((l) => l.op === "…").map((l) => l.text)).toEqual(["13", "12"]);
    expect(folded.length).toBe(8);
    expect(foldUnchanged([{ op: " ", text: "x" }])).toEqual([{ op: " ", text: "x" }]);
  });
});

describe("what the page says about a change", () => {
  it("turns a warning code and its parameters into a sentence", () => {
    expect(warningText({ code: "replaces", params: { file: "claude (user)", name: "docs" } })).toBe("claude (user) already has a server named docs: it is replaced.");
    expect(warningText({ code: "already_linked", params: {} })).toContain("already the same file");
    expect(warningText({ code: "something_new", params: {} })).toBe("something_new");
  });

  it("explains why a registry entry cannot be saved", () => {
    expect(registryProblem("package_name")).toContain("cannot be pinned");
    expect(registryProblem("version_not_exact")).toContain("no exact version");
    expect(registryProblem("odd")).toBe("odd");
  });

  it("quotes only the arguments whose boundaries would otherwise be lost", () => {
    expect([quoteArg("-y"), quoteArg("C:\\Program Files\\x"), quoteArg(""), quoteArg("a\"b c"), quoteArg("--root=/x")]).toEqual(["-y", "\"C:\\Program Files\\x\"", "\"\"", "\"a\\\"b c\"", "--root=/x"]);
  });
});

describe("arguments", () => {
  it("splits on spaces and keeps quoted text together", () => {
    expect(splitArgs("-y @acme/files")).toEqual(["-y", "@acme/files"]);
    expect(splitArgs("--root \"C:\\Program Files\\x y\" 'a b'")).toEqual(["--root", "C:\\Program Files\\x y", "a b"]);
    expect(splitArgs('  a   "" b ')).toEqual(["a", "", "b"]);
    expect(splitArgs("")).toEqual([]);
  });
});

describe("registry", () => {
  const entry: RegistryAgent = {
    id: "alpha", name: "Alpha", version: "1.0.0", description: "", repository: null, distribution: "npx", command: ["npx", "-y", "alpha@1.0.0", "--acp"], env_keys: [], pinned: "1.0.0",
    def: { id: "acp-alpha", label: "Alpha", bin: "npx", acp: ["-y", "alpha@1.0.0", "--acp"] }, problem: null,
  };

  it("shows the whole command and saves only what can run", () => {
    expect(registryCommandLine(entry)).toBe("npx -y alpha@1.0.0 --acp");
    expect(savedFromRegistry(entry)).toEqual({ id: "acp-alpha", label: "Alpha", bin: "npx", acp: ["-y", "alpha@1.0.0", "--acp"], version: "1.0.0", registryId: "alpha" });
    expect(savedFromRegistry({ ...entry, def: null, problem: "binary" })).toBeNull();
  });

  it("formats sizes", () => {
    expect([sizeText(12), sizeText(2048), sizeText(3 << 20)]).toEqual(["12 B", "2 KB", "3.0 MB"]);
  });
});
