import { useState } from "react";
import { FolderInput } from "lucide-react";
import { Banner, Button, Card, NativeProjectFiles, PageHeader, TextField } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";
import { ProjectionSelection } from "./ProjectionSelection";
import { OfflineFiles } from "./OfflineFiles";

const mutationTitles = { create: "Créer un fichier vide", rename: "Renommer le fichier", move: "Déplacer le fichier" } as const;

/**
 * Files of a limited-access folder: the shared file list (reserved names with a
 * lock, no content) and path changes, always previewed then applied.
 */
export function LimitedFiles() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const { data, files, ready, namespacePreview, directoryPreview } = projection;
  const [source, setSource] = useState("");
  const [destination, setDestination] = useState("");
  const blocked = workspace.busy ? `Opération en cours : ${workspace.busy}` : !data || !ready ? "Actualisez l’accès limité avant de continuer." : undefined;

  return (
    <div className="layrs-page">
      <PageHeader
        title="Fichiers"
        description="Les chemins exclus par votre choix, les contenus indisponibles et les chemins réservés par les droits restent visibles séparément."
      />
      <ProjectionSelection />
      <OfflineFiles />
      {/* Shared file list, redesigned with the collaboration components. */}
      <div className="layrs-legacy">
        <NativeProjectFiles
          scope={`${workspace.root}:${data?.identity.principalId || ""}`}
          snapshot={files}
          busy={Boolean(workspace.busy)}
          onRefresh={projection.refresh}
          onPreviewMutation={data?.capabilities.namespace === true ? projection.previewNamespace : undefined}
        />
      </div>
      {namespacePreview && (
        <Card className="limited-preview">
          <section aria-label="Aperçu de modification des chemins">
            <h2>{mutationTitles[namespacePreview.mutation.kind]}</h2>
            <dl className="settings-facts">
              {namespacePreview.mutation.source && (
                <>
                  <dt>Source</dt>
                  <dd>
                    <code>{namespacePreview.mutation.source}</code>
                  </dd>
                </>
              )}
              <dt>Destination</dt>
              <dd>
                <code>{namespacePreview.mutation.path}</code>
              </dd>
            </dl>
            <p className="sync-muted">Les droits et les fichiers locaux sont vérifiés à nouveau avant application ; le changement est capturé localement.</p>
            <div className="limited-merge__actions">
              <Button onClick={projection.clearPreviews}>Fermer l’aperçu</Button>
              <Button variant="primary" disabledReason={blocked ?? (data?.capabilities.namespace !== true ? "Modification de chemins indisponible." : undefined)} onClick={() => void projection.applyNamespace()}>
                Appliquer ce changement de chemin
              </Button>
            </div>
          </section>
        </Card>
      )}
      <Card className="limited-preview">
        <section aria-label="Déplacement de dossier">
          <h2>
            <FolderInput aria-hidden size={18} /> Déplacer un dossier
          </h2>
          <p className="sync-muted">Tous les fichiers du dossier doivent être modifiables ; un nom réservé à la destination bloque l’opération.</p>
          <div className="sync-advanced__grid">
            <TextField
              label="Dossier source"
              value={source}
              onChange={(event) => {
                setSource(event.target.value);
                projection.clearPreviews();
              }}
            />
            <TextField
              label="Dossier destination"
              value={destination}
              onChange={(event) => {
                setDestination(event.target.value);
                projection.clearPreviews();
              }}
            />
          </div>
          <div className="limited-merge__actions">
            <Button
              disabledReason={blocked ?? (data?.capabilities.namespace !== true ? "Modification de chemins indisponible." : !source.trim() || !destination.trim() ? "Indiquez les deux dossiers." : undefined)}
              onClick={() => void projection.previewDirectory(source, destination)}
            >
              Préparer le déplacement du dossier
            </Button>
          </div>
          {directoryPreview && (
            <>
              <Banner tone="info" title={`${directoryPreview.local.source} → ${directoryPreview.local.destination}`}>
                {Object.keys(directoryPreview.local.sourceFiles).length} fichier(s) déplacé(s), puis capturé(s) localement.
              </Banner>
              <div className="limited-merge__actions">
                <Button variant="primary" disabledReason={blocked} onClick={() => void projection.applyDirectory()}>
                  Appliquer le déplacement du dossier
                </Button>
              </div>
            </>
          )}
        </section>
      </Card>
    </div>
  );
}
