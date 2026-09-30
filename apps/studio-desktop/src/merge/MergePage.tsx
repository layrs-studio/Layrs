import { useCallback, useEffect, useRef, useState } from "react";
import { CheckCircle2, FileWarning, GitMerge, Link2 } from "lucide-react";
import { Badge, Banner, Button, Card, EmptyState, Link, Loading, PageHeader, SelectField, TechnicalId, TextField, navigate, segment, useLocation, useToast } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { nativeWorkspace } from "../tauri";
import { ConflictView } from "./ConflictView";
import { LimitedMerge } from "./LimitedMerge";
import { conflictPaths, remainingLabel, type Conflict, type MergePreview, type PendingMerge, type Request } from "./model";

/** P10: dedicated merge workspace. Limited-access folders use their own supported rebase. */
export function MergePage({ conflictId }: { conflictId?: string }) {
  const { kind } = useWorkspace();
  return kind === "projection" ? <LimitedMerge /> : <CompleteMerge conflictId={conflictId} />;
}

function CompleteMerge({ conflictId }: { conflictId?: string }) {
  const workspace = useWorkspace();
  const toast = useToast();
  const { data, activeLayer, conflicted, busy } = workspace;
  const { query } = useLocation();
  const layer = activeLayer?.layerId ?? "";
  const key = segment(workspace.root);
  const [conflicts, setConflicts] = useState<Conflict[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [pending, setPending] = useState<PendingMerge | null>(null);
  const [initial, setInitial] = useState<number | null>(null);
  const generation = useRef(0);

  const load = useCallback(async () => {
    if (!layer) return;
    const version = ++generation.current;
    setLoadError("");
    try {
      const result = await nativeWorkspace<{ conflicts: Conflict[] }>(workspace.root, { action: "conflicts", layer });
      if (version !== generation.current) return;
      setConflicts(result.conflicts);
      setInitial((value) => (value === null || result.conflicts.length > value ? result.conflicts.length : value));
    } catch (cause) {
      if (version === generation.current) setLoadError(String(cause));
    }
  }, [workspace.root, layer]);

  // Conflicts are re-read after each applied change and when the Layer changes.
  const operation = data?.state.operation.operationId;
  useEffect(() => {
    void load();
  }, [load, operation]);

  useEffect(() => setPending(null), [operation, layer]);

  const inspect = useCallback(
    async (title: string, request: Request, apply: Request) => {
      await workspace.task("Préparation de l’aperçu…", async () => {
        const value = await nativeWorkspace<MergePreview>(workspace.root, request);
        setPending({ title, value, apply: { ...apply, expectedOperation: value.expectedOperation, expectedStep: value.expectedStep } });
      });
    },
    [workspace],
  );

  async function apply() {
    if (!pending) return;
    const current = pending;
    const done = await workspace.task("Application…", async () => {
      await nativeWorkspace(workspace.root, current.apply);
      return true;
    });
    if (!done) return;
    setPending(null);
    await workspace.refresh();
    const result = await nativeWorkspace<{ conflicts: Conflict[] }>(workspace.root, { action: "conflicts", layer }).catch(() => null);
    if (result) {
      setConflicts(result.conflicts);
      toast.show({
        tone: "success",
        title: result.conflicts.length ? "Choix appliqué" : "Fusion terminée",
        description: result.conflicts.length
          ? `${result.conflicts.length} conflit${result.conflicts.length > 1 ? "s" : ""} restant${result.conflicts.length > 1 ? "s" : ""}. L’état précédent reste dans l’historique.`
          : "Les fichiers résultants sont installés ; l’état précédent reste dans l’historique.",
      });
      const next = result.conflicts.find((item) => item.conflictId === conflictId) ?? result.conflicts[0];
      navigate(next ? `/projects/${key}/merge/${segment(next.conflictId)}` : `/projects/${key}/merge`, { replace: true });
    }
  }

  const invalidate = useCallback(() => setPending(null), []);
  const list = conflicts ?? [];
  const selected = list.find((item) => item.conflictId === conflictId) ?? list[0];
  const locked = Boolean(busy) || Boolean(data?.needsRepair);

  return (
    <div className="merge">
      <PageHeader
        title="Fusion"
        description={
          list.length
            ? `Choisissez le résultat de chaque conflit dans ${activeLayer?.name ?? "ce Layer"}. Rien n’est appliqué sans aperçu.`
            : `Réunir un autre Layer ou des têtes concurrentes dans ${activeLayer?.name ?? "ce Layer"}.`
        }
        aside={
          list.length > 0 && (
            <p className="merge__progress" role="status">
              {initial && initial > list.length
                ? `${initial - list.length} sur ${initial} résolu${initial - list.length > 1 ? "s" : ""} · `
                : ""}
              {list.length} conflit{list.length > 1 ? "s" : ""} restant{list.length > 1 ? "s" : ""}
            </p>
          )
        }
      />
      {loadError && (
        <Banner tone="danger" title="Les conflits n’ont pas pu être lus." actions={<Button size="sm" onClick={() => void load()}>Réessayer</Button>}>
          <code className="sync-detail">{loadError}</code>
        </Banner>
      )}
      {conflicts === null && !loadError ? (
        <Loading label="Lecture des conflits…" />
      ) : list.length === 0 ? (
        <>
          {conflicted && (
            <Banner tone="warning" title="Fusion en cours">
              Le moteur signale encore une fusion inachevée. Actualisez ou ouvrez le diagnostic si ce message persiste.
            </Banner>
          )}
          <PrepareWeave source={query.get("source") ?? ""} locked={locked} onPreview={inspect} onChange={invalidate} />
        </>
      ) : (
        <div className="merge__workspace">
          <nav className="merge__list" aria-label="Conflits">
            <h2 className="merge__list-title">
              Conflits <span className="changes__count">({list.length})</span>
            </h2>
            <ul>
              {list.map((conflict) => {
                const paths = conflictPaths(conflict);
                const blocks = conflict.document.blocks ?? [];
                const open = blocks.filter((block) => !block.resolution).length;
                return (
                  <li key={conflict.conflictId}>
                    <Link
                      to={`/projects/${key}/merge/${segment(conflict.conflictId)}`}
                      replace
                      className="merge__item"
                      aria-current={conflict.conflictId === selected?.conflictId ? "true" : undefined}
                    >
                      <FileWarning aria-hidden size={18} />
                      <span className="merge__item-text">
                        <span className="merge__item-path">{paths.join(", ")}</span>
                        <span className="merge__item-meta">
                          {paths.length > 1 ? (
                            <>
                              <Link2 aria-hidden size={12} /> Chemins liés par un renommage
                            </>
                          ) : blocks.length ? (
                            `${open} passage${open > 1 ? "s" : ""} sur ${blocks.length} à résoudre`
                          ) : (
                            `${conflict.document.sides.length} variantes`
                          )}
                        </span>
                      </span>
                    </Link>
                  </li>
                );
              })}
            </ul>
          </nav>
          <section className="merge__detail" aria-label="Conflit sélectionné">
            {selected && (
              <ConflictView
                key={selected.conflictId}
                conflict={selected}
                layer={layer}
                locked={locked}
                onPreview={inspect}
                onChange={invalidate}
              />
            )}
          </section>
        </div>
      )}
      {pending && (
        <section className="merge__preview" aria-label="Aperçu de fusion">
          <div className="merge__preview-text">
            <p className="merge__preview-title">{pending.title}</p>
            <p>{remainingLabel(pending.value.conflicts.length)}</p>
            <p className="sync-muted">
              {pending.value.conflicts.length
                ? "Les variantes restantes sont conservées ; les fichiers actuels restent en place jusqu’à la résolution complète."
                : "Les fichiers résultants seront installés ; l’état précédent reste dans l’historique."}{" "}
              Si le projet change avant l’application, un nouvel aperçu sera demandé.
            </p>
            <details className="merge__technical">
              <summary>Capture résultante</summary>
              <TechnicalId value={pending.value.expectedStep} />
            </details>
          </div>
          <div className="merge__preview-actions">
            <Button onClick={() => setPending(null)} disabled={busy === "Application…"}>
              Fermer l’aperçu
            </Button>
            <Button variant="primary" busy={busy === "Application…"} busyLabel="Application…" disabledReason={busy && busy !== "Application…" ? `Opération en cours : ${busy}` : undefined} onClick={() => void apply()}>
              Appliquer cet aperçu
            </Button>
          </div>
        </section>
      )}
    </div>
  );
}

function PrepareWeave({
  source: initialSource,
  locked,
  onPreview,
  onChange,
}: {
  source: string;
  locked: boolean;
  onPreview: (title: string, request: Request, apply: Request) => Promise<void>;
  onChange: () => void;
}) {
  const workspace = useWorkspace();
  const { data, activeLayer } = workspace;
  const layers = data?.state.layers ?? [];
  const others = layers.filter((layer) => layer.layerId !== activeLayer?.layerId);
  const [source, setSource] = useState(initialSource || others[0]?.name || activeLayer?.name || "");
  const [reference, setReference] = useState("");
  const chosen = reference.trim() || source;
  const target = activeLayer?.layerId ?? "";

  return (
    <Card className="merge__prepare">
      <div className="merge__prepare-head">
        <GitMerge aria-hidden size={22} />
        <div>
          <h2>Préparer une fusion</h2>
          <p className="sync-muted">
            Les changements de la source sont réunis dans <strong>{activeLayer?.name ?? "le Layer courant"}</strong>. Un aperçu montre les conflits avant
            toute écriture ; rien n’est publié.
          </p>
        </div>
      </div>
      <SelectField
        label="Source de la fusion"
        value={source}
        onChange={(event) => {
          setSource(event.target.value);
          onChange();
        }}
      >
        {others.map((layer) => (
          <option key={layer.layerId} value={layer.name}>
            {layer.name}
          </option>
        ))}
        {activeLayer && <option value={activeLayer.name}>Têtes concurrentes de {activeLayer.name}</option>}
      </SelectField>
      <details className="merge__advanced">
        <summary>Autre référence (avancé)</summary>
        <TextField
          label="Référence de la source"
          value={reference}
          placeholder="@marker:version ou blake3:…"
          hint="Remplace le choix ci-dessus : repère, version ou capture exacte."
          onChange={(event) => {
            setReference(event.target.value);
            onChange();
          }}
        />
      </details>
      <div>
        <Button
          variant="primary"
          disabledReason={locked ? "Opération en cours." : !chosen ? "Choisissez la source." : !target ? "Aucun Layer courant." : undefined}
          onClick={() => void onPreview(`Fusionner ${chosen} dans ${activeLayer?.name ?? "ce Layer"}`, { action: "previewWeave", source: chosen, target }, { action: "applyWeave", source: chosen, target })}
        >
          Prévisualiser la fusion
        </Button>
      </div>
      <EmptyState compact icon={<CheckCircle2 size={22} />} title="Aucun conflit en cours">
        Les conflits d’une fusion appliquée apparaîtront ici, un par fichier, avec base, variantes et résultat proposé.
      </EmptyState>
      {layers.length < 2 && (
        <p className="sync-muted">
          Un seul Layer existe : créez-en un depuis <Link to={`/projects/${segment(workspace.root)}/layers`}>Layers</Link> pour travailler en parallèle.{" "}
          <Badge tone="info">{layers.length} Layer</Badge>
        </p>
      )}
    </Card>
  );
}
