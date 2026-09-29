import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  Camera,
  FolderOpen,
  FolderTree,
  GitBranch,
  History,
  Palette,
  Plus,
  RefreshCw,
  Search,
  Settings,
  Stethoscope,
  Tag,
} from "lucide-react";
import {
  AppShell,
  AppearanceSettings,
  Banner,
  Breadcrumbs,
  Button,
  CommandPalette,
  EmptyState,
  IconButton,
  Loading,
  NavItem,
  NavSection,
  SearchTrigger,
  Spinner,
  ThemeSwitcher,
  ToastProvider,
  matchRoute,
  navigate,
  segment,
  setAppearance,
  useCommandPaletteShortcut,
  useLocation,
  type Command,
} from "@layrs/ui";
import { NativePartialWorkbench } from "../NativePartialWorkbench";
import { selectFolder } from "../tauri";
import { ChangesPage } from "../pages/ChangesPage";
import { HealthPage } from "../pages/HealthPage";
import { HistoryPage } from "../pages/HistoryPage";
import { LayersPage } from "../pages/LayersPage";
import { NewProjectPage } from "../pages/NewProjectPage";
import { ProjectSettingsPage } from "../pages/ProjectSettingsPage";
import { ProjectsPage, openProject } from "../pages/ProjectsPage";
import { SyncPage } from "../pages/SyncPage";
import { VersionsPage } from "../pages/VersionsPage";
import { PreviewDialog } from "./PreviewDialog";
import { folderName, readRecents } from "./recents";
import { WorkspaceProvider, useWorkspace } from "./workspace";

type ProjectView = "changes" | "history" | "layers" | "versions" | "sync" | "settings" | "health";

const projectPages: { view: ProjectView; label: string; icon: ReactNode; secondary?: boolean }[] = [
  { view: "changes", label: "Changements", icon: <Camera aria-hidden size={20} /> },
  { view: "history", label: "Historique", icon: <History aria-hidden size={20} /> },
  { view: "layers", label: "Layers", icon: <GitBranch aria-hidden size={20} /> },
  { view: "versions", label: "Versions", icon: <Tag aria-hidden size={20} /> },
  { view: "sync", label: "Synchronisation", icon: <RefreshCw aria-hidden size={20} /> },
  { view: "settings", label: "Réglages du projet", icon: <Settings aria-hidden size={20} />, secondary: true },
  { view: "health", label: "Diagnostic", icon: <Stethoscope aria-hidden size={20} />, secondary: true },
];

export function DesktopApp() {
  return (
    <ToastProvider>
      <Router />
    </ToastProvider>
  );
}

function Router() {
  const { path } = useLocation();

  // Desktop opens the most recent local project; without one, the project list.
  useEffect(() => {
    if (path !== "/") return;
    const last = readRecents()[0];
    navigate(last ? `/projects/${segment(last.root)}/changes` : "/projects", { replace: true });
  }, [path]);

  const project = matchRoute("/projects/:project/*", path);
  if (project && project.project !== "new") {
    const view = (project["*"].split("/")[0] || "changes") as ProjectView;
    return (
      <WorkspaceProvider key={project.project} root={project.project}>
        <ProjectFrame view={projectPages.some((page) => page.view === view) ? view : "changes"} />
      </WorkspaceProvider>
    );
  }
  let page: ReactNode;
  let title = "Projets";
  if (matchRoute("/projects/new", path)) {
    page = <NewProjectPage />;
    title = "Nouveau projet";
  } else if (matchRoute("/settings/appearance", path)) {
    page = <AppearanceSettings surface="desktop" />;
    title = "Apparence";
  } else page = <ProjectsPage />;
  return (
    <Frame crumbs={[{ label: "Projets", to: "/projects" }, ...(title === "Projets" ? [] : [{ label: title }])]}>
      {page}
    </Frame>
  );
}

