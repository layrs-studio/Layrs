import type { LayrsLens, LensCapability } from "@layrs/lens-sdk";
import { createLensManifest } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { TextLensDiff } from "../text/diff";
import { TextLensPreview } from "../text/preview";
import { prepareTextReconcile } from "../text/reconcile";
import { RawLensReconcileSurface } from "../raw/reconcileSurface";

const CORE_CAPABILITIES: LensCapability[] = ["view", "diff", "reconcile"];

export const codeLens: LayrsLens<ReactNode> = {
  manifest: createLensManifest({
    id: "layrs.code",
    name: "Code",
    component: "CodeArtifactViewer",
    previewKinds: ["code"],
    diffKinds: ["textLines"],
    supportedMediaTypes: [
      "text/rust",
      "text/typescript",
      "text/javascript",
      "text/css",
      "text/html",
      "application/json",
      "application/toml",
      "application/yaml",
      "text/x-go",
      "text/x-python"
    ],
    fileExtensions: [
      "rs",
      "ts",
      "tsx",
      "js",
      "jsx",
      "mjs",
      "cjs",
      "json",
      "css",
      "html",
      "htm",
      "toml",
      "yaml",
      "yml",
      "py",
      "go",
      "java",
      "kt",
      "kts",
      "swift",
      "c",
      "h",
      "cc",
      "cpp",
      "cxx",
      "hpp",
      "cs",
      "php",
      "rb",
      "sh",
      "bash",
      "zsh",
      "ps1",
      "sql"
    ],
    capabilities: [...CORE_CAPABILITIES, "metadata", "preview", "references"]
  }),
  priority: 10,
  viewer: {
    renderPreview: TextLensPreview,
    renderDiff: TextLensDiff,
    renderReconcile: RawLensReconcileSurface
  },
  analyzer: {
    reconcile: prepareTextReconcile
  }
};
