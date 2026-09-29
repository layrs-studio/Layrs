import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { nativeCheckoutKind, nativeWorkspace } from "../tauri";
import type { RewriteSummary } from "../NativeRewriteImpact";
import { rememberProject } from "./recents";

export type WorkspaceRequest = { action: string; [key: string]: unknown };

export type FileChange = {
  path: string;
  status: string;
  binary: boolean;
  textOmitted: boolean;
  textDiff: { op: string; text: string }[] | null;
  beforeBytes: number | null;
  afterBytes: number | null;
};

export type Operation = { operationId: string; summary: string; kind: string; createdAt: string };

export type NamedPoint = {
  markerId?: string;
  releaseId?: string;
  name?: string;
  version?: string;
  targetStepId: string;
  notes: string;
};

export type LayerRecord = {
  layerId: string;
  name: string;
  settings: { automaticCapture: boolean; automaticCaptureIdleMs: number };
};

export type Snapshot = {
  state: {
    space: { name: string; spaceId: string };
    worktree: { worktreeId: string; checkoutRoot: string };
    operation: Operation;
    layers: LayerRecord[];
    view: { worktrees: { worktreeId: string; activeLayerId: string | null; status: string }[] };
  };
  needsRepair: boolean;
  changes: FileChange[] | null;
  operations: Operation[];
  markers: NamedPoint[];
  releases: NamedPoint[];
};

export type Preview = {
  expected?: unknown;
  collisions?: { path: string; reason: string }[];
  changes?: FileChange[];
  layerHeads?: unknown[];
  markerIds?: string[];
  releaseIds?: string[];
  rewrite?: RewriteSummary;
};

export type PendingPreview = { title: string; data: Preview; apply: WorkspaceRequest };

export type CheckoutKind = "complete" | "projection" | "absent";

type WorkspaceApi = {
  root: string;
  kind: CheckoutKind | null;
  data: Snapshot | null;
  /** Label of the foreground operation, or null. Reading other views stays possible. */
  busy: string | null;
  error: string;
  clearError: () => void;
  activeLayer: LayerRecord | undefined;
  conflicted: boolean;
  automatic: boolean;
  setAutomatic: (value: boolean) => void;
  /** Last automatic capture activity: "idle" | "capturing". */
  autoActivity: "idle" | "capturing";
  run: (request: WorkspaceRequest, label: string) => Promise<Snapshot | null>;
  preview: PendingPreview | null;
  inspect: (request: WorkspaceRequest, apply: WorkspaceRequest, title: string) => Promise<void>;
  closePreview: () => void;
  applyPreview: () => Promise<Snapshot | null>;
  /** Lets child panels hold the foreground lock (merge, remote, diagnostics). */
  setForeground: (label: string | null) => void;
  refresh: () => Promise<void>;
};

const WorkspaceContext = createContext<WorkspaceApi | null>(null);

export function useWorkspace() {
  const value = useContext(WorkspaceContext);
  if (!value) throw new Error("useWorkspace must be used inside <WorkspaceProvider>");
  return value;
}

