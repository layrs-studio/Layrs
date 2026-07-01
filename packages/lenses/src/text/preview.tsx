import type { LensPreviewRendererProps, PreviewModel } from "@layrs/lens-sdk";
import { LensSurfaceHeader } from "../shared/LensSurfaceHeader";
import { getStringField, joinClassNames, type FieldRecord } from "../shared/utils";

export function TextLensPreview({ className, preview, title }: LensPreviewRendererProps) {
  return (
    <section className={joinClassNames("layrs-lens-preview", className)} aria-label={title}>
      <LensSurfaceHeader mediaType={preview.mediaType} title={title} />
      <LinePreview lines={getPreviewLines(preview)} />
    </section>
  );
}

function LinePreview({ lines }: { lines: string[] }) {
  return (
    <ol className="layrs-lens-preview__lines">
      {lines.length > 0 ? (
        lines.map((line, index) => (
          <li key={index}>
            <span>{index + 1}</span>
            <code>{line || " "}</code>
          </li>
        ))
      ) : (
        <li>
          <span>1</span>
          <code> </code>
        </li>
      )}
    </ol>
  );
}

function getPreviewLines(preview: PreviewModel): string[] {
  const fieldLines = preview.fields.lines;
  if (Array.isArray(fieldLines)) {
    return fieldLines.map(previewLineText);
  }

  const text =
    preview.body ??
    getStringField(preview.fields, "text") ??
    getStringField(preview.fields, "content") ??
    "";

  return text.length === 0 ? [] : text.split(/\r\n|\n|\r/);
}

function previewLineText(line: unknown): string {
  if (typeof line === "string") {
    return line;
  }

  if (line && typeof line === "object" && !Array.isArray(line)) {
    const record = line as FieldRecord;
    return (
      getStringField(record, "text") ??
      getStringField(record, "content") ??
      getStringField(record, "textSegment") ??
      getStringField(record, "text_segment") ??
      ""
    );
  }

  return line === undefined || line === null ? "" : String(line);
}
