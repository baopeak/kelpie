// Claude Code in Yolo mode on a host that logs in as root: why it asks for permissions there
// ("warn", before the start), and a guide that sets up a regular user for Yolo mode ("guide").
// State and the start guard: lib/root-user.ts.
import { Show, createEffect, createSignal, onMount, type JSX } from "solid-js";
import { store } from "../lib/store";
import { closeRootUserDialog, rootUserDialog, validUserName, warnToGuide, type RootUserDialog } from "../lib/root-user";
import { CLAUDE_INSTALL, agentFolder, checkHost, connectHost, hostById, hostTarget, newHost, saveHost, vault, type SshHost } from "../lib/ssh-hosts";
import { remoteAddUser, type AddedUser, type HostCheck } from "../lib/remote";
import { Icon } from "./icons";
import { t, errorText } from "../lib/i18n";

/** POSIX shell single quoting, for commands typed into a terminal on the host. */
const sq = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

export function RootUserDialogHost() {
  // A host removed meanwhile: nothing to explain or set up (a waiting start is cancelled).
  createEffect(() => {
    const d = rootUserDialog();
    if (d && !hostById(d.hostId)) closeRootUserDialog();
  });
  // Keyed on the dialog only: saving or checking any host replaces every host object in the
  // vault, and the guide must keep its progress through that.
  return (
    <Show when={rootUserDialog()} keyed>
      {(d) => (
        <Show when={hostById(d.hostId)}>
          {(host) => (d.mode === "warn" ? <Warn host={host()} /> : <Guide host={host()} dialog={d} />)}
        </Show>
      )}
    </Show>
  );
}

function Frame(props: { title: string; children: JSX.Element; onClose: () => void; class?: string }) {
  return (
    <div class="overlay root-user-overlay" onPointerDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class={`modal root-user ${props.class ?? ""}`} data-root-user onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
        <h2>{props.title}</h2>
        {props.children}
      </div>
    </div>
  );
}

function Warn(props: { host: SshHost }) {
  const [quiet, setQuiet] = createSignal(false);
  let startBtn!: HTMLButtonElement;
  onMount(() => startBtn.focus());
  const start = () => {
    if (quiet()) store.setSettings({ rootYoloOk: [...(store.state.settings.rootYoloOk ?? []), props.host.id] });
    closeRootUserDialog(true);
  };
  return (
    <Frame title={t("Claude Code will ask for permissions on {host}", { host: props.host.label })} onClose={() => closeRootUserDialog(false)}>
      <p class="root-user-text">
        {t("{host} logs in as root. Claude Code refuses Yolo mode (skipping permission prompts) as root, to protect the server, and would quit at once.", { host: props.host.label })}
      </p>
      <p class="root-user-text">
        {t("This session starts in the normal mode instead: Claude asks before it edits files or runs commands. To use Yolo mode on this server, run Claude Code as a regular user — Kelpie can set one up for you.")}
      </p>
      <label class="check-label">
        <input type="checkbox" checked={quiet()} onChange={(e) => setQuiet(e.currentTarget.checked)} />
        {t("Don't explain this again for {host}", { host: props.host.label })}
      </label>
      <div class="footer">
        <button class="btn" onClick={() => closeRootUserDialog(false)}>{t("Cancel")}</button>
        <button class="btn" data-action="root-user-guide" onClick={warnToGuide}><Icon name="server" size={13} /> {t("Set up a regular user…")}</button>
        <button ref={startBtn} class="btn primary" data-action="root-user-start" onClick={start}>{t("Start, asking permissions")}</button>
      </div>
    </Frame>
  );
}

/** Whether a check says the host logs in as `name` and has Claude Code. */
const verified = (c: HostCheck | null, name: string) => !!c && !c.root && c.user === name;

