// Status-bar meters: Claude / Codex subscription windows (5 h, week) and Kelpie's memory use.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { daemon, memoryStats, onEvent, usageFetch } from "../lib/ipc";
import { KindGlyph } from "./icons";
import { openContextMenu, type MenuItem } from "./context-menu";
import { accountsOf, activeAccount, setActiveAccount, type AccountAgent } from "../lib/accounts";
import type { MemoryStats, ProviderUsage, UsageReport, UsageWindow } from "../lib/types";
import { t, formatDateTime, formatDuration, formatTime } from "../lib/i18n";

const USAGE_EVERY = 5 * 60_000;
/** Clicks refresh at most this often: the usage endpoints are strictly rate limited. */
const USAGE_MIN_GAP = 30_000;
const MEMORY_EVERY = 5_000;

/** Runs `fn` now and every `ms` while the window is visible. */
function poll(fn: () => void, ms: number) {
  let timer = 0;
  const tick = () => {
    if (!document.hidden) fn();
    timer = window.setTimeout(tick, ms);
  };
  onMount(tick);
  onCleanup(() => clearTimeout(timer));
}

const level = (pct: number) => (pct >= 100 ? " full" : pct >= 80 ? " high" : "");

function resetText(at: number | null): string {
  if (!at) return "";
  return t("resets in {rel} ({when})", { rel: formatDuration(at - Date.now()), when: formatDateTime(at, { weekday: "short", hour: "2-digit", minute: "2-digit" }) });
}

/** Ticks once a minute so the inline countdowns stay current between usage refreshes. */
const [now, setNow] = createSignal(Date.now());

