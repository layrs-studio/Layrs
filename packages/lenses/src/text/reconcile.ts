import type { LensReconcileRequest, ReconcileModel } from "@layrs/lens-sdk";

export function prepareTextReconcile(_request: LensReconcileRequest): ReconcileModel {
  return {
    status: "unsupported",
    summary: "Text reconciliation is declared but not implemented yet.",
    fields: {}
  };
}
