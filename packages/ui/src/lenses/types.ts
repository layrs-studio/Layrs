/** Wire contract shared by Web and the allowlisted native transport. */
export type LensPin = { lensId: string; version: string; digest: string };
export type LensManifest = {
  contractVersion: string; id: string; name: string; version: string; digest?: string;
  analyzer: { fileExtensions: string[]; supportedMediaTypes: string[]; capabilities: string[] };
  viewer: { component: string; [key: string]: unknown };
  bundle?: { format: string; signature?: unknown; entrypoints: { viewer?: { runtime: string; path: string; export: string; digest: string; resources: { maxInputBytes: number; maxOutputBytes: number; maxExecutionMs: number } }; [key: string]: unknown } };
  [key: string]: unknown;
};
export type LensCatalogItem = { manifest: LensManifest; pin: LensPin; source: string; available: boolean; usageCount?: number; state?: string };
export type LensCatalog = { items: LensCatalogItem[]; errors: unknown[] };
export type LensConfiguration = {
  revision: number; pins: LensPin[]; associations: { extension: string; lensId: string }[];
  availableCatalog: LensCatalogItem[]; canManage?: boolean;
};
export type LensPacket = { manifest: LensManifest; files: { path: string; bytesBase64: string }[] };
export type LensTransport = <T>(command: { action: string; [key: string]: unknown }) => Promise<T>;
export type LensView = { title?: string; blocks: { kind: "text" | "code" | "heading"; text: string }[] };
export type LensViewInput = { kind: "preview" | "diff"; path: string; text?: string; lines?: unknown[]; mediaType?: string };
export const pinKey = (pin: LensPin) => `${pin.lensId}@${pin.version}:${pin.digest}`;
