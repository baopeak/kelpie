// Priorities: the person's ranking of work (daemon API 52). Projects sit in three tiers — critical, normal, background — and
// are dragged into order inside one (the top of a tier gets half again what its bottom gets). Under each project its live
// sessions: their own tier, a boost (top of critical for a while, two at most) and a reservation of cores and memory kept
// free for them. Sessions waiting for the person still come first everywhere; this decides the order among the rest.
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { t } from "../lib/i18n";
import { boostLeft, orderTier, priorities, projectsByTier, setProjectPolicy, reorder, TIERS, type Reserve, type Tier } from "../lib/priorities";
import { applied, boostSession, endBoost, priorityFocus, setSessionReserve, setSessionTier } from "../lib/priority-actions";
import { Icon } from "./icons";
import type { Project, SessionInfo } from "../lib/types";
import "./priorities-page.css";

const tierName = (tier: Tier) => (tier === "critical" ? t("Critical") : tier === "normal" ? t("Normal") : t("Background"));
const gb = (mb: number) => `${(mb / 1024).toFixed(mb % 1024 ? 1 : 0)} GB`;

/** Cores and memory (GB) typed into a reservation; null when both are empty. */
function readReserve(cores: string, ramGb: string): Reserve | null {
  const c = Number(cores.replace(",", ".")) || 0;
  const r = Math.round((Number(ramGb.replace(",", ".")) || 0) * 1024);
  return c > 0 || r > 0 ? { cores: c, ram_mb: r, vram_mb: 0 } : null;
}

function ReserveEditor(props: { value: Reserve | null | undefined; onSave: (r: Reserve | null) => Promise<boolean> }) {
  const [open, setOpen] = createSignal(false);
  const [cores, setCores] = createSignal("");
  const [ram, setRam] = createSignal("");
  const start = () => {
    setCores(props.value?.cores ? String(props.value.cores) : "");
    setRam(props.value?.ram_mb ? String(props.value.ram_mb / 1024) : "");
    setOpen(true);
  };
  return (
    <Show when={open()} fallback={
      <button class="link-btn reserve-btn" onClick={start} title={t("Keep cores and memory free for it while it runs")}>
        {props.value ? t("Reserved {cores} cores · {ram}", { cores: props.value.cores, ram: gb(props.value.ram_mb) }) : t("Reserve…")}
      </button>
    }>
      <span class="reserve-edit">
        <input type="number" min="0" step="0.5" placeholder={t("cores")} value={cores()} onInput={(e) => setCores(e.currentTarget.value)} aria-label={t("Cores")} />
        <input type="number" min="0" step="0.5" placeholder={t("GB")} value={ram()} onInput={(e) => setRam(e.currentTarget.value)} aria-label={t("Memory (GB)")} />
        {/* A refused reservation (more than the computer can keep free) leaves the fields open to correct. */}
        <button class="btn small" onClick={() => void props.onSave(readReserve(cores(), ram())).then((ok) => ok && setOpen(false))}>{t("Save")}</button>
        <Show when={props.value}><button class="btn small" onClick={() => void props.onSave(null).then((ok) => ok && setOpen(false))}>{t("Remove")}</button></Show>
        <button class="btn small" onClick={() => setOpen(false)}>{t("Cancel")}</button>
      </span>
    </Show>
  );
}

function SessionLine(props: { s: SessionInfo; now: number }) {
  const eff = () => priorities()?.effective[props.s.id];
  const own = () => priorities()?.policy.sessions[props.s.id];
  return (
    <div class={`prio-session${priorityFocus() === props.s.id ? " focused" : ""}`} data-prio-session={props.s.id}>
      <span class="name" title={props.s.cwd}>{sessionLabel(props.s)}</span>
      <select value={own()?.tier ?? "project"} aria-label={t("Priority of {name}", { name: sessionLabel(props.s) })}
        onChange={(e) => {
          const el = e.currentTarget;
          void setSessionTier(props.s, el.value as Tier | "project").then((ok) => { if (!ok) el.value = own()?.tier ?? "project"; });
        }}>
        <option value="project">{t("As its project")}</option>
        <For each={TIERS}>{(tier) => <option value={tier}>{tierName(tier)}</option>}</For>
      </select>
      <Show when={eff()?.source === "parent"}><span class="muted-inline">{t("capped by the session that opened it")}</span></Show>
      <Show when={eff()?.boosted} fallback={
        <span class="boosts">
          <span class="muted-inline">{t("Boost:")}</span>
          <button class="link-btn" onClick={() => void boostSession(props.s, "30m")}>{t("30 min")}</button>
          <button class="link-btn" onClick={() => void boostSession(props.s, "1h")}>{t("1 h")}</button>
          <button class="link-btn" onClick={() => void boostSession(props.s, "2h")}>{t("2 h")}</button>
          <button class="link-btn" onClick={() => void boostSession(props.s, "turn")}>{t("this turn")}</button>
        </span>
      }>
        <span class="boosted"><Icon name="bolt" size={11} />
          {eff()?.boost_until ? t("Boosted, {left} left", { left: boostLeft(eff()!.boost_until!, props.now) }) : t("Boosted until this turn ends")}
          <button class="link-btn" onClick={() => void endBoost(props.s)}>{t("End")}</button>
        </span>
      </Show>
      <ReserveEditor value={own()?.reserve} onSave={(r) => setSessionReserve(props.s, r)} />
    </div>
  );
}

