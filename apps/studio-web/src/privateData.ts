import type { Artifact, StudioSnapshot } from "@layrs/client-sdk";

export function isArtifactRestricted(artifact: Artifact): boolean {
  return artifact.access?.isRedacted === true || artifact.access?.canOpen === false;
}

export function sanitizeArtifact(artifact: Artifact): Artifact {
  if (!isArtifactRestricted(artifact)) {
    return artifact;
  }
  return {
    id: artifact.id,
    spaceId: artifact.spaceId,
    layerId: artifact.layerId,
    name: artifact.name,
    type: artifact.type,
    summary: "Restricted by Layer access policy",
    location: artifact.location,
    updatedAt: artifact.updatedAt,
    sizeLabel: "restricted",
    proofIds: [],
    access: artifact.access
  };
}

export function sanitizeArtifacts(artifacts: Artifact[]): Artifact[] {
  return artifacts.map(sanitizeArtifact);
}

export function sanitizeStudioSnapshot(snapshot: StudioSnapshot): StudioSnapshot {
  return {
    ...snapshot,
    artifacts: sanitizeArtifacts(snapshot.artifacts)
  };
}

export function artifactAccessSignature(artifacts: Artifact[]): string {
  return artifacts
    .filter(isArtifactRestricted)
    .map((artifact) => `${artifact.id}:${artifact.location}`)
    .sort()
    .join("|");
}
