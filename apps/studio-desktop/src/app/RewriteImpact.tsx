import { Banner, TechnicalId } from "@layrs/ui";

export type RewriteSummary = {
  kind: string;
  expectedOperationId: string;
  targetLayerId: string;
  targetHeadsBefore: string[];
  proposedStep: { stepId: string };
  conflictCount: number;
  rewriteMap: { oldStepId: string; newStepIds: string[] }[];
  markersUnchanged: string[];
  releasesUnchanged: string[];
};

type Point = { markerId?: string; releaseId?: string; name?: string; version?: string; targetStepId: string };

/** Impact of an advanced rewrite: moved Layer head, history mapping, retained named points. */
export function RewriteImpact({
  rewrite,
  layers,
  points,
}: {
  rewrite: RewriteSummary;
  layers: { layerId: string; name: string }[];
  points: Point[];
}) {
  const retained = [...rewrite.markersUnchanged, ...rewrite.releasesUnchanged];
  const layerName = layers.find((layer) => layer.layerId === rewrite.targetLayerId)?.name || rewrite.targetLayerId;
  return (
    <section aria-label="Déplacement des références" className="impact">
      <h3>Layer déplacé : {layerName}</h3>
      <table className="impact__table">
        <thead>
          <tr>
            <th scope="col">Capture actuelle</th>
            <th scope="col">Capture après application</th>
          </tr>
        </thead>
        <tbody>
          {rewrite.targetHeadsBefore.map((head) => (
            <tr key={head}>
              <td>
                <TechnicalId value={head} label="Copier" />
              </td>
              <td>
                <TechnicalId value={rewrite.proposedStep.stepId} label="Copier" />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {rewrite.rewriteMap.length > 0 && (
        <details className="impact__details">
          <summary>Correspondances de l’historique ({rewrite.rewriteMap.length})</summary>
          <ul>
            {rewrite.rewriteMap.map((item) => (
              <li key={item.oldStepId}>
                <code>{item.oldStepId}</code> → {item.newStepIds.map((id) => <code key={id}>{id}</code>)}
              </li>
            ))}
          </ul>
        </details>
      )}
      <h3>Points nommés conservés sur leur capture d’origine</h3>
      {!retained.length ? (
        <p className="impact__muted">Aucun repère ni version dans cette vue.</p>
      ) : (
        <ul className="impact__points">
          {retained.map((id) => {
            const point = points.find((candidate) => (candidate.markerId || candidate.releaseId) === id);
            return (
              <li key={id}>
                <strong>{point?.name || point?.version || id}</strong>
                {point && <span className="impact__muted"> · capture conservée</span>}
              </li>
            );
          })}
        </ul>
      )}
      {rewrite.conflictCount > 0 && (
        <Banner tone="warning" title={`${rewrite.conflictCount} conflit${rewrite.conflictCount > 1 ? "s" : ""} seront conservés dans le résultat`}>
          Choisissez ensuite les variantes dans l’espace de fusion ; les fichiers actuels restent en place.
        </Banner>
      )}
    </section>
  );
}
