import { useEffect, useState } from "react";
import { Bot, Workflow } from "lucide-react";
import { Button } from "../components/Button";
import { Badge, Banner, Card, EmptyState, Loading } from "../components/Feedback";
import { personName, type LayerRef, type Protection, type TeamTransport } from "./types";
import type { Directory } from "./useDirectory";
import { useTask } from "./useTask";

/**
 * P17 · Automatisations: which Layers require an automatic validation, with
 * which recipe and trusted runners. Editing happens in Protections, because
 * the server stores recipe and approvals as one protection.
 */
export function AutomationsPanel({
  scope,
  directory,
  administration,
  layers,
  onEdit,
  onOpenRuns,
}: {
  scope: string;
  directory: Directory;
  administration: TeamTransport;
  layers: LayerRef[];
  onEdit: (layerId: string) => void;
  onOpenRuns?: () => void;
}) {
  const task = useTask(scope);
  const [values, setValues] = useState<Map<string, Protection | null> | null>(null);

  useEffect(() => {
    void task.run("load", async (live) => {
      const entries = await Promise.all(layers.map(async (layer) => [layer.layerId, await administration<Protection | null>({ action: "protection", layer: layer.layerId })] as const));
      if (live()) setValues(new Map(entries));
    }, "Les protections n’ont pas pu être lues.");
  }, [scope, layers.map((layer) => layer.layerId).join("|")]);

  if (!layers.length)
    return (
      <EmptyState icon={<Workflow size={22} />} title="Aucun Layer publié">
        Les validations automatiques s’attachent à un Layer publié.
      </EmptyState>
    );
  return (
    <div className="team-panel team-panel--narrow">
      <p className="team-muted">
        Une validation automatique exécute une recette figée sur la capture exacte proposée. Seuls des comptes de service de confiance, membres du projet avec le rôle
        Automatisation, peuvent déposer la preuve. Les exécutions et leurs journaux se consultent depuis les revues.
      </p>
      {task.error && <Banner tone="danger">{task.error}</Banner>}
      {!values ? (
        <Loading label="Lecture des validations…" />
      ) : (
        <ul className="team-list" aria-label="Validations automatiques par Layer">
          {layers.map((layer) => {
            const value = values.get(layer.layerId) ?? null;
            const ci = Boolean(value?.recipeObjectId);
            return (
              <li key={layer.layerId} className="team-row">
                <Bot aria-hidden size={20} />
                <div className="team-row__main">
                  <p className="team-row__title">{layer.name}</p>
                  {ci && value ? (
                    <>
                      <p className="team-row__meta">
                        Exécuteurs : {value.trustedRunnerPrincipalIds.map((id) => personName(directory.people, id)).join(", ")} · preuve de moins de{" "}
                        {Math.round((value.maximumProofAgeSeconds / 3600) * 10) / 10} h
                      </p>
                      <p className="team-row__meta">
                        Recette <code className="team-code">{value.recipeObjectId}</code>
                      </p>
                    </>
                  ) : (
                    <p className="team-row__meta">{value ? "Protégé sans validation automatique." : "Aucune protection."}</p>
                  )}
                </div>
                <div className="team-row__aside">
                  <Badge tone={ci ? "success" : "info"}>{ci ? "Validation exigée" : "Non exigée"}</Badge>
                  <Button size="sm" onClick={() => onEdit(layer.layerId)} aria-label={`Régler la protection de ${layer.name}`}>
                    Régler
                  </Button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {onOpenRuns && (
        <div>
          <Button onClick={onOpenRuns}>Voir les exécutions dans les revues</Button>
        </div>
      )}
      <Card className="settings-card">
        <h3 className="team-section-title">Préparer un exécuteur</h3>
        <ol className="team-steps">
          <li>L’administrateur de l’instance crée un compte de service et remet son jeton à l’outil CI (Administration → Comptes de service).</li>
          <li>Un administrateur du projet lui donne le rôle Automatisation (Équipe et accès → Membres → Accorder un accès direct).</li>
          <li>La protection du Layer le désigne comme exécuteur de confiance, avec la recette exacte.</li>
        </ol>
      </Card>
    </div>
  );
}
