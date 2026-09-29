import type { LensManifest } from "./contracts";

export type LensMatchReason = "explicit" | "mediaType" | "extension" | "fallback";

export interface LensSelectionRequest {
  lensId?: string;
  mediaType?: string;
  path?: string;
  fallbackLensId?: string;
}

export interface LensManifestSelection {
  manifest: LensManifest;
  reason: LensMatchReason;
}

export class LensSelectionError extends Error {
  constructor(
    public readonly code: "ambiguous_lens_match",
    message: string
  ) {
    super(message);
    this.name = "LensSelectionError";
  }
}

export function selectLensManifest(
  manifests: readonly LensManifest[],
  request: LensSelectionRequest
): LensManifestSelection | undefined {
  if (request.lensId) {
    const explicit = uniqueMatch(manifests, "lens id", request.lensId, (manifest) => manifest.id === request.lensId);
    return explicit ? { manifest: explicit, reason: "explicit" } : undefined;
  }

  const mediaType = normalizeMediaType(request.mediaType);
  if (mediaType) {
    const mediaMatch = uniqueMatch(manifests, "media type", mediaType, (manifest) =>
      manifest.analyzer.supportedMediaTypes.some((candidate) => candidate.toLowerCase() === mediaType)
    );
    if (mediaMatch) {
      return { manifest: mediaMatch, reason: "mediaType" };
    }
  }

  const extension = extensionFromPath(request.path);
  if (extension) {
    const extensionMatch = uniqueMatch(manifests, "extension", extension, (manifest) =>
      manifest.analyzer.fileExtensions.some((candidate) => candidate.toLowerCase() === extension)
    );
    if (extensionMatch) {
      return { manifest: extensionMatch, reason: "extension" };
    }
  }

  if (request.fallbackLensId) {
    const fallback = uniqueMatch(
      manifests,
      "fallback lens id",
      request.fallbackLensId,
      (manifest) => manifest.id === request.fallbackLensId
    );
    if (fallback) {
      return { manifest: fallback, reason: "fallback" };
    }
  }
  return undefined;
}

function uniqueMatch(
  manifests: readonly LensManifest[],
  criterion: string,
  value: string,
  predicate: (manifest: LensManifest) => boolean
): LensManifest | undefined {
  const matches = manifests.filter(predicate);
  if (matches.length <= 1) {
    return matches[0];
  }
  const ids = matches.map((manifest) => manifest.id).sort();
  throw new LensSelectionError(
    "ambiguous_lens_match",
    `${criterion} \`${value}\` matches multiple Lenses: ${ids.join(", ")}`
  );
}

function normalizeMediaType(mediaType: string | undefined): string | undefined {
  const normalized = mediaType?.split(";")[0]?.trim().toLowerCase();
  return normalized || undefined;
}

function extensionFromPath(path: string | undefined): string | undefined {
  const name = path?.split(/[\\/]/).pop();
  const dotIndex = name?.lastIndexOf(".") ?? -1;
  if (!name || dotIndex < 0 || dotIndex === name.length - 1) {
    return undefined;
  }
  return name.slice(dotIndex + 1).toLowerCase();
}
