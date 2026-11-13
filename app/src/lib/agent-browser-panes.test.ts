import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BROWSER_FEATURES, NO_PANE, PRIVACY_WAIT_MS, addSite, browserFeatures, callContext, decidePrivate, isLoopback, openedForSession, planCall, paneCapabilities, parseSite, refusal, resolvePane, sharedOrigins, tabLine, tabsReply, toAuditEntries,
  toBrowserPolicy, usablePanes, webOrigin, withPrivacy, type BrowserMode, type PaneShare, type PrivacyInputs, type SessionRow, type UsablePane,
} from "./agent-browser-panes";
import { isWindows } from "./platform";
import type { LayoutNode, Pane, Project, Workspace } from "./types";

const leaf = (pane: Pane): LayoutNode => ({ type: "leaf", id: `l-${pane.id}`, pane });
const browser = (id: string, url?: string, openedBy?: string): Pane => ({ id, kind: "browser", ...(url ? { url } : {}), ...(openedBy ? { openedBy } : {}) });

const projects = [{ id: "shop", name: "Shop" }, { id: "docs", name: "Docs" }] as Project[];
const workspaces: Record<string, Workspace> = {
  shop: {
    activeTab: "t1",
    rightPanel: null,
    tabs: [
      { id: "t1", focus: "term", root: { type: "split", id: "s", dir: "row", sizes: [0.5, 0.5], children: [leaf({ id: "term", kind: "terminal", sessionId: "lead" }), leaf(browser("dev", "http://localhost:5173/", "lead"))] } },
      { id: "t2", focus: "kid", root: leaf(browser("kid", "http://localhost:3000/", "child")) },
      { id: "t3", focus: "mine", root: leaf(browser("mine", "https://accounts.example.com/login")) },
      { id: "t4", focus: "blank", root: leaf(browser("blank", undefined, "lead")) },
      { id: "t5", focus: "file", root: leaf(browser("file", "file:///C:/secret.txt", "lead")) },
    ],
  },
  docs: { activeTab: "d1", rightPanel: null, tabs: [{ id: "d1", focus: "other", root: leaf(browser("other", "https://docs.example.com/", "elsewhere")) }, { id: "d2", focus: "theirs", root: leaf(browser("theirs", "https://docs.example.com/a")) }] },
};
const workspace = (id: string) => workspaces[id] ?? { tabs: [], activeTab: null, rightPanel: null };
const sessionRows: Record<string, SessionRow> = {
  lead: { project_id: "shop" },
  child: { project_id: "shop", parent: "lead" },
  grandchild: { project_id: "shop-worktree", parent: "child" },
  peer: { project_id: "shop" },
  elsewhere: { project_id: "docs" },
};
const sessions = (id: string) => sessionRows[id];
const pages = { dev: { title: "Studio", url: "http://localhost:5173/admin" }, mine: { title: "Sign in", url: "https://accounts.example.com/login" } };
const none: Readonly<Record<string, PaneShare | undefined>> = {};
const ids = (sessionId: string, shared = none, stopped: ReadonlySet<string> = new Set()) =>
  usablePanes(sessionId, projects, workspace, sessions, pages, shared, stopped).map((p) => `${p.id}:${p.kind}`);

