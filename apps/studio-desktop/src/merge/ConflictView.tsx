import { useEffect, useRef, useState } from "react";
import { Eye, FileCode2, PenLine } from "lucide-react";
import { Badge, Banner, Button, Checkbox, DiffView, EmptyState, SegmentedControl, SelectField, TechnicalId, TextAreaField, type DiffMode } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { nativeWorkspace } from "../tauri";
import { ConflictBlocks } from "./ConflictBlocks";
import { conflictPaths, namedTerms, shortStep, type Comparison, type Conflict, type Request, type VariantContent } from "./model";

type Props = {
  conflict: Conflict;
  layer: string;
  locked: boolean;
  onPreview: (title: string, request: Request, apply: Request) => Promise<void>;
  onChange: () => void;
};

/** One conflict: named sources, comparison, choices per file or passage, and the final text. */
export function ConflictView({ conflict, layer, locked, onPreview, onChange }: Props) {
  const workspace = useWorkspace();
  const paths = conflictPaths(conflict);
  const coupled = paths.length > 1;
  const terms = namedTerms(conflict);
  const variants = terms.filter((term) => term.label.startsWith("Variante"));
  const draftKey = `${layer}:${conflict.conflictId}:text`;
  const saved = workspace.mergeDrafts.get(draftKey) as { text: string; executable: boolean } | undefined;
  const [draft, setDraft] = useState(saved?.text ?? "");
  const [executable, setExecutable] = useState(saved?.executable ?? false);
  const [contents, setContents] = useState<Record<string, VariantContent>>({});
  const [error, setError] = useState("");

  function updateDraft(text: string, exec = executable) {
    setDraft(text);
    setExecutable(exec);
    workspace.mergeDrafts.set(draftKey, { text, executable: exec });
    onChange();
  }

  async function read(termId: string) {
    setError("");
    try {
      const content = await nativeWorkspace<VariantContent>(workspace.root, { action: "conflictVariant", layer, conflict: conflict.conflictId, term: termId });
      setContents((values) => ({ ...values, [termId]: content }));
    } catch (cause) {
      setError(String(cause));
    }
  }

  return (
    <article className="conflict" aria-label={`Conflit ${paths.join(", ")}`}>
      <header className="conflict__head">
        <FileCode2 aria-hidden size={20} />
        <h2 className="conflict__path" title={paths.join(", ")}>
          {paths.join(", ")}
        </h2>
        {coupled ? <Badge tone="warning">Renommage lié</Badge> : <Badge tone="info">{variants.length} variantes</Badge>}
      </header>

      {coupled ? (
        <Banner tone="info" title="Ce conflit concerne plusieurs chemins liés par un renommage.">
          Choisir une variante applique ensemble ses créations, suppressions et contenus à tous les chemins indiqués. La comparaison et l’édition d’un
          fichier seul ne sont pas disponibles ici.
        </Banner>
      ) : (
        <VariantComparison conflict={conflict} layer={layer} />
      )}

      {!!conflict.document.blocks?.length && (
        <ConflictBlocks conflict={conflict} layer={layer} locked={locked} onPreview={onPreview} onChange={onChange} />
      )}

      <section className="conflict__section" aria-label="Choisir une variante entière">
        <h3>Garder une variante entière</h3>
        <p className="sync-muted">Chaque variante reste dans l’historique, quel que soit votre choix.</p>
        <ul className="conflict__variants">
          {conflict.document.sides.map((side, index) => {
            const content = contents[side.termId];
            return (
              <li key={side.termId} className="conflict__variant" aria-label={`Variante ${index + 1}`}>
                <div className="conflict__variant-head">
                  <p className="conflict__variant-title">
                    Variante {index + 1} <span className="sync-muted">· capture {shortStep(side.provenance.stepId)}</span>
                  </p>
                  <div className="conflict__variant-actions">
                    {!coupled && (
                      <Button size="sm" variant="ghost" icon={<Eye aria-hidden size={14} />} disabledReason={locked ? "Opération en cours." : undefined} onClick={() => void read(side.termId)}>
                        Lire le contenu
                      </Button>
                    )}
                    <Button
                      size="sm"
                      disabledReason={locked ? "Opération en cours." : undefined}
                      onClick={() => {
                        const choices = { [conflict.conflictId]: side.termId };
                        void onPreview(`Garder la variante ${index + 1}`, { action: "previewResolution", layer, choices }, { action: "applyResolution", layer, choices });
                      }}
                    >
                      Prévisualiser cette variante
                    </Button>
                  </div>
                </div>
                {content &&
                  (content.text === null ? (
                    <p className="sync-muted">Contenu non textuel ou trop volumineux pour cet éditeur. Choisissez la variante entière ou utilisez la CLI.</p>
                  ) : (
                    <>
                      <pre className="conflict__content">{content.text}</pre>
                      <Button size="sm" variant="ghost" icon={<PenLine aria-hidden size={14} />} onClick={() => updateDraft(content.text ?? "")}>
                        Utiliser comme texte final
                      </Button>
                    </>
                  ))}
                <details className="merge__technical">
                  <summary>Identifiants</summary>
                  <TechnicalId value={side.termId} label="Copier la variante" />
                  <TechnicalId value={side.provenance.stepId} label="Copier la capture" />
                </details>
              </li>
            );
          })}
        </ul>
      </section>

      {!coupled && (
        <section className="conflict__section" aria-label="Texte final">
          <h3>Rédiger le texte final</h3>
          <p className="sync-muted">
            Le brouillon reste disponible tant que le projet est ouvert, même en changeant de conflit. Un texte vide crée un fichier vide.
          </p>
          <TextAreaField label="Contenu final" value={draft} rows={10} maxLength={1024 * 1024} className="conflict__editor" onChange={(event) => updateDraft(event.target.value)} />
          <Checkbox label="Fichier exécutable (Unix)" checked={executable} onChange={(event) => updateDraft(draft, event.target.checked)} />
          <div>
            <Button
              disabledReason={locked ? "Opération en cours." : undefined}
              onClick={() => {
                const params = { layer, conflict: conflict.conflictId, text: draft, executable };
                void onPreview("Utiliser le texte final", { action: "previewReplacement", ...params }, { action: "applyReplacement", ...params });
              }}
            >
              Prévisualiser le texte final
            </Button>
          </div>
        </section>
      )}
      {error && <Banner tone="danger" title="Lecture impossible."><code className="sync-detail">{error}</code></Banner>}
      <details className="merge__technical">
        <summary>Identifiant du conflit</summary>
        <TechnicalId value={conflict.conflictId} />
      </details>
    </article>
  );
}

