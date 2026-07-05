import type { LocalSpaceSummary, WeaveSessionSummary } from "./tauri";
import type { CommandKey } from "./desktopTypes";
import { useState } from "react";

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
  const [manualTextByBlock, setManualTextByBlock] = useState<Record<string, string>>({});
  const manualKey = (conflictPath: string, blockId: string) => `${conflictPath}:${blockId}`;

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
                <article className={conflict.status === "resolved" ? "desktop-conflict-card is-resolved" : "desktop-conflict-card"} key={conflict.conflictId}>
                  <div>
                    <strong>{conflict.path}</strong>
                    <span>{conflict.lensId}</span>
                    <em>{conflict.message}</em>
                  </div>
                  <div className="desktop-conflict-actions">
                    <button type="button" className="desktop-secondary-button" disabled={conflict.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`} onClick={() => onResolveConflict(conflict.path, "ours")}>
                      Keep target
                    </button>
                    <button type="button" className="desktop-secondary-button" disabled={conflict.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`} onClick={() => onResolveConflict(conflict.path, "theirs")}>
                      Take source
                    </button>
                    <button type="button" className="desktop-secondary-button" disabled={conflict.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`} onClick={() => onResolveConflict(conflict.path, "base")}>
                      Restore base
                    </button>
                  </div>
                  {conflict.blocks.length > 0 ? (
                    <div className="desktop-conflict-blocks">
                      {conflict.blocks.map((block) => (
                        <div className={block.status === "resolved" ? "desktop-conflict-block is-resolved" : "desktop-conflict-block"} key={block.blockId}>
                          <div className="desktop-conflict-block-header">
                            <strong>{block.blockId}</strong>
                            <span>{block.status}</span>
                            {block.resolution ? <em>{block.resolution}</em> : null}
                          </div>
                          <div className="desktop-conflict-versions">
                            <ConflictVersion label="Base" value={block.base} />
                            <ConflictVersion label="Target" value={block.ours} />
                            <ConflictVersion label="Source" value={block.theirs} />
                          </div>
                          <div className="desktop-conflict-actions">
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() => onResolveConflict(conflict.path, `block:${block.blockId}:ours`)}
                            >
                              Keep target block
                            </button>
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() => onResolveConflict(conflict.path, `block:${block.blockId}:theirs`)}
                            >
                              Take source block
                            </button>
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() => onResolveConflict(conflict.path, `block:${block.blockId}:base`)}
                            >
                              Restore base block
                            </button>
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() => onResolveConflict(conflict.path, `block:${block.blockId}:both_ours_then_theirs`)}
                            >
                              Target then source
                            </button>
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() => onResolveConflict(conflict.path, `block:${block.blockId}:both_theirs_then_ours`)}
                            >
                              Source then target
                            </button>
                          </div>
                          <label className="desktop-field desktop-conflict-manual">
                            <span>Manual block resolution</span>
                            <textarea
                              value={manualTextByBlock[manualKey(conflict.path, block.blockId)] ?? block.ours}
                              onChange={(event) =>
                                setManualTextByBlock((current) => ({
                                  ...current,
                                  [manualKey(conflict.path, block.blockId)]: event.currentTarget.value
                                }))
                              }
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              rows={6}
                            />
                            <button
                              type="button"
                              className="desktop-secondary-button"
                              disabled={block.status === "resolved" || busyAction === `weave-resolve:${conflict.path}`}
                              onClick={() =>
                                onResolveConflict(
                                  conflict.path,
                                  `block:${block.blockId}:manual`,
                                  manualTextByBlock[manualKey(conflict.path, block.blockId)] ?? block.ours
                                )
                              }
                            >
                              Use manual block
                            </button>
                          </label>
                        </div>
                      ))}
                    </div>
                  ) : null}
                </article>
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

function ConflictVersion({ label, value }: { label: string; value: string }) {
  return (
    <div className="desktop-conflict-version">
      <span>{label}</span>
      <pre>{previewText(value)}</pre>
    </div>
  );
}

function previewText(value: string) {
  if (!value) {
    return "(empty)";
  }
  return value.length > 1_200 ? `${value.slice(0, 1_200)}\n...` : value;
}
