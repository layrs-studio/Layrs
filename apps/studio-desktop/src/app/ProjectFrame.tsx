import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
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
import { Banner, Button, EmptyState, Link, Loading, NavItem, NavSection, PageHeader, Spinner, ProjectLensProvider, navigate, segment, useLocation, type Command, type LensTransport, type SpaceCapabilities } from "@layrs/ui";
import { MergePage } from "../merge/MergePage";
import { ChangesPage } from "../pages/ChangesPage";
import { HealthPage } from "../pages/HealthPage";
import { GitPage } from "../pages/GitPage";
import { HistoryPage } from "../pages/HistoryPage";
import { LayersPage } from "../pages/LayersPage";
import { LimitedChanges } from "../pages/limited/LimitedChanges";
import { LimitedFiles } from "../pages/limited/LimitedFiles";
import { LimitedWorktrees } from "../pages/limited/LimitedWorktrees";
import { ProjectSettingsPage } from "../pages/ProjectSettingsPage";
import { TeamPage } from "../pages/TeamPage";
import { VersionsPage } from "../pages/VersionsPage";
import { WorktreesPage } from "../pages/WorktreesPage";
import { GitBridgePage } from "../pages/GitBridgePage";
import { ConnectFlow } from "../sync/ConnectFlow";
import { SyncButton } from "../sync/SyncPanel";
import { SyncPage } from "../sync/SyncPage";
import { explainError } from "./errors";
import { Frame, openProject } from "./Frame";
import { PreviewDialog } from "./PreviewDialog";
import { ProjectionProvider, useProjection } from "./projection";
import { folderName, readRecents } from "./recents";
import { RemoteProvider, useRemote } from "./remote";
import { WorkspaceProvider, useWorkspace } from "./workspace";
import { useDesktopContexts } from "./contexts";
import { rememberRecentContext } from "./recentContext";

export type ProjectView = "changes" | "files" | "history" | "layers" | "worktrees" | "versions" | "team" | "merge" | "sync" | "connect" | "settings" | "health" | "git" | "git-assistant" | "reviews" | "published" | "access" | "admin" | "remote-settings";

type PageEntry = { view: ProjectView; label: string; icon: ReactNode; secondary?: boolean; complete?: boolean; limited?: boolean };

/** Same landmarks for complete and limited-access folders; entries follow real capabilities. */
const projectPages: PageEntry[] = [
  { view: "changes", label: "Changements", icon: <Camera aria-hidden size={20} /> },
  { view: "files", label: "Fichiers", icon: <FileStack aria-hidden size={20} />, limited: true },
  { view: "history", label: "Historique", icon: <History aria-hidden size={20} /> },
  { view: "layers", label: "Layers", icon: <GitBranch aria-hidden size={20} />, complete: true },
  { view: "worktrees", label: "Worktrees", icon: <FolderTree aria-hidden size={20} /> },
  { view: "versions", label: "Versions", icon: <Tag aria-hidden size={20} />, complete: true },
  { view: "team", label: "Équipe", icon: <Users aria-hidden size={20} /> },
  { view: "reviews", label: "Revues", icon: <GitMerge aria-hidden size={20} />, complete: true },
  { view: "published", label: "Contenu publié", icon: <FileStack aria-hidden size={20} />, complete: true },
  { view: "access", label: "Accès aux fichiers", icon: <Lock aria-hidden size={20} />, complete: true, secondary: true },
  { view: "remote-settings", label: "Réglages serveur", icon: <Settings aria-hidden size={20} />, complete: true, secondary: true },
  { view: "sync", label: "Synchronisation", icon: <RefreshCw aria-hidden size={20} />, secondary: true },
  { view: "settings", label: "Réglages du projet", icon: <Settings aria-hidden size={20} />, secondary: true },
  { view: "git-assistant", label: "Import / export Git", icon: <GitBranch aria-hidden size={20} />, secondary: true, complete: true },
  { view: "health", label: "Diagnostic", icon: <Stethoscope aria-hidden size={20} />, secondary: true },
  { view: "git", label: "Pont Git", icon: <GitBranch aria-hidden size={20} />, secondary: true, complete: true },
];

const contextual: Partial<Record<ProjectView, string>> = { merge: "Fusion", connect: "Connexion au serveur", admin: "Administration du serveur" };

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
      <RemoteLenses>{children}</RemoteLenses>
    </RemoteProvider>
  );
}

