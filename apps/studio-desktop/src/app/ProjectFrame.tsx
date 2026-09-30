import { useCallback, useMemo, type ReactNode } from "react";
import {
  Activity,
  Camera,
  FileStack,
  FolderTree,
  GitBranch,
  GitMerge,
  History,
  Lock,
  Plug,
  RefreshCw,
  Settings,
  Stethoscope,
  Tag,
  Users,
} from "lucide-react";
import { Banner, Button, EmptyState, Link, Loading, NavItem, NavSection, PageHeader, Spinner, navigate, segment, useLocation, type Command } from "@layrs/ui";
import { MergePage } from "../merge/MergePage";
import { ChangesPage } from "../pages/ChangesPage";
import { HealthPage } from "../pages/HealthPage";
import { HistoryPage } from "../pages/HistoryPage";
import { LayersPage } from "../pages/LayersPage";
import { LimitedChanges } from "../pages/limited/LimitedChanges";
import { LimitedFiles } from "../pages/limited/LimitedFiles";
import { ProjectSettingsPage } from "../pages/ProjectSettingsPage";
import { TeamPage } from "../pages/TeamPage";
import { VersionsPage } from "../pages/VersionsPage";
import { ConnectFlow } from "../sync/ConnectFlow";
import { SyncButton } from "../sync/SyncPanel";
import { SyncPage } from "../sync/SyncPage";
import { explainError } from "./errors";
import { Frame, openProject } from "./Frame";
import { PreviewDialog } from "./PreviewDialog";
import { ProjectionProvider, useProjection } from "./projection";
import { folderName, readRecents } from "./recents";
import { RemoteProvider } from "./remote";
import { WorkspaceProvider, useWorkspace } from "./workspace";

export type ProjectView = "changes" | "files" | "history" | "layers" | "versions" | "team" | "merge" | "sync" | "connect" | "settings" | "health";

type PageEntry = { view: ProjectView; label: string; icon: ReactNode; secondary?: boolean; complete?: boolean; limited?: boolean };

/** Same landmarks for complete and limited-access folders; entries follow real capabilities. */
const projectPages: PageEntry[] = [
  { view: "changes", label: "Changements", icon: <Camera aria-hidden size={20} /> },
  { view: "files", label: "Fichiers", icon: <FileStack aria-hidden size={20} />, limited: true },
  { view: "history", label: "Historique", icon: <History aria-hidden size={20} /> },
  { view: "layers", label: "Layers", icon: <GitBranch aria-hidden size={20} />, complete: true },
  { view: "versions", label: "Versions", icon: <Tag aria-hidden size={20} />, complete: true },
  { view: "team", label: "Équipe", icon: <Users aria-hidden size={20} /> },
  { view: "sync", label: "Synchronisation", icon: <RefreshCw aria-hidden size={20} />, secondary: true },
  { view: "settings", label: "Réglages du projet", icon: <Settings aria-hidden size={20} />, secondary: true },
  { view: "health", label: "Diagnostic", icon: <Stethoscope aria-hidden size={20} />, secondary: true },
];

const contextual: Partial<Record<ProjectView, string>> = { merge: "Fusion", connect: "Connexion au serveur" };

export function ProjectRoute({ root, rest }: { root: string; rest: string }) {
  const [view, ...tail] = rest.split("/");
  return (
    <WorkspaceProvider key={root} root={root}>
      <ProjectRemote>
        <ProjectKind view={(view || "changes") as ProjectView} detail={tail.join("/")} />
      </ProjectRemote>
    </WorkspaceProvider>
  );
}

function ProjectRemote({ children }: { children: ReactNode }) {
  const workspace = useWorkspace();
  const { root, acquire, release, refresh } = workspace;
  const onChanged = useCallback((next: string) => (next === root ? void refresh() : openProject(next)), [root, refresh]);
  return (
    <RemoteProvider root={root} acquire={acquire} release={release} onChanged={onChanged}>
      {children}
    </RemoteProvider>
  );
}

function ProjectKind({ view, detail }: { view: ProjectView; detail: string }) {
  const { kind } = useWorkspace();
  if (kind === "projection")
    return (
      <ProjectionProvider>
        <ProjectFrame view={view} detail={detail} />
      </ProjectionProvider>
    );
  return <ProjectFrame view={view} detail={detail} />;
}

function available(page: PageEntry, limited: boolean) {
  return limited ? !page.complete : !page.limited;
}

