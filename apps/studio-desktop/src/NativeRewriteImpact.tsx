export type RewriteSummary = {
  kind: string; expectedOperationId: string; targetLayerId: string; targetHeadsBefore: string[];
  proposedStep: { stepId: string }; conflictCount: number;
  rewriteMap: { oldStepId: string; newStepIds: string[] }[];
  markersUnchanged: string[]; releasesUnchanged: string[];
};
type Point = { markerId?: string; releaseId?: string; name?: string; version?: string; targetStepId: string };
export function NativeRewriteImpact({ rewrite, layers, points }: { rewrite: RewriteSummary; layers: { layerId: string; name: string }[]; points: Point[] }) {
  const retained = [...rewrite.markersUnchanged, ...rewrite.releasesUnchanged];
  return <section aria-label="Déplacement des références">
    <h3>Layer déplacé : {layers.find(layer => layer.layerId === rewrite.targetLayerId)?.name || rewrite.targetLayerId}</h3>
    <code>{rewrite.targetLayerId}</code><table><thead><tr><th>Capture actuelle</th><th>Capture après application</th></tr></thead><tbody>
      {rewrite.targetHeadsBefore.map(head => <tr key={head}><td><code>{head}</code></td><td><code>{rewrite.proposedStep.stepId}</code></td></tr>)}
    </tbody></table>
    {rewrite.rewriteMap.length > 0 && <details><summary>Correspondances de l’historique ({rewrite.rewriteMap.length})</summary><ul>{rewrite.rewriteMap.map(item => <li key={item.oldStepId}><code>{item.oldStepId}</code> → {item.newStepIds.map(id => <code key={id}>{id}</code>)}</li>)}</ul></details>}
    <h3>Points conservés sur leurs captures d’origine</h3>
    {!retained.length ? <p>Aucun marker ou release dans cette vue.</p> : <ul>{retained.map(id => {
      const point = points.find(point => (point.markerId || point.releaseId) === id);
      return <li key={id}>{point?.name || point?.version || id}<br /><code>{id}</code>{point && <><br /><span>Capture conservée : </span><code>{point.targetStepId}</code></>}</li>;
    })}</ul>}
    {rewrite.conflictCount > 0 && <p role="alert">{rewrite.conflictCount} conflits seront conservés dans le résultat. Choisissez ensuite les variantes dans le panneau de résolution ; les fichiers actuels restent en place.</p>}
  </section>;
}
