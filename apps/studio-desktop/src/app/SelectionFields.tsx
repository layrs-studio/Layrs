import { useRef } from "react";
import { Button, SelectField, TextField } from "@layrs/ui";
import type { SelectionRule } from "./selection";

/** A small literal-path list, reused for cloning and additive expansion. */
export function SelectionFields({ rules, onChange, disabled, error }: {
  rules: SelectionRule[];
  onChange: (rules: SelectionRule[]) => void;
  disabled?: boolean;
  error?: string;
}) {
  const nextId = useRef(1);
  return (
    <fieldset disabled={disabled} className="settings-form">
      <legend>Chemins à télécharger</legend>
      <p className="sync-muted">Chemins relatifs avec /, sans motifs glob. Un sous-arbre inclut son dossier et tous ses descendants.</p>
      {rules.map((rule, index) => (
        <div className="sync-advanced__grid" key={rule.id}>
          <SelectField label={`Type du chemin ${index + 1}`} value={rule.kind} onChange={event => onChange(rules.map(item => item.id === rule.id ? { ...item, kind: event.target.value as SelectionRule["kind"] } : item))}>
            <option value="path">Chemin exact</option>
            <option value="subtree">Sous-arbre</option>
          </SelectField>
          <TextField label={`Chemin ${index + 1}`} value={rule.path} placeholder={rule.kind === "path" ? "docs/guide.md" : "src"} onChange={event => onChange(rules.map(item => item.id === rule.id ? { ...item, path: event.target.value } : item))} />
          <Button size="sm" onClick={() => onChange(rules.filter(item => item.id !== rule.id))} aria-label={`Retirer le chemin ${index + 1}`}>Retirer</Button>
        </div>
      ))}
      <div><Button size="sm" onClick={() => {
        nextId.current = Math.max(nextId.current, ...rules.map(rule => rule.id + 1));
        onChange([...rules, { id: nextId.current++, kind: "path", path: "" }]);
      }}>Ajouter un chemin</Button></div>
      {error && <p role="status" className="sync-muted">{error}</p>}
    </fieldset>
  );
}