/** Shell shared by every Desktop page. */
function Frame({
  children,
  crumbs,
  projectNav,
  context,
  topbarExtra,
  commands = [],
}: {
  children: ReactNode;
  crumbs: { label: ReactNode; to?: string }[];
  projectNav?: ReactNode;
  context?: ReactNode;
  topbarExtra?: ReactNode;
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
                <NavItem
                  key={project.root}
                  to={`/projects/${segment(project.root)}`}
                  label={project.name || folderName(project.root)}
                  dot="muted"
                />
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
        topbarActions={<SearchTrigger onOpen={openPalette} label="Rechercher…" />}
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

function ProjectFrame({ view }: { view: ProjectView }) {
  const workspace = useWorkspace();
  const { data, kind, busy, error, activeLayer, root } = workspace;
  const key = segment(root);
  const name = data?.state.space.name || folderName(root);
  const current = projectPages.find((page) => page.view === view);

  const commands = useMemo<Command[]>(
    () =>
      projectPages.map((page) => ({
        id: `page:${page.view}`,
        group: `Dans ${name}`,
        label: page.label,
        icon: page.icon,
        run: () => navigate(`/projects/${key}/${page.view}`),
      })),
    [key, name],
  );

  const projectNav = (
    <>
      <NavSection title={name}>
        {projectPages
          .filter((page) => !page.secondary)
          .map((page) => (
            <NavItem key={page.view} to={`/projects/${key}/${page.view}`} label={page.label} icon={page.icon} />
          ))}
      </NavSection>
      <NavSection title="Projet">
        {projectPages
          .filter((page) => page.secondary)
          .map((page) => (
            <NavItem key={page.view} to={`/projects/${key}/${page.view}`} label={page.label} icon={page.icon} />
          ))}
      </NavSection>
    </>
  );

  const layerSelector = data && data.state.layers.length > 0 && (
    <label className="topbar-layer">
      <GitBranch aria-hidden size={16} />
      <span className="layrs-visually-hidden">Layer</span>
      <select
        aria-label="Layer"
        value={activeLayer?.layerId ?? ""}
        disabled={Boolean(busy) || data.needsRepair}
        onChange={(event) => {
          const layer = data.state.layers.find((item) => item.layerId === event.target.value);
          void workspace.run({ action: "switchLayer", layer: event.target.value }, `Passage à ${layer?.name ?? "ce Layer"}…`);
        }}
      >
        {data.state.layers.map((layer) => (
          <option key={layer.layerId} value={layer.layerId}>
            {layer.name}
          </option>
        ))}
      </select>
    </label>
  );

  let content: ReactNode;
  if (kind === null && !error) content = <div className="layrs-page"><Loading label="Ouverture du projet…" /></div>;
  else if (kind === "absent") content = <NotAProject root={root} />;
  else if (kind === "projection")
    content = (
      <div className="layrs-page">
        <div className="layrs-legacy">
          <NativePartialWorkbench root={root} onClose={() => navigate("/projects")} />
        </div>
      </div>
    );
  else if (!data) content = <div className="layrs-page">{!error && <Loading label="Lecture de l’état local…" />}</div>;
  else
    content = {
      changes: <ChangesPage />,
      history: <HistoryPage />,
      layers: <LayersPage />,
      versions: <VersionsPage />,
      sync: <SyncPage />,
      settings: <ProjectSettingsPage />,
      health: <HealthPage />,
    }[view];

  return (
    <Frame
      crumbs={[
        { label: name, to: `/projects/${key}/changes` },
        ...(kind === "projection" ? [] : [{ label: current?.label ?? "Changements" }]),
      ]}
      context={
        <button type="button" className="layrs-context" onClick={() => navigate("/projects")} title="Changer de projet">
          <span className="layrs-context__mark" aria-hidden>
            {name.slice(0, 1).toUpperCase()}
          </span>
          <span className="layrs-context__text">
            <span className="layrs-context__name">{name}</span>
            <span className="layrs-context__meta">{kind === "projection" ? "Accès limité · sur cet ordinateur" : "Sur cet ordinateur"}</span>
          </span>
        </button>
      }
      projectNav={kind === "projection" ? undefined : projectNav}
      topbarExtra={
        <>
          {layerSelector}
          {busy && (
            <span className="topbar-activity" role="status">
              <Spinner size={14} />
              {busy}
            </span>
          )}
        </>
      }
      commands={commands}
    >
      {error && (
        <div className="project-error">
          <Banner
            tone="danger"
            title="L’opération n’a pas abouti"
            actions={
              <>
                <Button size="sm" variant="ghost" onClick={() => navigate(`/projects/${key}/health`)} icon={<Activity aria-hidden size={14} />}>
                  Diagnostic
                </Button>
                <Button size="sm" onClick={workspace.clearError}>
                  Fermer
                </Button>
              </>
            }
          >
            <span className="project-error__detail">{error}</span>
          </Banner>
        </div>
      )}
      {content}
      <PreviewDialog />
    </Frame>
  );
}

function NotAProject({ root }: { root: string }) {
  return (
    <div className="layrs-page">
      <EmptyState
        icon={<FolderTree size={22} />}
        title="Ce dossier n’est pas encore un projet Layrs"
        actions={
          <>
            <Button onClick={() => navigate("/projects")}>Choisir un autre dossier</Button>
            <Button variant="primary" onClick={() => navigate("/projects/new", { query: { root } })}>
              Commencer un projet
            </Button>
          </>
        }
      >
        <code>{root}</code>
      </EmptyState>
    </div>
  );
}
