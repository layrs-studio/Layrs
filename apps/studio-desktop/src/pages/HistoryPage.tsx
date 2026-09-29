import { useState } from "react";
import { Redo2, RotateCcw, Undo2 } from "lucide-react";
import { Button, Card, EmptyState, PageHeader, Tabs, TextField, formatDateTime, formatRelativeTime, useLocation } from "@layrs/ui";
import { NativeHistoryPanel } from "../NativeHistoryPanel";
import { useWorkspace } from "../app/workspace";

type Tab = "captures" | "operations" | "restore";

const operationLabels: Record<string, string> = {
  initialize: "Initialisation",
  capture_worktree: "Capture",
  switch_layer: "Changement de Layer",
  move_layer_heads: "Déplacement de Layer",
  create_marker: "Repère créé",
  hide_marker: "Repère masqué",
  create_release: "Version créée",
  weave: "Fusion",
  resolve_conflict: "Conflit résolu",
  rewrite_steps: "Transformation",
  receive_remote_state: "Réception",
  integrate_operations: "Intégration",
  undo: "Annulation",
  redo: "Rétablissement",
  restore: "Restauration",
  revert: "Inversion",
  prune: "Nettoyage",
};

export function HistoryPage() {
  const workspace = useWorkspace();
  const { data, busy } = workspace;
  const { query } = useLocation();
  const [tab, setTab] = useState<Tab>(query.get("path") ? "restore" : "captures");
  const [step, setStep] = useState("");
  const [path, setPath] = useState(query.get("path") ?? "");
  const locked = Boolean(busy) || Boolean(data?.needsRepair);

  function previewStep(target: string, targetPath: string | null) {
    void workspace.inspect(
      { action: "previewRestore", step: target, path: targetPath },
      { action: "restore", step: target, path: targetPath },
      targetPath ? `Restaurer ${targetPath}` : "Restaurer le projet",
    );
  }

  function previewOperation(operation: string) {
    void workspace.inspect({ action: "previewView", operation }, { action: "restoreView", operation }, "Revenir à cet état");
  }

  return (
    <div className="layrs-page">
      <PageHeader
        title="Historique"
        description="Ce qui a été capturé et ce qui s’est passé sur cet ordinateur."
        aside={
          <>
            <Button
              icon={<Undo2 aria-hidden size={16} />}
              disabledReason={locked ? "Opération en cours" : (data?.operations.length ?? 0) < 2 ? "Rien à annuler." : undefined}
              onClick={() => void workspace.run({ action: "undo" }, "Annulation…")}
            >
              Annuler
            </Button>
            <Button
              icon={<Redo2 aria-hidden size={16} />}
              disabledReason={locked ? "Opération en cours" : data?.state.operation.kind !== "undo" ? "Rien à rétablir." : undefined}
              onClick={() => void workspace.run({ action: "redo", operation: null }, "Rétablissement…")}
            >
              Rétablir
            </Button>
          </>
        }
      />
      <Tabs<Tab>
        label="Vues de l’historique"
        value={tab}
        onChange={setTab}
        items={[
          { id: "captures", label: "Captures" },
          { id: "operations", label: "Opérations locales", count: data?.operations.length },
          { id: "restore", label: "Restaurer" },
        ]}
      >
        {tab === "captures" && (
          <div className="layrs-legacy">
            <NativeHistoryPanel
              root={workspace.root}
              disabled={locked}
              onStep={(id) => previewStep(id, null)}
              onOperation={previewOperation}
            />
          </div>
        )}
        {tab === "operations" &&
          (data?.operations.length ? (
            <Card>
              <ol className="history-list">
                {data.operations.map((operation) => (
                  <li key={operation.operationId} className="history-list__item">
                    <div className="history-list__text">
                      <p className="history-list__title">{operationLabels[operation.kind] ?? operation.kind}</p>
                      <p className="history-list__summary">{operation.summary}</p>
                      <time dateTime={operation.createdAt} title={formatDateTime(operation.createdAt)}>
                        {formatRelativeTime(operation.createdAt)}
                      </time>
                    </div>
                    <Button
                      size="sm"
                      icon={<RotateCcw aria-hidden size={14} />}
                      disabledReason={locked ? "Opération en cours" : undefined}
                      onClick={() => previewOperation(operation.operationId)}
                    >
                      Prévisualiser cet état
                    </Button>
                  </li>
                ))}
              </ol>
              <p className="history-list__note">Les 200 opérations les plus proches sont affichées ; la CLI permet d’en demander davantage.</p>
            </Card>
          ) : (
            <EmptyState title="Aucune opération locale">Les captures, changements de Layer et restaurations apparaîtront ici.</EmptyState>
          ))}
        {tab === "restore" && (
          <Card className="history-restore">
            <p>
              Choisissez une capture dans l’onglet Captures, ou saisissez sa référence. Un aperçu liste exactement ce qui sera
              remplacé, créé ou conservé avant toute écriture.
            </p>
            <TextField label="Capture ou point nommé" value={step} placeholder="blake3:… ou @marker:nom" onChange={(event) => setStep(event.target.value)} />
            <TextField
              label="Chemin facultatif"
              value={path}
              placeholder="Vide pour le projet entier"
              hint="Pour restaurer un seul fichier ou dossier."
              onChange={(event) => setPath(event.target.value)}
            />
            <div>
              <Button
                variant="primary"
                disabledReason={locked ? "Opération en cours" : !step ? "Indiquez la capture à restaurer." : undefined}
                onClick={() => previewStep(step, path || null)}
              >
                Prévisualiser
              </Button>
            </div>
          </Card>
        )}
      </Tabs>
    </div>
  );
}
