import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { Search } from "lucide-react";
import { Kbd } from "./Feedback";

export type Command = {
  id: string;
  /** Group heading, e.g. "Projets", "Pages", "Actions". */
  group: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
  keywords?: string[];
  /** Runs only after explicit selection; mutations must open their own preview. */
  run: () => void;
};

function normalize(value: string) {
  return value
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();
}

export function filterCommands(commands: Command[], query: string) {
  const terms = normalize(query).split(/\s+/).filter(Boolean);
  if (!terms.length) return commands;
  return commands.filter((command) => {
    const haystack = normalize([command.label, command.hint, command.group, ...(command.keywords ?? [])].filter(Boolean).join(" "));
    return terms.every((term) => haystack.includes(term));
  });
}

/** Opens with Ctrl/Cmd+K anywhere in the application. */
export function useCommandPaletteShortcut(onOpen: () => void) {
  useEffect(() => {
    function onKeyDown(event: globalThis.KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey && event.key.toLowerCase() === "k") {
        event.preventDefault();
        onOpen();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onOpen]);
}

export function CommandPalette({
  open,
  onClose,
  commands,
  placeholder = "Rechercher un projet, une page ou une action…",
  scopeNote,
}: {
  open: boolean;
  onClose: () => void;
  commands: Command[];
  placeholder?: string;
  /** States what is searched, e.g. "Projets accessibles et pages de ce projet". */
  scopeNote?: string;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const opener = useRef<Element | null>(null);
  const listId = useId();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const results = useMemo(() => filterCommands(commands, query), [commands, query]);
  const groups = useMemo(() => {
    const map = new Map<string, { command: Command; index: number }[]>();
    results.forEach((command, index) => {
      const bucket = map.get(command.group) ?? [];
      bucket.push({ command, index });
      map.set(command.group, bucket);
    });
    return [...map.entries()];
  }, [results]);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (open && !dialog.open) {
      opener.current = document.activeElement;
      setQuery("");
      setActive(0);
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
      const target = opener.current;
      if (target instanceof HTMLElement && target.isConnected) target.focus();
    }
  }, [open]);

  useEffect(() => setActive(0), [query]);

  useEffect(() => {
    document.getElementById(`${listId}-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [active, listId]);

  function choose(command: Command | undefined) {
    if (!command) return;
    onClose();
    command.run();
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") setActive((index) => Math.min(results.length - 1, index + 1));
    else if (event.key === "ArrowUp") setActive((index) => Math.max(0, index - 1));
    else if (event.key === "Enter") choose(results[active]);
    else return;
    event.preventDefault();
  }

  return (
    <dialog
      ref={dialogRef}
      className="layrs-palette"
      aria-label="Palette de commandes"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      {open && (
        <div className="layrs-palette__panel">
          <div className="layrs-palette__search">
            <Search aria-hidden size={18} />
            <input
              autoFocus
              role="combobox"
              aria-expanded
              aria-controls={listId}
              aria-activedescendant={results.length ? `${listId}-${active}` : undefined}
              aria-label="Rechercher"
              placeholder={placeholder}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={onKeyDown}
            />
            <Kbd>Échap</Kbd>
          </div>
          {scopeNote && <p className="layrs-palette__scope">{scopeNote}</p>}
          <div id={listId} role="listbox" aria-label="Résultats" className="layrs-palette__results">
            {groups.map(([group, entries]) => (
              <div key={group} role="group" aria-label={group}>
                <p className="layrs-palette__group" aria-hidden>
                  {group}
                </p>
                {entries.map(({ command, index }) => (
                  <div
                    key={command.id}
                    id={`${listId}-${index}`}
                    role="option"
                    aria-selected={index === active}
                    className="layrs-palette__item"
                    onMouseMove={() => setActive(index)}
                    onClick={() => choose(command)}
                  >
                    {command.icon}
                    <span className="layrs-palette__label">{command.label}</span>
                    {command.hint && <span className="layrs-palette__hint">{command.hint}</span>}
                  </div>
                ))}
              </div>
            ))}
            {!results.length && <p className="layrs-palette__empty">Aucun résultat pour « {query} ».</p>}
          </div>
        </div>
      )}
    </dialog>
  );
}
