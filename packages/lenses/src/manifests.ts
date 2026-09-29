import type { LensManifest } from "@layrs/lens-sdk";
import { BUILTIN_LENS_IMPLEMENTATION_DIGESTS } from "./implementationDigests.generated";

export const BUILTIN_LENS_VERSION = "1.0.0";
export { BUILTIN_LENS_IMPLEMENTATION_DIGESTS };

export const textManifest = {
  contractVersion: "layrs.lens.v1",
  id: "layrs.text",
  name: "Text",
  version: BUILTIN_LENS_VERSION,
  digest: BUILTIN_LENS_IMPLEMENTATION_DIGESTS["layrs.text"],
  analyzer: {
    supportedMediaTypes: ["text/plain", "text/markdown"],
    fileExtensions: ["txt", "md", "markdown", "rst", "log"],
    capabilities: ["view", "diff", "reconcile", "metadata", "preview", "references"]
  },
  viewer: {
    viewerId: "layrs.viewer.text",
    schemaVersion: "layrs.viewer.v1",
    component: "TextArtifactViewer",
    previewKinds: ["text"],
    diffKinds: ["textLines"],
    reconcileStatuses: [],
    inspectorFields: [
      { key: "line_count", label: "Lines", valueType: "number" },
      { key: "reference_count", label: "References", valueType: "number" },
      { key: "utf8_lossy", label: "UTF-8 lossy", valueType: "boolean" }
    ]
  },
  resolutionMethods: [
    { method: "existing", scope: "block" },
    { method: "incoming", scope: "block" },
    { method: "both", scope: "block" },
    { method: "manual", scope: "block" }
  ]
} satisfies LensManifest;

export const rawManifest = {
  contractVersion: "layrs.lens.v1",
  id: "layrs.raw",
  name: "Raw",
  version: BUILTIN_LENS_VERSION,
  digest: BUILTIN_LENS_IMPLEMENTATION_DIGESTS["layrs.raw"],
  analyzer: {
    supportedMediaTypes: ["application/octet-stream"],
    fileExtensions: [],
    capabilities: ["view", "diff", "reconcile", "metadata", "preview"]
  },
  viewer: {
    viewerId: "layrs.viewer.raw",
    schemaVersion: "layrs.viewer.v1",
    component: "RawArtifactViewer",
    previewKinds: ["raw"],
    diffKinds: ["binary"],
    reconcileStatuses: [],
    inspectorFields: [
      { key: "byte_len", label: "Size", valueType: "number" },
      { key: "content_hash", label: "Hash", valueType: "string" },
      { key: "media_type", label: "Type", valueType: "string" }
    ]
  },
  resolutionMethods: [
    { method: "existing", scope: "file" },
    { method: "incoming", scope: "file" }
  ]
} satisfies LensManifest;

export const codeManifest = {
  contractVersion: "layrs.lens.v1",
  id: "layrs.code",
  name: "Code",
  version: BUILTIN_LENS_VERSION,
  digest: BUILTIN_LENS_IMPLEMENTATION_DIGESTS["layrs.code"],
  analyzer: {
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
    capabilities: ["view", "diff", "metadata", "preview", "references"]
  },
  viewer: {
    viewerId: "layrs.viewer.code",
    schemaVersion: "layrs.viewer.v1",
    component: "CodeArtifactViewer",
    previewKinds: ["code"],
    diffKinds: ["textLines"],
    reconcileStatuses: [],
    inspectorFields: [
      { key: "language", label: "Language", valueType: "string" },
      { key: "line_count", label: "Lines", valueType: "number" },
      { key: "reference_count", label: "References", valueType: "number" }
    ]
  },
  resolutionMethods: []
} satisfies LensManifest;

export const imageManifest = {
  contractVersion: "layrs.lens.v1",
  id: "layrs.image",
  name: "Image",
  version: BUILTIN_LENS_VERSION,
  digest: BUILTIN_LENS_IMPLEMENTATION_DIGESTS["layrs.image"],
  analyzer: {
    supportedMediaTypes: ["image/png", "image/jpeg", "image/webp"],
    fileExtensions: ["png", "jpg", "jpeg", "webp"],
    capabilities: ["view", "diff", "metadata", "preview", "proofRecipes"]
  },
  viewer: {
    viewerId: "layrs.viewer.image",
    schemaVersion: "layrs.viewer.v1",
    component: "ImageArtifactViewer",
    previewKinds: ["image"],
    diffKinds: ["imageMetadata"],
    reconcileStatuses: [],
    inspectorFields: [
      { key: "format", label: "Format", valueType: "string" },
      { key: "width", label: "Width", valueType: "number" },
      { key: "height", label: "Height", valueType: "number" },
      { key: "pixel_count", label: "Pixels", valueType: "number" }
    ]
  },
  resolutionMethods: []
} satisfies LensManifest;
