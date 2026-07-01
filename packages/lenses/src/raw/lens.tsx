import type { LayrsLens, LensCapability, LensDiffRendererProps, LensPreviewRendererProps } from "@layrs/lens-sdk";
import { createLensManifest } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { RawLensFallback } from "./fallback";

const CORE_CAPABILITIES: LensCapability[] = ["view", "diff", "reconcile"];

export const rawLens: LayrsLens<ReactNode> = {
  manifest: createLensManifest({
    id: "layrs.raw",
    name: "Raw",
    component: "RawArtifactViewer",
    previewKinds: ["raw"],
    diffKinds: ["binary"],
    supportedMediaTypes: ["application/octet-stream"],
    fileExtensions: [],
    capabilities: [...CORE_CAPABILITIES, "metadata", "preview"]
  }),
  priority: 100,
  viewer: {
    renderPreview: RawLensPreview,
    renderDiff: RawLensDiff
  },
  analyzer: {
    reconcile: () => ({
      status: "unsupported",
      summary: "Raw reconciliation is not supported.",
      fields: {}
    })
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
