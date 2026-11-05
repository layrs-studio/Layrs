import { useState } from "react";
import { GitBranch, GitMerge, Plus } from "lucide-react";
import { Badge, Banner, Button, Card, Dialog, Link, PageHeader, TextField, navigate, segment, useToast } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";

export function LayersPage() {
  const workspace = useWorkspace();
  const { data, busy, activeLayer } = workspace;
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const locked = Boolean(busy) || Boolean(data?.needsRepair);
  const key = segment(workspace.root);

  async function create() {
    const next = await workspace.run({ action: "createLayer", name: name.trim() }, "Création du Layer…");
    if (next) {
      toast.show({ tone: "success", title: "Layer créé", description: `${name.trim()} part de votre travail courant.` });
      setCreating(false);
      setName("");
    }
  }

  return (
    <div className="layrs-page">
      <PageHeader
        title="Layers"
        description="Vos lignes de travail. Changer de Layer capture d’abord le travail présent et ne publie rien."
        aside={
          <Button variant="primary" icon={<Plus aria-hidden size={16} />} disabledReason={locked ? "Opération en cours" : undefined} onClick={() => setCreating(true)}>
            Nouveau Layer
          </Button>
        }
      />
      {workspace.conflicted && (
        <Banner tone="warning" title="Fusion en cours avec des conflits" actions={<Button size="sm" onClick={() => navigate(`/projects/${key}/merge`)}>Résoudre les conflits</Button>}>
          Changer de Layer est impossible tant que les conflits ne sont pas résolus.
        </Banner>
      )}
      <Card>
        <ul className="layer-list">
          {data?.state.layers.map((layer) => {
            const current = layer.layerId === activeLayer?.layerId;
            return (
              <li key={layer.layerId} className="layer-list__item">
                <GitBranch aria-hidden size={18} />
                <div className="layer-list__text">
                  <p className="layer-list__name">{layer.name}</p>
                  <p className="layer-list__meta">
                    {layer.settings.automaticCapture ? "Capture automatique activée" : "Capture automatique désactivée"}
                  </p>
                </div>
                <div className="points__actions">
                  {current ? (
                    <>
                      <Badge tone="accent">Layer courant</Badge>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<GitMerge aria-hidden size={14} />}
                        onClick={() => navigate(`/projects/${key}/merge`, { query: { source: layer.name } })}
                      >
                        Réunir ses têtes…
                      </Button>
                    </>
                  ) : (
                    <>
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<GitMerge aria-hidden size={14} />}
                        disabledReason={workspace.conflicted ? "Une fusion est déjà en cours." : undefined}
                        onClick={() => navigate(`/projects/${key}/merge`, { query: { source: layer.name } })}
                      >
                        Fusionner dans {activeLayer?.name ?? "le Layer courant"}…
                      </Button>
                      <Button
                        size="sm"
                        disabledReason={locked ? "Opération en cours" : workspace.conflicted ? "Résolvez d’abord les conflits." : undefined}
                        onClick={() => void workspace.run({ action: "switchLayer", layer: layer.layerId }, `Passage à ${layer.name}…`)}
                      >
                        Travailler sur ce Layer
                      </Button>
                    </>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      </Card>
      <p className="layer-list__note">
        Les protections d’un Layer publié (approbations, recette) se règlent depuis <Link to={`/projects/${key}/team`}>Équipe</Link>. Aucun renommage ni
        suppression de Layer n’est proposé : le moteur ne les fournit pas encore.
      </p>
      <Dialog
        open={creating}
        onClose={() => setCreating(false)}
        title="Nouveau Layer"
        description="Le nouveau Layer part de votre travail courant, qui est capturé avant."
        footer={
          <>
            <Button onClick={() => setCreating(false)}>Annuler</Button>
            <Button
              variant="primary"
              busy={busy === "Création du Layer…"}
              busyLabel="Création…"
              disabledReason={!name.trim() ? "Donnez un nom au Layer." : undefined}
              onClick={() => void create()}
            >
              Créer depuis le travail courant
            </Button>
          </>
        }
      >
        <TextField
          label="Nom"
          value={name}
          autoFocus
          onChange={(event) => setName(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && name.trim()) void create();
          }}
        />
      </Dialog>
    </div>
  );
}