describe("which panes a session may use", () => {
  it("gives a session the panes opened for it, with the page it is on now", () => {
    const [dev] = usablePanes("lead", projects, workspace, sessions, pages, none, new Set());
    expect(dev).toMatchObject({ id: "dev", label: "browser-dev", projectId: "shop", tabId: "t1", title: "Studio", url: "http://localhost:5173/admin", kind: "opened", origins: [] });
    expect(ids("lead")).toEqual(["dev:opened"]);
  });

  it("gives a sub-session the panes opened for the sessions above it, but not the other way round", () => {
    expect(ids("child")).toEqual(["dev:opened", "kid:opened"]);
    expect(ids("grandchild")).toEqual(["dev:opened", "kid:opened"]);
    expect(ids("lead")).not.toContain("kid:opened");
  });

  it("gives another session of the same project nothing that was not shared", () => {
    expect(ids("peer")).toEqual([]);
    expect(ids("nobody")).toEqual([]);
  });

  it("gives a shared pane to the sessions of its project only", () => {
    const shared = { mine: { origins: ["https://accounts.example.com"] }, theirs: { origins: ["https://docs.example.com"] } };
    expect(ids("peer", shared)).toEqual(["mine:shared"]);
    expect(ids("elsewhere", shared)).toEqual(["other:opened", "theirs:shared"]);
    const [mine] = usablePanes("peer", projects, workspace, sessions, pages, shared, new Set());
    expect(mine.origins).toEqual(["https://accounts.example.com"]);
  });

  it("treats a pane opened for the session as its own even when it is shared too", () => {
    expect(ids("lead", { dev: { origins: ["http://localhost:5173"] } })).toEqual(["dev:opened"]);
  });

  it("leaves out a pane the user stopped agents in", () => {
    expect(ids("child", none, new Set(["dev"]))).toEqual(["kid:opened"]);
    expect(ids("peer", { mine: { origins: [] } }, new Set(["mine"]))).toEqual([]);
  });

  it("leaves out panes with no web page", () => {
    const all = ids("lead", { blank: { origins: [] }, file: { origins: [] } });
    expect(all).not.toContain("blank:opened");
    expect(all).not.toContain("file:opened");
  });

  it("says which panes are on screen and which a phone controls", () => {
    const [dev, kid] = usablePanes("child", projects, workspace, sessions, pages, none, new Set(), {
      shown: (_p, tabId) => workspace("shop").activeTab === tabId,
      held: { kid: { tk: "1" } },
    });
    expect([dev.shown, dev.held, kid.shown, kid.held]).toEqual([true, false, false, true]);
  });

  it("stops at a loop in the sessions' parents", () => {
    const loop = (id: string): SessionRow | undefined => ({ a: { project_id: null, parent: "b" }, b: { project_id: null, parent: "a" } })[id];
    expect(openedForSession("c", "a", loop)).toBe(false);
    expect(openedForSession("b", "a", loop)).toBe(true);
  });
});

const pane = (id: string, extra: Partial<UsablePane> = {}): UsablePane => ({
  id, label: `browser-${id}`, projectId: "shop", tabId: "t", title: "T", url: "http://localhost:1/", kind: "opened", origins: [], offSite: false, shown: false, held: false, isolated: false, ...extra,
});

describe("which pane a tool call is about", () => {
  it("takes the only usable pane when none is named", () => {
    expect(resolvePane(undefined, [pane("a")])).toEqual({ pane: pane("a") });
    expect(resolvePane("", [pane("a")])).toEqual({ pane: pane("a") });
  });

  it("asks for a pane when several are usable, listing them", () => {
    expect(resolvePane(undefined, [pane("a"), pane("b")])).toEqual({ error: "Several panes are usable; pass pane: a, b." });
  });

  it("says how to get a pane when none is usable", () => {
    expect(resolvePane(undefined, [])).toEqual({ error: NO_PANE });
  });

  it("finds a named pane by its id or its webview's label", () => {
    expect(resolvePane("b", [pane("a"), pane("b")])).toEqual({ pane: pane("b") });
    expect(resolvePane("browser-b", [pane("a"), pane("b")])).toEqual({ pane: pane("b") });
  });

  it("tells a pane of someone else from one that does not exist, and lists the usable ones", () => {
    const theirs = resolvePane("x", [pane("a")], (id) => id === "x");
    expect(theirs).toEqual({ error: "Pane x is neither opened for this session nor shared in its project. Usable: a." });
    const gone = resolvePane("y", [pane("a")], () => false);
    expect(gone).toEqual({ error: "There is no browser pane y. Usable: a." });
    const nothing = resolvePane("y", [], () => false);
    expect("error" in nothing && nothing.error).toContain("kelpie_open_page");
  });

  it("refuses a pane a phone controls or the user is picking elements in", () => {
    expect(refusal(pane("a", { held: true }), {})).toContain("phone");
    expect(refusal(pane("a"), { design: true })).toContain("design mode");
    expect(refusal(pane("a"), { design: false })).toBeNull();
  });
});

describe("the sites a share records", () => {
  it("captures the origin of the page shown", () => {
    expect(sharedOrigins("https://Accounts.Example.com:443/login?next=/")).toEqual(["https://accounts.example.com"]);
    expect(sharedOrigins("http://localhost:5173/admin")).toEqual(["http://localhost:5173"]);
  });

  it("captures nothing for a page that is not on the web", () => {
    expect(sharedOrigins("about:blank")).toEqual([]);
    expect(sharedOrigins("")).toEqual([]);
    expect(sharedOrigins(undefined)).toEqual([]);
    expect(webOrigin("not a url")).toBeNull();
  });
});

