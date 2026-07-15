import {
  createPreviewModelFromArtifactContent,
  normalizeArtifactContentPayload,
  normalizeArtifactPreviewWindowPayload,
  type Artifact,
  type ArtifactWindowMetadata
} from "@layrs/client-sdk";
import { LensPreviewHost, listLenses, type LensSurfaceMetadata } from "@layrs/lenses";
import {
  LensSelectionError,
  parseLensManifestList,
  selectLensManifest,
  type DiffKind,
  type LensManifest,
  type PreviewKind,
  type PreviewModel
} from "@layrs/lens-sdk";
import { useEffect, useMemo, useState } from "react";
import { EmptyState } from "./common";
import { isArtifactRestricted } from "../privateData";
import { isPrivateRequestCancelled, privateRequestKey, useStudioPrivateApi } from "../studioPrivateApi";

export type LensRegistryStatus = "loading" | "available" | "fallback";

export interface LensRegistryState {
  status: LensRegistryStatus;
  manifests: LensManifest[];
  message?: string;
}

type ArtifactHints = Artifact & {
  body?: string;
  content?: string;
  contentUrl?: string;
  extension?: string;
  lensId?: string;
  mediaType?: string;
  preview?: Partial<PreviewModel>;
  previewUrl?: string;
  raw?: string;
  url?: string;
};

export const fallbackLensManifests: LensManifest[] = parseLensManifestList(
  listLenses().map((entry) => entry.manifest)
);

type ContentState =
  | { status: "idle" | "loading" }
  | { status: "available"; preview?: PreviewModel; window?: ArtifactWindowMetadata }
  | { status: "unavailable"; message: string };

type KeyedContentState = ContentState & { requestKey: string };

const SERVER_PREVIEW_WINDOW_LIMIT = 400;

export function LensFileViewer({
  artifact,
  layerId,
  workspaceId,
  lensRegistry
}: {
  artifact?: Artifact;
  layerId?: string;
  workspaceId: string;
  lensRegistry: LensRegistryState;
}) {
  const lens = useMemo(
    () => (artifact ? resolveLensForArtifact(artifact, lensRegistry.manifests) : undefined),
    [artifact, lensRegistry.manifests]
  );
  const fallbackPreview = useMemo(
    () => (artifact ? previewForArtifact(artifact, lens) : undefined),
    [artifact, lens]
  );
  const [windowStart, setWindowStart] = useState(0);
  const contentState = useArtifactContentPreview({ artifact, fallbackPreview, layerId, lens, windowStart, workspaceId });

  useEffect(() => {
    setWindowStart(0);
  }, [artifact?.id]);

  if (!artifact) {
    return <EmptyState title="No file selected" detail="Choose an accessible file to preview it in Studio." />;
  }

  const restriction = artifact.access?.reason || "Restricted by Layer access policy";
  if (isArtifactRestricted(artifact)) {
    return (
      <div className="studio-file-viewer is-restricted">
        <div className="studio-file-viewer__meta">
          <span>Restricted</span>
          <strong>{artifact.name}</strong>
          <p>{restriction}</p>
        </div>
      </div>
    );
  }

  const preview = contentState.status === "available" ? contentState.preview ?? fallbackPreview : fallbackPreview;
  const window = contentState.status === "available" ? contentState.window : undefined;
  const kind = preview?.kind ?? "raw";

  return (
    <div className="studio-file-viewer">
      <div className="studio-file-viewer__meta">
        <span>{lens?.name ?? "Raw"} Lens</span>
        <strong>{artifact.name}</strong>
        <p>{artifact.location}</p>
        {lensRegistry.message ? <small>{lensRegistry.message}</small> : null}
        {contentState.status === "unavailable" ? <small>{contentState.message}</small> : null}
        {window ? (
          <div className="studio-file-viewer__window-controls">
            <small>
              {window.totalLines === undefined
                ? `Lines ${window.start + 1}-${window.start + window.count}`
                : `Lines ${window.start + 1}-${window.start + window.count} of ${window.totalLines}`}
            </small>
            <div>
              <button
                disabled={contentState.status === "loading" || window.start === 0}
                onClick={() => setWindowStart(Math.max(0, window.start - window.limit))}
                type="button"
              >
                Previous
              </button>
              <button
                disabled={contentState.status === "loading" || !window.hasMore}
                onClick={() => setWindowStart(window.start + window.limit)}
                type="button"
              >
                Next
              </button>
            </div>
          </div>
        ) : null}
      </div>
      <div className={`studio-file-preview studio-file-preview--${kind}`}>
        {contentState.status === "loading" ? (
          <div className="studio-file-preview__placeholder">
            <strong>{artifact.name}</strong>
            <span>Loading preview...</span>
          </div>
        ) : preview ? (
          <LensPreviewHost metadata={metadataForArtifact(artifact, lens, preview)} preview={preview} />
        ) : (
          <dl>
            <div>
              <dt>Type</dt>
              <dd>{artifact.type}</dd>
            </div>
            <div>
              <dt>Media</dt>
              <dd>{mediaTypeForArtifact(artifact) ?? "unknown"}</dd>
            </div>
            <div>
              <dt>Size</dt>
              <dd>{artifact.sizeLabel}</dd>
            </div>
            <div>
              <dt>Lens</dt>
              <dd>{lens?.id ?? "layrs.raw"}</dd>
            </div>
          </dl>
        )}
      </div>
    </div>
  );
}

