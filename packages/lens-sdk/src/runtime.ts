import type {
  ArtifactMetadata,
  DiffModel,
  LensId,
  LensManifest,
  PreviewModel,
  LensReconcileInput,
  LensReconcileRendererProps,
  LensReconcileResult
} from "./contracts";

export type LensMatchReason = "explicit" | "pathRegex" | "mediaType" | "extension" | "utf8TextFallback" | "rawFallback";

export interface LensSurfaceMetadata extends Partial<ArtifactMetadata> {
  dimensions?: { width: number; height: number };
}

export interface LensPreviewRendererProps {
  preview: PreviewModel;
  metadata?: LensSurfaceMetadata | null;
  title: string;
  emptyMessage: string;
  className?: string;
}

export interface LensDiffRendererProps {
  diff: DiffModel;
  metadata?: LensSurfaceMetadata | null;
  title: string;
  emptyMessage: string;
  className?: string;
}

export type LensReconcileRequest = LensReconcileRendererProps;

export interface LensViewerModule<TNode = unknown> {
  renderPreview?: (props: LensPreviewRendererProps) => TNode;
  renderDiff?: (props: LensDiffRendererProps) => TNode;
  renderReconcile?: (request: LensReconcileRequest) => TNode;
}

export interface LensAnalyzerModule {
  preparePreview?: (input: unknown) => Promise<PreviewModel> | PreviewModel;
  prepareDiff?: (input: unknown) => Promise<DiffModel> | DiffModel;
  reconcile?: (request: LensReconcileInput) => Promise<LensReconcileResult> | LensReconcileResult;
}

export interface LayrsLens<TNode = unknown> {
  manifest: LensManifest;
  priority?: number;
  pathRegex?: RegExp[];
  viewer: LensViewerModule<TNode>;
  analyzer?: LensAnalyzerModule;
}

export interface LensResolutionRequest {
  lensId?: LensId;
  mediaType?: string;
  path?: string;
  bytes?: Uint8Array;
  isUtf8Text?: boolean;
}

export interface LensResolution<TNode = unknown> {
  lens: LayrsLens<TNode>;
  reason: LensMatchReason;
}

export interface LensRegistrySnapshot<TNode = unknown> {
  lenses: LayrsLens<TNode>[];
  manifests: LensManifest[];
}

export class LensRegistry<TNode = unknown> {
  #lenses = new Map<string, LayrsLens<TNode>>();

  register(lens: LayrsLens<TNode>): () => void {
    this.#lenses.set(lens.manifest.id, lens);
    return () => {
      if (this.#lenses.get(lens.manifest.id) === lens) {
        this.#lenses.delete(lens.manifest.id);
      }
    };
  }

  get(lensId: string | undefined): LayrsLens<TNode> | undefined {
    return lensId ? this.#lenses.get(lensId) : undefined;
  }

  list(): LayrsLens<TNode>[] {
    return [...this.#lenses.values()].sort((left, right) => (left.priority ?? 50) - (right.priority ?? 50));
  }

  manifests(): LensManifest[] {
    return this.list().map((lens) => lens.manifest);
  }

  snapshot(): LensRegistrySnapshot<TNode> {
    const lenses = this.list();
    return {
      lenses,
      manifests: lenses.map((lens) => lens.manifest)
    };
  }

  resolve(request: LensResolutionRequest): LensResolution<TNode> | undefined {
    if (request.lensId) {
      const explicit = this.get(request.lensId);
      if (explicit) {
        return { lens: explicit, reason: "explicit" };
      }
    }

    const path = request.path ?? "";
    const regexLens = this.list().find((lens) => lens.pathRegex?.some((regex) => regex.test(path)));
    if (regexLens) {
      return { lens: regexLens, reason: "pathRegex" };
    }

    const mediaType = normalizeMediaType(request.mediaType);
    if (mediaType) {
      const mediaLens = this.list().find((lens) => lens.manifest.analyzer.supportedMediaTypes.includes(mediaType));
      if (mediaLens) {
        return { lens: mediaLens, reason: "mediaType" };
      }
    }

    const extension = extensionFromPath(request.path);
    if (extension) {
      const extensionLens = this.list().find((lens) => lens.manifest.analyzer.fileExtensions.includes(extension));
      if (extensionLens) {
        return { lens: extensionLens, reason: "extension" };
      }
    }

    if (request.isUtf8Text === true || (request.bytes && isUtf8(request.bytes))) {
      const text = this.get("layrs.text");
      if (text) {
        return { lens: text, reason: "utf8TextFallback" };
      }
    }

    const raw = this.get("layrs.raw");
    return raw ? { lens: raw, reason: "rawFallback" } : undefined;
  }
}

export function createLensRegistry<TNode = unknown>(lenses: Array<LayrsLens<TNode>> = []): LensRegistry<TNode> {
  const registry = new LensRegistry<TNode>();
  for (const lens of lenses) {
    registry.register(lens);
  }
  return registry;
}

function normalizeMediaType(mediaType: string | undefined): string | undefined {
  return mediaType?.split(";")[0]?.trim().toLowerCase();
}

function extensionFromPath(path: string | undefined): string | undefined {
  const name = path?.split(/[\\/]/).pop();
  const dotIndex = name?.lastIndexOf(".") ?? -1;
  if (!name || dotIndex < 0 || dotIndex === name.length - 1) {
    return undefined;
  }
  return name.slice(dotIndex + 1).toLowerCase();
}

function isUtf8(bytes: Uint8Array): boolean {
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return true;
  } catch {
    return false;
  }
}