describe("what an agent can do in a pane", () => {
  const all = { trustedInput: true, screenshot: true, network: true };

  it("has trusted input and the network list on Windows only, screenshots everywhere", () => {
    expect(browserFeatures(true)).toEqual({ trustedInput: true, screenshot: true, network: true });
    expect(browserFeatures(false)).toEqual({ trustedInput: false, screenshot: true, network: false });
    expect(paneCapabilities(true, browserFeatures(false))).toEqual({ input: "synthetic", screenshot: true, network: false });
    expect(paneCapabilities(false, browserFeatures(false))).toEqual({ input: "synthetic", screenshot: false, network: false });
    expect(BROWSER_FEATURES).toEqual(browserFeatures(isWindows));
  });

  it("gives trusted input and screenshots only in a pane that is shown", () => {
    expect(paneCapabilities(true, all)).toEqual({ input: "trusted", screenshot: true, network: true });
    expect(paneCapabilities(false, all)).toEqual({ input: "synthetic", screenshot: false, network: true });
  });
});

describe("the tabs an agent is shown", () => {
  it("writes one line per pane", () => {
    const panes = [pane("dev", { title: "Studio", url: "http://localhost:5173/", shown: true }), pane("mine", { title: 'Say "hi"\n', url: "https://a.example/", kind: "shared", held: true })];
    // On macOS and Linux: screenshots of the tab shown.
    expect(tabsReply(panes, browserFeatures(false)).text.split("\n")).toEqual([
      'dev "Studio" http://localhost:5173/ — opened by you, shown, input: synthetic, screenshot: yes, network: no',
      'mine "Say \\"hi\\"" https://a.example/ — shared, background tab, input: synthetic, screenshot: no, network: no, a phone controls it',
    ]);
    // On Windows: trusted input and screenshots in the tab shown, the network list in both.
    expect(tabsReply(panes, browserFeatures(true)).text.split("\n")).toEqual([
      'dev "Studio" http://localhost:5173/ — opened by you, shown, input: trusted, screenshot: yes, network: yes',
      'mine "Say \\"hi\\"" https://a.example/ — shared, background tab, input: synthetic, screenshot: no, network: yes, a phone controls it',
    ]);
  });

  it("gives the same panes as data", () => {
    const reply = tabsReply([pane("dev", { shown: true })], { trustedInput: true, screenshot: true, network: false });
    expect(reply.data.tabs).toEqual([{ pane: "dev", title: "T", url: "http://localhost:1/", kind: "opened", shown: true, held: false, input: "trusted", screenshot: true, network: false, isolated: false }]);
    expect(tabLine(reply.data.tabs[0])).toBe('dev "T" http://localhost:1/ — opened by you, shown, input: trusted, screenshot: yes, network: no');
  });

  it("says how to get a pane when there is none", () => {
    expect(tabsReply([])).toEqual({ text: NO_PANE, data: { tabs: [] } });
  });

  it("says which panes have a private page", () => {
    const reply = tabsReply([pane("dev", { isolated: true }), pane("mine", { kind: "shared", held: true })], browserFeatures(false));
    expect(reply.text.split("\n")).toEqual([
      'dev "T" http://localhost:1/ — opened by you, background tab, input: synthetic, screenshot: no, network: no, private profile',
      'mine "T" http://localhost:1/ — shared, background tab, input: synthetic, screenshot: no, network: no, a phone controls it',
    ]);
    expect(reply.data.tabs.map((e) => [e.pane, e.isolated])).toEqual([["dev", true], ["mine", false]]);
  });
});

