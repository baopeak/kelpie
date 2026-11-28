// The machine's resources as the daemon samples them (daemon `resources.*`, API 51): a reading of the CPU, memory,
// disks, volumes and GPUs, a gauge per resource, what each session uses and the commands it runs, who uses a contended
// resource most, and what each command an agent ran has cost so far. The daemon samples on its own and sends
// `resources.changed` after each sample; these calls are for a view that opens and wants the latest at once.
import { daemon } from "./ipc";

/** First daemon API level with `resources.get`, `resources.ledger` and the `resources.changed` event. */
export const RESOURCES_API = 51;

/** The gauges, in the order the daemon keeps them. */
export const RESOURCE_KINDS = ["cpu", "ram", "disk", "space", "gpu"] as const;
export type ResourceKind = (typeof RESOURCE_KINDS)[number];
/** `null`: this computer cannot tell (never read as idle). */
export type ResourceState = "ok" | "busy" | "saturated" | null;

export interface DiskReading {
  name: string;
  busy_pct: number | null;
  queue: number | null;
  mbps: number | null;
}

export interface VolumeReading {
  path: string;
  free_mb: number;
  total_mb: number;
}

export interface GpuReading {
  id: string;
  util_pct: number | null;
  dedicated_mb: number | null;
  shared_mb: number | null;
}

export interface MachineReading {
  cores: number;
  cpu_pct: number | null;
  cpu_queue: number | null;
  mem_total_mb: number | null;
  mem_avail_mb: number | null;
  commit_limit_mb: number | null;
  commit_avail_mb: number | null;
  low_memory: boolean;
  pressure: { cpu: number | null; memory: number | null; io: number | null };
  disks: DiskReading[];
  volumes: VolumeReading[];
  gpus: GpuReading[];
}

/** What a session's processes took in one sample. */
export interface ResourceUsage {
  at: number;
  ws_mb: number;
  private_mb: number;
  /** CPU in cores (one busy core = 1). */
  cores: number;
  io_mbps: number;
  gpu_pct: number;
  gpu_mb: number;
  procs: number;
}

/** A shell command an agent runs, measured while it runs. */
export interface RunningCommand {
  fingerprint: string;
  jobs: number | null;
  began: number;
  samples: number;
  ram_mb: number;
  peak_cores: number;
  io_mb: number;
  gpu_pct: number;
}

export interface SessionResources {
  id: string;
  project: string | null;
  now: ResourceUsage;
  peak_private_mb: number;
  peak_commit_mb: number | null;
  commands: RunningCommand[];
  /** The last ten minutes, when asked for. */
  history?: ResourceUsage[];
}

/** A user of a contended resource: a session, or programs outside Kelpie grouped by name. */
export type ResourceUser = { session: string; project: string | null; value: number } | { program: string; count: number; value: number };

export interface ResourcesSnapshot {
  at: number;
  samples: number;
  machine: MachineReading;
  gauges: Record<ResourceKind, ResourceState>;
  sessions: SessionResources[];
  users: Partial<Record<Exclude<ResourceKind, "space">, ResourceUser[]>>;
}

/** What one command has cost, learned over its runs in one project at one parallelism. */
export interface CommandCost {
  project: string;
  fingerprint: string;
  jobs: number | null;
  runs: number;
  failed: number;
  last: number;
  /** The memory to plan for: the mean plus 1.5 spreads, or the largest recent peak. */
  plan_ram_mb: number;
  ram_mb: number;
  ram_sd: number;
  cores: number;
  peak_cores: number;
  io_mb: number;
  peak_io_mbps: number;
  gpu_pct: number;
  gpu_mb: number;
  secs: number;
}

export const getResources = (opts: { history?: boolean; session?: string } = {}) => daemon<ResourcesSnapshot>("resources.get", opts);
export const commandCosts = (project?: string) => daemon<{ commands: CommandCost[] }>("resources.ledger", project ? { project } : {});
