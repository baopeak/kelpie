// "New worktree" dialog: branch (suggested), base ref, folder, and — when the base commit has
// one — the `.kelpie.yaml` it would run, shown in full. Running it is opt-in unless the user
// approved that exact file for this repository before.
import { For, Show, createEffect, createResource, createSignal, on, onMount } from "solid-js";
import { store } from "../lib/store";
import { configAtRef, configApproved, checksApproved, createWorktree, validBranch, worktreeFolder, type KelpieConfig } from "../lib/worktrees";
import { gitRefs } from "../lib/ipc";
import { normPath } from "../lib/platform";
import { KeyHints } from "./key-hints";
import { t, errorText } from "../lib/i18n";

const [repoId, setRepoId] = createSignal<string | null>(null);
const [initialBranch, setInitialBranch] = createSignal<string | null>(null);
/** Runs once the worktree exists (e.g. start an agent on the issue it was made for). */
let afterCreate: ((project: import("../lib/types").Project) => void) | null = null;

/** `branch`: a name typed elsewhere (search), turned into a valid branch name. */
export function openWorktreeDialog(repo: string, branch?: string, then?: (project: import("../lib/types").Project) => void) {
  afterCreate = then ?? null;
  setRepoId(repo);
  setInitialBranch(branch ? branch.trim().replace(/\s+/g, "-").replace(/[^A-Za-z0-9._/-]/g, "").replace(/^[-./]+|[./]+$/g, "") || null : null);
  store.setOverlay("worktree");
}

const WORDS = ["amber", "brisk", "cedar", "delta", "ember", "fable", "grove", "harbor", "iris", "juniper", "kite", "lumen", "maple", "nova", "orbit", "pine", "quartz", "river", "sage", "tide"];

function suggestBranch(existing: string[]): string {
  for (let i = 0; i < 50; i++) {
    const name = `wt/${WORDS[Math.floor(Math.random() * WORDS.length)]}-${Math.floor(Math.random() * 90 + 10)}`;
    if (!existing.includes(name)) return name;
  }
  return `wt/${Date.now().toString(36)}`;
}

type ConfigState = { config: KelpieConfig | null } | { error: string };

