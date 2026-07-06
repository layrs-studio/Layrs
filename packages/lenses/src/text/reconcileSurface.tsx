import type {
  LensReconcileAction,
  LensReconcileRendererProps,
  ResolutionMethod
} from "@layrs/lens-sdk";
import { useState } from "react";
import { LensSurfaceHeader } from "../shared/LensSurfaceHeader";
import { joinClassNames } from "../shared/utils";

const TEXT_METHODS: ResolutionMethod[] = ["existing", "incoming", "both", "manual"];

export function TextLensReconcileSurface({
  busy = false,
  className,
  conflict,
  disabled = false,
  emptyMessage,
  labels,
  onResolve,
  title
}: LensReconcileRendererProps) {
  const [manualTextByBlock, setManualTextByBlock] = useState<Record<string, string>>({});
  const existingLabel = labels?.existing ?? "Existing";
  const incomingLabel = labels?.incoming ?? "Incoming";
  const canUseControls = Boolean(onResolve);

  return (
    <section className={joinClassNames("layrs-lens-reconcile layrs-lens-reconcile--text", className)} data-testid="lens-reconcile-surface" aria-label={title}>
      <LensSurfaceHeader summary={conflict.message} title={title} />
      {conflict.blocks.length === 0 ? (
        <div className="layrs-lens-reconcile__empty">
          <strong>{emptyMessage}</strong>
          <p>This text conflict does not expose block details in the current host response.</p>
        </div>
      ) : (
        <div className="layrs-lens-reconcile__blocks">
          {conflict.blocks.map((block) => {
            const isResolved = block.status === "resolved";
            const blockManualText = manualTextByBlock[block.blockId] ?? block.existing;
            const methods = supportedMethods(block.supportedMethods, TEXT_METHODS);
            const actions = methods.map((method) => textAction(method, block.blockId, existingLabel, incomingLabel));
            const manualAction = actions.find((action) => action.requiresManualText);
            const buttonActions = actions.filter((action) => !action.requiresManualText);
            const canResolveBlock = canUseControls && !isResolved;

            return (
              <article className={joinClassNames("layrs-lens-reconcile-block", isResolved ? "is-resolved" : undefined)} key={block.blockId}>
                <header className="layrs-lens-reconcile-block__header">
                  <strong>{block.blockId}</strong>
                  <span>{block.status}</span>
                  {block.resolution ? <em>{block.resolution}</em> : null}
                </header>
                <div className="layrs-lens-reconcile__versions">
                  <ConflictVersion label="Base" value={block.base ?? ""} />
                  <ConflictVersion label={existingLabel} value={block.existing} />
                  <ConflictVersion label={incomingLabel} value={block.incoming} />
                </div>
                {canResolveBlock ? (
                  <>
                    {buttonActions.length > 0 ? (
                      <div className="layrs-lens-reconcile__actions">
                        {buttonActions.map((action) => (
                          <button
                            type="button"
                            data-testid={`lens-resolve-${block.blockId}-${action.method}`}
                            disabled={disabled || busy}
                            key={action.resolution}
                            onClick={() =>
                              onResolve?.({
                                blockId: block.blockId,
                                lensId: conflict.lensId,
                                method: action.method,
                                path: conflict.path,
                                resolution: action.resolution,
                                scope: action.scope
                              })
                            }
                          >
                            {action.label}
                          </button>
                        ))}
                      </div>
                    ) : null}
                    {manualAction ? (
                      <label className="layrs-lens-reconcile__manual">
                        <span>{manualAction.label}</span>
                        <textarea
                          data-testid={`lens-manual-${block.blockId}`}
                          disabled={disabled || busy}
                          rows={6}
                          value={blockManualText}
                          onChange={(event) =>
                            setManualTextByBlock((current) => ({
                              ...current,
                              [block.blockId]: event.currentTarget.value
                            }))
                          }
                        />
                        <button
                          type="button"
                          data-testid={`lens-resolve-${block.blockId}-${manualAction.method}`}
                          disabled={disabled || busy}
                          onClick={() =>
                            onResolve?.({
                              blockId: block.blockId,
                              lensId: conflict.lensId,
                              manualText: blockManualText,
                              method: manualAction.method,
                              path: conflict.path,
                              resolution: manualAction.resolution,
                              scope: manualAction.scope
                            })
                          }
                        >
                          Use manual
                        </button>
                      </label>
                    ) : null}
                  </>
                ) : null}
              </article>
            );
          })}
        </div>
      )}
      {!canUseControls && conflict.status !== "resolved" ? (
        <p className="layrs-lens-reconcile__notice">Resolution controls are not available in this host yet.</p>
      ) : null}
    </section>
  );
}

function textAction(
  method: ResolutionMethod,
  blockId: string,
  existingLabel: string,
  incomingLabel: string
): LensReconcileAction {
  const scope = "block";
  switch (method) {
    case "existing":
      return {
        method,
        scope,
        label: `Use ${existingLabel}`,
        resolution: `block:${blockId}:existing`,
        blockId
      };
    case "incoming":
      return {
        method,
        scope,
        label: `Use ${incomingLabel}`,
        resolution: `block:${blockId}:incoming`,
        blockId
      };
    case "both":
      return {
        method,
        scope,
        label: "Use both",
        resolution: `block:${blockId}:both`,
        blockId
      };
    case "manual":
      return {
        method,
        scope,
        label: "Manual block resolution",
        resolution: `block:${blockId}:manual`,
        blockId,
        requiresManualText: true
      };
    default:
      return {
        method,
        scope,
        label: String(method),
        resolution: `block:${blockId}:${String(method)}`,
        blockId
      };
  }
}

function supportedMethods(methods: ResolutionMethod[] | undefined, defaults: ResolutionMethod[]) {
  if (!methods || methods.length === 0) {
    return defaults;
  }
  return defaults.filter((method) => methods.includes(method));
}

function ConflictVersion({ label, value }: { label: string; value: string }) {
  return (
    <div className="layrs-lens-reconcile-version">
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
