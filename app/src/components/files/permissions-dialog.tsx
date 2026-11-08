// Permissions of files and folders on a host: an rwx grid kept in step with an octal field, owner and group shown
// read-only (no chown), and for folders "apply to everything inside" (a queue job), optionally with x on folders only.
// Only the bits the person changes are applied, to each item over its own mode: one item's setuid, setgid or sticky
// bits never spread to the others.
import { For, Show, createSignal } from "solid-js";
import { t, errorText } from "../../lib/i18n";
import * as ipc from "../../lib/ipc";
import { changedBits, joinPath, modeBits, bitsMode, octal, parseOctal, sharedBits, withChange } from "../../lib/file-pane";
import type { RemoteEntry } from "../../lib/types";

export function PermissionsDialog(props: { dest: string; dir: string; entries: RemoteEntry[]; onClose: () => void; onDone: () => void }) {
  const first = props.entries[0];
  // What every selected item has (rwx only).
  const start = sharedBits(props.entries.map((e) => e.mode));
  const [bits, setBits] = createSignal(modeBits(start));
  const [text, setText] = createSignal(octal(start));
  const [recursive, setRecursive] = createSignal(false);
  const [dirsOnlyX, setDirsOnlyX] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const hasDir = props.entries.some((e) => e.kind === "dir");
  const toggle = (i: number) => {
    const next = bits().map((b, j) => (j === i ? !b : b));
    setBits(next);
    setText(octal(bitsMode(next)));
  };
  const typed = (v: string) => {
    setText(v);
    const m = parseOctal(v);
    if (m != null) setBits(modeBits(m & 0o777));
  };
  const target = () => (parseOctal(text()) ?? bitsMode(bits())) & 0o777;
  const apply = async () => {
    setError(null);
    const changed = changedBits(start, target());
    try {
      if (recursive() && hasDir) {
        await ipc.xferStart({ kind: "chmod", src: { dest: props.dest }, items: props.entries.map((e) => joinPath(props.dest, props.dir, e.name)), mode: target(), mask: changed, dirs_only_x: dirsOnlyX() });
      } else if (changed) {
        // One call per resulting mode: each item keeps the bits that were not changed.
        const byMode = new Map<number, string[]>();
        for (const e of props.entries) {
          const m = withChange(e.mode ?? 0, target(), changed);
          byMode.set(m, [...(byMode.get(m) ?? []), joinPath(props.dest, props.dir, e.name)]);
        }
        for (const [m, paths] of byMode) await ipc.sftpChmod(props.dest, paths, m);
      }
      props.onDone();
      props.onClose();
    } catch (e) {
      setError(errorText(String(e)));
    }
  };
  const who = [t("Owner"), t("Group"), t("Others")];
  const what = [t("Read"), t("Write"), t("Run / open")];
  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="modal permissions-dialog" role="dialog" aria-modal="true" aria-label={t("Permissions")} data-permissions>
        <h3>{props.entries.length === 1 ? t("Permissions of {name}", { name: first?.name ?? "" }) : t("Permissions of {n} items", { n: props.entries.length })}</h3>
        <Show when={first?.owner || first?.group}>
          <div class="muted-inline">{t("Owner {owner}, group {group} (changing them needs root on most servers)", { owner: first?.owner ?? "?", group: first?.group ?? "?" })}</div>
        </Show>
        <Show when={props.entries.length > 1}>
          <div class="muted-inline">{t("Only the boxes you change are applied; each item keeps the rest of its own permissions.")}</div>
        </Show>
        <table class="perm-grid">
          <thead><tr><th /><For each={what}>{(w) => <th>{w}</th>}</For></tr></thead>
          <tbody>
            <For each={who}>
              {(w, row) => (
                <tr>
                  <td>{w}</td>
                  <For each={[0, 1, 2]}>{(col) => <td><input type="checkbox" data-bit={row() * 3 + col} checked={bits()[row() * 3 + col]} onChange={() => toggle(row() * 3 + col)} /></td>}</For>
                </tr>
              )}
            </For>
          </tbody>
        </table>
        <label class="inline-fields">{t("Octal")} <input class="text-input mono" data-field="octal" style={{ width: "6em" }} value={text()} onInput={(e) => typed(e.currentTarget.value)} /></label>
        <Show when={hasDir}>
          <label class="check-inline"><input type="checkbox" data-field="recursive" checked={recursive()} onChange={(e) => setRecursive(e.currentTarget.checked)} /> {t("Apply to everything inside")}</label>
          <Show when={recursive()}>
            <label class="check-inline"><input type="checkbox" checked={dirsOnlyX()} onChange={(e) => setDirsOnlyX(e.currentTarget.checked)} /> {t("x for folders only: folders can always be opened, files keep their own x")}</label>
          </Show>
        </Show>
        <Show when={error()}><div class="form-note error">{error()}</div></Show>
        <div class="inline-fields">
          <button class="btn primary" data-action="apply-permissions" disabled={parseOctal(text()) == null} onClick={() => void apply()}>{t("Apply")}</button>
          <button class="btn" onClick={props.onClose}>{t("Cancel")}</button>
        </div>
      </div>
    </div>
  );
}