function ProjectFrame({ view, detail }: { view: ProjectView; detail: string }) {
  const workspace = useWorkspace();
  const { data, kind, busy, error, root } = workspace;
  const limited = kind === "projection";
  const key = segment(root);
  const name = data?.state.space.name || readRecents().find((entry) => entry.root === root)?.name || folderName(root);
  const pages = projectPages.filter((page) => available(page, limited));
  const current = pages.find((page) => page.view === view);
  const known = Boolean(current) || view in contextual;

  const commands = useMemo<Command[]>(
    () => [
      ...pages.map((page) => ({
        id: `page:${page.view}`,
        group: `Dans ${name}`,
        label: page.label,
        icon: page.icon,
        run: () => navigate(`/projects/${key}/${page.view}`),
      })),
      ...(limited
        ? []
        : [{ id: "page:merge", group: `Dans ${name}`, label: "Préparer une fusion", icon: <GitMerge aria-hidden size={16} />, run: () => navigate(`/projects/${key}/merge`) }]),
      { id: "page:connect", group: `Dans ${name}`, label: "Connexion au serveur", icon: <Plug aria-hidden size={16} />, run: () => navigate(`/projects/${key}/connect`) },
    ],
    // `pages` derives from `limited`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key, name, limited],
  );

  const projectNav = kind !== "absent" && (
    <>
      <NavSection title={name}>
        {pages
          .filter((page) => !page.secondary)
          .map((page) => (
            <NavItem key={page.view} to={`/projects/${key}/${page.view}`} label={page.label} icon={page.icon} />
          ))}
      </NavSection>
      <NavSection title="Projet">
        {pages
          .filter((page) => page.secondary)
          .map((page) => (
            <NavItem key={page.view} to={`/projects/${key}/${page.view}`} label={page.label} icon={page.icon} />
          ))}
      </NavSection>
    </>
  );

  let content: ReactNode;
  if (kind === null && !error) content = <div className="layrs-page"><Loading label="Ouverture du projet…" /></div>;
  else if (kind === "absent") content = <NotAProject root={root} />;
  else if (!known) content = <Unavailable view={view} limited={limited} />;
  else if (view === "connect") content = <ConnectPage />;
  else if (view === "sync") content = <SyncPage />;
  else if (view === "settings") content = <ProjectSettingsPage />;
  else if (view === "health") content = <HealthPage />;
  else if (view === "team") content = <TeamPage />;
  else if (view === "merge") content = <MergePage conflictId={detail ? decodeURIComponent(detail) : undefined} />;
  else if (limited) content = view === "files" ? <LimitedFiles /> : view === "history" ? <HistoryPage /> : <LimitedChanges />;
  else if (!data) content = <div className="layrs-page">{!error && <Loading label="Lecture de l’état local…" />}</div>;
  else
    content = {
      changes: <ChangesPage />,
      history: <HistoryPage />,
      layers: <LayersPage />,
      versions: <VersionsPage />,
    }[view as "changes" | "history" | "layers" | "versions"];

  const explained = error ? explainError(error) : null;
  return (
    <Frame
      crumbs={[{ label: name, to: `/projects/${key}/changes` }, { label: current?.label ?? contextual[view] ?? "Changements" }]}
      context={
        <button type="button" className="layrs-context" onClick={() => navigate("/projects")} title="Changer de projet">
          <span className="layrs-context__mark" aria-hidden>
            {name.slice(0, 1).toUpperCase()}
          </span>
          <span className="layrs-context__text">
            <span className="layrs-context__name">{name}</span>
            <span className="layrs-context__meta">{limited ? "Accès limité · sur cet ordinateur" : "Sur cet ordinateur"}</span>
          </span>
        </button>
      }
      projectNav={projectNav}
      topbarExtra={
        <>
          {limited ? <LimitedTarget /> : <LayerSelector />}
          {busy && (
            <span className="topbar-activity" role="status">
              <Spinner size={14} />
              {busy}
            </span>
          )}
        </>
      }
      topbarActions={kind === "complete" || kind === "projection" ? <SyncButton /> : undefined}
      commands={commands}
    >
      {explained && (
        <div className="project-error">
          <Banner
            tone="danger"
            title={explained.title}
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
            {explained.body}
            <code className="project-error__detail">{explained.detail}</code>
          </Banner>
        </div>
      )}
      {limited && <LimitedNotice />}
      {content}
      <PreviewDialog />
    </Frame>
  );
}

function LayerSelector() {
  const workspace = useWorkspace();
  const { data, activeLayer, busy } = workspace;
  if (!data || data.state.layers.length === 0) return null;
  return (
    <label className="topbar-layer">
      <GitBranch aria-hidden size={16} />
      <span className="layrs-visually-hidden">Layer</span>
      <select
        aria-label="Layer"
        value={activeLayer?.layerId ?? ""}
        disabled={Boolean(busy) || data.needsRepair || workspace.conflicted}
        title={workspace.conflicted ? "Résolvez d’abord les conflits de la fusion en cours." : undefined}
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
}

function LimitedTarget() {
  const workspace = useWorkspace();
  const { target } = useProjection();
  return (
    <Link className="topbar-layer topbar-layer--link" to={`/projects/${segment(workspace.root)}/sync`} title="Layer distant qui reçoit vos publications">
      <Lock aria-hidden size={14} />
      <span>{target?.name || target?.layerId || "Layer distant à choisir"}</span>
    </Link>
  );
}

function LimitedNotice() {
  const { notice, clearNotice } = useProjection();
  if (!notice) return null;
  return (
    <div className="project-error">
      <Banner
        tone={notice.tone}
        title={notice.text}
        actions={
          <Button size="sm" variant="ghost" onClick={clearNotice}>
            Fermer
          </Button>
        }
      />
    </div>
  );
}

function ConnectPage() {
  const workspace = useWorkspace();
  const from = useLocation().query.get("from");
  const back = from && from.startsWith(`/projects/${segment(workspace.root)}/`) && !from.includes("/connect") ? from : `/projects/${segment(workspace.root)}/changes`;
  return (
    <div className="layrs-page layrs-page--narrow">
      <PageHeader
        title="Connexion au serveur"
        description="Reliez ce projet au serveur de votre équipe pour publier, recevoir et collaborer. Vos captures locales restent utilisables sans connexion."
      />
      <ConnectFlow onDone={() => navigate(back)} />
    </div>
  );
}

function Unavailable({ view, limited }: { view: ProjectView; limited: boolean }) {
  const workspace = useWorkspace();
  return (
    <div className="layrs-page">
      <EmptyState
        icon={<Lock size={22} />}
        title={limited ? "Page indisponible pour un accès limité" : "Page inconnue"}
        actions={<Button onClick={() => navigate(`/projects/${segment(workspace.root)}/changes`)}>Revenir aux changements</Button>}
      >
        {limited
          ? `« ${view} » agit sur le projet complet. Cet accès limité ne propose que les actions prises en charge par ses capacités.`
          : "Cette adresse ne correspond à aucune vue du projet."}
      </EmptyState>
    </div>
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
