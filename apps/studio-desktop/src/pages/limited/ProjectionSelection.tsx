import { useState } from "react";
import { Button, Card, Link, segment } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useRemote } from "../../app/remote";
import { useWorkspace } from "../../app/workspace";
import { SelectionFields } from "../../app/SelectionFields";
import { selectionFromRules, type SelectionRule } from "../../app/selection";

export function ProjectionSelection() {
  const { data } = useProjection();
  if (!data) return null;
  return <SelectionDetails key={`${data.identity.worktreeId}:${data.projection.generation}`} />;
}

function SelectionDetails() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const remote = useRemote();
  const { data, ready } = projection;
  const [rules, setRules] = useState<SelectionRule[]>([]);
  if (!data) return null;
  const selection = data.selection;
  let additions: ReturnType<typeof selectionFromRules> | undefined;
  let error: string | undefined;
  try { additions = selectionFromRules(rules, data.projection.snapshot.pathSemantics); }
  catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
  const blocked = workspace.busy ? `Opération en cours : ${workspace.busy}`
    : !ready ? "Actualisez les fichiers avant de poursuivre."
    : data.pendingPublication ? "Terminez la publication en attente."
    : remote.session.state !== "connected" ? "Connectez-vous au serveur pour télécharger les chemins ajoutés."
    : error;
  return (
    <section className="settings-section" aria-labelledby="projection-selection">
      <h2 id="projection-selection">Sélection sur cet ordinateur</h2>
      <Card className="settings-card">
        <p className="sync-muted">La sélection détermine les chemins à télécharger. Les droits d’accès et les contenus indisponibles restent indiqués séparément.</p>
        {!selection ? <p>La sélection n’est pas exposée par cette version du moteur. Actualisez les fichiers avant de l’élargir.</p>
          : selection.kind === "all" ? <p>Tous les chemins sont sélectionnés. Seuls les contenus autorisés et pris en charge sont matérialisés.</p>
          : <>
            <details>
              <summary>{selection.paths.length} chemin(s) exact(s), {selection.subtrees.length} sous-arbre(s) · génération {data.projection.generation}</summary>
              <ul>
                {selection.paths.slice(0, 100).map(path => <li key={`path:${path}`}><code>{path}</code> · chemin exact</li>)}
                {selection.subtrees.slice(0, 100).map(path => <li key={`tree:${path}`}><code>{path}</code> · sous-arbre</li>)}
              </ul>
              {(selection.paths.length > 100 || selection.subtrees.length > 100) && <p>Affichage limité aux 100 premières règles de chaque type ; toutes les règles existantes sont conservées.</p>}
            </details>
            <SelectionFields rules={rules} onChange={setRules} disabled={Boolean(workspace.busy)} error={error} />
            <p className="sync-muted">Ajouter des chemins conserve votre sélection et votre travail local. Aucun retrait ni effacement de contenu n’est proposé.</p>
            {remote.session.state !== "connected" && <Link to={`/projects/${segment(workspace.root)}/connect`}>Connexion au serveur</Link>}
            <div><Button variant="primary" disabledReason={blocked} busy={workspace.busy === "Élargissement de la sélection…"} busyLabel="Téléchargement…" onClick={() => {
              if (!blocked && additions) void projection.expandSelection(data.projection.generation, additions);
            }}>Ajouter et télécharger</Button></div>
          </>}
      </Card>
    </section>
  );
}
