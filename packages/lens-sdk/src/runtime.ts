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
import { selectLensManifest, type LensMatchReason } from "./selection";

export type { LensMatchReason } from "./selection";

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

export type LensReconcileRenderRequest = LensReconcileRendererProps;

export interface LensViewerModule<TNode = unknown> {
  renderPreview?: (props: LensPreviewRendererProps) => TNode;
  renderDiff?: (props: LensDiffRendererProps) => TNode;
  renderReconcile?: (request: LensReconcileRenderRequest) => TNode;
}

export interface LensAnalyzerModule {
  preparePreview?: (input: unknown) => Promise<PreviewModel> | PreviewModel;
  prepareDiff?: (input: unknown) => Promise<DiffModel> | DiffModel;
  reconcile?: (request: LensReconcileInput) => Promise<LensReconcileResult> | LensReconcileResult;
}

export interface LayrsLens<TNode = unknown> {
  manifest: LensManifest;
  priority?: number;
  viewer: LensViewerModule<TNode>;
  analyzer?: LensAnalyzerModule;
}

export interface LensResolutionRequest {
  lensId?: LensId;
  mediaType?: string;
  path?: string;
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
    const lenses = this.list();
    const selection = selectLensManifest(
      lenses.map((lens) => lens.manifest),
      request
    );
    const lens = selection ? lenses.find((candidate) => candidate.manifest === selection.manifest) : undefined;
    return selection && lens ? { lens, reason: selection.reason as Exclude<LensMatchReason, "fallback"> } : undefined;
  }
}

export function createLensRegistry<TNode = unknown>(lenses: Array<LayrsLens<TNode>> = []): LensRegistry<TNode> {
  const registry = new LensRegistry<TNode>();
  for (const lens of lenses) {
    registry.register(lens);
  }
  return registry;
}
