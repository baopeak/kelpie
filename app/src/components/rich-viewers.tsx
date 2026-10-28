// Viewers for files that read better rendered: CSV / TSV tables, PDF, Jupyter notebooks and
// Mermaid diagrams (standalone `.mmd` or ```mermaid blocks in markdown).
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { readFile } from "../lib/ipc";
import { store } from "../lib/store";
import type { Project } from "../lib/types";
import { intlLocale, t } from "../lib/i18n";

const MAX_ROWS = 5000;

/** RFC 4180 parser: quoted fields, doubled quotes, newlines inside quotes. */
export function parseDelimited(text: string, sep: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        field += '"';
        i++;
      } else if (c === '"') quoted = false;
      else field += c;
    } else if (c === '"' && field === "") quoted = true;
    else if (c === sep) {
      row.push(field);
      field = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
    } else field += c;
  }
  if (field !== "" || row.length) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

/** `text`: the content when it is not the file on disk (a file as it was at a turn). */
export function CsvView(props: { project: Project; path: string; text?: string }) {
  const [rows, setRows] = createSignal<string[][]>([]);
  const [query, setQuery] = createSignal("");
  const [sort, setSort] = createSignal<{ col: number; desc: boolean } | null>(null);
  onMount(async () => {
    const text = props.text ?? (await readFile(props.project.path, props.path, 20 << 20).catch((e) => (store.notify(String(e)), null)))?.text;
    setRows(parseDelimited(text ?? "", props.path.toLowerCase().endsWith(".tsv") ? "\t" : ","));
  });
  const header = () => rows()[0] ?? [];
  const body = createMemo(() => {
    const q = query().trim().toLowerCase();
    let list = rows().slice(1);
    if (q) list = list.filter((r) => r.some((c) => c.toLowerCase().includes(q)));
    const s = sort();
    if (s) {
      const num = list.every((r) => r[s.col] === undefined || r[s.col] === "" || !isNaN(+r[s.col]));
      list = [...list].sort((a, b) => {
        const x = a[s.col] ?? "", y = b[s.col] ?? "";
        const d = num ? +x - +y : x.localeCompare(y, intlLocale(), { numeric: true });
        return s.desc ? -d : d;
      });
    }
    return list;
  });
  const clickHeader = (col: number) => setSort((s) => (s?.col === col ? (s.desc ? null : { col, desc: true }) : { col, desc: false }));
  return (
    <div class="table-view">
      <div class="table-tools">
        <input class="text-input" placeholder={t("Filter rows")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
        <span class="muted-inline">{t("{shown} of {total} rows", { shown: body().length, total: Math.max(0, rows().length - 1) })}{body().length > MAX_ROWS ? ` ${t("(first {n} shown)", { n: MAX_ROWS })}` : ""}</span>
      </div>
      <div class="table-scroll">
        <table class="data-table">
          <thead>
            <tr>
              <For each={header()}>
                {(h, i) => <th onClick={() => clickHeader(i())}>{h}{sort()?.col === i() ? (sort()!.desc ? " ▾" : " ▴") : ""}</th>}
              </For>
            </tr>
          </thead>
          <tbody>
            <For each={body().slice(0, MAX_ROWS)}>{(r) => <tr><For each={r}>{(c) => <td title={c}>{c}</td>}</For></tr>}</For>
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function PdfView(props: { project: Project; path: string }) {
  const [url, setUrl] = createSignal<string | null>(null);
  onMount(async () => {
    try {
      const f = await readFile(props.project.path, props.path, 60 << 20, true);
      const bin = atob(f.base64 ?? "");
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      setUrl(URL.createObjectURL(new Blob([bytes], { type: "application/pdf" })));
    } catch (e) {
      store.notify(String(e));
    }
  });
  onCleanup(() => url() && URL.revokeObjectURL(url()!));
  return (
    <Show when={url()} fallback={<div class="muted">{t("Loading…")}</div>}>
      <iframe class="pdf-frame" src={url()!} title={props.path} />
    </Show>
  );
}

// ---------- Mermaid ----------
let mermaidReady: Promise<typeof import("mermaid").default> | null = null;
let diagramSeq = 0;

function loadMermaid() {
  mermaidReady ??= import("mermaid").then((m) => m.default);
  return mermaidReady;
}

export async function renderMermaid(source: string): Promise<string> {
  const mermaid = await loadMermaid();
  mermaid.initialize({ startOnLoad: false, securityLevel: "strict", theme: store.state.settings.theme === "light" ? "default" : "dark" });
  const { svg } = await mermaid.render(`kelpie-mmd-${++diagramSeq}`, source);
  return DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true }, ADD_TAGS: ["foreignObject"] });
}

