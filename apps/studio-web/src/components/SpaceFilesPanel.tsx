import { useEffect, useMemo, useState } from "react";
import type { Artifact, Layer } from "@layrs/client-sdk";
import { EmptyState, PanelTitle, formatDate } from "./common";
import { LensFileViewer, resolveLensForArtifact, type LensRegistryState } from "./LensFileViewer";

export function SpaceFilesPanel({
  artifacts,
  layer,
  workspaceId,
  lensRegistry
}: {
  artifacts: Artifact[];
  layer?: Layer;
  workspaceId: string;
  lensRegistry: LensRegistryState;
}) {
  const visibleArtifacts = useMemo(
    () => (layer ? artifacts.filter((artifact) => artifact.layerId === layer.id) : artifacts),
    [artifacts, layer]
  );
  const [selectedArtifactId, setSelectedArtifactId] = useState<string>();
  const selectedArtifact =
    visibleArtifacts.find((artifact) => artifact.id === selectedArtifactId && canOpenArtifact(artifact)) ??
    visibleArtifacts.find(canOpenArtifact);

  useEffect(() => {
    if (selectedArtifactId && visibleArtifacts.some((artifact) => artifact.id === selectedArtifactId && canOpenArtifact(artifact))) {
      return;
    }

    setSelectedArtifactId(visibleArtifacts.find(canOpenArtifact)?.id);
  }, [selectedArtifactId, visibleArtifacts]);

  return (
    <section className="studio-panel studio-panel--wide" id="files">
      <PanelTitle eyebrow="Files" title={layer ? `${layer.name} files` : "Layer files"} />
      {visibleArtifacts.length === 0 ? (
        <EmptyState title="No files" detail="This Layer has no files yet." />
      ) : (
        <div className="studio-files-layout">
          <div className="studio-artifact-table" role="table" aria-label="Layer files">
            <div className="studio-artifact-table__head" role="row">
              <span role="columnheader">File</span>
              <span role="columnheader">Lens</span>
              <span role="columnheader">Updated</span>
            </div>
            {visibleArtifacts.map((artifact) => {
              const isRestricted = artifact.access?.isRedacted === true || artifact.access?.canOpen === false;
              const restriction = artifact.access?.reason || "Restricted by Layer access policy";
              const lens = resolveLensForArtifact(artifact, lensRegistry.manifests);

              return (
                <div
                  aria-selected={selectedArtifact?.id === artifact.id}
                  className={[
                    "studio-artifact-row",
                    isRestricted ? "is-redacted" : "",
                    selectedArtifact?.id === artifact.id ? "is-selected" : ""
                  ]
                    .filter(Boolean)
                    .join(" ")}
                  key={artifact.id}
                  onClick={() => {
                    if (!isRestricted) {
                      setSelectedArtifactId(artifact.id);
                    }
                  }}
                  onKeyDown={(event) => {
                    if (!isRestricted && (event.key === "Enter" || event.key === " ")) {
                      event.preventDefault();
                      setSelectedArtifactId(artifact.id);
                    }
                  }}
                  role="row"
                  tabIndex={isRestricted ? -1 : 0}
                >
                  <div role="cell">
                    <strong>{artifact.name}</strong>
                    <code>{isRestricted ? restriction : artifact.location}</code>
                    <p>{isRestricted ? "Restricted by Layer access policy" : artifact.summary}</p>
                  </div>
                  <span role="cell">{isRestricted ? "redacted" : lens?.name ?? "Raw"}</span>
                  <time role="cell">{formatDate(artifact.updatedAt)}</time>
                </div>
              );
            })}
          </div>
          <LensFileViewer artifact={selectedArtifact} layerId={layer?.id} lensRegistry={lensRegistry} workspaceId={workspaceId} />
        </div>
      )}
    </section>
  );
}

function canOpenArtifact(artifact: Artifact): boolean {
  return !artifact.access?.isRedacted && artifact.access?.canOpen !== false;
}
