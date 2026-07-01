import type { LayrsLens, LensCapability, LensDiffRendererProps } from "@layrs/lens-sdk";
import { createLensManifest } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { RawLensFallback } from "../raw/fallback";
import { ImageLensPreview } from "./preview";

const CORE_CAPABILITIES: LensCapability[] = ["view", "diff", "reconcile"];

export const imageLens: LayrsLens<ReactNode> = {
  manifest: createLensManifest({
    id: "layrs.image",
    name: "Image",
    component: "ImageArtifactViewer",
    previewKinds: ["image"],
    diffKinds: ["imageMetadata"],
    supportedMediaTypes: ["image/png", "image/jpeg", "image/webp"],
    fileExtensions: ["png", "jpg", "jpeg", "webp"],
    capabilities: [...CORE_CAPABILITIES, "metadata", "preview", "proofRecipes"]
  }),
  priority: 20,
  viewer: {
    renderPreview: ImageLensPreview,
    renderDiff: RawLensDiff
  },
  analyzer: {
    reconcile: () => ({
      status: "unsupported",
      summary: "Image reconciliation is declared but not implemented yet.",
      fields: {}
    })
  }
};

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
