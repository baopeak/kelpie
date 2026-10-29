// The Vietnamese strings the phone bundle needs: the desktop's dictionaries (app/src/locales/vi/*.json,
// where the English text is the key) cut down to the `t("…")` keys found in the files the phone bundle
// is made of. The phone's own strings live in app/src/locales/vi/phone.json like any other namespace;
// the chat adapters' strings come from wherever the desktop has them, so nothing is translated twice.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";

const APP = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const KEY = /\b(?:t|msg)\(\s*("(?:[^"\\\n]|\\.)*")/g;

/** Source files whose strings reach the phone: its own, and the shared chat model and adapters. */
export function phoneSources(): string[] {
  const out: string[] = [];
  const walk = (dir: string, skip: (name: string) => boolean) => {
    for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) walk(p, skip);
      else if (/\.(ts|tsx)$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) && !skip(e.name)) out.push(p);
    }
  };
  walk(path.join(APP, "phone", "src"), () => false);
  walk(path.join(APP, "src", "chat"), (n) => n === "mode-switch.ts" || n === "desktop-transport.ts" || n === "agent-settings.ts");
  out.push(path.join(APP, "src", "lib", "composer-core.ts"));
  return out;
}

export function usedKeys(files: string[]): Set<string> {
  const keys = new Set<string>();
  for (const f of files) {
    for (const m of fs.readFileSync(f, "utf8").matchAll(KEY)) keys.add(JSON.parse(m[1]));
  }
  return keys;
}

export function viDictionary(): Record<string, string> {
  const dir = path.join(APP, "src", "locales", "vi");
  const all: Record<string, string> = {};
  for (const f of fs.readdirSync(dir).filter((f) => f.endsWith(".json"))) Object.assign(all, JSON.parse(fs.readFileSync(path.join(dir, f), "utf8")));
  return all;
}

export function phoneDictionary(): Plugin {
  const id = "virtual:phone-vi";
  return {
    name: "phone-vi-dictionary",
    resolveId: (source) => (source === id ? "\0" + id : null),
    load(source) {
      if (source !== "\0" + id) return null;
      const all = viDictionary();
      const subset: Record<string, string> = {};
      for (const k of usedKeys(phoneSources())) if (k in all) subset[k] = all[k];
      return `export default ${JSON.stringify(subset)};`;
    },
  };
}