export function resolveLensForArtifact(artifact: Artifact, manifests: LensManifest[]): LensManifest | undefined {
  try {
    return selectLensManifest(manifests, {
      lensId: optionalString(artifact, "lensId"),
      mediaType: mediaTypeForArtifact(artifact),
      path: artifact.location,
      fallbackLensId: "layrs.raw"
    })?.manifest;
  } catch (error) {
    if (error instanceof LensSelectionError) {
      return undefined;
    }
    throw error;
  }
}

export function diffKindForArtifact(artifact: Artifact, manifests: LensManifest[]): DiffKind {
  const lens = resolveLensForArtifact(artifact, manifests);
  return lens?.viewer.diffKinds[0] ?? "binary";
}

function previewForArtifact(artifact: Artifact, lens?: LensManifest): Pick<PreviewModel, "body" | "fields" | "kind" | "mediaType" | "title"> {
  const hintedPreview = objectValue(optionalUnknown(artifact, "preview"));
  const hintedKind = stringValue(hintedPreview?.kind) as PreviewKind | undefined;
  const inferredKind = hintedKind ?? inferPreviewKind(artifact, lens);
  const body =
    stringValue(hintedPreview?.body) ??
    optionalString(artifact, "content") ??
    optionalString(artifact, "body") ??
    optionalString(artifact, "raw") ??
    fallbackBodyForArtifact(artifact, inferredKind);
  const fields = objectValue(hintedPreview?.fields) ?? {};

  return {
    body,
    fields,
    kind: inferredKind,
    mediaType: stringValue(hintedPreview?.mediaType) ?? mediaTypeForArtifact(artifact) ?? "application/octet-stream",
    title: stringValue(hintedPreview?.title) ?? artifact.name
  };
}

