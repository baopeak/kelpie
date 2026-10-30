import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import solid from "vite-plugin-solid";
import { phoneDictionary } from "./phone/vite-dictionary.ts";

// The phone client (`app/phone`): its own build, served by the daemon to phones and bundled by the native
// shell later. `pnpm build` stays the desktop's only (Tauri's `frontendDist` is `../dist`); this one writes
// `dist-phone/`, which `crates/kelpied/build.rs` embeds in the daemon. Nothing here may import `@tauri-apps/*`.
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

/** The service worker is a build entry of its own with a fixed name; the files it keeps are listed in it once the rest is known. */
function serviceWorker(): Plugin {
  return {
    name: "phone-service-worker",
    generateBundle(_, bundle) {
      const files = Object.keys(bundle).filter((f) => f !== "sw.js" && !f.endsWith(".map")).map((f) => `/${f}`);
      const shell = ["/", "/icon.svg", ...files.filter((f) => f !== "/index.html")];
      const version = createHash("sha256").update(shell.join("\n")).digest("hex").slice(0, 10);
      const sw = bundle["sw.js"];
      if (sw?.type !== "chunk") throw new Error("the service worker entry was not built");
      sw.code = sw.code.replace("__PRECACHE__", JSON.stringify(shell)).replace("__VERSION__", JSON.stringify(version));
    },
  };
}

export default defineConfig({
  root: phone,
  base: "/",
  plugins: [noDesktopOnly(), solid(), phoneDictionary(), serviceWorker()],
  clearScreen: false,
  resolve: {
    // The chat model and adapters are shared with the desktop and import its translator by path: the phone
    // build points that at its own, which carries only the strings this bundle uses.
    alias: [{ find: /^\.\.\/lib\/i18n$/, replacement: path.join(phone, "src", "i18n.ts") }],
  },
  build: {
    outDir: path.join(here, "dist-phone"),
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
});
