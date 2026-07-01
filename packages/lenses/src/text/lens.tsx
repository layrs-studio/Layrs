import type { LayrsLens, LensCapability } from "@layrs/lens-sdk";
import { createLensManifest } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { TextLensDiff } from "./diff";
import { TextLensPreview } from "./preview";
import { prepareTextReconcile } from "./reconcile";

const CORE_CAPABILITIES: LensCapability[] = ["view", "diff", "reconcile"];

export const textLens: LayrsLens<ReactNode> = {
  manifest: createLensManifest({
    id: "layrs.text",
    name: "Text",
    component: "TextArtifactViewer",
    previewKinds: ["text"],
    diffKinds: ["textLines"],
    supportedMediaTypes: ["text/plain", "text/markdown"],
    fileExtensions: ["txt", "md", "markdown", "rst", "log"],
    capabilities: [...CORE_CAPABILITIES, "metadata", "preview", "references"]
  }),
  priority: 30,
  viewer: {
    renderPreview: TextLensPreview,
    renderDiff: TextLensDiff
  },
  analyzer: {
    reconcile: prepareTextReconcile
  }
};
