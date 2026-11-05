/*
 * Non-secret server preferences of this computer: which server a project
 * talks to, the public certificate authority trusted for a server, the
 * remote Layer targeted by a limited-access folder, and a short log of
 * transfers. Access tokens never pass through here: they stay in the OS
 * credential store, managed by the native side.
 */

const STORAGE_KEY = "layrs.desktop.servers.v1";
const LEGACY_ORIGIN_KEY = "layrs.native.origin";
const ACTIVITY_LIMIT = 20;
const PEM_LIMIT = 64 * 1024;

export type RemoteTarget = { layerId: string; name?: string };

export type ActivityKind = "publish" | "publishIntegrate" | "receive" | "candidates" | "clone" | "provision" | "migrate" | "choose";
export type ActivityOutcome = "running" | "success" | "cancelled" | "failed" | "unconfirmed" | "interrupted";

/** One transfer or server operation as observed by this computer. */
export type ActivityEntry = {
  id: string;
  kind: ActivityKind;
  label: string;
  origin: string;
  startedAt: string;
  endedAt?: string;
  outcome: ActivityOutcome;
  /** Readable result: what happened and what is preserved. */
  message?: string;
  /** Real steps observed (never a guessed percentage). */
  steps: string[];
};

type Memory = {
  projects: Record<string, string>;
  authorities: Record<string, string>;
  targets: Record<string, RemoteTarget>;
  activity: Record<string, ActivityEntry[]>;
  last?: string;
};

function empty(): Memory {
  return { projects: {}, authorities: {}, targets: {}, activity: {} };
}

function read(): Memory {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return empty();
    const parsed = JSON.parse(raw) as Partial<Memory>;
    return {
      projects: parsed.projects && typeof parsed.projects === "object" ? parsed.projects : {},
      authorities: parsed.authorities && typeof parsed.authorities === "object" ? parsed.authorities : {},
      targets: parsed.targets && typeof parsed.targets === "object" ? parsed.targets : {},
      activity: parsed.activity && typeof parsed.activity === "object" ? parsed.activity : {},
      last: typeof parsed.last === "string" ? parsed.last : undefined,
    };
  } catch {
    return empty();
  }
}

function write(update: (memory: Memory) => void) {
  const memory = read();
  update(memory);
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(memory));
  } catch {
    // Preferences are optional: the session keeps working without them.
  }
}

/** Canonical display form; the native side validates the origin again. */
export function normalizeOrigin(value: string) {
  return value.trim().replace(/\/+$/, "");
}

export function hostOf(origin: string) {
  try {
    return new URL(origin).host || origin;
  } catch {
    return origin;
  }
}

export function lastOrigin(): string {
  const memory = read();
  if (memory.last) return memory.last;
  try {
    return normalizeOrigin(localStorage.getItem(LEGACY_ORIGIN_KEY) ?? "");
  } catch {
    return "";
  }
}

export function rememberedOrigin(root: string): string {
  return read().projects[root] ?? "";
}

export function rememberOrigin(root: string, origin: string) {
  const value = normalizeOrigin(origin);
  if (!value) return;
  write((memory) => {
    if (root) memory.projects[root] = value;
    memory.last = value;
  });
}

export function forgetOrigin(root: string) {
  write((memory) => {
    delete memory.projects[root];
  });
}

/** Public certificate of a private authority; not a secret. */
export function rememberedAuthority(origin: string): string {
  return read().authorities[normalizeOrigin(origin)] ?? "";
}

export function rememberAuthority(origin: string, pem: string) {
  const key = normalizeOrigin(origin);
  if (!key) return;
  write((memory) => {
    if (pem.trim() && pem.length <= PEM_LIMIT) memory.authorities[key] = pem;
    else delete memory.authorities[key];
  });
}

export function rememberedTarget(root: string): RemoteTarget | null {
  const target = read().targets[root];
  return target && typeof target.layerId === "string" && target.layerId ? target : null;
}

export function rememberTarget(root: string, target: RemoteTarget | null) {
  write((memory) => {
    if (target?.layerId) memory.targets[root] = { layerId: target.layerId.trim(), name: target.name };
    else delete memory.targets[root];
  });
}

export function readActivity(root: string): ActivityEntry[] {
  const entries = read().activity[root];
  return Array.isArray(entries) ? entries.filter((entry) => entry && typeof entry.id === "string") : [];
}

/**
 * Records an entry. A `running` entry left behind means Layrs stopped during
 * the transfer; the next start reports it as interrupted instead of guessing.
 */
export function saveActivity(root: string, entry: ActivityEntry) {
  write((memory) => {
    const current = Array.isArray(memory.activity[root]) ? memory.activity[root] : [];
    memory.activity[root] = [entry, ...current.filter((item) => item.id !== entry.id)].slice(0, ACTIVITY_LIMIT);
  });
}

export function clearActivity(root: string) {
  write((memory) => {
    delete memory.activity[root];
  });
}
