import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { FileLock, FileText, FolderTree, History, Inbox, LogOut, MessageSquare, Palette, Settings, Settings2, Shield, Tag, User, Users } from "lucide-react";
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
import { AccessPage } from "../pages/AccessPage";
import { AdminPage } from "../pages/AdminPage";
import { InboxPage } from "../pages/InboxPage";
import { ProjectSettingsPage } from "../pages/ProjectSettingsPage";
import { TeamPage } from "../pages/TeamPage";
import { FilesPage } from "../pages/FilesPage";
import { HistoryPage } from "../pages/HistoryPage";
import { ReviewDetailPage } from "../pages/ReviewDetailPage";
import { ReviewsPage } from "../pages/ReviewsPage";
import { RunPage } from "../pages/RunPage";
import { VersionsPage } from "../pages/VersionsPage";
import { ProjectProvider, forgetKnownProjects, knownProjects, useProject } from "./project";
import { SessionProvider, useSession } from "./session";

type ProjectView = "files" | "history" | "reviews" | "versions" | "team" | "access" | "settings";

const projectPages: { view: ProjectView; label: string; icon: ReactNode }[] = [
  { view: "files", label: "Fichiers", icon: <FileText aria-hidden size={20} /> },
  { view: "history", label: "Historique", icon: <History aria-hidden size={20} /> },
  { view: "reviews", label: "Revues", icon: <MessageSquare aria-hidden size={20} /> },
  { view: "versions", label: "Versions", icon: <Tag aria-hidden size={20} /> },
  { view: "team", label: "Équipe et accès", icon: <Users aria-hidden size={20} /> },
  { view: "access", label: "Accès aux fichiers", icon: <FileLock aria-hidden size={20} /> },
  { view: "settings", label: "Réglages", icon: <Settings2 aria-hidden size={20} /> },
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
    const [view = "files", ...detail] = project["*"].split("/").map((part) => {
      try {
        return decodeURIComponent(part);
      } catch {
        return "";
      }
    });
    const known = projectPages.some((page) => page.view === view) ? (view as ProjectView) : "files";
    return (
      <ProjectProvider key={project.project} spaceId={project.project}>
        <ProjectFrame view={known} detail={detail.filter(Boolean)} layer={query.get("layer") ?? undefined} />
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
  } else if (matchRoute("/admin", path) || matchRoute("/admin/:section", path)) {
    page = <AdminPage section={matchRoute("/admin/:section", path)?.section} />;
    title = "Administration";
  } else if (matchRoute("/inbox", path) || matchRoute("/inbox/:section", path)) {
    page = <InboxPage section={matchRoute("/inbox/:section", path)?.section} />;
    title = "Boîte de réception";
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
      { id: "inbox", group: "Pages", label: "Boîte de réception", icon: <Inbox aria-hidden size={16} />, run: () => navigate("/inbox") },
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
              <NavItem to="/inbox" label="Boîte de réception" icon={<Inbox aria-hidden size={20} />} />
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

/** `detail` is the sub-route: a capture, a review, a review's run, or a team/settings section. */
function ProjectFrame({ view, detail, layer }: { view: ProjectView; detail: string[]; layer?: string }) {
  const project = useProject();
  const key = segment(project.spaceId);
  const current = projectPages.find((page) => page.view === view);
  const subPage =
    view === "history" && detail[0]
      ? "Capture"
      : view === "reviews" && detail[0]
        ? detail[1] === "runs" && detail[2]
          ? "Validation"
          : "Revue"
        : null;

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
      history: <HistoryPage key={detail[0] ?? ""} capture={detail[0]} />,
      reviews: !detail[0] ? (
        <ReviewsPage />
      ) : detail[1] === "runs" && detail[2] ? (
        <RunPage reviewId={detail[0]} runId={detail[2]} />
      ) : (
        <ReviewDetailPage reviewId={detail[0]} />
      ),
      versions: <VersionsPage />,
      team: <TeamPage section={detail[0]} />,
      access: <AccessPage />,
      settings: <ProjectSettingsPage section={detail[0]} layer={layer} />,
    }[view];

  return (
    <Frame
      crumbs={[
        { label: project.name, to: `/projects/${key}/files` },
        { label: current?.label ?? "Fichiers", to: subPage ? `/projects/${key}/${view}` : undefined },
        ...(subPage ? [{ label: subPage }] : []),
      ]}
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
