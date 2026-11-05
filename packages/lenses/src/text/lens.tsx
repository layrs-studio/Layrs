import type { LayrsLens } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { textManifest } from "../manifests";
import { TextLensDiff } from "./diff";
import { TextLensPreview } from "./preview";
import { prepareTextReconcile } from "./reconcile";
import { TextLensReconcileSurface } from "./reconcileSurface";

export const textLens: LayrsLens<ReactNode> = {
  manifest: textManifest,
  priority: 30,
  viewer: {
    renderPreview: TextLensPreview,
    renderDiff: TextLensDiff,
    renderReconcile: TextLensReconcileSurface
  },
  analyzer: {
    reconcile: prepareTextReconcile
  }
};
