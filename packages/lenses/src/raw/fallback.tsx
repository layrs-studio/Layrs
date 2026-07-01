import type { LensSurfaceMetadata } from "@layrs/lens-sdk";
import { formatBytes, getNumberValue, joinClassNames, shortenHash, stringifyValue, type FieldRecord } from "../shared/utils";

export interface RawLensFallbackProps {
  title?: string;
  message?: string;
  metadata?: LensSurfaceMetadata | null;
  fields?: FieldRecord;
  className?: string;
}

export function RawLensFallback({
  title = "Raw artifact",
  message = "Preview not available",
  metadata,
  fields,
  className
}: RawLensFallbackProps) {
  const rows = getMetadataRows(metadata, fields);

  return (
    <section className={joinClassNames("layrs-raw-lens", className)} aria-label={title}>
      <div className="layrs-raw-lens__message">
        <strong>{title}</strong>
        <p>{message}</p>
      </div>
      {rows.length > 0 ? (
        <dl className="layrs-raw-lens__metadata">
          {rows.map((row) => (
            <div key={row.label}>
              <dt>{row.label}</dt>
              <dd>{row.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
    </section>
  );
}

function getMetadataRows(metadata?: LensSurfaceMetadata | null, fields: FieldRecord = {}) {
  const mergedFields = {
    ...(metadata?.fields ?? {}),
    ...fields
  };

  const dimensions = metadata?.dimensions ?? getDimensions(mergedFields);
  const rows = [
    { label: "Kind", value: stringifyValue(metadata?.kind ?? mergedFields.kind) },
    { label: "Media type", value: stringifyValue(metadata?.mediaType ?? mergedFields.mediaType) },
    { label: "Size", value: formatBytes(getNumberValue(metadata?.byteLen ?? mergedFields.byteLen ?? mergedFields.size)) },
    { label: "Root tree", value: shortenHash(stringifyValue(mergedFields.rootTreeId)) },
    { label: "File object", value: shortenHash(stringifyValue(mergedFields.fileObjectId)) },
    { label: "Chunks", value: stringifyValue(mergedFields.chunkCount) },
    { label: "Dimensions", value: formatDimensions(dimensions) },
    { label: "Lens", value: stringifyValue(metadata?.lensId ?? mergedFields.lensId) },
    { label: "Hash", value: shortenHash(stringifyValue(metadata?.contentHash ?? mergedFields.contentHash)) }
  ];

  return rows.filter((row): row is { label: string; value: string } => Boolean(row.value));
}

function getDimensions(fields: FieldRecord): { width: number; height: number } | undefined {
  const width = getNumberValue(fields.width);
  const height = getNumberValue(fields.height);
  return width && height ? { width, height } : undefined;
}

function formatDimensions(dimensions: { width: number; height: number } | undefined): string | undefined {
  return dimensions ? `${dimensions.width} x ${dimensions.height}` : undefined;
}
