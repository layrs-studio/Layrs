import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { FileText, FolderTree, History, LogOut, MessageSquare, Palette, Settings, Shield, Tag, User, Users } from "lucide-react";
import {
  AppShell,
  AppearanceSettings,
  Avatar,
  Banner,
  Breadcrumbs,
  Button,
  CommandPalette,
  EmptyState,
  Loading,
  Menu,
  NavItem,
  NavSection,
  SearchTrigger,
  StatusDot,
  ThemeSwitcher,
  ToastProvider,
  matchRoute,
  navigate,
  segment,
  setAppearance,
  useCommandPaletteShortcut,
  useLocation,
  type Command,
  type Crumb,
} from "@layrs/ui";
import { AccountPage } from "../pages/AccountPage";
import { LoginPage } from "../pages/LoginPage";
import { ProjectsPage, openProject } from "../pages/ProjectsPage";
import { AdminPage, FilesPage, HistoryPage, ReviewsPage, TeamPage, VersionsPage } from "../pages/project-pages";
import { ProjectProvider, forgetKnownProjects, knownProjects, useProject } from "./project";
import { SessionProvider, useSession } from "./session";

type ProjectView = "files" | "history" | "reviews" | "versions" | "team";

const projectPages: { view: ProjectView; label: string; icon: ReactNode }[] = [
  { view: "files", label: "Fichiers", icon: <FileText aria-hidden size={20} /> },
  { view: "history", label: "Historique", icon: <History aria-hidden size={20} /> },
  { view: "reviews", label: "Revues", icon: <MessageSquare aria-hidden size={20} /> },
  { view: "versions", label: "Versions", icon: <Tag aria-hidden size={20} /> },
  { view: "team", label: "Équipe et accès", icon: <Users aria-hidden size={20} /> },
];

export function WebApp() {
  return (
    <ToastProvider>
      <SessionProvider>
        <Gate />
      </SessionProvider>
    </ToastProvider>
  );
}

function Gate() {
  const { principal, generation } = useSession();
  // Signing out or switching account purges everything learned in the session.
  useEffect(() => {
    if (!principal) forgetKnownProjects();
  }, [principal]);
  if (!principal) return <LoginPage />;
  return <Routes key={generation} />;
}

function Routes() {
  const { path, query } = useLocation();

  useEffect(() => {
    if (path === "/") navigate("/projects", { replace: true });
  }, [path]);

  const project = matchRoute("/projects/:project/*", path);
  if (project) {
    const [view = "files", detail] = project["*"].split("/");
    const known = projectPages.some((page) => page.view === view) ? (view as ProjectView) : "files";
    const version = Number(query.get("version"));
    return (
      <ProjectProvider key={project.project} spaceId={project.project}>
        <ProjectFrame
          view={known}
          review={known === "reviews" && detail ? { reviewId: decodeURIComponent(detail), version: Number.isFinite(version) && version > 0 ? version : 1 } : null}
        />
      </ProjectProvider>
    );
  }
  let page: ReactNode;
  let title = "";
  if (matchRoute("/settings/appearance", path)) {
    page = <AppearanceSettings surface="web" />;
    title = "Apparence";
  } else if (matchRoute("/settings/account", path)) {
    page = <AccountPage />;
    title = "Compte";
  } else if (matchRoute("/admin", path)) {
    page = <AdminPage />;
    title = "Administration";
  } else page = <ProjectsPage />;
  return (
    <Frame crumbs={[{ label: "Projets", to: "/projects" }, ...(title ? [{ label: title }] : [])]}>
      {page}
    </Frame>
  );
}

