import { projectionCaptureMatches, type NativeCapturedProjectionPatch, type NativeProjectionStatus } from "@layrs/ui";
import { exactPaths, versionKey, type Version } from "./offline";

type File = { fileId: string; executable: boolean; bytes: number };
export type RestoreTarget = { version: Version; paths: string[] };
export type LocalRestorePreview = {
  destination: { identity: NativeProjectionStatus["identity"]; checkoutIdentity: string };
  operationId: string; source: RestoreTarget & { kind: "version" }; projectionGeneration: number;
  projectionDigest: string; checkoutDigest: string; afterCheckoutDigest: string;
  before: Record<string, File | null>; after: Record<string, File | null>; digest: string;
};
export type LocalRestoreReceipt = { operationId: string; capture: NativeCapturedProjectionPatch };
const fail = (): never => { throw new Error("Aperçu ou reçu de restauration incohérent. Préparez un nouvel aperçu local."); };
const record = (v: unknown): Record<string, unknown> => v && typeof v === "object" && !Array.isArray(v) ? v as Record<string, unknown> : fail();
const hash = (v: unknown) => typeof v === "string" && /^blake3:[a-f0-9]{64}$/.test(v);
function keys(value: object, expected: string[]) {
  return JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort());
}
export function localRestoreRequest(target: RestoreTarget, status: NativeProjectionStatus) {
  if (status.offline?.shared !== true || status.needsRepair || status.pendingPublication || status.capabilities.restore !== true) return fail();
  if (target.version.kind !== "remote" && target.version.worktreeId !== status.identity.worktreeId) return fail();
  const paths = exactPaths(target.paths.join("\n"), status.projection.snapshot.pathSemantics);
  if (JSON.stringify(paths) !== JSON.stringify(target.paths)) return fail();
  versionKey(target.version);
  return { action: "previewRestore", expectedGeneration: status.projection.generation, version: target.version, paths };
}
export function localRestorePreview(reply: unknown, target: RestoreTarget, status: NativeProjectionStatus): LocalRestorePreview {
  localRestoreRequest(target, status);
  const p = record(record(reply).preview), source = record(p.source);
  if (!keys(p, ["operationId", "destination", "source", "projectionGeneration", "projectionDigest", "checkoutDigest", "afterCheckoutDigest", "before", "after", "digest"]) || typeof p.operationId !== "string" || !/^[0-9-]{1,96}$/.test(p.operationId) || p.projectionGeneration !== status.projection.generation || p.projectionDigest !== status.projection.digest || ![p.digest, p.checkoutDigest, p.afterCheckoutDigest].every(hash)) return fail();
  const destination = record(p.destination), identity = record(destination.identity);
  const identityKeys = ["storeId", "worktreeId", "spaceId", "origin", "principalId"] as const;
  if (!keys(destination, ["identity", "checkoutIdentity"]) || !keys(identity, [...identityKeys]) || identityKeys.some(k => typeof identity[k] !== "string" || !identity[k] || identity[k] !== status.identity[k]) || typeof destination.checkoutIdentity !== "string" || !destination.checkoutIdentity.trim() || destination.checkoutIdentity.length > 4096 || /[\u0000-\u001f\u007f]/.test(destination.checkoutIdentity)) return fail();
  if (!keys(source, ["kind", "version", "paths"]) || source.kind !== "version" || versionKey(source.version as Version) !== versionKey(target.version) || JSON.stringify(source.paths) !== JSON.stringify(target.paths)) return fail();
  for (const side of [record(p.before), record(p.after)]) {
    if (!keys(side, target.paths)) return fail();
    for (const value of Object.values(side)) {
      if (value === null) continue;
      const file = record(value);
      if (!keys(file, ["fileId", "executable", "bytes"]) || !hash(file.fileId) || typeof file.executable !== "boolean" || !Number.isSafeInteger(file.bytes) || (file.bytes as number) < 0 || (file.bytes as number) > 1024 * 1024 * 1024) return fail();
    }
  }
  // Retain the exact typed native payload for apply, including its digest.
  return p as LocalRestorePreview;
}
export function localRestoreReceipt(reply: unknown, preview: LocalRestorePreview, status: NativeProjectionStatus): LocalRestoreReceipt {
  const receipt = record(record(reply).restoration);
  if (receipt.operationId !== preview.operationId || !projectionCaptureMatches(status, receipt.capture as NativeCapturedProjectionPatch)) return fail();
  return receipt as LocalRestoreReceipt;
}