function useArtifactContentPreview({
  artifact,
  fallbackPreview,
  layerId,
  lens,
  windowStart,
  workspaceId
}: {
  artifact?: Artifact;
  fallbackPreview?: Pick<PreviewModel, "body" | "fields" | "kind" | "mediaType" | "title">;
  layerId?: string;
  lens?: LensManifest;
  windowStart: number;
  workspaceId: string;
}): ContentState {
  const privateApi = useStudioPrivateApi();
  const artifactLayerId = artifact?.layerId ?? layerId;
  const requestKey = privateRequestKey(
    "artifact-preview",
    workspaceId,
    artifact?.spaceId,
    artifactLayerId,
    artifact?.id,
    windowStart
  );
  const [contentState, setContentState] = useState<KeyedContentState>({ status: "idle", requestKey: "" });

  useEffect(() => {
    if (!artifact || !workspaceId || !artifactLayerId || isArtifactRestricted(artifact)) {
      setContentState({ status: "idle", requestKey });
      return;
    }

    const baseUrl = runtimeApiBaseUrl();
    const path = [
      "/v1/workspaces",
      encodeURIComponent(workspaceId),
      "spaces",
      encodeURIComponent(artifact.spaceId),
      "layers",
      encodeURIComponent(artifactLayerId),
      "artifacts",
      encodeURIComponent(artifact.id)
    ].join("/");
    const windowQuery = new URLSearchParams({
      start: String(windowStart),
      limit: String(SERVER_PREVIEW_WINDOW_LIMIT)
    });

    setContentState({ status: "loading", requestKey });

    void privateApi
      .request("artifact-preview", requestKey, async ({ fetchJson, signal }) => {
        let diffError: unknown;
        try {
          const payload = await fetchJson(`${baseUrl}${path}/diff?${windowQuery}`);
          const windowed = previewFromWindowPayload(payload);
          if (windowed) {
            return { status: "available", ...windowed } as ContentState;
          }
        } catch (error) {
          if (signal.aborted) {
            throw error;
          }
          diffError = error;
        }

        try {
          const payload = await fetchJson(`${baseUrl}${path}/content`);
          const preview = previewFromContentPayload(payload, artifact, lens, fallbackPreview);
          return preview
            ? ({ status: "available", preview } as ContentState)
            : ({ status: "unavailable", message: "Preview content is not available for this file." } as ContentState);
        } catch (error) {
          if (signal.aborted) {
            throw error;
          }
          return {
            status: "unavailable",
            message: `Preview content is unavailable. ${optionalStatus(diffError ?? error)}`
          } as ContentState;
        }
      })
      .then((next) => setContentState({ ...next, requestKey }))
      .catch((error) => {
        if (!isPrivateRequestCancelled(error)) {
          setContentState({
            status: "unavailable",
            message: `Preview content is unavailable. ${optionalStatus(error)}`,
            requestKey
          });
        }
      });

    return () => privateApi.cancel("artifact-preview", requestKey);
  }, [artifact, artifactLayerId, fallbackPreview, lens, privateApi, requestKey, workspaceId]);

  return contentState.requestKey === requestKey ? contentState : { status: "loading" };
}

function previewFromWindowPayload(payload: unknown):
  | { preview?: PreviewModel; window: ArtifactWindowMetadata }
  | undefined {
  const windowed = normalizeArtifactPreviewWindowPayload(payload);
  if (!windowed?.preview) {
    return undefined;
  }

  return {
    preview: windowed.preview,
    window: windowed.window
  };
}

function previewFromContentPayload(
  payload: unknown,
  artifact: Artifact,
  lens: LensManifest | undefined,
  fallbackPreview: Pick<PreviewModel, "body" | "fields" | "kind" | "mediaType" | "title"> | undefined
): PreviewModel | undefined {
  const contentPayload = normalizeArtifactContentPayload(payload);
  if (!contentPayload) {
    return undefined;
  }

  const mediaType = contentPayload.content.mediaType ?? fallbackPreview?.mediaType ?? mediaTypeForArtifact(artifact) ?? "application/octet-stream";
  const artifactWithMedia = {
    ...artifact,
    mediaType,
    contentHash: artifact.contentHash ?? contentPayload.content.sha256,
    fileObject: artifact.fileObject ?? contentPayload.content.fileObject,
    fileObjectId: artifact.fileObjectId ?? contentPayload.content.fileObject?.fileObjectId,
    chunks: artifact.chunks ?? contentPayload.content.chunks
  };
  const inferredKind = inferPreviewKind(artifactWithMedia, lens);
  const value = contentPayload.content.value ?? "";
  const kind = isImagePreviewValue(value, mediaType) ? "image" : fallbackPreview?.kind ?? inferredKind;
  const fields = {
    ...(fallbackPreview?.fields ?? {}),
    ...contentPayload.fields,
    ...(artifact.rootTreeId ? { rootTreeId: artifact.rootTreeId } : {}),
    ...(kind === "image" && isRenderableUrl(value) ? { src: value, url: value } : {}),
    ...(kind === "image" && value && !isRenderableUrl(value) ? { data: value } : {})
  };

  return createPreviewModelFromArtifactContent({
    payload: {
      ...contentPayload,
      content: {
        ...contentPayload.content,
        mediaType
      },
      fields
    },
    artifact: artifactWithMedia,
    kind,
    title: fallbackPreview?.title ?? artifact.name,
    fields
  });
}

