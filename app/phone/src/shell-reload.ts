// A page keeps the Content Security Policy it was loaded with. Once the relay is turned on or moved, the page's `connect-src` must name it,
// and the worker keeps a shell with the new header (sw.ts); a page that is already open, or alive in the background, still has the old one
// and cannot reach the relay until it loads again. So it loads again:
// - when the page refuses the relay's address itself (`securitypolicyviolation`), at once: it has no other way to the computer then;
// - when the worker says the kept shell's CSP changed, the next time the person comes back to the app, so that nothing moves under them;
// - likewise when a new version's worker takes the page over (`watchWorker`): the old version's files are kept for it until then (sw.ts).
// What is typed is kept across a reload (composer drafts, kept-messages.ts). A reload that does not help is not done again: each reason
// reloads once in this tab (a violation) or once a minute (a changed shell), as `sessionStorage` remembers.

/** What the worker tells open pages once the kept shell's CSP changed. */
export const SHELL_CHANGED = "kelpie-shell-changed";
export const RELOADS_KEY = "kelpie-shell-reloads";
/** A changed shell reloads at most this often: the worker tells only of a change, so more would be a loop. */
export const SHELL_RELOAD_GAP_MS = 60 * 1000;

export interface ReloadEnv {
  now(): number;
  /** This tab's record of reloads, or null; throws when there is no storage. */
  read(): string | null;
  write(value: string): void;
  reload(): void;
}

/** Reloads for `why` unless this tab already did within `gapMs`. With nowhere to write that down it never reloads: it could not tell a loop. */
export function reloadOnce(env: ReloadEnv, why: string, gapMs: number): boolean {
  let done: Record<string, number> = {};
  try {
    const parsed: unknown = JSON.parse(env.read() ?? "{}");
    if (parsed && typeof parsed === "object") done = parsed as Record<string, number>;
  } catch {
    /* none kept, or unreadable: start a new record */
  }
  const now = env.now();
  const at = done[why];
  if (typeof at === "number" && now - at < gapMs) return false;
  try {
    env.write(JSON.stringify({ ...done, [why]: now }));
  } catch {
    return false;
  }
  env.reload();
  return true;
}

/** The relay's host when a refused address is the relay's (a browser reports it whole or as its origin), else null. */
export function relayRefused(blocked: string, relay: string | null): string | null {
  if (!blocked || !relay) return null;
  try {
    const host = new URL(relay).host;
    return host && new URL(blocked).host === host ? host : null;
  } catch {
    return null;
  }
}

export class ShellReload {
  private due = false;

  constructor(private readonly env: ReloadEnv) {}

  /** The worker kept a shell whose CSP differs from the one this page may have loaded with. */
  shellChanged(): void {
    this.due = true;
  }

  /** The person came back to the app. */
  visible(): void {
    if (!this.due) return;
    this.due = false;
    reloadOnce(this.env, "shell", SHELL_RELOAD_GAP_MS);
  }

  /** The page refused to connect to `blocked`; `relay` is where the relay is now. */
  violation(blocked: string, relay: string | null): void {
    const host = relayRefused(blocked, relay);
    if (host) reloadOnce(this.env, `relay ${host}`, Number.POSITIVE_INFINITY);
  }
}

/** What of `navigator.serviceWorker` a page needs to see a new worker take it over. */
export interface WorkerContainer {
  controller: unknown;
  addEventListener(type: "controllerchange", listener: () => void): void;
}

/** A new version's worker took this page over: the page is still the old version's, so it loads again at the next foreground, as for a changed
 *  shell. The first worker taking a page that had none (just installed) is no new version. */
export function watchWorker(container: WorkerContainer, reload: ShellReload): void {
  let had = !!container.controller;
  container.addEventListener("controllerchange", () => {
    if (had) reload.shellChanged();
    had = true;
  });
}

/** The page's own: this tab's `sessionStorage`, which outlives a reload and nothing more. */
export function browserReloadEnv(): ReloadEnv {
  return {
    now: () => Date.now(),
    read: () => sessionStorage.getItem(RELOADS_KEY),
    write: (value) => sessionStorage.setItem(RELOADS_KEY, value),
    reload: () => location.reload(),
  };
}
