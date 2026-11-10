import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { headersFile, hostedOrigin, NOT_FOUND_PAGE } from "./phone/security-headers.ts";
import { phoneDictionary } from "./phone/vite-dictionary.ts";

// The phone client (`app/phone`): its own build, served by the daemon to phones and bundled by the native
// shell later. `pnpm build` stays the desktop's only (Tauri's `frontendDist` is `../dist`); this one writes
// `dist-phone/`, which `crates/kelpied/build.rs` embeds in the daemon. Nothing here may import `@tauri-apps/*`.
// `--mode hosted` builds the same app for the relay's own origin (`__HOSTED__`) into `dist-phone-hosted/`, with
// Cloudflare's `_headers` and a static `404.html`; that one is never embedded in the daemon.
const here = path.dirname(fileURLToPath(import.meta.url));
const phone = path.join(here, "phone");

/** The phone is not the Tauri app: a desktop-only module reaching this bundle (through the chat model or an adapter) fails the build. */
function noDesktopOnly(): Plugin {
  return {
    name: "phone-no-desktop-only",
    // Before Vite's own resolver, which would answer a relative import first.
    enforce: "pre",
    resolveId(source, importer) {
      const ipc = /(^|\/)lib\/ipc(\.ts)?$/.test(source);
      if (source.startsWith("@tauri-apps/") || ipc) this.error(`the phone bundle must not import ${source} (from ${importer ?? "?"}): keep it behind chat/transport.ts`);
      return null;
    },
  };
}

/** Files of the hosted bundle that are no part of the app the worker keeps: Cloudflare's rules and the page for unknown paths. */
const NOT_SHELL = new Set(["_headers", "404.html"]);

/** The service worker is a build entry of its own with a fixed name; the files it keeps are listed in it once the rest is known. */
function serviceWorker(): Plugin {
  return {
    name: "phone-service-worker",
    generateBundle(_, bundle) {
      const files = Object.keys(bundle).filter((f) => f !== "sw.js" && !f.endsWith(".map") && !NOT_SHELL.has(f)).map((f) => `/${f}`);
      const shell = ["/", "/icon.svg", "/icon-192.png", ...files.filter((f) => f !== "/index.html")];
      const version = createHash("sha256").update(shell.join("\n")).digest("hex").slice(0, 10);
      const sw = bundle["sw.js"];
      if (sw?.type !== "chunk") throw new Error("the service worker entry was not built");
      sw.code = sw.code.replace("__PRECACHE__", JSON.stringify(shell)).replace("__VERSION__", JSON.stringify(version));
    },
  };
}

/** The hosted bundle's own files: `_headers` (from security-headers.ts) and the page for unknown paths. */
function hostedFiles(origin: string): Plugin {
  return {
    name: "phone-hosted-files",
    generateBundle() {
      this.emitFile({ type: "asset", fileName: "_headers", source: headersFile(origin) });
      this.emitFile({ type: "asset", fileName: "404.html", source: NOT_FOUND_PAGE });
    },
  };
}

/** What only the hosted app does must not reach the daemon's bundle: its reach probe is the plainest sign that it did. */
function noHostedCode(): Plugin {
  return {
    name: "phone-no-hosted-code",
    generateBundle(_, bundle) {
      for (const [name, file] of Object.entries(bundle)) {
        if (file.type === "chunk" && file.code.includes("/v1/health")) this.error(`hosted-only code reached the daemon's phone bundle (${name})`);
      }
    },
  };
}

/** The app's version: `KELPIE_APP_VERSION` when set (to make a second, different bundle), else the frontend manifest's. */
function appVersion(): string {
  const set = process.env.KELPIE_APP_VERSION?.trim();
  if (set) return set;
  return (JSON.parse(fs.readFileSync(path.join(here, "package.json"), "utf8")) as { version: string }).version;
}

export default defineConfig(({ mode }) => {
  const hosted = mode === "hosted";
  return {
    root: phone,
    base: "/",
    plugins: [noDesktopOnly(), solid(), phoneDictionary(), serviceWorker(), hosted ? hostedFiles(hostedOrigin(process.env.KELPIE_HOSTED_ORIGIN)) : noHostedCode()],
    clearScreen: false,
    define: { __HOSTED__: JSON.stringify(hosted), __APP_VERSION__: JSON.stringify(appVersion()) },
    resolve: {
      // The chat model and adapters are shared with the desktop and import its translator by path: the phone
      // build points that at its own, which carries only the strings this bundle uses.
      alias: [{ find: /^\.\.\/lib\/i18n$/, replacement: path.join(phone, "src", "i18n.ts") }],
    },
    build: {
      outDir: path.join(here, hosted ? "dist-phone-hosted" : "dist-phone"),
      emptyOutDir: true,
      target: ["es2022", "safari16.4", "chrome120"],
      chunkSizeWarningLimit: 400,
      rollupOptions: {
        input: { index: path.join(phone, "index.html"), sw: path.join(phone, "src", "sw.ts") },
        output: {
          entryFileNames: (chunk) => (chunk.name === "sw" ? "sw.js" : "assets/[name]-[hash].js"),
        },
      },
    },
  };
});
