import type { LayrsLens, LensDiffRendererProps, LensPreviewRendererProps } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { rawManifest } from "../manifests";
import { RawLensFallback } from "./fallback";
import { prepareRawReconcile } from "./reconcile";
import { RawLensReconcileSurface } from "./reconcileSurface";

export const rawLens: LayrsLens<ReactNode> = {
  manifest: rawManifest,
  priority: 100,
  viewer: {
    renderPreview: RawLensPreview,
    renderDiff: RawLensDiff,
    renderReconcile: RawLensReconcileSurface
  },
  analyzer: {
    reconcile: prepareRawReconcile
  }
};

function RawLensPreview({ className, emptyMessage, metadata, preview, title }: LensPreviewRendererProps) {
  return (
    <RawLensFallback
      className={className}
      fields={preview.fields}
      message={emptyMessage}
      metadata={{
        ...(metadata ?? {}),
        kind: preview.kind,
        mediaType: preview.mediaType,
        dimensions: preview.dimensions
      }}
      title={title}
    />
  );
}

function RawLensDiff({ className, diff, metadata, title }: LensDiffRendererProps) {
  return (
    <RawLensFallback
      className={className}
      fields={diff.fields}
      message="visual diff not available yet"
      metadata={metadata}
      title={title}
    />
  );
}
