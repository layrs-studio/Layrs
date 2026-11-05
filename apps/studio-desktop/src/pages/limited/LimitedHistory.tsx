import { useEffect, useState } from "react";
import { RotateCcw, Undo2 } from "lucide-react";
import { Badge, Banner, Button, Card, Checkbox, EmptyState, Loading, PageHeader, TechnicalId } from "@layrs/ui";
import type { NativeCapturedProjectionPatch } from "@layrs/ui";
import { useProjection } from "../../app/projection";
import { useWorkspace } from "../../app/workspace";

function changeLabel(change: NativeCapturedProjectionPatch["changes"][number]) {
  return change.expected === null ? "Ajout" : change.replacement === null ? "Suppression" : "Modification";
}

/** Local captures of a limited-access folder and path-level restore through a preview. */
export function LimitedHistory() {
  const workspace = useWorkspace();
  const projection = useProjection();
  const { data, ready, history, restorePreview, lastRestore } = projection;
  const [selected, setSelected] = useState<number | null>(null);
  const [paths, setPaths] = useState<string[]>([]);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!data || loaded) return;
    setLoaded(true);
    void projection.loadHistory();
  }, [data, loaded, projection]);

  const capture = history.find((item) => item.sequence === selected);
  const restoreReason = workspace.busy
    ? `Opération en cours : ${workspace.busy}`
    : !data || !ready
      ? "Actualisez l’accès limité avant de continuer."
      : data.capabilities.restore !== true
        ? "La restauration n’est pas disponible tant qu’une publication attend son reçu."
        : undefined;

  function select(item: NativeCapturedProjectionPatch) {
    setSelected(item.sequence);
    setPaths(item.changes.map((change) => change.path));
    projection.clearPreviews();
  }

  return (
    <div className="layrs-page">
      <PageHeader
        title="Historique"
        description="Captures de cet accès limité sur cet ordinateur. Une restauration ne concerne que des chemins autorisés et passe par un aperçu."
        aside={
          lastRestore && (
            <Button icon={<Undo2 aria-hidden size={16} />} disabledReason={restoreReason} onClick={() => void projection.prepareRestore("inverse")}>
              Annuler la dernière restauration…
            </Button>
          )
        }
      />
      <div className="limited-history">
        <Card className="limited-history__list">
          <h2 className="merge__list-title">Captures locales</h2>
          {!loaded || (workspace.busy === "Lecture de l’historique…" && !history.length) ? (
            <Loading label="Lecture de l’historique…" />
          ) : history.length ? (
            <ol aria-label="Captures locales">
              {history.map((item) => (
                <li key={item.sequence}>
                  <button type="button" className="merge__item" aria-current={item.sequence === selected ? "true" : undefined} onClick={() => select(item)}>
                    <span className="merge__item-text">
                      <span className="merge__item-path">Capture {item.sequence}</span>
                      <span className="merge__item-meta">
                        {item.changes.length} chemin{item.changes.length > 1 ? "s" : ""}
                      </span>
                    </span>
                  </button>
                </li>
              ))}
            </ol>
          ) : (
            <EmptyState compact title="Aucune capture locale">Capturez vos modifications autorisées depuis Changements.</EmptyState>
          )}
          <Button size="sm" variant="ghost" disabledReason={workspace.busy ? "Opération en cours." : undefined} onClick={() => void projection.loadHistory()}>
            Actualiser (50 dernières)
          </Button>
        </Card>
        <section className="limited-history__detail" aria-label="Capture sélectionnée">
          {capture ? (
            <Card className="limited-history__card">
              <header className="limited-merge__head">
                <h2>Capture {capture.sequence}</h2>
                <TechnicalId value={capture.digest} label="Copier l’empreinte" />
              </header>
              <fieldset className="limited-history__paths">
                <legend>Chemins à restaurer depuis cette capture</legend>
                {capture.changes.map((change) => (
                  <Checkbox
                    key={change.path}
                    label={
                      <>
                        <code>{change.path}</code> <Badge tone="info">{changeLabel(change)}</Badge>
                      </>
                    }
                    checked={paths.includes(change.path)}
                    onChange={(event) => {
                      projection.clearPreviews();
                      setPaths((current) => (event.target.checked ? [...current, change.path] : current.filter((path) => path !== change.path)));
                    }}
                  />
                ))}
                {!capture.changes.length && <p className="sync-muted">Aucune modification dans cette capture.</p>}
              </fieldset>
              <p className="sync-muted">Les droits actuels sont vérifiés à nouveau ; les autres fichiers restent inchangés.</p>
              <div>
                <Button
                  icon={<RotateCcw aria-hidden size={16} />}
                  disabledReason={restoreReason ?? (!paths.length ? "Choisissez au moins un chemin." : undefined)}
                  onClick={() => void projection.prepareRestore({ sequence: capture.sequence, paths: [...new Set(paths)] })}
                >
                  Préparer la restauration locale
                </Button>
              </div>
            </Card>
          ) : (
            <EmptyState compact title="Choisissez une capture">Sélectionnez une capture pour voir ses chemins et préparer une restauration.</EmptyState>
          )}
          {restorePreview && (
            <Card className="limited-history__card">
              <section aria-label="Aperçu de restauration locale">
                <h2>Fichiers à restaurer</h2>
                <ul className="preview__list">
                  {Object.entries(restorePreview.local.after).map(([path, target]) => (
                    <li key={path}>
                      <span className="preview__status">
                        {target === null ? "Suppression" : restorePreview.local.before[path] === null ? "Création" : "Remplacement"}
                      </span>
                      <code>{path}</code>
                    </li>
                  ))}
                </ul>
                <Banner tone="info" title="Les octets remplacés sont conservés dans le journal pour permettre l’annulation." />
                <div className="limited-merge__actions">
                  <Button onClick={projection.clearPreviews}>Fermer l’aperçu</Button>
                  <Button variant="primary" disabledReason={restoreReason} busy={workspace.busy === "Restauration…"} busyLabel="Restauration…" onClick={() => void projection.applyRestore()}>
                    Appliquer la restauration locale
                  </Button>
                </div>
              </section>
            </Card>
          )}
        </section>
      </div>
    </div>
  );
}
