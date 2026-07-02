import type { DesktopShortcutSettings, LensDiffEntry, LocalDiffStats } from "./tauri";

export type LoadState = "loading" | "ready" | "error";
export type DesktopPage = "available" | "local" | "draft" | "settings";
export type LocalSpaceTab = "changes" | "files" | "steps" | "layers" | "sync" | "settings";
export type FileState = "clean" | "modified" | "added" | "deleted" | "redacted";
export type ChangeState = "modified" | "added" | "deleted";
export const FOCUS_SCAN_THROTTLE_MS = 1500;
export type CommandKey =
  | "available"
  | "local"
  | "create"
  | "create-draft"
  | "init-local"
  | "forget-local"
  | "open"
  | "switch"
  | "create-layer"
  | "delete-layer"
  | "scan"
  | "diff-window"
  | "receive"
  | "save-step"
  | "publish"
  | "send-draft"
  | "settings";

export interface LayerFile {
  path: string;
  kind: "Code" | "Document" | "Image" | "Data";
  state: FileState;
  lensId?: string;
  sizeLabel: string;
  redacted?: boolean;
}

export interface LocalChange {
  path: string;
  state: ChangeState;
  summary: string;
  lensId: string;
  diff: LensDiffEntry;
}

export interface TimelineItem {
  id: string;
  kind: "active" | "scan" | "step";
  title: string;
  actor: string;
  at: string;
  summary: string;
  isActive: boolean;
  diffStats?: LocalDiffStats;
}

export interface CreateDraft {
  targetFolder: string;
  layerId: string;
}

export const statusLabels: Record<string, string> = {
  pending: "Waiting for approval",
  authorization_pending: "Waiting for approval",
  slow_down: "Server asked to slow down",
  connected: "Connected",
  denied: "Denied",
  expired: "Expired"
};

export const defaultShortcuts: DesktopShortcutSettings = {
  enabled: true,
  saveStep: "Ctrl+S",
  publish: "Ctrl+P",
  smartSavePublishesPendingStep: true
};

export type PulseTarget = "changes" | "layer" | "steps" | "sync";

