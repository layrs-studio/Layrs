import type { DiffModel, LensDiffRendererProps, LensSurfaceMetadata } from "@layrs/lens-sdk";
import { RawLensFallback } from "../raw/fallback";
import { builtinLensRegistry } from "../registry";
import { getStringField } from "../shared/utils";

export interface LensDiffHostProps {
  diff?: DiffModel | null;
  metadata?: LensSurfaceMetadata | null;
  title?: string;
  emptyMessage?: string;
  className?: string;
}

export function LensDiffHost({
  diff,
  metadata,
  title = "Diff",
  emptyMessage = "Diff not available",
  className
}: LensDiffHostProps) {
  if (!diff) {
    return <RawLensFallback className={className} message={emptyMessage} metadata={metadata} title={title} />;
  }

  const lens = builtinLensRegistry.get(lensIdForDiff(diff, metadata));
  const renderer = lens?.viewer.renderDiff;

  if (renderer) {
    return (
      <>
        {renderer({
          diff,
          metadata,
          title,
          emptyMessage,
          className
        } satisfies LensDiffRendererProps)}
      </>
    );
  }

  return (
    <RawLensFallback
      className={className}
      fields={diff.fields}
      message="visual diff not available yet"
      metadata={metadata}
      title={title}
    />
  );
}

function lensIdForDiff(diff: DiffModel, metadata?: LensSurfaceMetadata | null): string | undefined {
  return metadata?.lensId ?? diff.metadata?.lensId ?? getStringField(diff.fields, "lensId");
}
