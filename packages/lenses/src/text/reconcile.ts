import type { LensReconcileInput, LensReconcileResult } from "@layrs/lens-sdk";

export function prepareTextReconcile(_request: LensReconcileInput): LensReconcileResult {
  return {
    status: "unsupported",
    summary: "Text reconciliation is declared but not implemented yet.",
    blocks: [],
    segments: [],
    fields: {}
  };
}
