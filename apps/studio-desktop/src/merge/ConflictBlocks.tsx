import { useState } from "react";
import { CheckCircle2, CircleDashed, Eye } from "lucide-react";
import { Badge, Button, Checkbox, TechnicalId, TextAreaField } from "@layrs/ui";
import { useWorkspace } from "../app/workspace";
import { nativeWorkspace } from "../tauri";
import type { BlockDraft, Conflict, ConflictBlock, Request, VariantContent } from "./model";

type Props = {
  conflict: Conflict;
  layer: string;
  locked: boolean;
  onPreview: (title: string, request: Request, apply: Request) => Promise<void>;
  onChange: () => void;
};

/** Passage-level resolution: each applied choice keeps an intermediate capture. */
export function ConflictBlocks({ conflict, layer, locked, onPreview, onChange }: Props) {
  const blocks = conflict.document.blocks ?? [];
  const order = conflict.document.segments?.filter((segment) => segment.kind === "block").map((segment) => segment.blockId) ?? [];
  const ordered = [...blocks].sort((a, b) => order.indexOf(a.blockId) - order.indexOf(b.blockId));
  const open = blocks.filter((block) => !block.resolution).length;
  return (
    <section className="conflict__section" aria-label="Résolution par passages">
      <h3>
        Passages en conflit{" "}
        <span className="sync-muted">
          · {blocks.length - open} sur {blocks.length} résolu{blocks.length - open > 1 ? "s" : ""}
        </span>
      </h3>
      <p className="sync-muted">
        {open} passage{open > 1 ? "s" : ""} à résoudre. Chaque application conserve une capture intermédiaire ; le fichier est installé après le dernier
        passage. Les brouillons restent disponibles tant que le projet est ouvert.
      </p>
      <ol className="conflict__blocks">
        {ordered.map((block, index) => (
          <BlockEditor key={block.blockId} conflict={conflict} block={block} index={index} layer={layer} locked={locked} onPreview={onPreview} onChange={onChange} />
        ))}
      </ol>
    </section>
  );
}

function BlockEditor({ conflict, block, index, layer, locked, onPreview, onChange }: Props & { block: ConflictBlock; index: number }) {
  const workspace = useWorkspace();
  const terms = [
    ...block.baseValues.map((term) => ({ ...term, label: "Base" })),
    ...block.sideValues.map((term, position) => ({ ...term, label: `Variante ${position + 1}` })),
  ];
  // Block identities survive a partial resolution; the conflict identity may not.
  const key = `${layer}:block:${block.blockId}`;
  const [draft, setDraft] = useState<BlockDraft>(() => (workspace.mergeDrafts.get(key) as BlockDraft | undefined) ?? { text: "", sources: terms.map((term) => term.termId).sort() });
  const [contents, setContents] = useState<Record<string, VariantContent>>({});
  const [expanded, setExpanded] = useState(false);
  const number = index + 1;

  function update(value: BlockDraft) {
    setDraft(value);
    workspace.mergeDrafts.set(key, value);
    onChange();
  }

  function preview(title: string, choice: unknown) {
    const params = { layer, conflict: conflict.conflictId, block: block.blockId, choice };
    return onPreview(title, { action: "previewBlockResolution", ...params }, { action: "applyBlockResolution", ...params });
  }

  async function read(termId: string) {
    const content = await nativeWorkspace<VariantContent>(workspace.root, { action: "conflictBlockVariant", layer, conflict: conflict.conflictId, block: block.blockId, term: termId }).catch(
      () => ({ text: null }),
    );
    setContents((previous) => ({ ...previous, [termId]: content }));
  }

  return (
    <li className="conflict__block">
      <button type="button" className="conflict__block-toggle" aria-expanded={expanded} onClick={() => setExpanded((value) => !value)}>
        {block.resolution ? <CheckCircle2 aria-hidden size={16} className="conflict__resolved" /> : <CircleDashed aria-hidden size={16} />}
        <span>Passage {number}</span>
        <Badge tone={block.resolution ? "success" : "warning"}>{block.resolution ? "Résolu" : "À résoudre"}</Badge>
      </button>
      {expanded &&
        (block.resolution ? (
          <p className="sync-muted conflict__block-body">Ce choix est enregistré dans la capture. L’historique conserve les variantes et les captures précédentes.</p>
        ) : (
          <div className="conflict__block-body">
            <ul className="conflict__terms">
              {terms.map((term) => {
                const content = contents[term.termId];
                return (
                  <li key={term.termId} aria-label={`${term.label} du passage ${number}`} className="conflict__term">
                    <div className="conflict__variant-head">
                      <p className="conflict__variant-title">{term.label}</p>
                      <div className="conflict__variant-actions">
                        <Button size="sm" variant="ghost" icon={<Eye aria-hidden size={14} />} onClick={() => void read(term.termId)}>
                          Lire ce passage
                        </Button>
                        <Button size="sm" disabledReason={locked ? "Opération en cours." : undefined} onClick={() => void preview(`Passage ${number} : garder ${term.label}`, { method: "selectTerm", term: term.termId })}>
                          Prévisualiser ce choix
                        </Button>
                      </div>
                    </div>
                    {content &&
                      (content.text === null ? (
                        <p className="sync-muted">Passage illisible ici (trop volumineux ou non textuel). Utilisez la CLI.</p>
                      ) : (
                        <>
                          <pre className="conflict__content">{content.text}</pre>
                          <Button size="sm" variant="ghost" onClick={() => update({ text: content.text ?? "", sources: [term.termId] })}>
                            Copier dans le brouillon
                          </Button>
                        </>
                      ))}
                    <details className="merge__technical">
                      <summary>Identifiant</summary>
                      <TechnicalId value={term.termId} label="Copier" />
                    </details>
                  </li>
                );
              })}
            </ul>
            <TextAreaField label={`Texte du passage ${number}`} rows={5} maxLength={1024 * 1024} value={draft.text} onChange={(event) => update({ ...draft, text: event.target.value })} />
            <fieldset className="conflict__sources">
              <legend>Sources du passage {number}</legend>
              {terms.map((term) => (
                <Checkbox
                  key={term.termId}
                  label={term.label}
                  checked={draft.sources.includes(term.termId)}
                  onChange={(event) =>
                    update({ ...draft, sources: event.target.checked ? [...draft.sources, term.termId].sort() : draft.sources.filter((id) => id !== term.termId) })
                  }
                />
              ))}
            </fieldset>
            <p className="sync-muted">Un texte vide supprime ce passage. Les sources choisies sont enregistrées avec la résolution.</p>
            <div>
              <Button
                disabledReason={locked ? "Opération en cours." : !draft.sources.length ? "Choisissez au moins une source." : undefined}
                onClick={() => void preview(`Passage ${number} : texte rédigé`, { method: "manual", text: draft.text, sourceTermIds: draft.sources })}
              >
                Prévisualiser le texte de ce passage
              </Button>
            </div>
          </div>
        ))}
    </li>
  );
}
