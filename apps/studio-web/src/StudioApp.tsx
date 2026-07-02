import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import type { FormEvent } from "react";
import type { Layer, LayerAccessPolicy, TeamMemberRole } from "@layrs/client-sdk";
import { AppShell, StatusPill, useNotifications } from "@layrs/ui";
import { HomePage } from "./pages/HomePage";
import { SpacePage } from "./pages/SpacePage";
import { TeamPage } from "./pages/TeamPage";
import {
  currentStudioRoute,
  homeHref,
  layerHref,
  parseStudioRoute,
  type StudioRoute,
  spaceHref,
  teamHref
} from "./routes";
import { studioReducer, type AuthScreenMode, type StudioState } from "./state/studioReducer";
import { InlineAlert, SystemScreen, TextField, WorkspaceSwitcher } from "./components/common";
import { fallbackLensManifests, type LensRegistryState } from "./components/LensFileViewer";
import {
  bootstrap,
  createClientForRuntime,
  errorMessage,
  isMockMode,
  loadArtifactFeed,
  loadFavoriteSpaceIds,
  loadLensRegistry,
  loadTimelineFeed,
  reloadReadyWorkspace,
  resolveAuthenticatedSession,
  resolveSelectedLayer,
  resolveTimelineFeedTarget,
  loadWorkspace,
  runtimeApiBaseUrl,
  saveFavoriteSpaceIds,
  type ArtifactFeedState,
  type TimelineFeedState
} from "./studioRuntime";

