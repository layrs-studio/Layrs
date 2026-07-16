import type { LensManifest } from "./contracts";
import { parseLensManifestList } from "./manifestValidation";

export interface LensRegistryResponse {
  items: LensManifest[];
  errors?: unknown[];
}

export function parseLensRegistryResponse(payload: unknown): LensRegistryResponse {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) {
    throw new Error("Lens registry response must be an object");
  }
  const record = payload as Record<string, unknown>;
  if (!Array.isArray(record.items)) {
    throw new Error("Lens registry response must contain a canonical items array");
  }
  if (Object.keys(record).some((key) => key !== "items" && key !== "errors")) {
    throw new Error("Lens registry response contains unknown fields");
  }
  if (record.errors !== undefined && !Array.isArray(record.errors)) {
    throw new Error("Lens registry response errors must be an array");
  }
  return {
    items: parseLensManifestList(record.items),
    ...(record.errors === undefined ? {} : { errors: record.errors })
  };
}
