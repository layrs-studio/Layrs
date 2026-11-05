import { useId, useMemo, useState } from "react";
import { Check, Search } from "lucide-react";
import { Avatar } from "../components/Feedback";
import { TextField } from "../components/Field";
import type { Person } from "./types";

const PRINCIPAL = /^principal:[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/;

/**
 * Chooses a person by readable name among the candidates the server allowed
 * this account to see. Homonyms show a short reference. A manual reference is
 * a secondary, explicitly labelled fallback for people outside that list.
 */
export function PersonPicker({
  label,
  people,
  value,
  onChange,
  exclude = [],
  hint,
  allowManual = true,
  manualHint = "Demandez à la personne son identifiant technique (Compte → Identifiant technique).",
  emptyText = "Aucune personne proposée.",
  disabled,
}: {
  label: string;
  people: Person[];
  value: string;
  onChange: (principalId: string) => void;
  exclude?: string[];
  hint?: string;
  allowManual?: boolean;
  manualHint?: string;
  emptyText?: string;
  disabled?: boolean;
}) {
  const [query, setQuery] = useState("");
  const [manual, setManual] = useState("");
  const listId = useId();
  const candidates = useMemo(() => people.filter((person) => !exclude.includes(person.principalId)), [people, exclude]);
  const names = useMemo(() => {
    const counts = new Map<string, number>();
    for (const person of candidates) counts.set(person.displayName, (counts.get(person.displayName) ?? 0) + 1);
    return counts;
  }, [candidates]);
  const filtered = candidates
    .filter((person) => person.displayName.toLocaleLowerCase("fr").includes(query.trim().toLocaleLowerCase("fr")))
    .sort((a, b) => a.displayName.localeCompare(b.displayName, "fr"));
  const selectedManual = value && !candidates.some((person) => person.principalId === value);

  return (
    <fieldset className="team-picker" disabled={disabled}>
      <legend className="layrs-field__label">{label}</legend>
      {hint && <p className="layrs-field__hint">{hint}</p>}
      {candidates.length > 6 && (
        <TextField
          label="Rechercher par nom"
          hideLabel
          placeholder="Rechercher par nom…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          trailing={<Search aria-hidden size={16} className="team-picker__search-icon" />}
        />
      )}
      {candidates.length ? (
        <ul className="team-picker__list" id={listId} aria-label={label}>
          {filtered.map((person) => {
            const selected = person.principalId === value;
            return (
              <li key={person.principalId}>
                <button type="button" className="team-picker__option" aria-pressed={selected} onClick={() => onChange(person.principalId)}>
                  <Avatar name={person.displayName} size={28} />
                  <span className="team-picker__name">
                    {person.displayName}
                    {(names.get(person.displayName) ?? 0) > 1 && <small> · {person.principalId.replace(/^principal:/, "").slice(0, 8)}</small>}
                    {person.kind === "service" && <small> · compte de service</small>}
                  </span>
                  {selected && <Check aria-hidden size={16} />}
                </button>
              </li>
            );
          })}
          {!filtered.length && <li className="team-muted">Aucun nom ne correspond.</li>}
        </ul>
      ) : (
        <p className="team-muted">{emptyText}</p>
      )}
      {allowManual && (
        <details className="team-picker__manual" open={Boolean(selectedManual)}>
          <summary>La personne n’est pas dans la liste</summary>
          <p className="layrs-field__hint">{manualHint}</p>
          <div className="team-inline">
            <TextField
              label="Identifiant technique du compte"
              placeholder="principal:…"
              value={manual}
              spellCheck={false}
              autoComplete="off"
              error={manual && !PRINCIPAL.test(manual.trim()) ? "Format attendu : principal:…" : undefined}
              onChange={(event) => setManual(event.target.value)}
            />
            <button
              type="button"
              className="layrs-button layrs-button--secondary layrs-button--md"
              disabled={!PRINCIPAL.test(manual.trim())}
              onClick={() => onChange(manual.trim())}
            >
              Utiliser cet identifiant
            </button>
          </div>
          {selectedManual && <p className="team-muted">Identifiant saisi : {value}. Le serveur vérifiera qu’il désigne un compte actif.</p>}
        </details>
      )}
    </fieldset>
  );
}