/** Read-only comparison between two named sources, shown with the shared diff. */
function VariantComparison({ conflict, layer }: { conflict: Conflict; layer: string }) {
  const workspace = useWorkspace();
  const terms = namedTerms(conflict);
  const [before, setBefore] = useState(terms[0]?.termId ?? "");
  const [after, setAfter] = useState(terms.at(-1)?.termId ?? "");
  const [mode, setMode] = useState<DiffMode>("split");
  const [result, setResult] = useState<Comparison | null>(null);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const generation = useRef(0);

  useEffect(() => {
    if (!before || !after) return;
    const version = ++generation.current;
    setLoading(true);
    setError("");
    nativeWorkspace<Comparison>(workspace.root, { action: "compareConflictVariants", layer, conflict: conflict.conflictId, before, after })
      .then((value) => version === generation.current && setResult(value))
      .catch((cause) => version === generation.current && (setResult(null), setError(String(cause))))
      .finally(() => version === generation.current && setLoading(false));
  }, [workspace.root, layer, conflict.conflictId, before, after]);

  const label = (termId: string) => {
    const term = terms.find((item) => item.termId === termId);
    return term ? `${term.label}${term.stepId ? ` · ${shortStep(term.stepId)}` : ""}` : termId;
  };

  return (
    <section className="conflict__section" aria-label="Comparaison des variantes">
      <div className="conflict__compare-head">
        <h3>Comparer</h3>
        <SelectField label="Départ" value={before} onChange={(event) => setBefore(event.target.value)}>
          {terms.map((term) => (
            <option key={`before:${term.termId}`} value={term.termId}>
              {label(term.termId)}
            </option>
          ))}
        </SelectField>
        <SelectField label="Arrivée" value={after} onChange={(event) => setAfter(event.target.value)}>
          {terms.map((term) => (
            <option key={`after:${term.termId}`} value={term.termId}>
              {label(term.termId)}
            </option>
          ))}
        </SelectField>
        <SegmentedControl<DiffMode>
          label="Présentation de la comparaison"
          size="sm"
          value={mode}
          onChange={setMode}
          options={[
            { value: "split", label: "Côte à côte" },
            { value: "inline", label: "En ligne" },
          ]}
        />
      </div>
      <p className="sync-muted">Aperçu en lecture seule : aucune résolution n’est enregistrée.</p>
      {loading && !result && <p role="status">Comparaison…</p>}
      {error && <Banner tone="danger" title="Comparaison impossible."><code className="sync-detail">{error}</code></Banner>}
      {result &&
        (!result.available ? (
          <EmptyState compact title="Comparaison textuelle indisponible">
            Contenu binaire, volumineux ou structurel. Les variantes restent sélectionnables entières.
          </EmptyState>
        ) : !result.changes?.length ? (
          <EmptyState compact title="Contenu et mode identiques" />
        ) : (
          result.changes.map((change) => (
            <div key={change.path} className="conflict__diff">
              {change.executableChanged && <p className="sync-muted">Le mode exécutable change.</p>}
              {change.textOmitted || !change.textDiff ? (
                <p className="sync-muted">Le diff dépasse les limites de cet aperçu.</p>
              ) : (
                <DiffView lines={change.textDiff} mode={mode} beforeLabel={label(before)} afterLabel={label(after)} />
              )}
            </div>
          ))
        ))}
    </section>
  );
}