export function WorktreeDialog() {
  const repo = () => store.state.projects.find((p) => p.id === repoId());
  const existing = () => store.state.projects.filter((p) => p.parentId === repoId()).map((p) => p.branch ?? "");
  const [branch, setBranch] = createSignal(initialBranch() ?? suggestBranch(existing()));
  const [base, setBase] = createSignal(store.projectInfo[repoId() ?? ""]?.branch ?? "HEAD");
  const [folderEdited, setFolderEdited] = createSignal<string | null>(null);
  const folder = () => folderEdited() ?? (repo() ? worktreeFolder(repo()!, branch()) : "");
  const [runConfig, setRunConfig] = createSignal(false);
  // Start-from suggestions: local and remote branches, tags (a commit id works too).
  const [refs] = createResource(() => repo()?.path, (root) => gitRefs(root).catch(() => []));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // The config of the base commit (debounced while the base is being typed).
  const [baseRef, setBaseRef] = createSignal(base());
  let timer = 0;
  createEffect(on(base, (b) => {
    clearTimeout(timer);
    timer = window.setTimeout(() => setBaseRef(b.trim() || "HEAD"), 300);
  }));
  const [config] = createResource(
    () => (repo() ? { root: repo()!, ref: baseRef() } : null),
    (k): Promise<ConfigState> => configAtRef(k.root, k.ref).then((config) => ({ config }), (e) => ({ error: String(e).replace(/^Error: /, "") })),
  );
  const loaded = () => {
    const c = config();
    return c && "config" in c ? c.config : null;
  };
  // Approved before (same file, same repository) → ticked; otherwise the user opts in.
  createEffect(on(loaded, (c) => setRunConfig(!!c && !!repo() && configApproved(repo()!, c) && checksApproved(repo()!, c))));

  /** The check list approved before (for marking what changed). */
  const approvedChecks = () => (repo() ? store.state.approvedChecks?.[normPath(repo()!.path)] : undefined);

  let input!: HTMLInputElement;
  onMount(() => (input.focus(), input.select()));

  const create = async () => {
    const r = repo();
    if (!r || busy()) return;
    if (!validBranch(branch())) return setError(t("\"{branch}\" is not a valid branch name", { branch: branch() }));
    setBusy(true);
    setError(null);
    try {
      const cfg = runConfig() ? loaded() : null;
      // What was shown above is what is approved: the commands that start a worktree, and the checks Finish runs.
      if (cfg) {
        store.approveSetup(r.path, cfg.setupKey);
        store.approveChecks(r.path, cfg.check);
      }
      const project = await createWorktree(r, { branch: branch(), base: base(), path: folder(), config: cfg });
      if (store.overlay() === "worktree") store.setOverlay(null);
      const then = afterCreate;
      afterCreate = null;
      then?.(project);
    } catch (e) {
      setError(String(e).replace(/^Error: /, ""));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && !busy() && store.setOverlay(null)}>
      <div class="modal worktree-dialog" onKeyDown={(e) => {
        if (e.key === "Escape" && !busy()) store.setOverlay(null);
        if (e.key === "Enter" && !(e.target as HTMLElement).matches("textarea, input[type=checkbox]")) void create();
      }}>
        <h2>{t("New worktree of {name}", { name: repo()?.name ?? "" })}</h2>
        <div class="form-row">
          <label>{t("Branch")}</label>
          <input ref={input} class="text-input" value={branch()} onInput={(e) => setBranch(e.currentTarget.value.trim())} />
        </div>
        <div class="form-row">
          <label>{t("From")}</label>
          <input class="text-input" list="wt-refs" value={base()} placeholder={t("HEAD, a branch, origin/branch, a tag or a commit")} onInput={(e) => setBase(e.currentTarget.value)} />
          <datalist id="wt-refs">
            <For each={refs() ?? []}>{(r) => <option value={r.name}>{r.kind}</option>}</For>
          </datalist>
        </div>
        <div class="form-row">
          <label>{t("Folder")}</label>
          <input class="text-input mono" value={folder()} onInput={(e) => setFolderEdited(e.currentTarget.value)} />
        </div>
        <Show when={config()}>
          {(c) => (
            <Show when={"config" in c()} fallback={<div class="form-note error">{(c() as { error: string }).error}</div>}>
              <Show when={loaded()}>
                {(cfg) => (
                  <div class="kelpie-config">
                    <label class="form-check">
                      <input type="checkbox" checked={runConfig()} onChange={(e) => setRunConfig(e.currentTarget.checked)} />
                      {t("Run {file} from {ref}", { file: cfg().file ?? ".kelpie.yaml", ref: baseRef() })}
                    </label>
                    <div class="kelpie-config-body">
                      <For each={cfg().setup}>{(cmd) => <div class="mono">$ {cmd}</div>}</For>
                      <For each={cfg().tabs}>{(tab) => <div class="mono">+ tab: {"kind" in tab ? tab.kind : tab.command}</div>}</For>
                      <For each={cfg().copy}>{(f) => <div class="mono">copy: {f}</div>}</For>
                      <For each={cfg().link}>{(f) => <div class="mono">link: {f}</div>}</For>
                      <For each={cfg().check}>{(c) => <div class="mono" classList={{ "check-changed": !!approvedChecks() && !approvedChecks()!.includes(c) }} title={t("Run by Finish before a worktree is merged, in a temporary worktree of the result")}>check: {c}{approvedChecks() && !approvedChecks()!.includes(c) ? "  " + t("(new since you approved the checks)") : ""}</div>}</For>
                    </div>
                  </div>
                )}
              </Show>
            </Show>
          )}
        </Show>
        <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
        <div class="footer">
          <KeyHints items={[["Enter", t("Create")], ["Esc", t("Cancel")]]} />
          <span style={{ flex: 1 }} />
          <button class="btn" disabled={busy()} onClick={() => store.setOverlay(null)}>{t("Cancel")}</button>
          <button class="btn primary" disabled={busy()} onClick={() => void create()}>{busy() ? t("Creating…") : t("Create worktree")}</button>
        </div>
      </div>
    </div>
  );
}