/** Owns one local project: status, foreground operations and automatic capture. */
export function WorkspaceProvider({ root, children }: { root: string; children: ReactNode }) {
  const [kind, setKind] = useState<CheckoutKind | null>(null);
  const [data, setData] = useState<Snapshot | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [automatic, setAutomatic] = useState(true);
  const [autoActivity, setAutoActivity] = useState<"idle" | "capturing">("idle");
  const [preview, setPreview] = useState<PendingPreview | null>(null);
  const running = useRef(false);
  const foregroundRevision = useRef(0);
  const idle = useRef({ signature: "", since: 0 });

  const setForeground = useCallback((label: string | null) => {
    foregroundRevision.current++;
    running.current = label !== null;
    setBusy(label);
  }, []);

  const accept = useCallback((next: Snapshot) => {
    setData(next);
    idle.current = { signature: "", since: 0 };
    rememberProject({ root, name: next.state.space.name, kind: "complete" });
  }, [root]);

  const run = useCallback(
    async (request: WorkspaceRequest, label: string) => {
      if (running.current) return null;
      setForeground(label);
      setError("");
      try {
        const next = await nativeWorkspace<Snapshot>(root, request);
        accept(next);
        setPreview(null);
        return next;
      } catch (cause) {
        setError(String(cause));
        return null;
      } finally {
        setForeground(null);
      }
    },
    [root, accept, setForeground],
  );

  const refresh = useCallback(async () => {
    if (running.current) return;
    setForeground("Ouverture du projet…");
    setError("");
    try {
      const checkout = await nativeCheckoutKind(root);
      setKind(checkout);
      if (checkout === "projection") {
        rememberProject({ root, name: "", kind: "projection" });
        return;
      }
      if (checkout === "absent") return;
      accept(await nativeWorkspace<Snapshot>(root, { action: "status" }));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setForeground(null);
    }
  }, [root, accept, setForeground]);

  useEffect(() => {
    setKind(null);
    setData(null);
    setPreview(null);
    setError("");
    void refresh();
  }, [refresh]);

  const inspect = useCallback(
    async (request: WorkspaceRequest, apply: WorkspaceRequest, title: string) => {
      if (running.current) return;
      setForeground("Préparation de l’aperçu…");
      setError("");
      try {
        const next = await nativeWorkspace<Preview>(root, request);
        setPreview({
          title,
          data: next,
          apply: next.rewrite
            ? { ...apply, expectedOperation: next.rewrite.expectedOperationId, expectedStep: next.rewrite.proposedStep.stepId }
            : next.expected
              ? { ...apply, action: apply.action === "restore" ? "applyRestore" : "applyView", expected: next.expected }
              : apply,
        });
      } catch (cause) {
        setError(String(cause));
      } finally {
        setForeground(null);
      }
    },
    [root, setForeground],
  );

  const applyPreview = useCallback(async () => {
    if (!preview) return null;
    return run(preview.apply, "Application…");
  }, [preview, run]);

  // Automatic capture: after a quiet period, while this project is open and visible.
  useEffect(() => {
    if (kind !== "complete" || !data) return;
    let disposed = false;
    let polling = false;
    const timer = window.setInterval(async () => {
      if (polling || running.current || document.visibilityState !== "visible" || preview) return;
      polling = true;
      const revision = foregroundRevision.current;
      let capturing = false;
      try {
        let next = await nativeWorkspace<Snapshot>(root, { action: "status" });
        if (disposed || revision !== foregroundRevision.current || running.current) return;
        const signature = JSON.stringify(next.changes);
        const now = Date.now();
        const own = next.state.view.worktrees.find((w) => w.worktreeId === next.state.worktree.worktreeId);
        const settings = next.state.layers.find((l) => l.layerId === own?.activeLayerId)?.settings;
        if (signature !== idle.current.signature) idle.current = { signature, since: now };
        else if (
          own?.status !== "conflicted" &&
          automatic &&
          settings?.automaticCapture &&
          next.changes?.length &&
          !next.needsRepair &&
          now - idle.current.since >= Math.max(500, settings.automaticCaptureIdleMs)
        ) {
          capturing = true;
          setForeground("Capture automatique…");
          setAutoActivity("capturing");
          next = await nativeWorkspace<Snapshot>(root, { action: "captureAutomatic" });
          idle.current = { signature: "", since: 0 };
        }
        if (!disposed) setData(next);
      } catch (cause) {
        if (!disposed && (capturing || revision === foregroundRevision.current)) setError(String(cause));
      } finally {
        polling = false;
        if (capturing) {
          setForeground(null);
          setAutoActivity("idle");
        }
      }
    }, 1500);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
    // Only the presence of `data` gates the loop; its content is re-read each tick.
  }, [root, kind, Boolean(data), automatic, preview, setForeground]);

  const own = data?.state.view.worktrees.find((w) => w.worktreeId === data.state.worktree.worktreeId);
  const activeLayer = data?.state.layers.find((l) => l.layerId === own?.activeLayerId);
  const conflicted = own?.status === "conflicted";

  const value = useMemo<WorkspaceApi>(
    () => ({
      root,
      kind,
      data,
      busy,
      error,
      clearError: () => setError(""),
      activeLayer,
      conflicted,
      automatic,
      setAutomatic,
      autoActivity,
      run,
      preview,
      inspect,
      closePreview: () => setPreview(null),
      applyPreview,
      setForeground,
      refresh,
    }),
    [root, kind, data, busy, error, activeLayer, conflicted, automatic, autoActivity, run, preview, inspect, applyPreview, setForeground, refresh],
  );

  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}