function metadataForArtifact(
  artifact: Artifact,
  lens: LensManifest | undefined,
  preview: Pick<PreviewModel, "fields" | "kind" | "mediaType" | "title">
): LensSurfaceMetadata {
  return {
    artifactId: artifact.id,
    lensId: lens?.id ?? "layrs.raw",
    kind: preview.kind,
    mediaType: preview.mediaType,
    byteLen: artifact.byteLen,
    contentHash: artifact.contentHash ?? stringValue(preview.fields.contentHash) ?? stringValue(preview.fields.sha256),
    fields: {
      ...preview.fields,
      fileObjectId: artifact.fileObjectId ?? stringValue(preview.fields.fileObjectId),
      rootTreeId: artifact.rootTreeId ?? stringValue(preview.fields.rootTreeId),
      location: artifact.location,
      sizeLabel: artifact.sizeLabel,
      type: artifact.type
    }
  };
}

function inferPreviewKind(artifact: Artifact, lens?: LensManifest): PreviewKind {
  const lensKind = lens?.viewer.previewKinds[0];
  if (lensKind === "code" || lensKind === "text" || lensKind === "image" || lensKind === "raw") {
    return lensKind;
  }

  const mediaType = mediaTypeForArtifact(artifact);
  if (mediaType?.startsWith("image/") || artifact.type === "image") {
    return "image";
  }

  const extension = extensionForArtifact(artifact);
  if (extension && ["css", "html", "js", "json", "jsx", "rs", "ts", "tsx"].includes(extension)) {
    return "code";
  }

  if (extension && ["md", "mdx", "txt"].includes(extension)) {
    return "text";
  }

  return "raw";
}

function fallbackBodyForArtifact(artifact: Artifact, kind: PreviewKind): string {
  if (kind === "code" || kind === "text") {
    return [
      artifact.summary,
      "",
      `Path: ${artifact.location}`,
      `Type: ${artifact.type}`,
      `Size: ${artifact.sizeLabel}`,
      "",
      "Preview content is not available. Studio is rendering file metadata."
    ].join("\n");
  }

  return "";
}

function extensionForArtifact(artifact: Artifact): string | undefined {
  const hintedExtension = optionalString(artifact, "extension");
  if (hintedExtension) {
    return hintedExtension.replace(/^\./, "").toLowerCase();
  }

  const match = artifact.location.toLowerCase().match(/\.([a-z0-9]+)(?:[?#].*)?$/);
  return match?.[1];
}

function mediaTypeForArtifact(artifact: Artifact): string | undefined {
  const hintedMediaType = optionalString(artifact, "mediaType");
  if (hintedMediaType) {
    return hintedMediaType;
  }

  const extension = extensionForArtifact(artifact);
  if (!extension) {
    return undefined;
  }

  const mediaTypes: Record<string, string> = {
    css: "text/css",
    gif: "image/gif",
    html: "text/html",
    jpeg: "image/jpeg",
    jpg: "image/jpeg",
    js: "application/javascript",
    json: "application/json",
    md: "text/markdown",
    png: "image/png",
    rs: "text/x-rust",
    svg: "image/svg+xml",
    ts: "application/typescript",
    tsx: "application/typescript",
    txt: "text/plain",
    webp: "image/webp"
  };

  return mediaTypes[extension];
}

function optionalString(source: object, key: keyof ArtifactHints): string | undefined {
  return stringValue((source as Record<string, unknown>)[key]);
}

function optionalUnknown(source: object, key: keyof ArtifactHints): unknown {
  return (source as Record<string, unknown>)[key];
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function objectValue(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function isRenderableUrl(value: string): boolean {
  return value.startsWith("http://") || value.startsWith("https://") || value.startsWith("data:");
}

function isImagePreviewValue(value: string, mediaType: string): boolean {
  return mediaType.startsWith("image/") || value.startsWith("data:image/") || isRenderableUrl(value);
}

function runtimeEnv(): Record<string, string | undefined> {
  return (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {};
}

function runtimeApiBaseUrl(): string {
  return (runtimeEnv().VITE_LAYRS_API_URL ?? "").replace(/\/$/, "");
}

function optionalStatus(error: unknown): string {
  return error instanceof Error && error.message ? `(${error.message})` : "";
}
