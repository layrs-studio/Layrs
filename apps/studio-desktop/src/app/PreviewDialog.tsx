import { Banner, Button, Dialog, diffStats } from "@layrs/ui";
import { NativeRewriteImpact } from "../NativeRewriteImpact";
import { useWorkspace } from "./workspace";

const statusLabel: Record<string, string> = { added: "Créé", deleted: "Supprimé", modified: "Remplacé" };

/** Shows exactly what an operation will change before it is applied. */
export function PreviewDialog() {
  const { preview, closePreview, applyPreview, busy, data } = useWorkspace();
  const blocked = Boolean(preview?.data.collisions?.length);
  const rewrite = preview?.data.rewrite;
  return (
    <Dialog
      open={Boolean(preview)}
      onClose={closePreview}
      size="lg"
      title={preview?.title ?? ""}
      description="Rien n’est modifié avant votre confirmation. Le travail présent sera capturé et les fichiers remplacés seront sauvegardés."
      dismissible={!busy}
      footer={
        <>
          <Button onClick={closePreview} disabled={Boolean(busy)}>
            Fermer l’aperçu
          </Button>
          <Button
            variant="primary"
            busy={busy === "Application…"}
            busyLabel="Application…"
            disabledReason={blocked ? "Des collisions bloquent cette opération." : undefined}
            onClick={() => void applyPreview()}
          >
            {rewrite ? "Appliquer la transformation" : "Appliquer cet aperçu"}
          </Button>
        </>
      }
    >
      {preview && (
        <div className="preview" role="region" aria-label="Prévisualisation de restauration">
          <h3>{rewrite ? "Effet de la transformation" : "Effet de la restauration"}</h3>
          {blocked && (
            <Banner tone="danger" title="Opération bloquée par des collisions">
              <ul className="preview__list">
                {preview.data.collisions?.map((collision) => (
                  <li key={collision.path}>
                    <code>{collision.path}</code> : {collision.reason}
                  </li>
                ))}
              </ul>
            </Banner>
          )}
          {preview.data.changes && (
            <ul className="preview__list" aria-label="Fichiers concernés">
              {preview.data.changes.length === 0 && <li>Aucun fichier ne change.</li>}
              {preview.data.changes.map((change) => {
                const stats = change.textDiff ? diffStats(change.textDiff) : null;
                return (
                  <li key={change.path}>
                    <span className="preview__status">{statusLabel[change.status] ?? change.status}</span>
                    <code>{change.path}</code>
                    {stats && (
                      <span className="preview__stats">
                        <span className="changes__added">+{stats.added}</span> <span className="changes__removed">−{stats.removed}</span>
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {rewrite && data && (
            <div className="layrs-legacy">
              <NativeRewriteImpact rewrite={rewrite} layers={data.state.layers} points={[...data.markers, ...data.releases]} />
            </div>
          )}
          {preview.data.layerHeads && (
            <p>
              La visibilité des Layers et des points nommés sera aussi restaurée : {preview.data.layerHeads.length} Layer
              {preview.data.layerHeads.length > 1 ? "s" : ""}, {preview.data.markerIds?.length || 0} repère(s),{" "}
              {preview.data.releaseIds?.length || 0} version(s).
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
