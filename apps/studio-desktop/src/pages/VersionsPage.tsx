import { useState, type ReactNode } from "react";
import { Bookmark, EyeOff, Plus, RotateCcw, Tag } from "lucide-react";
import { Button, Card, Dialog, EmptyState, PageHeader, TechnicalId, TextAreaField, TextField, useToast } from "@layrs/ui";
import { useWorkspace, type NamedPoint } from "../app/workspace";

type Kind = "marker" | "release";

export function VersionsPage() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const toast = useToast();
  const [creating, setCreating] = useState<Kind | null>(null);
  const [name, setName] = useState("");
  const [notes, setNotes] = useState("");
  const [hiddenMarker, setHiddenMarker] = useState("");
  const locked = Boolean(busy) || Boolean(data?.needsRepair);
  const markers = data?.markers ?? [];
  const releases = data?.releases ?? [];

  async function create(kind: Kind) {
    const request = kind === "marker" ? { action: "createMarker", name, notes } : { action: "createRelease", version: name, notes };
    if (await workspace.run(request, "Création…")) {
      toast.show({ tone: "success", title: kind === "marker" ? "Repère créé" : "Version créée", description: name });
      setCreating(null);
      setName("");
      setNotes("");
    }
  }

  function restore(point: NamedPoint) {
    void workspace.inspect(
      { action: "previewRestore", step: point.targetStepId, path: null },
      { action: "restore", step: point.targetStepId, path: null },
      `Restaurer ${point.name ?? point.version}`,
    );
  }

  return (
    <div className="layrs-page">
      <PageHeader
        title="Versions et repères"
        description="Nommez les états importants. Un repère local n’est pas une version rendue disponible à l’équipe."
        aside={
          <>
            <Button icon={<Bookmark aria-hidden size={16} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => setCreating("marker")}>
              Nouveau repère
            </Button>
            <Button variant="primary" icon={<Plus aria-hidden size={16} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => setCreating("release")}>
              Nouvelle version
            </Button>
          </>
        }
      />
      <PointList title="Versions" icon={<Tag aria-hidden size={18} />} points={releases} empty="Aucune version locale." locked={locked} onRestore={restore} />
      <PointList
        title="Repères"
        icon={<Bookmark aria-hidden size={18} />}
        points={markers}
        empty="Aucun repère."
        locked={locked}
        onRestore={restore}
        onHide={(point) => void workspace.run({ action: "hideMarker", id: point.markerId }, "Masquage…")}
      />
      <details className="versions-hidden">
        <summary>Rétablir un repère masqué</summary>
        <div className="versions-hidden__body">
          <TextField label="Identifiant du repère" value={hiddenMarker} placeholder="marker:…" onChange={(event) => setHiddenMarker(event.target.value)} />
          <Button
            disabledReason={locked ? "Opération en cours" : !hiddenMarker ? "Indiquez l’identifiant du repère." : undefined}
            onClick={() => void workspace.run({ action: "showMarker", id: hiddenMarker }, "Rétablissement…")}
          >
            Rétablir ce repère
          </Button>
          <p className="versions-hidden__note">Son identité et sa capture restent identiques. Le travail courant est conservé.</p>
        </div>
      </details>
      <Dialog
        open={creating !== null}
        onClose={() => setCreating(null)}
        title={creating === "marker" ? "Nouveau repère" : "Nouvelle version"}
        description="Le travail présent est capturé, puis nommé."
        footer={
          <>
            <Button onClick={() => setCreating(null)}>Annuler</Button>
            <Button
              variant="primary"
              busy={busy === "Création…"}
              disabledReason={!name ? "Indiquez un nom." : undefined}
              onClick={() => creating && void create(creating)}
            >
              {creating === "marker" ? "Créer un marker" : "Créer une release"}
            </Button>
          </>
        }
      >
        <TextField label="Nom ou version" value={name} autoFocus onChange={(event) => setName(event.target.value)} />
        <TextAreaField label="Notes" value={notes} onChange={(event) => setNotes(event.target.value)} />
      </Dialog>
    </div>
  );
}

function PointList({
  title,
  icon,
  points,
  empty,
  locked,
  onRestore,
  onHide,
}: {
  title: string;
  icon: ReactNode;
  points: NamedPoint[];
  empty: string;
  locked: boolean;
  onRestore: (point: NamedPoint) => void;
  onHide?: (point: NamedPoint) => void;
}) {
  return (
    <section aria-label={title} className="points">
      <h2>{title}</h2>
      {points.length ? (
        <Card>
          <ul className="points__list">
            {points.map((point) => (
              <li key={point.markerId ?? point.releaseId} className="points__item">
                {icon}
                <div className="points__text">
                  <h3>{point.name ?? point.version}</h3>
                  {point.notes && <p className="points__notes">{point.notes}</p>}
                  <TechnicalId value={point.targetStepId} label="Copier la capture" />
                </div>
                <div className="points__actions">
                  <Button size="sm" icon={<RotateCcw aria-hidden size={14} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => onRestore(point)}>
                    Prévisualiser
                  </Button>
                  {onHide && point.markerId && (
                    <Button size="sm" variant="ghost" icon={<EyeOff aria-hidden size={14} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => onHide(point)}>
                      Masquer
                    </Button>
                  )}
                </div>
              </li>
            ))}
          </ul>
        </Card>
      ) : (
        <EmptyState compact title={empty} />
      )}
    </section>
  );
}
