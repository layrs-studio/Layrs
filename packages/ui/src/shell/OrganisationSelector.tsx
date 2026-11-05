import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Building2, Check, ChevronDown, FolderTree, Plus, UserRound } from "lucide-react";
import type { ProjectContextEntry } from "../api/catalog-types";
import { Link, useLocation } from "./router";

type Props = {
  contexts: ProjectContextEntry[];
  selectedId: string | null;
  canCreateOrganisation: boolean;
  onSelect: (id: string) => void;
  onCreateOrganisation: () => void;
};

/** Server-scoped organisation navigation; personal projects have their own sidebar entry. */
export function OrganisationSelector({ contexts, selectedId, canCreateOrganisation, onSelect, onCreateOrganisation }: Props) {
  const organisations = contexts.filter((entry) => entry.kind !== "personal")
    .sort((a, b) => a.kind === b.kind ? a.name.localeCompare(b.name, "fr") : a.kind === "company" ? -1 : 1);
  const current = organisations.find((entry) => entry.id === selectedId);
  const label = (entry: ProjectContextEntry) => organisations.filter(item => item.name.trim().toLocaleLowerCase("fr") === entry.name.trim().toLocaleLowerCase("fr")).length > 1
    ? `${entry.name} · ${entry.slug ?? entry.id}` : entry.name;
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [position, setPosition] = useState({ top: 0, left: 0, width: 220, maxHeight: 320 });
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const count = organisations.length + Number(canCreateOrganisation);

  useLayoutEffect(() => {
    if (!open) return;
    const update = () => {
      const rect = trigger.current?.getBoundingClientRect();
      if (!rect) return;
      const width = Math.min(Math.max(rect.width, 300), window.innerWidth - 16);
      const height = Math.min(count * 38 + (canCreateOrganisation ? 18 : 8), 320);
      const below = window.innerHeight - rect.bottom;
      const top = below >= Math.min(height, 160) || below >= rect.top
        ? rect.bottom + 4 : Math.max(8, rect.top - height - 4);
      setPosition({ top, left: Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)), width, maxHeight: Math.max(80, window.innerHeight - top - 8) });
    };
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [open, count, canCreateOrganisation]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!trigger.current?.contains(target) && !menu.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointer);
    return () => document.removeEventListener("pointerdown", onPointer);
  }, [open]);

  useEffect(() => {
    if (open) menu.current?.querySelectorAll<HTMLButtonElement>("[role^=menuitem]")[active]?.focus();
  }, [open, active]);

  function close(returnFocus = true) {
    setOpen(false);
    if (returnFocus) trigger.current?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "ArrowDown") setActive((index) => (index + 1) % count);
    else if (event.key === "ArrowUp") setActive((index) => (index - 1 + count) % count);
    else if (event.key === "Home") setActive(0);
    else if (event.key === "End") setActive(count - 1);
    else if (event.key === "Escape") close();
    else if (event.key === "Tab") close(false);
    else return;
    if (event.key !== "Tab") event.preventDefault();
  }

  if (!organisations.length) return null;
  return <div className="layrs-organisation-switcher">
    <span className="layrs-organisation-switcher__label">Organisation</span>
    <button
      ref={trigger}
      type="button"
      className="layrs-organisation-switcher__trigger"
      aria-label={`Organisation : ${current ? label(current) : "choisir"}`}
      title={`Organisation : ${current ? label(current) : "choisir"}`}
      aria-haspopup="menu"
      aria-expanded={open}
      aria-controls={open ? menuId : undefined}
      onClick={() => { if (open) close(); else { setActive(Math.max(0, organisations.findIndex((entry) => entry.id === selectedId))); setOpen(true); } }}
      onKeyDown={(event) => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          setActive(event.key === "ArrowDown" ? 0 : count - 1);
          setOpen(true);
        }
      }}
    >
      <span className="layrs-organisation-switcher__mark"><Building2 aria-hidden size={18} /></span>
      <span className="layrs-organisation-switcher__name">{current ? label(current) : "Choisir une organisation"}</span>
      <ChevronDown className="layrs-organisation-switcher__chevron" aria-hidden size={16} />
    </button>
    {open && createPortal(<div
      ref={menu}
      id={menuId}
      role="menu"
      aria-label="Organisations"
      className="layrs-menu__list layrs-organisation-menu"
      style={{ top: position.top, left: position.left, width: position.width, maxHeight: position.maxHeight }}
      onKeyDown={onKeyDown}
    >
      {organisations.map((entry, index) => <button
        key={entry.id}
        type="button"
        role="menuitemradio"
        title={`${entry.name} · ${entry.slug ?? entry.id}`}
        aria-checked={entry.id === selectedId}
        tabIndex={active === index ? 0 : -1}
        className="layrs-menu__item layrs-organisation-menu__item"
        onClick={() => { close(); onSelect(entry.id); }}
      >
        <span className="layrs-organisation-menu__name">{label(entry)}</span>
        {entry.id === selectedId && <Check aria-hidden size={16} />}
      </button>)}
      {canCreateOrganisation && <>
        <div role="separator" className="layrs-menu__separator" />
        <button
          type="button"
          role="menuitem"
          tabIndex={active === organisations.length ? 0 : -1}
          className="layrs-menu__item layrs-organisation-menu__item"
          onClick={() => { close(); onCreateOrganisation(); }}
        ><Plus aria-hidden size={16} /><span>Créer une nouvelle organisation</span></button>
      </>}
    </div>, document.body)}
  </div>;
}

export function PersonalProjectsNav({ context, selected, onSelect }: { context: ProjectContextEntry | undefined; selected: boolean; onSelect: (id: string) => void }) {
  if (!context) return null;
  return <li>
    <button
      type="button"
      className="layrs-nav__item layrs-personal-nav"
      aria-pressed={selected}
      title="Projets personnels"
      onClick={() => onSelect(context.id)}
    >
      <UserRound aria-hidden size={20} />
      <span className="layrs-nav__label">Projets personnels</span>
    </button>
  </li>;
}

export function AllProjectsNav({ currentContextIsPersonal, onOpen }: { currentContextIsPersonal: boolean; onOpen: () => void }) {
  const { path } = useLocation();
  return <li>
    <Link
      to="/projects"
      className="layrs-nav__item"
      aria-current={path === "/projects" && !currentContextIsPersonal ? "page" : undefined}
      title="Tous les projets"
      onClick={(event) => {
        if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
        event.preventDefault();
        onOpen();
      }}
    >
      <FolderTree aria-hidden size={20} />
      <span className="layrs-nav__label">Tous les projets</span>
    </Link>
  </li>;
}
