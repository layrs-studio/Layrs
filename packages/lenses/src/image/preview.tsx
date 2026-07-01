import type { LensPreviewRendererProps, PreviewModel } from "@layrs/lens-sdk";
import { RawLensFallback } from "../raw/fallback";
import { LensSurfaceHeader } from "../shared/LensSurfaceHeader";
import { getStringField, joinClassNames } from "../shared/utils";

export function ImageLensPreview({ className, emptyMessage, metadata, preview, title }: LensPreviewRendererProps) {
  const imageSource = getImageSource(preview);

  if (imageSource) {
    return (
      <section className={joinClassNames("layrs-lens-preview", className)} aria-label={title}>
        <LensSurfaceHeader mediaType={preview.mediaType} title={title} />
        <div className="layrs-lens-preview__image">
          <img alt={title} src={imageSource} />
        </div>
      </section>
    );
  }

  return (
    <RawLensFallback
      className={className}
      fields={preview.fields}
      message="Image preview not available"
      metadata={{
        ...(metadata ?? {}),
        kind: preview.kind,
        mediaType: preview.mediaType,
        dimensions: preview.dimensions
      }}
      title={title ?? emptyMessage}
    />
  );
}

function getImageSource(preview: PreviewModel): string | undefined {
  const explicitSource =
    getStringField(preview.fields, "dataUrl") ??
    getStringField(preview.fields, "url") ??
    getStringField(preview.fields, "src") ??
    getStringField(preview.fields, "uri");

  if (explicitSource) {
    return explicitSource;
  }

  const base64 = getStringField(preview.fields, "data");
  if (!base64) {
    return undefined;
  }

  return base64.startsWith("data:") ? base64 : `data:${preview.mediaType};base64,${base64}`;
}
