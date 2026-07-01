import type { LensPreviewRendererProps, LensSurfaceMetadata, PreviewModel } from "@layrs/lens-sdk";
import { builtinLensRegistry } from "../registry";
import { RawLensFallback } from "../raw/fallback";
import { getStringField } from "../shared/utils";

export interface LensPreviewHostProps {
  preview?: PreviewModel | null;
  metadata?: LensSurfaceMetadata | null;
  title?: string;
  emptyMessage?: string;
  className?: string;
}

export function LensPreviewHost({
  preview,
  metadata,
  title,
  emptyMessage = "Preview not available",
  className
}: LensPreviewHostProps) {
  if (!preview) {
    return <RawLensFallback className={className} message={emptyMessage} metadata={metadata} title={title ?? "Preview"} />;
  }

  const surfaceTitle = title ?? preview.title;
  const lens = builtinLensRegistry.get(lensIdForPreview(preview, metadata));
  const renderer = lens?.viewer.renderPreview;

  if (renderer) {
    return (
      <>
        {renderer({
          preview,
          metadata,
          title: surfaceTitle,
          emptyMessage,
          className
        } satisfies LensPreviewRendererProps)}
      </>
    );
  }

  return (
    <RawLensFallback
      className={className}
      fields={preview.fields}
      message={emptyMessage}
      metadata={{
        ...(metadata ?? {}),
        kind: preview.kind,
        mediaType: preview.mediaType,
        dimensions: preview.dimensions
      }}
      title={surfaceTitle}
    />
  );
}

function lensIdForPreview(preview: PreviewModel, metadata?: LensSurfaceMetadata | null): string | undefined {
  return metadata?.lensId ?? getStringField(preview.fields, "lensId");
}
