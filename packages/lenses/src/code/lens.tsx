import type { LayrsLens } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { codeManifest } from "../manifests";
import { TextLensDiff } from "../text/diff";
import { TextLensPreview } from "../text/preview";

export const codeLens: LayrsLens<ReactNode> = {
  manifest: codeManifest,
  priority: 10,
  viewer: {
    renderPreview: TextLensPreview,
    renderDiff: TextLensDiff
  }
};