/** Replaces ```mermaid code blocks inside rendered markdown with diagrams. */
export async function renderMermaidBlocks(root: HTMLElement) {
  for (const code of Array.from(root.querySelectorAll<HTMLElement>("pre > code.language-mermaid"))) {
    const holder = document.createElement("div");
    holder.className = "mermaid-diagram";
    try {
      holder.innerHTML = await renderMermaid(code.textContent ?? "");
      code.parentElement!.replaceWith(holder);
    } catch (e) {
      code.parentElement!.title = `Mermaid: ${String(e)}`;
    }
  }
}

export function MermaidView(props: { project: Project; path: string; text?: string }) {
  const [svg, setSvg] = createSignal("");
  const [error, setError] = createSignal("");
  const [zoom, setZoom] = createSignal(1);
  onMount(async () => {
    try {
      setSvg(await renderMermaid(props.text ?? (await readFile(props.project.path, props.path, 1 << 20)).text ?? ""));
    } catch (e) {
      setError(String(e));
    }
  });
  return (
    <div class="diagram-view" onWheel={(e) => { if (e.ctrlKey || e.metaKey) { e.preventDefault(); setZoom((z) => Math.min(4, Math.max(0.25, z * (e.deltaY < 0 ? 1.1 : 0.9)))); } }}>
      <Show when={!error()} fallback={<div class="muted">{error()}</div>}>
        <div class="diagram-tools">
          <button class="btn" onClick={() => setZoom((z) => Math.max(0.25, z / 1.2))}>−</button>
          <span class="muted-inline">{Math.round(zoom() * 100)}%</span>
          <button class="btn" onClick={() => setZoom((z) => Math.min(4, z * 1.2))}>+</button>
          <button class="btn" onClick={() => setZoom(1)}>{t("Reset")}</button>
        </div>
        <div class="diagram-canvas" style={{ transform: `scale(${zoom()})` }} innerHTML={svg()} />
      </Show>
    </div>
  );
}

// ---------- Jupyter ----------
interface NbOutput { output_type: string; text?: string | string[]; data?: Record<string, string | string[]>; ename?: string; evalue?: string }
interface NbCell { cell_type: "markdown" | "code" | "raw"; source: string | string[]; outputs?: NbOutput[]; execution_count?: number | null }

const joined = (v: string | string[] | undefined) => (Array.isArray(v) ? v.join("") : v ?? "");

function Output(props: { o: NbOutput }) {
  const d = props.o.data ?? {};
  if (d["image/png"]) return <img class="nb-image" src={`data:image/png;base64,${joined(d["image/png"]).replace(/\s/g, "")}`} alt={t("output")} />;
  if (d["image/jpeg"]) return <img class="nb-image" src={`data:image/jpeg;base64,${joined(d["image/jpeg"]).replace(/\s/g, "")}`} alt={t("output")} />;
  if (d["image/svg+xml"]) return <div class="nb-html" innerHTML={DOMPurify.sanitize(joined(d["image/svg+xml"]), { USE_PROFILES: { svg: true } })} />;
  if (d["text/html"]) return <div class="nb-html" innerHTML={DOMPurify.sanitize(joined(d["text/html"]))} />;
  if (props.o.output_type === "error") return <pre class="nb-out nb-err">{`${props.o.ename}: ${props.o.evalue}`}</pre>;
  const text = props.o.text ?? d["text/plain"];
  return <pre class="nb-out">{joined(text)}</pre>;
}

export function NotebookView(props: { project: Project; path: string }) {
  const [cells, setCells] = createSignal<NbCell[]>([]);
  const [error, setError] = createSignal("");
  onMount(async () => {
    try {
      const f = await readFile(props.project.path, props.path, 40 << 20);
      setCells(JSON.parse(f.text ?? "{}").cells ?? []);
    } catch (e) {
      setError(t("Not a readable notebook: {error}", { error: String(e) }));
    }
  });
  return (
    <div class="notebook">
      <Show when={error()}><div class="muted">{error()}</div></Show>
      <For each={cells()}>
        {(c) => (
          <div class={`nb-cell nb-${c.cell_type}`}>
            <Show when={c.cell_type === "markdown"} fallback={
              <>
                <div class="nb-prompt">[{c.execution_count ?? " "}]</div>
                <div class="nb-body">
                  <pre class="nb-code">{joined(c.source)}</pre>
                  <For each={c.outputs ?? []}>{(o) => <Output o={o} />}</For>
                </div>
              </>
            }>
              <div class="markdown nb-md" innerHTML={DOMPurify.sanitize(marked.parse(joined(c.source), { gfm: true, async: false }) as string)} />
            </Show>
          </div>
        )}
      </For>
    </div>
  );
}
