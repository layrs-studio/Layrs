import { useCallback, useMemo, useState, type ReactNode } from "react";
import { FolderOpen, FolderTree, Palette, Plus, Search, Settings } from "lucide-react";
import {
  AppShell,
  Breadcrumbs,
  CommandPalette,
  IconButton,
  NavItem,
  NavSection,
  SearchTrigger,
  ThemeSwitcher,
  navigate,
  segment,
  setAppearance,
  useCommandPaletteShortcut,
  type Command,
} from "@layrs/ui";
import { selectFolder } from "../tauri";
import { folderName, readRecents } from "./recents";

export function openProject(root: string) {
  navigate(`/projects/${segment(root)}/changes`);
}

/** Shell shared by every Desktop page. */
export function Frame({
  children,
  crumbs,
  projectNav,
  context,
  topbarExtra,
  topbarActions,
  commands = [],
}: {
  children: ReactNode;
  crumbs: { label: ReactNode; to?: string }[];
  projectNav?: ReactNode;
  context?: ReactNode;
  topbarExtra?: ReactNode;
  topbarActions?: ReactNode;
  commands?: Command[];
}) {
  const [paletteOpen, setPaletteOpen] = useState(false);
  const openPalette = useCallback(() => setPaletteOpen(true), []);
  useCommandPaletteShortcut(openPalette);
  const recents = readRecents();

  const allCommands = useMemo<Command[]>(
    () => [
      ...commands,
      ...recents.map((project) => ({
        id: `project:${project.root}`,
        group: "Projets",
        label: project.name || folderName(project.root),
        hint: project.root,
        icon: <FolderTree aria-hidden size={16} />,
        run: () => openProject(project.root),
      })),
      { id: "projects", group: "Pages", label: "Tous les projets", icon: <FolderTree aria-hidden size={16} />, run: () => navigate("/projects") },
      { id: "appearance", group: "Pages", label: "Apparence", icon: <Palette aria-hidden size={16} />, run: () => navigate("/settings/appearance") },
      { id: "new", group: "Actions", label: "Nouveau projet", icon: <Plus aria-hidden size={16} />, run: () => navigate("/projects/new") },
      {
        id: "open",
        group: "Actions",
        label: "Ouvrir un dossier…",
        icon: <FolderOpen aria-hidden size={16} />,
        run: () => void selectFolder().then((root) => root && openProject(root)),
      },
      { id: "theme-dark", group: "Actions", label: "Thème sombre", keywords: ["apparence"], run: () => setAppearance({ theme: "dark" }) },
      { id: "theme-light", group: "Actions", label: "Thème clair", keywords: ["apparence"], run: () => setAppearance({ theme: "light" }) },
      { id: "theme-system", group: "Actions", label: "Thème du système", keywords: ["apparence"], run: () => setAppearance({ theme: "system" }) },
    ],
    // Recents are re-read on each render; commands change with the project.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [commands, recents.map((project) => project.root).join("|")],
  );

  return (
    <>
      <AppShell
        sidebar={
          <>
            {context}
            <NavSection
              title="Projets"
              actions={
                <>
                  <IconButton size="sm" label="Rechercher un projet" icon={<Search size={15} />} onClick={openPalette} />
                  <IconButton size="sm" label="Nouveau projet" icon={<Plus size={15} />} onClick={() => navigate("/projects/new")} />
                </>
              }
            >
              {recents.slice(0, 5).map((project) => (
                <NavItem key={project.root} to={`/projects/${segment(project.root)}`} label={project.name || folderName(project.root)} dot="muted" />
              ))}
              <NavItem to="/projects" exact label="Tous les projets" icon={<FolderTree aria-hidden size={20} />} />
            </NavSection>
            {projectNav}
          </>
        }
        sidebarFooter={
          <>
            <ul className="layrs-nav">
              <NavItem to="/settings/appearance" label="Paramètres" icon={<Settings aria-hidden size={20} />} />
            </ul>
            <ThemeSwitcher />
          </>
        }
        topbar={
          <>
            <Breadcrumbs items={crumbs} />
            {topbarExtra}
          </>
        }
        topbarActions={
          <>
            {topbarActions}
            <SearchTrigger onOpen={openPalette} label="Rechercher…" />
          </>
        }
      >
        {children}
      </AppShell>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={allCommands}
        scopeNote="Projets de cet ordinateur, pages et actions disponibles. Aucune action n’est lancée sans votre choix."
      />
    </>
  );
}
