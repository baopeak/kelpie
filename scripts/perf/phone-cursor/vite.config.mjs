// The phone's screen view alone, for the cursor measurement (phone_cursor.py): the real screen-view.tsx and screen.ts, with the phone's state
// and words replaced by stubs here, so no computer, link or dictionary is needed. Served by the app's own Vite (app/node_modules).
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");
const app = path.join(root, "app");
const require = createRequire(path.join(app, "package.json"));
const solid = (await import(pathToFileURL(require.resolve("vite-plugin-solid")).href)).default;
const anchor = path.join(app, "phone", "src", "main.tsx");

/** The phone's state and words come from here; bare imports of the harness resolve as the app's own do. */
function stubs() {
  return {
    name: "phone-cursor-stubs",
    enforce: "pre",
    async resolveId(source, importer) {
      if (!importer) return null;
      const from = importer.replaceAll("\\", "/");
      if (from.includes("/app/phone/src/") && source === "./phone-state") return path.join(here, "phone-state-stub.ts");
      if (from.includes("/app/phone/src/") && source === "./i18n") return path.join(here, "i18n-stub.ts");
      if (from.includes("/scripts/perf/phone-cursor/") && !source.startsWith(".") && !source.startsWith("/")) return this.resolve(source, anchor, { skipSelf: true });
      return null;
    },
  };
}

export default {
  root: here,
  configFile: false,
  plugins: [stubs(), solid({ hot: false, dev: false })],
  define: { __HOSTED__: "false", __APP_VERSION__: JSON.stringify("harness") },
  optimizeDeps: { noDiscovery: true, include: [] },
  server: { port: Number(process.env.PHONE_CURSOR_PORT || 1447), strictPort: true, host: "127.0.0.1", fs: { allow: [root] } },
};
