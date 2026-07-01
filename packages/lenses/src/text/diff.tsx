import type { LensDiffRendererProps } from "@layrs/lens-sdk";
import { LensSurfaceHeader } from "../shared/LensSurfaceHeader";
import { TextLinesDiffViewer } from "../shared/TextLinesDiffViewer";
import { joinClassNames } from "../shared/utils";

export function TextLensDiff({ className, diff, title }: LensDiffRendererProps) {
  return (
    <section className={joinClassNames("layrs-lens-diff", className)} aria-label={title}>
      <LensSurfaceHeader summary={diff.summary} title={title} />
      <TextLinesDiffViewer diff={diff} />
    </section>
  );
}
