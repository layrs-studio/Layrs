import type { LayrsLens, LensDiffRendererProps } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { imageManifest } from "../manifests";
import { RawLensFallback } from "../raw/fallback";
import { ImageLensPreview } from "./preview";

export const imageLens: LayrsLens<ReactNode> = {
  manifest: imageManifest,
  priority: 20,
  viewer: {
    renderPreview: ImageLensPreview,
    renderDiff: RawLensDiff
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