describe("whether a pane's page is private", () => {
  it("expects a private page for a pane opened for an agent, whichever session lists it", () => {
    const shared = { dev: { origins: ["http://localhost:5173"] }, mine: { origins: ["https://accounts.example.com"] } };
    const [dev] = usablePanes("lead", projects, workspace, sessions, pages, none, new Set());
    const listed = usablePanes("peer", projects, workspace, sessions, pages, shared, new Set());
    expect(dev.isolated).toBe(true);
    expect(listed.map((p) => [p.id, p.kind, p.isolated])).toEqual([["dev", "shared", true], ["mine", "shared", false]]);
  });

  it("takes Rust's word for the pages that exist", () => {
    const panes = [pane("made", { isolated: true }), pane("before", { isolated: true }), pane("later", { isolated: true }), pane("person")];
    const made = { "browser-made": true, "browser-before": false, "browser-person": false };
    expect(withPrivacy(panes, made).map((p) => [p.id, p.isolated])).toEqual([["made", true], ["before", false], ["later", true], ["person", false]]);
  });

  it("lists the panes with Rust's word in tabs", () => {
    const panes = [pane("a", { isolated: true }), pane("b", { isolated: true })];
    const plan = planCall("tabs", {}, panes, { made: { "browser-a": false } });
    expect("answer" in plan && plan.answer.data.tabs.map((e) => e.isolated)).toEqual([false, true]);
  });
});

describe("the browser policy in Settings", () => {
  it("reads the daemon's keys", () => {
    expect(toBrowserPolicy({ sub_sessions: "ask", browser_agents: "act", browser_eval: true, browser_sites: ["https://a.example", 3] })).toEqual({ mode: "act", eval: true, sites: ["https://a.example"] });
    expect(toBrowserPolicy({ browser_agents: "read" })).toEqual({ mode: "read", eval: false, sites: [] });
  });

  it("has nothing from a daemon without them", () => {
    expect(toBrowserPolicy({ sub_sessions: "ask" })).toBeNull();
    expect(toBrowserPolicy({ browser_agents: "all" })).toBeNull();
    expect(toBrowserPolicy(null)).toBeNull();
  });

  it("takes a site as an http(s) origin", () => {
    expect(parseSite(" https://Example.com/ ")).toEqual({ origin: "https://example.com" });
    expect(parseSite("http://localhost:8080")).toEqual({ origin: "http://localhost:8080" });
    expect(parseSite("https://example.com:443")).toEqual({ origin: "https://example.com" });
    expect(parseSite("example.com")).toEqual({ error: "scheme" });
    expect(parseSite("ftp://example.com")).toEqual({ error: "scheme" });
    expect(parseSite("https://example.com/app")).toEqual({ error: "path" });
    expect(parseSite("https://example.com/?q=1")).toEqual({ error: "path" });
    expect(parseSite("https://user:pw@example.com")).toEqual({ error: "invalid" });
    expect(parseSite("https://")).toEqual({ error: "invalid" });
  });

  it("adds a site once, up to the limit", () => {
    expect(addSite(["https://a.example"], "https://b.example")).toEqual({ sites: ["https://a.example", "https://b.example"] });
    expect(addSite(["https://a.example"], "https://A.example/")).toEqual({ error: "duplicate" });
    const full = Array.from({ length: 50 }, (_, i) => `https://s${i}.example`);
    expect(addSite(full, "https://new.example")).toEqual({ error: "full" });
    expect(addSite([], "nope")).toEqual({ error: "scheme" });
  });

  it("reads the log of agent actions, newest first, without malformed lines", () => {
    const entries = toAuditEntries({
      entries: [
        { at: 1000, session: "s1", label: "browser-a", tool: "click", host: "localhost:5173", target: 'button "Sign in"', ok: true, error: "" },
        { at: 2000, session: "s2", label: "browser-b", tool: "type", host: "a.example", target: 'textbox "Email"', ok: false, error: "refused" },
        { at: "x", tool: "click" },
        null,
      ],
    });
    expect(entries.map((e) => [e.at, e.tool, e.ok, e.error])).toEqual([[2000, "type", false, "refused"], [1000, "click", true, ""]]);
    expect(toAuditEntries({})).toEqual([]);
    expect(toAuditEntries(null)).toEqual([]);
  });
});