export function StudioApp() {
  const { notify } = useNotifications();
  const client = useMemo(createClientForRuntime, []);
  const lastToastRef = useRef<{ notice?: string; error?: string }>({});
  const isMock = isMockMode();
  const [route, setRoute] = useState<StudioRoute>(currentStudioRoute);
  const [state, dispatch] = useReducer(studioReducer, { status: "checking" });
  const [lensRegistry, setLensRegistry] = useState<LensRegistryState>({
    status: "fallback",
    manifests: fallbackLensManifests
  });
  const [timelineFeed, setTimelineFeed] = useState<TimelineFeedState>({ status: "idle" });
  const [artifactFeed, setArtifactFeed] = useState<ArtifactFeedState>({ status: "idle" });
  const timelineFeedTarget = useMemo(() => resolveTimelineFeedTarget(state, route), [route, state]);

  const navigate = useCallback((href: string) => {
    globalThis.history?.pushState({}, "", href);
    setRoute(parseStudioRoute(globalThis.location?.pathname ?? href));
  }, []);

  useEffect(() => {
    void bootstrap(client, dispatch);
  }, [client]);

  useEffect(() => {
    if (!timelineFeedTarget) {
      setTimelineFeed({ status: "idle" });
      setArtifactFeed({ status: "idle" });
      return;
    }

    if (isMock) {
      setLensRegistry({ status: "available", manifests: fallbackLensManifests });
      setTimelineFeed({ status: "fallback" });
      setArtifactFeed({ status: "fallback" });
      return;
    }

    const controller = new AbortController();
    const baseUrl = runtimeApiBaseUrl();

    setLensRegistry({ status: "loading", manifests: fallbackLensManifests });
    setTimelineFeed({ status: "loading" });
    setArtifactFeed({ status: "loading" });

    void loadLensRegistry(baseUrl, controller.signal).then((next) => {
      if (!controller.signal.aborted) {
        setLensRegistry(next);
      }
    });
    void loadTimelineFeed(baseUrl, timelineFeedTarget, controller.signal).then((next) => {
      if (!controller.signal.aborted) {
        setTimelineFeed(next);
      }
    });
    void loadArtifactFeed(baseUrl, timelineFeedTarget, controller.signal).then((next) => {
      if (!controller.signal.aborted) {
        setArtifactFeed(next);
      }
    });

    return () => controller.abort();
  }, [isMock, timelineFeedTarget?.workspaceId, timelineFeedTarget?.spaceId, timelineFeedTarget?.layerId]);

  useEffect(() => {
    function handlePopState() {
      setRoute(currentStudioRoute());
    }

    globalThis.addEventListener?.("popstate", handlePopState);
    return () => globalThis.removeEventListener?.("popstate", handlePopState);
  }, []);

  useEffect(() => {
    if (state.status === "ready" && state.notice && lastToastRef.current.notice !== state.notice) {
      lastToastRef.current.notice = state.notice;
      notify({ tone: "success", title: state.notice, dedupeKey: "studio-notice" });
    }
    if (state.status === "ready" && state.error && lastToastRef.current.error !== state.error) {
      lastToastRef.current.error = state.error;
      notify({ tone: "danger", title: "Action failed", message: state.error, dedupeKey: "studio-error" });
    }
  }, [notify, state.status, state.status === "ready" ? state.notice : undefined, state.status === "ready" ? state.error : undefined]);

  useEffect(() => {
    if (state.status !== "ready") {
      return;
    }

    if (route.name === "space") {
      dispatch({ type: "select-route", spaceId: route.spaceId, layerId: route.layerId });
      return;
    }

    dispatch({ type: "select-route" });
  }, [route, state.status]);

  async function handleLogin(email: string, password: string) {
    dispatch({ type: "checking" });
    await resolveAuthenticatedSession(() => client.login({ email, password }), client, dispatch);
  }

  async function handleSignup(name: string, email: string, password: string) {
    dispatch({ type: "checking" });
    await resolveAuthenticatedSession(() => client.signup({ name, email, password }), client, dispatch);
  }

  async function handleCreateWorkspace(name: string, slug: string) {
    if (state.status !== "onboarding") {
      return;
    }

    try {
      const workspace = await client.createWorkspace({ name, slug, description: "Server-backed Layrs workspace." });
      const auth = {
        account: state.account,
        session: { ...state.session, activeWorkspaceId: workspace.id },
        workspaces: [...state.workspaces, workspace]
      };
      await loadWorkspace(client, dispatch, auth, workspace.id, "Workspace created.");
      navigate(homeHref());
    } catch (error) {
      dispatch({ type: "onboarding", ...state, error: errorMessage(error) });
    }
  }

  async function handleWorkspaceChange(workspaceId: string) {
    if (state.status !== "ready") {
      return;
    }

    dispatch({ type: "checking" });
    navigate(homeHref());
    await loadWorkspace(
      client,
      dispatch,
      { account: state.account, session: state.session, workspaces: state.workspaces },
      workspaceId
    );
  }

  async function handleCreateTeam(input: { name: string }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const team = await client.createTeam({ workspaceId: state.activeWorkspaceId, name: input.name });
      navigate(teamHref(team.id));
      await reloadReadyWorkspace(client, dispatch, state, "Team created.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleCreateSpace(input: { name: string; key: string; teamId?: string; description?: string }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const space = await client.createSpace({ workspaceId: state.activeWorkspaceId, ...input });
      navigate(spaceHref(space.id));
      await reloadReadyWorkspace(client, dispatch, state, "Space created.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleDeleteSpace(spaceId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const result = await client.deleteSpace(state.activeWorkspaceId, spaceId);
      navigate(homeHref());
      await reloadReadyWorkspace(
        client,
        dispatch,
        state,
        `Space deleted. ${result.deletedLayers} Layer(s) and ${result.deletedArtifacts} artifact(s) were removed.`
      );
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleDeleteLayer(spaceId: string, layerId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await client.deleteLayer(state.activeWorkspaceId, spaceId, layerId);
      navigate(spaceHref(spaceId));
      await reloadReadyWorkspace(client, dispatch, state, "Layer deleted.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleAddTeamMember(teamId: string, input: { email: string; role: TeamMemberRole }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await client.addTeamMember({ workspaceId: state.activeWorkspaceId, teamId, ...input });
      await reloadReadyWorkspace(client, dispatch, state, "Team member added.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleRemoveTeamMember(teamId: string, accountId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await client.removeTeamMember(state.activeWorkspaceId, teamId, accountId);
      await reloadReadyWorkspace(client, dispatch, state, "Team member removed.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleCreateInvitation(teamId: string, email: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await client.createInvitation({ workspaceId: state.activeWorkspaceId, email, role: "member", teamIds: [teamId] });
      await reloadReadyWorkspace(client, dispatch, state, "Invitation created.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  async function handleSaveAccessPolicies(policies: LayerAccessPolicy[]) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await Promise.all(policies.map((policy) => client.replaceLayerAccessPolicy(policy)));
      await reloadReadyWorkspace(client, dispatch, state, "Layer access rules saved.");
    } catch (error) {
      dispatch({ type: "notice", error: errorMessage(error) });
    }
  }

  if (state.status === "checking") {
    return <SystemScreen title="Connecting to Layrs Studio" detail="Checking session and workspace access." />;
  }

  if (state.status === "server-error") {
    return (
      <SystemScreen
        title="Server unavailable"
        detail={state.message}
        actionLabel="Retry"
        onAction={() => void bootstrap(client, dispatch)}
      />
    );
  }

  if (state.status === "signed-out") {
    return (
      <AuthScreen
        error={state.error}
        isMock={isMock}
        mode={state.mode}
        onLogin={handleLogin}
        onSignup={handleSignup}
        onToggle={(mode) => dispatch({ type: "signed-out", mode })}
      />
    );
  }

  if (state.status === "onboarding") {
    return (
      <OnboardingScreen
        accountName={state.account.name}
        error={state.error}
        isMock={isMock}
        onCreateWorkspace={handleCreateWorkspace}
      />
    );
  }

  return (
    <StudioWorkspace
      isMock={isMock}
      lensRegistry={lensRegistry}
      onCreateSpace={handleCreateSpace}
      onCreateTeam={handleCreateTeam}
      onDeleteLayer={handleDeleteLayer}
      onDeleteSpace={handleDeleteSpace}
      onAddTeamMember={handleAddTeamMember}
      onCreateInvitation={handleCreateInvitation}
      onNavigate={navigate}
      onRemoveTeamMember={handleRemoveTeamMember}
      onSaveAccessPolicies={handleSaveAccessPolicies}
      onWorkspaceChange={handleWorkspaceChange}
      route={route}
      state={state}
      artifactFeed={artifactFeed}
      timelineFeed={timelineFeed}
    />
  );
}
function StudioWorkspace({
  isMock,
  lensRegistry,
  onCreateSpace,
  onCreateTeam,
  onDeleteLayer,
  onDeleteSpace,
  onAddTeamMember,
  onCreateInvitation,
  onNavigate,
  onRemoveTeamMember,
  onSaveAccessPolicies,
  onWorkspaceChange,
  route,
  state,
  artifactFeed,
  timelineFeed
}: {
  isMock: boolean;
  lensRegistry: LensRegistryState;
  onCreateSpace: (input: { name: string; key: string; teamId?: string; description?: string }) => Promise<void>;
  onCreateTeam: (input: { name: string }) => Promise<void>;
  onDeleteLayer: (spaceId: string, layerId: string) => Promise<void>;
  onDeleteSpace: (spaceId: string) => Promise<void>;
  onAddTeamMember: (teamId: string, input: { email: string; role: TeamMemberRole }) => Promise<void>;
  onCreateInvitation: (teamId: string, email: string) => Promise<void>;
  onNavigate: (href: string) => void;
  onRemoveTeamMember: (teamId: string, accountId: string) => Promise<void>;
  onSaveAccessPolicies: (policies: LayerAccessPolicy[]) => Promise<void>;
  onWorkspaceChange: (workspaceId: string) => void;
  route: StudioRoute;
  state: Extract<StudioState, { status: "ready" }>;
  artifactFeed: ArtifactFeedState;
  timelineFeed: TimelineFeedState;
}) {
  const { notify } = useNotifications();
  const { snapshot } = state;
  const selectedSpace =
    route.name === "space"
      ? snapshot.spaces.find((space) => space.id === route.spaceId)
      : snapshot.spaces.find((space) => space.id === state.selectedSpaceId);
  const selectedLayers = selectedSpace ? snapshot.layers.filter((layer) => layer.spaceId === selectedSpace.id) : [];
  const selectedLayer = resolveSelectedLayer(selectedLayers, selectedSpace, route.name === "space" ? route.layerId : undefined);
  const selectedTeam = route.name === "team" ? snapshot.teams.find((team) => team.id === route.teamId) : undefined;
  const selectedTeamMembers = selectedTeam
    ? snapshot.teamMembers.filter((member) => member.teamId === selectedTeam.id)
    : [];
  const selectedTeamInvitations = selectedTeam
    ? snapshot.invitations.filter((invitation) => invitation.status === "pending" && invitation.teamIds.includes(selectedTeam.id))
    : [];
  const [favoriteSpaceIds, setFavoriteSpaceIds] = useState<string[]>(() => loadFavoriteSpaceIds(snapshot.workspace.id));

  useEffect(() => {
    setFavoriteSpaceIds(loadFavoriteSpaceIds(snapshot.workspace.id));
  }, [snapshot.workspace.id]);

  function toggleFavoriteSpace(spaceId: string) {
    setFavoriteSpaceIds((current) => {
      const next = current.includes(spaceId) ? current.filter((id) => id !== spaceId) : [...current, spaceId];
      saveFavoriteSpaceIds(snapshot.workspace.id, next);
      notify({
        tone: "success",
        title: current.includes(spaceId) ? "Removed from favorites" : "Added to favorites",
        dedupeKey: "studio-favorite-toggle"
      });
      return next;
    });
  }

  return (
    <AppShell
      productName="Layrs Studio"
      workspaceName={snapshot.workspace.name}
      sidebar={
        <StudioSidebar
          isMock={isMock}
          route={route}
          accountEmail={state.account.email}
          favoriteSpaceIds={favoriteSpaceIds}
          snapshot={snapshot}
          selectedLayer={selectedLayer}
          onNavigate={onNavigate}
        />
      }
      toolbar={
        <>
          <WorkspaceSwitcher
            activeWorkspaceId={state.activeWorkspaceId}
            onChange={(workspaceId) => void onWorkspaceChange(workspaceId)}
            workspaces={state.workspaces}
          />
          <div className="studio-toolbar-actions" aria-label="Studio status">
            <StatusPill status={snapshot.workspace.health} />
            {selectedSpace ? <StatusPill status={selectedSpace.status} label="Space gate" /> : null}
          </div>
        </>
      }
    >
      {state.error ? <InlineAlert tone="danger">{state.error}</InlineAlert> : null}

      {route.name === "team" ? (
        <TeamPage
          invitations={selectedTeamInvitations}
          members={selectedTeamMembers}
          onAddMember={(input) => selectedTeam ? onAddTeamMember(selectedTeam.id, input) : Promise.resolve()}
          onCreateInvitation={(email) => selectedTeam ? onCreateInvitation(selectedTeam.id, email) : Promise.resolve()}
          onNavigate={onNavigate}
          onRemoveMember={(accountId) => selectedTeam ? onRemoveTeamMember(selectedTeam.id, accountId) : Promise.resolve()}
          spaces={snapshot.spaces}
          team={selectedTeam}
        />
      ) : route.name === "space" ? (
        <SpacePage
          account={state.account}
          lensRegistry={lensRegistry}
          layers={selectedLayers}
          onNavigate={onNavigate}
          onDeleteLayer={onDeleteLayer}
          onDeleteSpace={onDeleteSpace}
          onSaveAccessPolicies={onSaveAccessPolicies}
          selectedLayer={selectedLayer}
          snapshot={snapshot}
          serverArtifacts={artifactFeed.status === "available" ? artifactFeed.items : undefined}
          space={selectedSpace}
          workspaceId={snapshot.workspace.id}
          teams={snapshot.teams}
        />
      ) : (
        <HomePage
          activeSpaceId={selectedSpace?.id}
          favoriteSpaceIds={favoriteSpaceIds}
          onCreateSpace={onCreateSpace}
          onCreateTeam={onCreateTeam}
          onNavigate={onNavigate}
          onToggleFavorite={toggleFavoriteSpace}
          snapshot={snapshot}
        />
      )}
    </AppShell>
  );
}

function StudioSidebar({
  accountEmail,
  isMock,
  favoriteSpaceIds,
  onNavigate,
  route,
  selectedLayer,
  snapshot
}: {
  accountEmail: string;
  favoriteSpaceIds: string[];
  isMock: boolean;
  onNavigate: (href: string) => void;
  route: StudioRoute;
  selectedLayer?: Layer;
  snapshot: Extract<StudioState, { status: "ready" }>["snapshot"];
}) {
  const workspaceItems = [
    { href: homeHref(), label: "Overview", eyebrow: "Workspace", meta: snapshot.workspace.health, isActive: route.name === "home" }
  ];
  const spaceItems = snapshot.spaces.map((space) => ({
    id: space.id,
    href: selectedLayer && selectedLayer.spaceId === space.id ? layerHref(space.id, selectedLayer.id) : spaceHref(space.id),
    label: space.name,
    eyebrow: "Space",
    meta: `${snapshot.layers.filter((layer) => layer.spaceId === space.id).length}`,
    isActive: route.name === "space" && route.spaceId === space.id
  })).filter((item) => favoriteSpaceIds.includes(item.id));

  return (
    <nav className="layrs-sidebar" aria-label="Studio navigation">
      <div className="layrs-sidebar__items">
        <SidebarSection title="Workspace" items={workspaceItems} onNavigate={onNavigate} />
        <SidebarSection title="Favorite Spaces" items={spaceItems} emptyLabel="No favorite Spaces" onNavigate={onNavigate} />
        <div className="layrs-sidebar__section">
          <span className="layrs-sidebar__section-title">Roadmap</span>
          {["Weaves", "Gates", "Audit", "Settings"].map((label) => (
            <span className="layrs-sidebar__item layrs-sidebar__item--disabled" key={label}>
              <span>
                <small>Coming later</small>
                <strong>{label}</strong>
              </span>
              <em>0</em>
            </span>
          ))}
        </div>
      </div>
      <div className="layrs-sidebar__footer">{isMock ? "Explicit dev/mock mode" : `${accountEmail} authenticated`}</div>
    </nav>
  );
}

function SidebarSection({
  emptyLabel,
  items,
  onNavigate,
  title
}: {
  emptyLabel?: string;
  items: Array<{ href: string; label: string; eyebrow: string; meta: string; isActive: boolean }>;
  onNavigate: (href: string) => void;
  title: string;
}) {
  return (
    <div className="layrs-sidebar__section">
      <span className="layrs-sidebar__section-title">{title}</span>
      {items.length === 0 && emptyLabel ? <span className="layrs-sidebar__empty">{emptyLabel}</span> : null}
      {items.map((item) => (
        <a
          className={item.isActive ? "layrs-sidebar__item is-active" : "layrs-sidebar__item"}
          href={item.href}
          key={item.href}
          onClick={(event) => {
            event.preventDefault();
            onNavigate(item.href);
          }}
        >
          <span>
            <small>{item.eyebrow}</small>
            <strong>{item.label}</strong>
          </span>
          <em>{item.meta}</em>
        </a>
      ))}
    </div>
  );
}

function AuthScreen({
  error,
  isMock,
  mode,
  onLogin,
  onSignup,
  onToggle
}: {
  error?: string;
  isMock: boolean;
  mode: AuthScreenMode;
  onLogin: (email: string, password: string) => Promise<void>;
  onSignup: (name: string, email: string, password: string) => Promise<void>;
  onToggle: (mode: AuthScreenMode) => void;
}) {
  const [pending, setPending] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    const form = new FormData(event.currentTarget);
    try {
      if (mode === "login") {
        await onLogin(String(form.get("email") ?? ""), String(form.get("password") ?? ""));
      } else {
        await onSignup(
          String(form.get("name") ?? ""),
          String(form.get("email") ?? ""),
          String(form.get("password") ?? "")
        );
      }
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="studio-auth-shell">
      <aside>
        <span className="studio-auth-mark">L</span>
        <h1>Layrs Studio</h1>
        <p>Server-backed workspaces, teams, spaces, layers and access policy review.</p>
        {isMock ? <InlineAlert tone="warning">Running with explicit dev/mock data.</InlineAlert> : null}
      </aside>
      <main className="studio-auth-panel" aria-labelledby="auth-title">
        <div className="studio-auth-tabs" role="tablist" aria-label="Authentication mode">
          <button className={mode === "login" ? "is-active" : ""} onClick={() => onToggle("login")} type="button">
            Login
          </button>
          <button className={mode === "signup" ? "is-active" : ""} onClick={() => onToggle("signup")} type="button">
            Signup
          </button>
        </div>
        <h2 id="auth-title">{mode === "login" ? "Login" : "Signup"}</h2>
        {error ? <InlineAlert tone="danger">{error}</InlineAlert> : null}
        <form className="studio-form" onSubmit={submit}>
          {mode === "signup" ? <TextField label="Name" name="name" required /> : null}
          <TextField label="Email" name="email" required type="email" />
          <TextField label="Password" name="password" required type="password" />
          <button className="studio-primary-button" disabled={pending} type="submit">
            {pending ? "Connecting..." : mode === "login" ? "Login" : "Create account"}
          </button>
        </form>
      </main>
    </div>
  );
}

function OnboardingScreen({
  accountName,
  error,
  isMock,
  onCreateWorkspace
}: {
  accountName: string;
  error?: string;
  isMock: boolean;
  onCreateWorkspace: (name: string, slug: string) => Promise<void>;
}) {
  const [pending, setPending] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    const form = new FormData(event.currentTarget);
    try {
      await onCreateWorkspace(String(form.get("name") ?? ""), String(form.get("slug") ?? ""));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="studio-auth-shell">
      <aside>
        <span className="studio-auth-mark">L</span>
        <h1>Welcome, {accountName}</h1>
        <p>Create the first server workspace before entering Studio.</p>
        {isMock ? <InlineAlert tone="warning">Running with explicit dev/mock data.</InlineAlert> : null}
      </aside>
      <main className="studio-auth-panel" aria-labelledby="onboarding-title">
        <h2 id="onboarding-title">Onboarding Workspace</h2>
        {error ? <InlineAlert tone="danger">{error}</InlineAlert> : null}
        <form className="studio-form" onSubmit={submit}>
          <TextField label="Workspace name" name="name" required />
          <TextField label="Slug" name="slug" pattern="[a-z0-9-]+" required />
          <button className="studio-primary-button" disabled={pending} type="submit">
            {pending ? "Creating..." : "Create workspace"}
          </button>
        </form>
      </main>
    </div>
  );
}
