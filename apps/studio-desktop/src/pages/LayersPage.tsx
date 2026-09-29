import { useState } from "react";
import { GitBranch, Plus } from "lucide-react";
import { Badge, Button, Card, Dialog, PageHeader, TextField, useToast } from "@layrs/ui";
import { NativeMergePanel } from "../NativeMergePanel";
import { useWorkspace } from "../app/workspace";

export function LayersPage() {
  const workspace = useWorkspace();
  const { data, busy, activeLayer } = workspace;
  const toast = useToast();
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const locked = Boolean(busy) || Boolean(data?.needsRepair);

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
                {current ? (
                  <Badge tone="accent">Layer courant</Badge>
                ) : (
                  <Button
                    size="sm"
                    disabledReason={locked ? "Opération en cours" : workspace.conflicted ? "Résolvez d’abord les conflits." : undefined}
                    onClick={() => void workspace.run({ action: "switchLayer", layer: layer.layerId }, `Passage à ${layer.name}…`)}
                  >
                    Travailler sur ce Layer
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      </Card>
      <section aria-label="Fusion et résolution" className="layrs-legacy">
        <h2>Fusionner un autre Layer</h2>
        <NativeMergePanel
          key={`${workspace.root}:${activeLayer?.layerId}`}
          root={workspace.root}
          layer={activeLayer?.layerId ?? ""}
          disabled={locked}
          onBusy={(value) => workspace.setForeground(value ? "Fusion…" : null)}
          onChanged={() => void workspace.refresh()}
        />
      </section>
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