function ProjectCard(props: { p: Project; tier: Tier; ids: string[]; now: number; dragging: () => string | null; setDragging: (id: string | null) => void }) {
  const sessions = createMemo(() => Object.values(store.sessions).filter((s) => s.project_id === props.p.id && s.alive));
  const policy = () => priorities()?.policy.projects[props.p.id];
  const save = (change: Parameters<typeof setProjectPolicy>[1]) => applied(setProjectPolicy(props.p.id, change));
  return (
    <div
      class={`prio-project${priorityFocus() === props.p.id ? " focused" : ""}${props.dragging() === props.p.id ? " dragging" : ""}`}
      data-prio-project={props.p.id}
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        e.stopPropagation();
        const moved = props.dragging();
        props.setDragging(null);
        if (moved && moved !== props.p.id) void applied(orderTier(props.tier, reorder(props.ids, moved, props.p.id)));
      }}
    >
      <div class="prio-project-head">
        {/* Only the handle drags: the fields and selects in the card stay usable. */}
        <span class="grip" title={t("Drag to rank")} draggable={true}
          onDragStart={(e) => { e.dataTransfer?.setData("text/plain", props.p.id); props.setDragging(props.p.id); }}
          onDragEnd={() => props.setDragging(null)}>
          <Icon name="grid" size={11} />
        </span>
        <span class="swatch" style={{ background: props.p.color }} />
        <span class="name">{props.p.name}</span>
        <select value={props.tier} aria-label={t("Tier of {name}", { name: props.p.name })}
          onChange={(e) => {
            const el = e.currentTarget;
            void save({ tier: el.value as Tier }).then((ok) => { if (!ok) el.value = props.tier; });
          }}>
          <For each={TIERS}>{(tier) => <option value={tier}>{tierName(tier)}</option>}</For>
        </select>
        <span style={{ flex: 1 }} />
        <ReserveEditor value={policy()?.reserve} onSave={(r) => save({ reserve: r })} />
      </div>
      <For each={sessions()}>{(s) => <SessionLine s={s} now={props.now} />}</For>
    </div>
  );
}

export default function PrioritiesPage() {
  let el!: HTMLDivElement;
  const [now, setNow] = createSignal(Date.now());
  const [dragging, setDragging] = createSignal<string | null>(null);
  const timer = setInterval(() => setNow(Date.now()), 15_000);
  onCleanup(() => clearInterval(timer));
  onMount(() => {
    el.focus();
    const focus = priorityFocus();
    if (focus) queueMicrotask(() => el.querySelector(`[data-prio-session="${CSS.escape(focus)}"], [data-prio-project="${CSS.escape(focus)}"]`)?.scrollIntoView({ block: "center" }));
  });
  const close = () => store.setOverlay(null);
  // Every project row the sidebar shows is listed, worktrees included: each ranks on its own.
  const tiers = createMemo(() => projectsByTier(priorities(), store.state.projects));
  const p = () => priorities();
  return (
    <div class="priorities-page" ref={el} tabIndex={-1} onKeyDown={(e) => e.key === "Escape" && !e.defaultPrevented && close()}>
      <div class="board-head">
        <button class="settings-back" onClick={close} title={t("Back (Esc)")}><Icon name="back" size={14} /></button>
        <h2>{t("Priorities")}</h2>
        <span class="muted-inline">{t("Rank your work: what is higher is served first as Kelpie shares out the computer. Sessions waiting for you still come first.")}</span>
      </div>
      <Show when={p()} fallback={<div class="muted prio-empty">{t("This daemon is older than the priorities: update Kelpie to rank your work.")}</div>}>
        {(pr) => (
          <>
            <div class="prio-summary">
              <span>{t("Boosted: {n} of {max}", { n: pr().boosted.length, max: pr().max_boosts })}</span>
              <span>{t("Reserved: {cores} of {maxCores} cores, {ram} of {maxRam}", {
                cores: pr().reserved.cores, maxCores: pr().reservable.cores, ram: gb(pr().reserved.ram_mb), maxRam: gb(pr().reservable.ram_mb),
              })}</span>
            </div>
            <For each={TIERS}>
              {(tier) => {
                const ids = () => tiers()[tier].map((x) => x.id);
                return (
                  <section
                    class={`prio-tier tier-${tier}`}
                    onDragOver={(e) => e.preventDefault()}
                    onDrop={(e) => {
                      e.preventDefault();
                      const moved = dragging();
                      setDragging(null);
                      if (moved) void applied(orderTier(tier, reorder(ids(), moved, null)));
                    }}
                  >
                    <h3>{tierName(tier)}</h3>
                    <For each={tiers()[tier]} fallback={<div class="muted prio-drop">{t("Drag a project here")}</div>}>
                      {(project) => <ProjectCard p={project} tier={tier} ids={ids()} now={now()} dragging={dragging} setDragging={setDragging} />}
                    </For>
                  </section>
                );
              }}
            </For>
          </>
        )}
      </Show>
    </div>
  );
}
