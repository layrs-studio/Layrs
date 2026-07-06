import type { LensReconcileConflict, LensReconcileRendererProps, LensReconcileResolution } from "@layrs/lens-sdk";
import { RawLensFallback } from "../raw/fallback";
import { builtinLensRegistry } from "../registry";

export interface LensReconcileSurfaceProps {
  conflict?: LensReconcileConflict | null;
  title?: string;
  emptyMessage?: string;
  className?: string;
  disabled?: boolean;
  busy?: boolean;
  labels?: LensReconcileRendererProps["labels"];
  onResolve?: (resolution: LensReconcileResolution) => void;
}

export function LensReconcileSurface({
  busy,
  className,
  conflict,
  disabled,
  emptyMessage = "Reconciliation details are not available",
  labels,
  onResolve,
  title
}: LensReconcileSurfaceProps) {
  if (!conflict) {
    return <RawLensFallback className={className} message={emptyMessage} title={title ?? "Reconcile"} />;
  }

  const surfaceTitle = title ?? conflict.path;
  const renderer = builtinLensRegistry.get(conflict.lensId)?.viewer.renderReconcile;

  if (renderer) {
    return (
      <>
        {renderer({
          busy,
          className,
          conflict,
          disabled,
          emptyMessage,
          labels,
          onResolve,
          title: surfaceTitle
        } satisfies LensReconcileRendererProps)}
      </>
    );
  }

  return (
    <RawLensFallback
      className={className}
      fields={{
        path: conflict.path,
        status: conflict.status,
        message: conflict.message,
        resolution: conflict.resolution,
        ...(conflict.fields ?? {})
      }}
      message="Reconciliation surface not available for this Lens."
      metadata={{ lensId: conflict.lensId, fields: conflict.fields ?? {} }}
      title={surfaceTitle}
    />
  );
}