function Frame({ children, crumbs, projectNav, commands = [] }: { children: ReactNode; crumbs: Crumb[]; projectNav?: ReactNode; commands?: Command[] }) {
  const { principal, signOut } = useSession();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const openPalette = useCallback(() => setPaletteOpen(true), []);
  useCommandPaletteShortcut(openPalette);
  const admin = principal?.instanceAdministrator === true;
  const projects = knownProjects();

  const allCommands = useMemo<Command[]>(
    () => [
      ...commands,
      ...projects.map((project) => ({
        id: `project:${project.spaceId}`,
        group: "Projets",
        label: project.name,
        icon: <FolderTree aria-hidden size={16} />,
        run: () => openProject(project.spaceId),
      })),
      { id: "projects", group: "Pages", label: "Tous les projets", icon: <FolderTree aria-hidden size={16} />, run: () => navigate("/projects") },
      { id: "account", group: "Pages", label: "Compte", icon: <User aria-hidden size={16} />, run: () => navigate("/settings/account") },
      { id: "appearance", group: "Pages", label: "Apparence", icon: <Palette aria-hidden size={16} />, run: () => navigate("/settings/appearance") },
      ...(admin ? [{ id: "admin", group: "Pages", label: "Administration de l’instance", icon: <Shield aria-hidden size={16} />, run: () => navigate("/admin") }] : []),
      { id: "theme-dark", group: "Actions", label: "Thème sombre", keywords: ["apparence"], run: () => setAppearance({ theme: "dark" }) },
      { id: "theme-light", group: "Actions", label: "Thème clair", keywords: ["apparence"], run: () => setAppearance({ theme: "light" }) },
      { id: "theme-system", group: "Actions", label: "Thème du système", keywords: ["apparence"], run: () => setAppearance({ theme: "system" }) },
      { id: "logout", group: "Actions", label: "Se déconnecter", icon: <LogOut aria-hidden size={16} />, run: () => void signOut() },
    ],
    [commands, projects.map((project) => project.spaceId).join("|"), admin, signOut],
  );

  return (
    <>
      <AppShell
        sidebar={
          <>
            <NavSection title="Projets">
              {projects.slice(0, 6).map((project) => (
                <NavItem key={project.spaceId} to={`/projects/${segment(project.spaceId)}`} label={project.name} dot="muted" />
              ))}
              <NavItem to="/projects" exact label="Tous les projets" icon={<FolderTree aria-hidden size={20} />} />
            </NavSection>
            {projectNav}
          </>
        }
        sidebarFooter={
          <>
            <ul className="layrs-nav">
              {admin && <NavItem to="/admin" label="Administration" icon={<Shield aria-hidden size={20} />} />}
              <NavItem to="/settings/appearance" label="Paramètres" icon={<Settings aria-hidden size={20} />} />
            </ul>
            <ThemeSwitcher />
            <StatusDot tone="success">Connecté à {location.host}</StatusDot>
          </>
        }
        topbar={<Breadcrumbs items={crumbs} />}
        topbarActions={
          <>
            <SearchTrigger onOpen={openPalette} label="Rechercher…" />
            <Menu
              label={`Compte de ${principal?.displayName || "cet utilisateur"}`}
              trigger={<Avatar name={principal?.displayName || principal?.principalId || "?"} size={32} />}
              items={[
                { id: "account", label: "Compte et sécurité", icon: <User aria-hidden size={16} />, onSelect: () => navigate("/settings/account") },
                { id: "appearance", label: "Apparence", icon: <Palette aria-hidden size={16} />, onSelect: () => navigate("/settings/appearance") },
                { id: "logout", label: "Se déconnecter", icon: <LogOut aria-hidden size={16} />, danger: true, onSelect: () => void signOut() },
              ]}
            />
          </>
        }
      >
        {children}
      </AppShell>
      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        commands={allCommands}
        scopeNote="Projets ouverts pendant cette session, pages et actions disponibles."
      />
    </>
  );
}

function ProjectFrame({ view, review }: { view: ProjectView; review: { reviewId: string; version: number } | null }) {
  const project = useProject();
  const key = segment(project.spaceId);
  const current = projectPages.find((page) => page.view === view);

  const commands = useMemo<Command[]>(
    () =>
      projectPages.map((page) => ({
        id: `page:${page.view}`,
        group: `Dans ${project.name}`,
        label: page.label,
        icon: page.icon,
        run: () => navigate(`/projects/${key}/${page.view}`),
      })),
    [key, project.name],
  );

  let content: ReactNode;
  if (project.loading && !project.bootstrap)
    content = (
      <div className="layrs-page">
        <Loading label="Ouverture du projet…" />
      </div>
    );
  else if (project.error)
    content = (
      <div className="layrs-page">
        <EmptyState
          icon={<FolderTree size={22} />}
          title="Ce projet n’est pas accessible"
          actions={
            <>
              <Button onClick={() => navigate("/projects")}>Retour aux projets</Button>
              <Button variant="primary" onClick={() => void project.reload()}>
                Réessayer
              </Button>
            </>
          }
        >
          <Banner tone="danger">{project.error}</Banner>
        </EmptyState>
      </div>
    );
  else
    content = {
      files: <FilesPage />,
      history: <HistoryPage />,
      reviews: <ReviewsPage selected={review} />,
      versions: <VersionsPage />,
      team: <TeamPage />,
    }[view];

  return (
    <Frame
      crumbs={[{ label: project.name, to: `/projects/${key}/files` }, { label: current?.label ?? "Fichiers" }]}
      projectNav={
        <NavSection title={project.name}>
          {projectPages.map((page) => (
            <NavItem key={page.view} to={`/projects/${key}/${page.view}`} label={page.label} icon={page.icon} />
          ))}
        </NavSection>
      }
      commands={commands}
    >
      {content}
    </Frame>
  );
}
