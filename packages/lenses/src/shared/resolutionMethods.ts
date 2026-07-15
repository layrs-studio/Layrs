import type {
  LensManifest,
  ResolutionMethod,
  ResolutionScope,
  ScopedResolutionMethod
} from "@layrs/lens-sdk";

export interface ResolutionMethodCarrier {
  resolutionMethods?: ScopedResolutionMethod[];
  supportedMethods?: ResolutionMethod[];
}

export function declaredResolutionMethods(
  manifest: LensManifest,
  scope: ResolutionScope
): ResolutionMethod[] {
  return manifest.resolutionMethods
    .filter((entry) => entry.scope === scope)
    .map((entry) => entry.method);
}

export function surfaceResolutionMethods(
  manifest: LensManifest,
  scope: ResolutionScope,
  carrier: ResolutionMethodCarrier
): ResolutionMethod[] {
  const declared = declaredResolutionMethods(manifest, scope);
  if (carrier.resolutionMethods !== undefined) {
    return carrier.resolutionMethods.map((entry) => {
      if (entry.scope !== scope || !declared.includes(entry.method)) {
        throw new Error(
          `Lens ${manifest.id} does not declare ${entry.method} for ${entry.scope}`
        );
      }
      return entry.method;
    });
  }

  if (carrier.supportedMethods !== undefined) {
    // Legacy host responses expose unscoped methods; the manifest remains the allowlist.
    return declared.filter((method) => carrier.supportedMethods?.includes(method));
  }

  return declared;
}
