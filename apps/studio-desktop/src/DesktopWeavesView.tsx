import { LensReconcileSurface, type LensReconcileConflict } from "@layrs/lenses";
import type { LocalSpaceSummary, WeaveConflictSummary, WeaveSessionSummary } from "./tauri";
import type { CommandKey } from "./desktopTypes";

export function WeavesPanel({
  busyAction,
  commandErrors,
  onAbort,
  onApply,
  onContinue,
  onPreview,
  onResolveConflict,
  onSourceLayerChange,
  onTargetLayerChange,
  selectedSpace,
  session,
  sourceLayerId,
  targetLayerId
}: {
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  selectedSpace: LocalSpaceSummary;
  session: WeaveSessionSummary | null;
  sourceLayerId: string;
  targetLayerId: string;
  onSourceLayerChange: (value: string) => void;
  onTargetLayerChange: (value: string) => void;
  onPreview: () => void;
  onApply: () => void;
  onResolveConflict: (path: string, resolution: string, manualText?: string) => void;
  onContinue: () => void;
  onAbort: () => void;
}) {
  const canStart = sourceLayerId && targetLayerId && sourceLayerId !== targetLayerId;
  const unresolved = session?.conflicts.filter((conflict) => conflict.status !== "resolved") ?? [];

  return (
    <section className="desktop-subpanel desktop-weaves-panel">
      <div className="desktop-subheading">
        <strong>Weaves</strong>
        <span>{session ? session.status : "No active Weave"}</span>
      </div>
      <div className="desktop-weave-builder">
        <label className="desktop-field">
          <span>Source Layer</span>
          <select value={sourceLayerId} onChange={(event) => onSourceLayerChange(event.currentTarget.value)}>
            <option value="">Choose source</option>
            {selectedSpace.layers.map((layer) => (
              <option value={layer.layerId} key={layer.layerId}>
                {layer.displayName}
              </option>
            ))}
          </select>
        </label>
        <label className="desktop-field">
          <span>Target Layer</span>
          <select value={targetLayerId} onChange={(event) => onTargetLayerChange(event.currentTarget.value)}>
            <option value="">Choose target</option>
            {selectedSpace.layers.map((layer) => (
              <option value={layer.layerId} key={layer.layerId}>
                {layer.displayName}
              </option>
            ))}
          </select>
        </label>
        <div className="desktop-weave-actions">
          <button type="button" className="desktop-secondary-button" disabled={!canStart || busyAction === "weave-preview"} onClick={onPreview}>
            Preview
          </button>
          <button type="button" className="desktop-primary-button" disabled={!canStart || Boolean(session) || busyAction === "weave-apply"} onClick={onApply}>
            Start Weave
          </button>
        </div>
      </div>
      {commandErrors.weave ? <p className="desktop-alert desktop-alert--error">{commandErrors.weave}</p> : null}
      {session ? (
        <div className="desktop-weave-session">
          <div className="desktop-settings-grid desktop-settings-grid--cards">
            <div className="desktop-setting-card">
              <span>Status</span>
              <strong>{session.status}</strong>
              <em>{session.weaveId}</em>
            </div>
            <div className="desktop-setting-card">
              <span>Steps</span>
              <strong>{session.plannedSteps.length}</strong>
              <em>{session.appliedSteps.length} applied</em>
            </div>
            <div className="desktop-setting-card">
              <span>Conflicts</span>
              <strong>{session.conflicts.length}</strong>
              <em>{unresolved.length} unresolved</em>
            </div>
          </div>
          {session.conflicts.length > 0 ? (
            <div className="desktop-weave-conflicts">
              {session.conflicts.map((conflict) => (
                <LensReconcileSurface
                  busy={busyAction === `weave-resolve:${conflict.path}`}
                  conflict={toLensReconcileConflict(conflict)}
                  disabled={conflict.status === "resolved"}
                  emptyMessage="Conflict details unavailable"
                  key={conflict.conflictId}
                  labels={{ existing: "Existing", incoming: "Incoming" }}
                  title={conflict.path}
                  onResolve={(resolution) => onResolveConflict(conflict.path, resolution.resolution, resolution.manualText)}
                />
              ))}
            </div>
          ) : null}
          <div className="desktop-weave-actions desktop-weave-actions--session">
            <button type="button" className="desktop-primary-button" disabled={unresolved.length > 0 || busyAction === "weave-continue"} onClick={onContinue}>
              Continue Weave
            </button>
            <button type="button" className="desktop-danger-button" disabled={busyAction === "weave-abort"} onClick={onAbort}>
              Abort Weave
            </button>
          </div>
        </div>
      ) : (
        <p className="desktop-empty">Start a Weave to move resolved changes between Layers. Conflicts can always be aborted back to the pre-Weave state.</p>
      )}
    </section>
  );
}

function toLensReconcileConflict(conflict: WeaveConflictSummary): LensReconcileConflict {
  return {
    blocks: conflict.blocks.map((block) => ({
      base: block.base,
      blockId: block.blockId,
      existing: block.ours,
      incoming: block.theirs,
      resolution: block.resolution,
      status: block.status,
      supportedMethods: toSupportedMethods(block.supportedMethods)
    })),
    conflictId: conflict.conflictId,
    lensId: conflict.lensId,
    message: conflict.message,
    path: conflict.path,
    resolution: conflict.resolution,
    segments: conflict.segments?.map((segment) => ({
      blockId: segment.blockId,
      kind: segment.kind === "block" ? "block" : "text",
      text: segment.text
    })),
    status: conflict.status,
    supportedMethods: toSupportedMethods(conflict.supportedMethods)
  };
}

function toSupportedMethods(methods: string[] | undefined): LensReconcileConflict["supportedMethods"] {
  return methods as LensReconcileConflict["supportedMethods"];
}