function Guide(props: { host: SshHost; dialog: RootUserDialog }) {
  const project = () => store.state.projects.find((p) => p.id === props.dialog.projectId);
  const [name, setName] = createSignal("dev");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [added, setAdded] = createSignal<AddedUser | null>(null);
  const [userHost, setUserHost] = createSignal<SshHost | null>(null);
  const [check, setCheck] = createSignal<HostCheck | null>(null);
  const [copying, setCopying] = createSignal<string | null>(null);
  const linkedOnRoot = () => agentFolder(props.host, project());
  const [source, setSource] = createSignal(linkedOnRoot().startsWith("/root/") ? linkedOnRoot() : "");
  const home = () => added()?.home || check()?.home || `/home/${name()}`;
  const leaf = () => source().trim().replace(/\/+$/, "").split("/").pop() ?? "";
  const copyTarget = () => `${home()}/${leaf()}`;
  const sourceOk = () => /^\/.+/.test(source().trim()) && !["/", "/root", home()].includes(source().trim().replace(/\/+$/, "")) && !!leaf();

  const run = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(errorText(String(e)));
    } finally {
      setBusy(null);
    }
  };

  const create = () => run("create", async () => {
    setAdded(await remoteAddUser(props.host.alias, name()));
  });

  /** A saved host for the new user: the root host's address, key and options, with its login. */
  const addHost = () => run("host", async () => {
    const h = props.host;
    const same = vault().hosts.find((x) => x.hostname === h.hostname && (x.port ?? 22) === (h.port ?? 22) && x.user === name());
    const saved = same ?? (await saveHost(newHost({
      label: `${h.label}-${name()}`, group: h.group, tags: h.tags, hostname: h.hostname, port: h.port, user: name(), auth: "key", identityFile: h.identityFile,
      jump: h.jump, forwardAgent: h.forwardAgent, keepalive: h.keepalive, env: h.env, tmux: h.tmux, color: h.color, terminalTheme: h.terminalTheme, os: h.os,
    })));
    setUserHost(saved);
    setCheck(await checkHost(saved));
  });

  const copyProject = () => run("copy", async () => {
    const src = source().trim().replace(/\/+$/, "");
    const dst = copyTarget();
    // Typed into a terminal on the root host, so the copy shows its progress and errors.
    const cmd = `if [ -e ${sq(dst)} ]; then echo ${sq(`${dst} already exists`)}; else cp -a -- ${sq(src)} ${sq(`${home()}/`)} && chown -R -- ${sq(`${name()}:`)} ${sq(dst)} && echo ${sq(`Copied to ${dst}`)}; fi`; // i18n-ignore
    await connectHost(props.host, "shell", { prompt: cmd, projectId: project() && !project()!.remote ? project()!.id : undefined });
    setCopying(dst);
  });

  const linkProject = () => {
    const p = project();
    const h = userHost();
    if (p && h) store.linkHost(p.id, { hostId: h.id, path: sourceOk() ? copyTarget() : home() });
  };

  // The dialog closes once the terminal is open (it would hide it); an error keeps it open.
  const signIn = () => run("signin", async () => {
    const h = userHost();
    if (!h) return;
    await connectHost(h, "shell", { prompt: "claude", projectId: project() && !project()!.remote ? project()!.id : undefined });
    closeRootUserDialog();
  });

  const installClaude = () => run("install", async () => {
    const h = userHost();
    if (h) await connectHost(h, "shell", { prompt: CLAUDE_INSTALL });
  });

  const startClaude = () => run("start", async () => {
    const p = project();
    const h = userHost();
    if (!p || !h) return;
    const target = await hostTarget(h, p);
    store.switchProject(p.id);
    if (!(await store.newSession(p.id, "claude", { target }))) throw new Error(t("Claude Code did not start on {host}", { host: h.label }));
    closeRootUserDialog();
  });

  const canStart = () => {
    const p = project();
    const h = userHost();
    return !!p && !p.remote && !!h && verified(check(), name()) && !!agentFolder(h, p) && !!check()?.agents.includes("claude");
  };

  return (
    <Frame title={t("Set up a regular user on {host}", { host: props.host.label })} class="root-user-guide" onClose={() => closeRootUserDialog()}>
      <p class="root-user-text">{t("Claude Code runs in Yolo mode only as a regular user. Kelpie creates one with root's SSH keys, adds it as a saved host and checks that it works. Your root host stays as it is, for admin work.")}</p>

      <section class="root-user-step" data-step="create">
        <h3><span class="root-user-num">1</span>{t("Create the user")}</h3>
        <div class="inline-fields">
          <input class="text-input mono" style={{ width: "160px" }} data-field="root-user-name" value={name()} disabled={!!added()}
            onInput={(e) => setName(e.currentTarget.value.trim())} onKeyDown={(e) => e.key === "Enter" && validUserName(name()) && !added() && void create()} />
          <button class="btn primary" data-action="root-user-create" disabled={!validUserName(name()) || !!busy() || !!added()} onClick={() => void create()}>
            {busy() === "create" ? t("Creating…") : t("Create {name} on {host}", { name: name() || "…", host: props.host.label })}
          </button>
        </div>
        <Show when={!validUserName(name())}><div class="form-note error">{t("Lowercase letters, digits, - and _, starting with a letter; not root.")}</div></Show>
        <div class="form-note">{t("No password and no sudo: it logs in with the SSH keys root has (yours included). Kelpie runs useradd as root and copies root's authorized_keys to it.")}</div>
        <Show when={props.host.auth === "password"}>
          <div class="form-note warn">{t("{host} logs in with a password: the new user only gets the SSH keys root has. Install your key on {host} first (SSH hosts → Keys).", { host: props.host.label })}</div>
        </Show>
        <Show when={added()}>
          {(a) => (
            <>
              <div class="form-note ok" data-root-user-created><Icon name="check" size={12} /> {a().existed ? t("{name} already existed: your SSH keys were added to it.", { name: name() }) : t("{name} created, home {home}.", { name: name(), home: a().home })}</div>
              <Show when={a().sshd.length}>
                <div class="form-note warn">{t("sshd only lets some users in ({rules}). Add {name} there (/etc/ssh/sshd_config), then reload sshd.", { rules: a().sshd.join("; "), name: name() })}</div>
              </Show>
            </>
          )}
        </Show>
      </section>

      <section class="root-user-step" data-step="host">
        <h3><span class="root-user-num">2</span>{t("Add it to Kelpie and check it")}</h3>
        <div class="inline-fields">
          <button class="btn" data-action="root-user-host" disabled={!added() || !!busy()} onClick={() => void addHost()}>
            {busy() === "host" ? t("Connecting…") : userHost() ? t("Check again") : t("Add host {label}", { label: `${props.host.label}-${name()}` })}
          </button>
        </div>
        <Show when={userHost() && check()}>
          <div class="remote-check" data-root-user-check>
            <span class={verified(check(), name()) ? "ok" : "warn"}>{verified(check(), name()) ? t("logs in as {user} ✓", { user: check()!.user }) : t("logs in as {user}", { user: check()!.user })}</span>
            <span class={check()!.agents.includes("claude") ? "ok" : "warn"}>{check()!.agents.includes("claude") ? t("Claude Code ✓") : t("Claude Code missing")}</span>
            <span class={check()!.tmux ? "ok" : "muted-inline"}>{check()!.tmux ? t("tmux ✓") : t("tmux not installed")}</span>
            <Show when={!check()!.agents.includes("claude")}>
              <button class="btn" title={CLAUDE_INSTALL} onClick={() => void installClaude()}>{t("Install Claude Code")}</button>
            </Show>
          </div>
        </Show>
      </section>

      <section class="root-user-step" data-step="projects">
        <h3><span class="root-user-num">3</span>{t("Give it your project")}</h3>
        <div class="form-note">{t("Folders under /root are private to root, so {name} cannot open them. Copy the project into {home}; the copy belongs to {name} and the original stays.", { name: name(), home: home() })}</div>
        <div class="inline-fields">
          <input class="text-input mono" style={{ flex: 1, width: "auto" }} data-field="root-user-source" placeholder="/root/my-project" value={source()} onInput={(e) => setSource(e.currentTarget.value)} />
          <button class="btn" data-action="root-user-copy" disabled={!added() || !sourceOk() || !!busy()} onClick={() => void copyProject()}><Icon name="terminal" size={13} /> {t("Copy in a terminal on {host}", { host: props.host.label })}</button>
        </div>
        <Show when={sourceOk()}><div class="form-note mono">{`${source().trim()} → ${copyTarget()}`}</div></Show>
        <Show when={copying()}>{(dst) => <div class="form-note">{t("Copying to {path} in a terminal behind this dialog: it says when it is done.", { path: dst() })}</div>}</Show>
        <Show when={project() && !project()!.remote}>
          <div class="inline-fields">
            <button class="btn" data-action="root-user-link" disabled={!userHost()} onClick={linkProject}>
              {t("Use {path} for {project}", { path: sourceOk() ? copyTarget() : home(), project: project()!.name })}
            </button>
            <Show when={userHost() && project() && agentFolder(userHost()!, project())}>
              <span class="form-note ok"><Icon name="check" size={12} /> {t("linked: {path}", { path: agentFolder(userHost()!, project()) })}</span>
            </Show>
          </div>
        </Show>
      </section>

      <section class="root-user-step" data-step="signin">
        <h3><span class="root-user-num">4</span>{t("Sign in to Claude Code as {name}", { name: name() })}</h3>
        <div class="form-note">{t("Each user has its own Claude Code sign-in: the first time Claude Code starts as {name} it asks you to sign in. Start it below, or open a terminal as {name} (it runs claude).", { name: name() })}</div>
        <div class="inline-fields">
          <button class="btn" data-action="root-user-signin" disabled={!verified(check(), name()) || !!busy()} onClick={() => void signIn()}><Icon name="terminal" size={13} /> {t("Open a terminal as {name}", { name: name() })}</button>
        </div>
      </section>

      <div class="form-note warn">{t("Do not give {name} passwordless sudo: Yolo agents could act as root again.", { name: name() })}</div>
      <Show when={error()}><div class="form-note error" data-root-user-error>{error()}</div></Show>
      <div class="footer">
        <button class="btn" onClick={() => closeRootUserDialog()}>{t("Close")}</button>
        <Show when={project() && !project()!.remote}>
          <button class="btn primary" data-action="root-user-claude" disabled={!canStart() || !!busy()} title={canStart() ? "" : t("Add the host, link the project's folder and install Claude Code first")} onClick={() => void startClaude()}>
            <Icon name="sparkle" size={13} /> {t("Start Claude Code as {name}", { name: name() })}
          </button>
        </Show>
      </div>
    </Frame>
  );
}