/** Compact time left until a window resets, for the status bar: "45m", "2h 13m", "4d 5h". */
function remainText(at: number | null): string {
  if (!at) return "";
  const m = Math.ceil((at - now()) / 60_000);
  if (m <= 0) return "";
  if (m < 60) return t("{n}m", { n: m });
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${t("{n}h", { n: h })} ${t("{n}m", { n: m % 60 })}` : t("{n}h", { n: h });
  const d = Math.floor(h / 24);
  return h % 24 ? `${t("{n}d", { n: d })} ${t("{n}h", { n: h % 24 })}` : t("{n}d", { n: d });
}

/** One toast per window and reset period when a limit is reached. */
const warned = new Set<string>();

/** What a meter shows: the latest windows, or the last good ones when a refresh failed. */
type Shown = ProviderUsage & { stale?: string; at?: number };
type ProviderId = "claude" | "codex";

/** Last successful windows per provider and account, kept across launches: a failed refresh
 *  (rate limit, network) keeps showing them, marked stale, instead of a bare "!". */
const CACHE_KEY = "kelpie-usage";
const accountKey = (p: ProviderId) => activeAccount(p)?.id ?? "default";
const CACHE_MAX_AGE = 12 * 3600_000;
type Cache = Partial<Record<string, { windows: UsageWindow[]; at: number }>>;
const cacheKey = (p: ProviderId) => (accountKey(p) === "default" ? p : `${p}:${accountKey(p)}`);

function loadCache(): Cache {
  try {
    return JSON.parse(localStorage.getItem(CACHE_KEY) ?? "{}");
  } catch {
    return {};
  }
}

function saveCache(c: Cache) {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(c));
  } catch {
    /* storage unavailable: only this session remembers */
  }
}

function shown(fresh: ProviderUsage | null, cached: Cache[string]): Shown | null {
  if (fresh?.windows.length) return fresh;
  const usable = cached && Date.now() - cached.at < CACHE_MAX_AGE;
  if (usable && (!fresh || (fresh.error && !fresh.error.startsWith("not signed in")))) {
    return { windows: cached.windows, error: null, stale: fresh?.error ?? "not refreshed yet", at: cached.at };
  }
  return fresh;
}

/** Right-click: which account new sessions of this agent use. */
function accountMenu(e: MouseEvent, agent: AccountAgent) {
  const current = activeAccount(agent)?.id ?? null;
  const items: MenuItem[] = [
    { label: `${current === null ? "● " : ""}${t("Default")}`, run: () => setActiveAccount(agent, null) },
    ...accountsOf(agent).map((a): MenuItem => ({ label: `${current === a.id ? "● " : ""}${a.label}`, run: () => setActiveAccount(agent, a.id) })),
    null,
    { label: t("Usage & cost…"), run: () => store.setOverlay("cost") },
    { label: t("Manage accounts…"), run: () => store.openSettings("accounts") },
  ];
  openContextMenu(e, items);
}

function Provider(props: { kind: ProviderId; name: string; usage: Shown | null }) {
  const u = (): Shown => props.usage ?? { windows: [], error: null };
  const who = () => (accountsOf(props.kind).length ? ` · ${activeAccount(props.kind)?.label ?? t("Default")}` : "");
  const hint = () => "\n" + t("Click to refresh · right-click to switch account");
  const title = () => {
    if (u().error) return t("{name}{who} usage: {error}", { name: props.name, who: who(), error: u().error! }) + hint();
    const lines = u().windows.map((w) => `${windowLabel(w.label)}: ${Math.round(w.used_percent)}% ${resetText(w.resets_at)}`);
    const stale = u().stale ? "\n" + t("Last refresh failed ({reason}); showing data from {time}.", { reason: t(u().stale!), time: formatTime(u().at!) }) : "";
    return t("{name}{who} usage", { name: props.name, who: who() }) + `\n${lines.join("\n")}${stale}${hint()}`;
  };
  return (
    <Show when={u().windows.length || (u().error && !u().error!.startsWith("not signed in"))}>
      <span class={`item meter${u().stale ? " stale" : ""}`} title={title()} data-provider={props.kind} onContextMenu={(e) => accountMenu(e, props.kind)}>
        <KindGlyph kind={props.kind} size={12} />
        <Show when={!u().error} fallback={<span class="meter-err">!</span>}>
          <For each={u().windows}>
            {(w: UsageWindow) => (
              <span class={`meter-win${level(w.used_percent)}`}>
                <span class="meter-label">{windowLabel(w.label)}</span>
                <span class="meter-bar"><span style={{ width: `${Math.min(100, w.used_percent)}%` }} /></span>
                {Math.round(w.used_percent)}%
                <Show when={remainText(w.resets_at)}>{(r) => <span class="meter-reset">{r()}</span>}</Show>
              </span>
            )}
          </For>
        </Show>
      </span>
    </Show>
  );
}

/** "5h" / "week" / "2w" as the usage endpoint names its windows, in the interface language. */
function windowLabel(label: string) {
  if (label === "week") return t("week");
  const m = /^(\d+)([hdw])$/.exec(label);
  return m ? t(m[2] === "h" ? "{n}h" : m[2] === "d" ? "{n}d" : "{n}w", { n: m[1] }) : label;
}

export function UsageMeters() {
  const cache = loadCache();
  const [report, setReport] = createSignal<UsageReport | null>(null);
  const view = (p: ProviderId) => shown(report()?.[p] ?? null, cache[cacheKey(p)]);
  let last = 0;
  const refresh = () => {
    if (Date.now() - last < USAGE_MIN_GAP) return;
    last = Date.now();
    fetchUsage();
  };
  const fetchUsage = () =>
    void usageFetch(activeAccount("claude")?.dir, activeAccount("codex")?.dir)
      .then((r) => {
        for (const p of ["claude", "codex"] as const) if (r[p].windows.length) cache[cacheKey(p)] = { windows: r[p].windows, at: Date.now() };
        saveCache(cache);
        setReport(r);
        for (const [name, u] of [["Claude", r.claude], ["Codex", r.codex]] as const) {
          for (const w of u.windows) {
            const key = `${name}:${w.label}:${w.resets_at ?? ""}`;
            if (w.used_percent >= 100 && !warned.has(key)) {
              warned.add(key);
              store.notify(t("{name} {window} limit reached — {reset}", { name, window: windowLabel(w.label), reset: resetText(w.resets_at) }), undefined, 8000);
            }
          }
        }
      })
      .catch(() => {});
  poll(refresh, USAGE_EVERY);
  poll(() => setNow(Date.now()), 60_000);
  // Another active account: fetch its own numbers at once.
  createEffect(on(() => `${accountKey("claude")}|${accountKey("codex")}`, () => {
    setReport(null);
    last = Date.now();
    fetchUsage();
  }, { defer: true }));
  return (
    <Show when={view("claude") || view("codex")}>
      <span class="meters clickable" onClick={refresh} title={t("Click to refresh")}>
        <Provider kind="claude" name="Claude" usage={view("claude")} />
        <Provider kind="codex" name="Codex" usage={view("codex")} />
        <span class="item clickable" title={t("Usage & cost")} onClick={(e) => { e.stopPropagation(); store.setOverlay("cost"); }}>{t("Cost")}</span>
      </span>
    </Show>
  );
}

const gb = (b: number) => (b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : `${Math.round(b / (1 << 20))} MB`);

export function MemoryMeter() {
  const [stats, setStats] = createSignal<MemoryStats | null>(null);
  let daemonPid: number | null = null;
  // Any (re)connection may be to a different daemon process.
  const unlisten = onEvent("daemon-status", () => (daemonPid = null));
  onCleanup(() => void unlisten.then((f) => f()));
  const refresh = async () => {
    if (daemonPid === null && store.connected()) daemonPid = (await daemon<{ pid: number }>("daemon.info").catch(() => null))?.pid ?? null;
    setStats(await memoryStats(daemonPid).catch(() => null));
  };
  poll(() => void refresh(), MEMORY_EVERY);
  const total = () => (stats() ? stats()!.app + stats()!.daemon + stats()!.sessions : 0);
  return (
    <Show when={stats()}>
      <span
        class="item"
        data-meter="memory"
        title={[
          t("Memory"),
          t("Kelpie app + WebView: {size}", { size: gb(stats()!.app) }),
          t("Session daemon: {size}", { size: gb(stats()!.daemon) }),
          t("Sessions ({n} processes): {size}", { n: stats()!.session_processes, size: gb(stats()!.sessions) }),
        ].join("\n")}
      >
        {t("RAM {size}", { size: gb(total()) })}
      </span>
    </Show>
  );
}