describe("a shared pane the user took to another site", () => {
  const shared = { mine: { origins: ["https://accounts.example.com"] } };
  const at = (url: string) => usablePanes("peer", projects, workspace, sessions, { mine: { title: "Bank", url } }, shared, new Set());

  it("stays usable on its shared site and on this computer", () => {
    expect(at("https://accounts.example.com/profile")[0].offSite).toBe(false);
    expect(at("http://localhost:3000/")[0].offSite).toBe(false);
    expect(at("http://127.0.0.1:8080/x")[0].offSite).toBe(false);
  });

  it("is not listed, nor its address or title told, on another site", () => {
    const panes = at("https://bank.example/account?token=abc");
    expect(panes[0].offSite).toBe(true);
    const reply = tabsReply(panes);
    expect(reply).toEqual({ text: NO_PANE, data: { tabs: [] } });
    expect(JSON.stringify(reply)).not.toContain("bank.example");
  });

  it("refuses calls in it with a clear reason, named or not", () => {
    const panes = at("https://bank.example/");
    expect(resolvePane("mine", panes)).toEqual({ error: "Pane mine is shared, but its page is now on another site; it is usable again once it is back." });
    expect(resolvePane(undefined, panes)).toEqual({ error: NO_PANE });
    expect(refusal(panes[0], {})).toContain("another site");
  });

  it("never counts a pane opened for the session as off site", () => {
    expect(usablePanes("lead", projects, workspace, sessions, { dev: { title: "", url: "https://anywhere.example/" } }, none, new Set())[0].offSite).toBe(false);
  });

  it("knows this computer's addresses", () => {
    for (const u of ["http://localhost:1/", "http://app.localhost/", "http://127.0.0.1/", "http://[::1]:3000/"]) expect(isLoopback(u), u).toBe(true);
    for (const u of ["https://example.com/", "http://10.0.0.2/", "about:blank", "http://localhost.example.com/"]) expect(isLoopback(u), u).toBe(false);
  });
});

describe("what the window tells Rust about a call", () => {
  const who = { id: "s1", name: "Claude" };

  it("lets scripts run only in a pane opened for the session, and only when the policy allows them", () => {
    expect(callContext(pane("a"), { eval: true }, who).mayEval).toBe(true);
    expect(callContext(pane("a"), { eval: false }, who).mayEval).toBe(false);
    expect(callContext(pane("a"), null, who).mayEval).toBe(false);
    expect(callContext(pane("b", { kind: "shared", origins: ["https://a.example"] }), { eval: true }, who).mayEval).toBe(false);
  });

  it("gives an opened pane the policy's sites and a shared pane its own sites", () => {
    const policy = { eval: false, sites: ["https://docs.example", 7] };
    expect(callContext(pane("a"), policy, who)).toEqual({
      session: "s1", sessionName: "Claude", kind: "opened", origins: [], sites: ["https://docs.example"], mayEval: false, project: "shop",
    });
    const shared = callContext(pane("b", { kind: "shared", origins: ["https://a.example"], projectId: "docs" }), policy, who);
    expect([shared.kind, shared.origins, shared.sites, shared.project]).toEqual(["shared", ["https://a.example"], ["https://docs.example"], "docs"]);
    expect(callContext(pane("a"), { mode: "act" }, who).sites).toEqual([]);
  });

  it("leaves whether the page is private to Rust, which made it", () => {
    expect(callContext(pane("a", { isolated: true }), { eval: true }, who)).not.toHaveProperty("isolated");
  });
});

describe("what the window does with a call", () => {
  it("answers tabs itself, also with no pane", () => {
    expect(planCall("tabs", {}, [])).toEqual({ answer: { text: NO_PANE, data: { tabs: [] } } });
    expect(planCall("tabs", { pane: "zz" }, [pane("a")])).toMatchObject({ answer: { data: { tabs: [{ pane: "a" }] } } });
  });

  it("runs another tool in the pane it is about", () => {
    expect(planCall("snapshot", {}, [pane("a")])).toEqual({ pane: pane("a") });
    expect(planCall("click", { pane: "b" }, [pane("a"), pane("b")])).toEqual({ pane: pane("b") });
  });

  it("refuses a pane a phone holds, one in design mode, and one it cannot find", () => {
    expect(planCall("snapshot", {}, [pane("a", { held: true })])).toMatchObject({ error: expect.stringContaining("phone") });
    expect(planCall("click", {}, [pane("a")], { design: (id) => id === "a" })).toMatchObject({ error: expect.stringContaining("design mode") });
    expect(planCall("click", { pane: "q" }, [pane("a")], { exists: () => false })).toEqual({ error: "There is no browser pane q. Usable: a." });
  });

  it("keeps every error within what the provider passes on", () => {
    const many = Array.from({ length: 30 }, (_, i) => pane(`pane${String(i).padStart(10, "0")}`));
    const errors = [
      resolvePane(undefined, many),
      resolvePane("x".repeat(12), many, () => true),
      resolvePane("x".repeat(12), [], () => true),
      resolvePane("y".repeat(12), [], () => false),
    ].map((r) => ("error" in r ? r.error : ""));
    for (const e of errors) expect(e.length, e).toBeLessThanOrEqual(300);
    expect(errors[0]).toContain(", …");
  });
});

