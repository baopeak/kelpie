// Checks the interface translations (see src/lib/i18n.ts):
//   - every t("…") / msg("…") key has a Vietnamese entry in src/locales/vi/*.json, with the same
//     {placeholders};
//   - keys are string literals (no template literals);
//   - likely untranslated interface text: JSX text, title / placeholder / aria-label attributes,
//     notify() / confirm() / alert() with a bare literal.
// Usage: node scripts/i18n-check.mjs [files…] [--unused]   (exit 1 when something is missing)
// A line with `i18n-ignore` is skipped (brand names, text that goes to a terminal or agent).
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const SRC = path.join(ROOT, "src");
// The phone client (app/phone) is checked like the desktop: its strings are in the same dictionaries.
const PHONE = path.join(ROOT, "phone", "src");
const args = process.argv.slice(2);
const onlyFiles = args.filter((a) => !a.startsWith("--")).map((f) => path.resolve(f));
const showUnused = args.includes("--unused");

function walk(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "locales" ? [] : walk(p);
    return /\.(ts|tsx)$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) && !e.name.endsWith(".d.ts") ? [p] : [];
  });
}

const vi = {};
for (const f of fs.readdirSync(path.join(SRC, "locales", "vi")).filter((f) => f.endsWith(".json"))) {
  const data = JSON.parse(fs.readFileSync(path.join(SRC, "locales", "vi", f), "utf8"));
  for (const [k, v] of Object.entries(data)) {
    if (k in vi && vi[k] !== v) console.warn(`note: ${JSON.stringify(k)} translated differently in ${f}`);
    vi[k] = v;
  }
}

// Text that is the same in every language.
const SAME = /^(Kelpie|kelpied|Claude|Claude Code|Codex|OMP|Gemini|GitHub|GitLab|Linear|Jira|VS Code|Cursor|SSH|QR|PR|MR|CLI|URL|API|JSON|HTTPS?|TLS|Git|English|Tiếng Việt|OK|[\d\s%:.,·×+\-–—/()#|→←↑↓⌘⇧⌥⏎↩…]*|[A-Z]?[a-z]?\+?[A-Z0-9]?)$/;

const placeholders = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
const KEY = /\b(?:t|msg)\(\s*("(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*')/g;
const TEMPLATE_KEY = /\b(?:t|msg)\(\s*`/g;
const JSX_TEXT = />([^<>{}`]*[A-Za-zÀ-ỹ]{2,}[^<>{}`]*)</g;
const ATTR = /\b(?:title|placeholder|aria-label)="([^"]*[A-Za-z]{2,}[^"]*)"/g;
const CALL = /\b(?:notify|confirm|alert)\(\s*("(?:[^"\\\n]|\\.)*"|`[^`$]*`)/g;

const used = new Set();
const missing = [];
const problems = [];
const untranslated = [];

for (const file of [...walk(SRC), ...walk(PHONE)]) {
  if (file.endsWith(path.join("lib", "i18n.ts"))) continue;
  const rel = path.relative(ROOT, file).replace(/\\/g, "/");
  const checkThis = !onlyFiles.length || onlyFiles.includes(file);
  const lines = fs.readFileSync(file, "utf8").split(/\r?\n/);
  lines.forEach((line, i) => {
    const at = `${rel}:${i + 1}`;
    for (const m of line.matchAll(KEY)) {
      const lit = m[1];
      const key = lit.startsWith("'") ? JSON.parse(`"${lit.slice(1, -1).replace(/\\'/g, "'").replace(/"/g, '\\"')}"`) : JSON.parse(lit);
      used.add(key);
      if (!checkThis) continue;
      if (!(key in vi)) missing.push(`${at}  ${JSON.stringify(key)}`);
      else if (placeholders(key) !== placeholders(vi[key])) problems.push(`${at}  placeholders differ: ${JSON.stringify(key)} → ${JSON.stringify(vi[key])}`);
    }
    if (!checkThis || line.includes("i18n-ignore") || /^\s*(\/\/|\*|\/\*)/.test(line)) return;
    if (TEMPLATE_KEY.test(line)) problems.push(`${at}  t() / msg() with a template literal: use "…{name}…" placeholders`);
    TEMPLATE_KEY.lastIndex = 0;
    if (file.endsWith(".tsx")) {
      for (const m of line.matchAll(JSX_TEXT)) {
        const text = m[1].trim();
        // Skip TypeScript generics and comparisons that look like tags.
        // Skip code that looks like text between `<` / `>` comparisons and generics.
        if (text && !SAME.test(text) && !/[=;()*]|&&|\|\||=>|^\s*[a-z]+\s*[|&]/.test(m[1]) && !/^[\w.]+(\[\])?$/.test(text)) untranslated.push(`${at}  text: ${JSON.stringify(text)}`);
      }
      // URLs and paths are examples, not text.
      for (const m of line.matchAll(ATTR)) if (!SAME.test(m[1]) && !/^(https?:\/\/|~|\/|[A-Za-z]:\\)/.test(m[1])) untranslated.push(`${at}  attribute: ${JSON.stringify(m[1])}`);
    }
    // A single word with no space is a protocol / method name (notify("session/cancel")).
    for (const m of line.matchAll(CALL)) if (/\s/.test(m[1])) untranslated.push(`${at}  call: ${m[0].slice(0, 80)}`);
  });
}

const section = (title, list) => list.length && console.log(`\n${title} (${list.length}):\n  ${list.join("\n  ")}`);
section("Missing Vietnamese translations", missing);
section("Problems", problems);
section("Probably untranslated interface text", untranslated);
if (showUnused) section("Dictionary entries no code uses", Object.keys(vi).filter((k) => !used.has(k)));
console.log(`\n${used.size} keys used · ${Object.keys(vi).length} translated · ${missing.length} missing · ${problems.length} problems · ${untranslated.length} untranslated-looking`);
process.exit(missing.length || problems.length || untranslated.length ? 1 : 0);
