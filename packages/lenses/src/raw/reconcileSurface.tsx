import type { LensReconcileAction, LensReconcileRendererProps, ResolutionMethod } from "@layrs/lens-sdk";
import { LensSurfaceHeader } from "../shared/LensSurfaceHeader";
import { joinClassNames } from "../shared/utils";

const RAW_METHODS: ResolutionMethod[] = ["existing", "incoming"];

export function RawLensReconcileSurface({
  busy = false,
  className,
  conflict,
  disabled = false,
  labels,
  onResolve,
  title
}: LensReconcileRendererProps) {
  const existingLabel = labels?.existing ?? "Existing";
  const incomingLabel = labels?.incoming ?? "Incoming";
  const isResolved = conflict.status === "resolved";
  const actions = supportedMethods(conflict.supportedMethods, RAW_METHODS).map((method) =>
    rawAction(method, existingLabel, incomingLabel)
  );
  const canResolve = Boolean(onResolve) && !isResolved;

  return (
    <section className={joinClassNames("layrs-lens-reconcile layrs-lens-reconcile--raw", className)} data-testid="lens-reconcile-surface" aria-label={title}>
      <LensSurfaceHeader summary={conflict.message} title={title} />
      <div className="layrs-lens-reconcile__raw-body">
        <div className="layrs-lens-reconcile__summary">
          <strong>{conflict.path}</strong>
          <span>{conflict.status}</span>
          {conflict.resolution ? <em>{conflict.resolution}</em> : null}
        </div>
        {canResolve && actions.length > 0 ? (
          <div className="layrs-lens-reconcile__actions">
            {actions.map((action) => (
              <button
                type="button"
                data-testid={`lens-resolve-${action.method}`}
                disabled={disabled || busy}
                key={action.resolution}
                onClick={() =>
                  onResolve?.({
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
        {!onResolve && !isResolved ? (
          <p className="layrs-lens-reconcile__notice">Resolution controls are not available in this host yet.</p>
        ) : null}
      </div>
    </section>
  );
}

function rawAction(method: ResolutionMethod, existingLabel: string, incomingLabel: string): LensReconcileAction {
  const scope = "file";
  switch (method) {
    case "existing":
      return {
        method,
        scope,
        label: `Use ${existingLabel}`,
        resolution: "existing"
      };
    case "incoming":
      return {
        method,
        scope,
        label: `Use ${incomingLabel}`,
        resolution: "incoming"
      };
    default:
      return {
        method,
        scope,
        label: String(method),
        resolution: String(method)
      };
  }
}

function supportedMethods(methods: ResolutionMethod[] | undefined, defaults: ResolutionMethod[]) {
  if (!methods || methods.length === 0) {
    return defaults;
  }
  return defaults.filter((method) => methods.includes(method));
}