function RemoteLenses({ children }: { children: ReactNode }) {
  const workspace = useWorkspace();
  const remote = useRemote();
  const identityKey = `layrs.lens-account.v1:${remote.origin}:${workspace.root}`;
  const [remembered, setRemembered] = useState<{ key: string; principal: string } | null>(null);
  useEffect(() => {
    try {
      if (remote.session.state === "signedOut") { localStorage.removeItem(identityKey); setRemembered(null); }
      else if (remote.session.state === "connected") { localStorage.setItem(identityKey, remote.session.principalId); setRemembered({ key: identityKey, principal: remote.session.principalId }); }
      else { const value = localStorage.getItem(identityKey); setRemembered(value ? { key: identityKey, principal: value } : null); }
    } catch { setRemembered(null); }
  }, [identityKey, remote.session]);
  const offline = remote.session.state === "unreachable";
  const principalId = remote.session.state === "connected" ? remote.session.principalId : offline && remembered?.key === identityKey ? remembered.principal : "";
  const transport = useCallback<LensTransport>(command => remote.transport({ action: "administration", command }, { root: workspace.root, origin: remote.origin, principalId }), [remote.transport, remote.origin, workspace.root, principalId]);
  if (!principalId || !workspace.data?.state.space.spaceId) return <>{children}</>;
  const scope = `${workspace.root}|${remote.origin}|${principalId}|${workspace.data.state.space.spaceId}`;
  return <ProjectLensProvider scope={scope} transport={transport} offline={offline} persist>{children}</ProjectLensProvider>;
}

function ProjectKind({ view, detail }: { view: ProjectView; detail: string }) {
  const { kind } = useWorkspace();
  if (kind === "projection")
    return (
      <ProjectionProvider mode={view === "worktrees" || view === "health" ? "inspection" : "ordinary"}>
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
  const remote = useRemote();
  const contexts = useDesktopContexts();
  const { data, kind, busy, error, root } = workspace;
  const [projectContextKind, setProjectContextKind] = useState<SpaceCapabilities["contextKind"] | null>(null);
  const [contextChecked, setContextChecked] = useState(false);
  useEffect(() => {
    const spaceId = data?.state.space.spaceId;
    if (!spaceId || remote.session.state !== "connected") { setProjectContextKind(null); setContextChecked(false); return; }
    let active = true;
    setProjectContextKind(null);
    setContextChecked(false);
    void remote.transport<SpaceCapabilities>({ action: "spaceCapabilities", space: spaceId }).then((result) => {
      if (!active) return;
      setProjectContextKind(result.contextKind);
      setContextChecked(true);
      if (remote.origin === contexts.origin && remote.session.state === "connected" && remote.session.principalId === contexts.principalId) {
        contexts.adopt(result.contextId);
        rememberRecentContext(root, remote.origin, remote.session.principalId, result.contextId, spaceId);
      }
    }).catch(() => { if (active) { setProjectContextKind(null); setContextChecked(true); } });
    return () => { active = false; };
  }, [data?.state.space.spaceId, remote.origin, remote.session, remote.transport, contexts.origin, contexts.principalId, contexts.adopt, root]);
  const limited = kind === "projection";
  const key = segment(root);
  const name = data?.state.space.name || readRecents().find((entry) => entry.root === root)?.name || folderName(root);
  const pages = projectPages.filter((page) => available(page, limited));
  const current = pages.find((page) => page.view === view);
  const known = Boolean(current) || (view in contextual && !(limited && view === "admin"));

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
  if (view === "health") content = <HealthPage />;
  else if (kind === null && !error) content = <div className="layrs-page"><Loading label="Ouverture du projet…" /></div>;
  else if (kind === "absent") content = <NotAProject root={root} />;
  else if (!known) content = <Unavailable view={view} limited={limited} />;
  else if (view === "connect") content = <ConnectPage />;
  else if (view === "sync") content = <SyncPage />;
  else if (view === "settings") content = <ProjectSettingsPage />;
  else if (view === "worktrees" && (limited || data)) content = limited ? <LimitedWorktrees /> : <WorktreesPage />;
  else if (view === "git") content = <GitBridgePage />;
  else if (view === "git-assistant") content = <GitPage />;
  else if (view === "team" && remote.session.state === "connected" && !contextChecked) content = <div className="layrs-page"><Loading label="Vérification de l’espace du projet…" /></div>;
  else if (view === "team" && projectContextKind === "personal") content = <div className="layrs-page"><PageHeader title="Projet personnel" /><EmptyState icon={<Lock size={22} />} title="Accès réservé à votre compte">Ce projet personnel ne possède ni équipe ni invitations.</EmptyState></div>;
  else if (["team", "reviews", "published", "access", "admin", "remote-settings"].includes(view)) content = <TeamPage section={view} detail={detail} />;
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
          {limited ? <LimitedTarget /> : <><LayerSelector /><WorktreeContext /></>}
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
        disabled={Boolean(busy) || data.needsRepair || workspace.conflicted || data.state.view.worktrees.some(item => item.worktreeId === data.state.worktree.worktreeId && ["stale", "needs_repair"].includes(item.status))}
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

function WorktreeContext() {
  const { root, data } = useWorkspace();
  if (!data) return null;
  return <Link className="topbar-layer topbar-layer--link" to={`/projects/${segment(root)}/worktrees`} title={`${data.state.worktree.checkoutRoot} · ${data.state.worktree.worktreeId}`}>
    <FolderTree aria-hidden size={14} />
    <span>Worktree · {folderName(root)}</span>
  </Link>;
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
            <Button onClick={() => navigate(`/projects/${segment(root)}/health`)}>Diagnostic et reprise</Button>
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