describe("a restricted session", () => {
  const shared = { mine: { origins: ["https://accounts.example.com"] }, dev: { origins: ["http://localhost:5173"] } };
  const restricted = (sessionId: string) =>
    usablePanes(sessionId, projects, workspace, sessions, pages, shared, new Set(), { restricted: true }).map((p) => `${p.id}:${p.kind}`);

  it("uses the panes opened for it or a session above it", () => {
    expect(restricted("lead")).toEqual(["dev:opened"]);
    expect(restricted("child")).toEqual(["dev:opened", "kid:opened"]);
  });

  it("never uses a pane the user shared", () => {
    expect(restricted("peer")).toEqual([]);
    expect(ids("peer", shared)).toEqual(["dev:shared", "mine:shared"]);
  });

  it("is told about and given only its own panes", () => {
    const panes = usablePanes("peer", projects, workspace, sessions, pages, shared, new Set(), { restricted: true });
    expect(planCall("tabs", {}, panes)).toEqual({ answer: { text: NO_PANE, data: { tabs: [] } } });
    expect(planCall("snapshot", { pane: "mine" }, panes, { exists: () => true })).toMatchObject({ error: expect.stringContaining("neither opened for this session") });
  });
});
describe("whether a page made for an agent's pane is private", () => {
  const API = 39;
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());
  /** A window that connects at `connectAt` ms, learns the API level at `apiAt`, and has the policy read in `readMs`. */
  const fakeWindow = (o: { connectAt?: number; apiAt?: number; api?: number; mode?: BrowserMode | null; readMode?: BrowserMode | null; readMs?: number; readFails?: boolean } = {}) => {
    const t0 = Date.now();
    let mode = o.mode ?? null;
    const reads = { n: 0 };
    const io: PrivacyInputs = {
      connected: () => Date.now() - t0 >= (o.connectAt ?? 0),
      api: () => (Date.now() - t0 >= (o.apiAt ?? 0) ? (o.api ?? API) : 0),
      mode: () => mode,
      read: () => {
        reads.n++;
        return new Promise((ok, fail) => setTimeout(() => (o.readFails ? fail(new Error("no")) : ok((mode = o.readMode ?? null))), o.readMs ?? 0));
      },
    };
    return { io, reads };
  };
  const decide = async (io: PrivacyInputs, advance = PRIVACY_WAIT_MS + 1000) => {
    const out = decidePrivate(io, API);
    await vi.advanceTimersByTimeAsync(advance);
    return out;
  };

  it("is private while browser control is on, and not while it is off", async () => {
    expect(await decide(fakeWindow({ mode: "act" }).io)).toBe(true);
    expect(await decide(fakeWindow({ mode: "read" }).io)).toBe(true);
    expect(await decide(fakeWindow({ mode: "off" }).io)).toBe(false);
  });

  it("waits for the first connection and the daemon's API level at start, then reads the policy", async () => {
    const w = fakeWindow({ connectAt: 1500, apiAt: 2500, readMode: "act", readMs: 300 });
    expect(await decide(w.io)).toBe(true);
    expect(w.reads.n).toBe(1);
  });

  it("is not private when the daemon is not known within the wait, or has no browser control", async () => {
    expect(await decide(fakeWindow({ connectAt: PRIVACY_WAIT_MS + 500, mode: "act" }).io)).toBe(false);
    expect(await decide(fakeWindow({ apiAt: PRIVACY_WAIT_MS + 500, mode: "act" }).io)).toBe(false);
    expect(await decide(fakeWindow({ api: API - 1, mode: "act" }).io)).toBe(false);
  });

  it("does not wait for a policy read past the bound, and a failed read is not private", async () => {
    const slow = fakeWindow({ connectAt: 2000, readMode: "act", readMs: 40_000 });
    const out = decidePrivate(slow.io, API);
    let done = false;
    void out.then(() => (done = true));
    await vi.advanceTimersByTimeAsync(PRIVACY_WAIT_MS);
    expect(done).toBe(true);
    expect(await out).toBe(false);
    expect(await decide(fakeWindow({ readFails: true, readMode: "act" }).io)).toBe(false);
  });
});