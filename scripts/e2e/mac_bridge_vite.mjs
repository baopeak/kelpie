// The app's Vite dev server with a small bridge that lets a script run code in the page, for the Mac app, which has no WebDriver.
//
//     node scripts/e2e/mac_bridge_vite.mjs <app-dir>
//
// A debug Kelpie built with plain `cargo build` loads its page from this server (devUrl, port 1427). The bridge is a module the
// page loads from this same server, so the app's content policy ('self') lets it in; it talks over Vite's own HMR socket.
// POST /__shots {"code": "...", "args": [...], "timeout": s} runs `code` as the body of an async function in the page (its
// arguments are `args`, as WebDriver's execute) and answers {"value": ...} or {"error": "..."}; GET /__shots answers how many
// pages are connected. Each call's code is served as a module of its own (no eval). It listens on localhost only, as the app's
// own dev server does.
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

const appDir = path.resolve(process.argv[2] || "app");
const require = createRequire(path.join(appDir, "package.json"));
const vite = await import(pathToFileURL(require.resolve("vite")).href);
const createServer = vite.createServer ?? vite.default.createServer;

const CLIENT = `
if (import.meta.hot) {
  const hot = import.meta.hot;
  hot.on("shots:run", async ({ id, args }) => {
    let value = null, error = null;
    try {
      const mod = await import(/* @vite-ignore */ "/@id/__x00__shots-call-" + id);
      value = await mod.default.apply(null, args || []);
      value = value === undefined ? null : JSON.parse(JSON.stringify(value));
    } catch (e) {
      error = String((e && e.stack) || e);
    }
    hot.send("shots:done", { id, value, error });
  });
  hot.send("shots:hello", {});
}
`;

const calls = new Map();
let next = 1;
let pages = 0;

const bridge = {
  name: "kelpie-shots-bridge",
  apply: "serve",
  resolveId(id) {
    if (id === "virtual:shots-bridge") return "\0shots-bridge";
    if (id.startsWith("\0shots-call-") || id.startsWith("shots-call-")) return "\0" + id.replace(/^\0/, "");
    return null;
  },
  load(id) {
    if (id === "\0shots-bridge") return CLIENT;
    if (id.startsWith("\0shots-call-")) {
      const call = calls.get(Number(id.slice("\0shots-call-".length)));
      return `export default async function () {\n${call ? call.code : "throw new Error('no such call')"}\n}`;
    }
    return null;
  },
  transformIndexHtml() {
    return [{ tag: "script", attrs: { type: "module", src: "/@id/__x00__shots-bridge" }, injectTo: "head" }];
  },
  configureServer(server) {
    server.ws.on("shots:hello", () => { pages += 1; });
    server.ws.on("shots:done", (data) => {
      const call = calls.get(data.id);
      if (call && !call.answered) {
        call.answered = true;
        call.resolve(data);
      }
    });
    server.middlewares.use("/__shots", (req, res) => {
      const reply = (status, body) => {
        res.statusCode = status;
        res.setHeader("content-type", "application/json");
        res.end(JSON.stringify(body));
      };
      if (req.method === "GET") return reply(200, { pages });
      let body = "";
      req.on("data", (c) => { body += c; });
      req.on("end", () => {
        let job;
        try { job = JSON.parse(body); } catch { return reply(400, { error: "not JSON" }); }
        const id = next++;
        const done = new Promise((resolve) => calls.set(id, { code: String(job.code), resolve, answered: false }));
        const timer = setTimeout(() => calls.get(id)?.resolve({ error: "timed out" }), (job.timeout || 30) * 1000);
        server.ws.send({ type: "custom", event: "shots:run", data: { id, args: job.args || [] } });
        done.then((r) => {
          clearTimeout(timer);
          calls.delete(id);
          reply(200, r.error ? { error: r.error } : { value: r.value });
        });
      });
    });
  },
};

const server = await createServer({ root: appDir, configFile: path.join(appDir, "vite.config.ts"), plugins: [bridge] });
await server.listen();
console.log("vite ready " + server.resolvedUrls.local.join(" "));
