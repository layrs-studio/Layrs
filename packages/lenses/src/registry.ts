import { createLensRegistry, type LayrsLens, type LensRegistry } from "@layrs/lens-sdk";
import type { ReactNode } from "react";
import { codeLens } from "./code/lens";
import { imageLens } from "./image/lens";
import { rawLens } from "./raw/lens";
import { textLens } from "./text/lens";

export const builtinLenses = [codeLens, imageLens, textLens, rawLens] satisfies Array<LayrsLens<ReactNode>>;

export function createBuiltinLensRegistry(extraLenses: Array<LayrsLens<ReactNode>> = []): LensRegistry<ReactNode> {
  return createLensRegistry([...builtinLenses, ...extraLenses]);
}

export const builtinLensRegistry = createBuiltinLensRegistry();

export function listLenses() {
  return builtinLensRegistry.list().map((lens) => ({
    manifest: lens.manifest,
    priority: lens.priority ?? 50
  }));
}

export function getLensManifest(lensId: string) {
  return builtinLensRegistry.get(lensId)?.manifest;
}

export function resolveLens(request: Parameters<typeof builtinLensRegistry.resolve>[0]) {
  return builtinLensRegistry.resolve(request);
}
