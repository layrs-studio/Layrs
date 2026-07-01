import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import type { FormEvent } from "react";
import {
  createLayrsClient,
  createMockLayrsClient,
  LayrsApiError,
  normalizeArtifactCollection,
  type Artifact,
  type AuthSessionResponse,
  type GateStatus,
  type Layer,
  type LayerAccessPolicy,
  type LayrsClientLike,
  type LensManifest,
  type TeamMemberRole,
  type TimelineItem
} from "@layrs/client-sdk";
import { AppShell, StatusPill } from "@layrs/ui";
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
import {
  defaultLayerForSpace,
  studioReducer,
  type AuthScreenMode,
  type RequiredAuth,
  type StudioDispatch,
  type StudioSessionLoader,
  type StudioState
} from "./state/studioReducer";
import { InlineAlert, SystemScreen, TextField, WorkspaceSwitcher } from "./components/common";
import { fallbackLensManifests, type LensRegistryState } from "./components/LensFileViewer";

type TimelineFeedState =
  | { status: "idle" | "loading" | "fallback"; message?: string }
  | { status: "available"; items: TimelineItem[]; message?: string };

type ArtifactFeedState =
  | { status: "idle" | "loading" | "fallback"; message?: string }
  | { status: "available"; items: Artifact[]; message?: string };

type TimelineFeedTarget = {
  workspaceId: string;
  spaceId: string;
  layerId: string;
};

export function StudioApp() {
  const client = useMemo(createClientForRuntime, []);
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
      {state.notice ? <InlineAlert tone="success">{state.notice}</InlineAlert> : null}

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

async function bootstrap(client: LayrsClientLike, dispatch: StudioDispatch) {
  dispatch({ type: "checking" });
  await resolveAuthenticatedSession(() => client.getSession(), client, dispatch);
}

async function resolveAuthenticatedSession(
  loadSession: StudioSessionLoader,
  client: LayrsClientLike,
  dispatch: StudioDispatch
) {
  try {
    const auth: AuthSessionResponse = await loadSession();
    if (auth.state !== "authenticated" || !auth.account || !auth.session) {
      dispatch({ type: "signed-out", mode: "login" });
      return;
    }

    if (auth.workspaces.length === 0) {
      dispatch({ type: "onboarding", account: auth.account, session: auth.session, workspaces: auth.workspaces });
      return;
    }

    await loadWorkspace(
      client,
      dispatch,
      { account: auth.account, session: auth.session, workspaces: auth.workspaces },
      auth.activeWorkspaceId ?? auth.session.activeWorkspaceId ?? auth.workspaces[0].id
    );
  } catch (error) {
    if (error instanceof LayrsApiError && error.status === 401) {
      dispatch({ type: "signed-out", mode: "login" });
      return;
    }

    dispatch({ type: "server-error", message: errorMessage(error) });
  }
}

async function loadWorkspace(
  client: LayrsClientLike,
  dispatch: StudioDispatch,
  auth: RequiredAuth,
  workspaceId: string,
  notice?: string
) {
  try {
    const snapshot = await client.getStudioSnapshot(workspaceId);
    dispatch({ type: "ready", auth, snapshot, activeWorkspaceId: workspaceId, notice });
  } catch (error) {
    dispatch({ type: "server-error", message: errorMessage(error) });
  }
}

async function reloadReadyWorkspace(
  client: LayrsClientLike,
  dispatch: StudioDispatch,
  state: Extract<StudioState, { status: "ready" }>,
  notice: string
) {
  await loadWorkspace(
    client,
    dispatch,
    { account: state.account, session: state.session, workspaces: state.workspaces },
    state.activeWorkspaceId,
    notice
  );
}

function resolveSelectedLayer(layers: Layer[], selectedSpace: Extract<StudioState, { status: "ready" }>["snapshot"]["spaces"][number] | undefined, routeLayerId?: string) {
  if (!selectedSpace) {
    return undefined;
  }

  return (
    layers.find((layer) => layer.id === routeLayerId) ??
    defaultLayerForSpace(layers, selectedSpace)
  );
}

function resolveTimelineFeedTarget(state: StudioState, route: StudioRoute): TimelineFeedTarget | undefined {
  if (state.status !== "ready") {
    return undefined;
  }

  const selectedSpace =
    route.name === "space"
      ? state.snapshot.spaces.find((space) => space.id === route.spaceId)
      : state.snapshot.spaces.find((space) => space.id === state.selectedSpaceId);
  const selectedLayers = selectedSpace ? state.snapshot.layers.filter((layer) => layer.spaceId === selectedSpace.id) : [];
  const selectedLayer = resolveSelectedLayer(selectedLayers, selectedSpace, route.name === "space" ? route.layerId : undefined);

  if (!selectedSpace || !selectedLayer) {
    return undefined;
  }

  return {
    workspaceId: state.activeWorkspaceId,
    spaceId: selectedSpace.id,
    layerId: selectedLayer.id
  };
}

function createClientForRuntime(): LayrsClientLike {
  const env = runtimeEnv();
  if (isMockMode()) {
    return createMockLayrsClient();
  }

  return createLayrsClient({ baseUrl: env.VITE_LAYRS_API_URL ?? "" });
}

function isMockMode(): boolean {
  const env = runtimeEnv();
  return env.VITE_LAYRS_STUDIO_MODE === "mock" || env.VITE_LAYRS_API_MOCK === "true";
}

function runtimeEnv(): Record<string, string | undefined> {
  return (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {};
}

function runtimeApiBaseUrl(): string {
  return (runtimeEnv().VITE_LAYRS_API_URL ?? "").replace(/\/$/, "");
}

async function loadLensRegistry(baseUrl: string, signal: AbortSignal): Promise<LensRegistryState> {
  try {
    const payload = await fetchJson(`${baseUrl}/v1/lenses`, signal);
    const manifests = normalizeLensManifests(payload);
    if (manifests.length === 0) {
      return {
        status: "fallback",
        manifests: fallbackLensManifests
      };
    }

    return { status: "available", manifests };
  } catch (error) {
    return {
      status: "fallback",
      manifests: fallbackLensManifests,
      message: `Lens registry unavailable. Studio is using canonical local lenses. ${optionalStatus(error)}`
    };
  }
}

async function loadTimelineFeed(
  baseUrl: string,
  target: TimelineFeedTarget,
  signal: AbortSignal
): Promise<TimelineFeedState> {
  const path = `/v1/workspaces/${encodeURIComponent(target.workspaceId)}/spaces/${encodeURIComponent(target.spaceId)}/layers/${encodeURIComponent(target.layerId)}/timeline?limit=50`;

  try {
    const payload = await fetchJson(`${baseUrl}${path}`, signal);
    return { status: "available", items: normalizeTimelineItems(payload) };
  } catch (error) {
    return {
      status: "fallback",
      message: `Timeline endpoint unavailable. Studio is using snapshot activity. ${optionalStatus(error)}`
    };
  }
}

async function loadArtifactFeed(
  baseUrl: string,
  target: TimelineFeedTarget,
  signal: AbortSignal
): Promise<ArtifactFeedState> {
  const path = `/v1/workspaces/${encodeURIComponent(target.workspaceId)}/spaces/${encodeURIComponent(target.spaceId)}/layers/${encodeURIComponent(target.layerId)}/artifacts`;

  try {
    const payload = await fetchJson(`${baseUrl}${path}`, signal);
    return { status: "available", items: normalizeArtifactCollection(payload) };
  } catch (error) {
    return {
      status: "fallback",
      message: `Artifact endpoint unavailable. Studio is using snapshot files. ${optionalStatus(error)}`
    };
  }
}

async function fetchJson(url: string, signal: AbortSignal): Promise<unknown> {
  const response = await fetch(url, { credentials: "include", signal });
  const payload = await readOptionalJson(response);

  if (!response.ok) {
    throw new LayrsApiError(errorMessageFromPayload(payload) ?? response.statusText, response.status);
  }

  return payload;
}

async function readOptionalJson(response: Response): Promise<unknown> {
  if (response.status === 204) {
    return undefined;
  }

  const text = await response.text();
  if (!text) {
    return undefined;
  }

  try {
    return JSON.parse(text) as unknown;
  } catch {
    return undefined;
  }
}

function normalizeLensManifests(payload: unknown): LensManifest[] {
  const items = arrayPayload(payload, "items") ?? arrayPayload(payload, "manifests") ?? (Array.isArray(payload) ? payload : []);
  return items.filter(isLensManifest);
}

function isLensManifest(value: unknown): value is LensManifest {
  const record = objectPayload(value);
  const analyzer = objectPayload(record?.analyzer);
  const viewer = objectPayload(record?.viewer);

  return Boolean(
    record &&
      typeof record.id === "string" &&
      typeof record.name === "string" &&
      typeof record.version === "string" &&
      analyzer &&
      Array.isArray(analyzer.supportedMediaTypes) &&
      Array.isArray(analyzer.fileExtensions) &&
      Array.isArray(analyzer.capabilities) &&
      viewer &&
      typeof viewer.viewerId === "string" &&
      Array.isArray(viewer.previewKinds) &&
      Array.isArray(viewer.diffKinds)
  );
}

function normalizeTimelineItems(payload: unknown): TimelineItem[] {
  const items = arrayPayload(payload, "items") ?? (Array.isArray(payload) ? payload : []);
  return items.map(timelineItemFromPayload).filter((item): item is TimelineItem => Boolean(item));
}

function timelineItemFromPayload(value: unknown): TimelineItem | undefined {
  const record = objectPayload(value);
  if (!record) {
    return undefined;
  }

  const id = stringPayload(record.id) ?? stringPayload(record.eventId) ?? stringPayload(record.event_id);
  const body = objectPayload(record.body);
  const at =
    stringPayload(record.at) ??
    stringPayload(record.createdAt) ??
    stringPayload(record.created_at) ??
    stringPayload(record.occurredAt) ??
    stringPayload(record.occurred_at);
  const summary =
    stringPayload(record.summary) ??
    stringPayload(body?.summary) ??
    stringPayload(body?.message) ??
    stringPayload(record.kind) ??
    stringPayload(record.eventKind) ??
    stringPayload(record.event_kind);

  if (!id || !at || !summary) {
    return undefined;
  }

  const title =
    stringPayload(record.title) ??
    stringPayload(record.eventKind) ??
    stringPayload(record.event_kind) ??
    "Timeline event";
  const status = gateStatusPayload(record.status) ?? "pending";
  const relatedIds = arrayPayload(record, "relatedIds") ?? arrayPayload(record, "related_ids") ?? [];
  const bodyRelatedIds = arrayPayload(body, "relatedIds") ?? arrayPayload(body, "related_ids") ?? [];
  const objectId = stringPayload(record.objectId) ?? stringPayload(record.object_id);
  const spaceId = stringPayload(record.spaceId) ?? stringPayload(record.space_id);
  const layerId = stringPayload(record.layerId) ?? stringPayload(record.layer_id);
  const bodyArtifactId = stringPayload(body?.artifactId) ?? stringPayload(body?.artifact_id);
  const bodyPath = stringPayload(body?.path) ?? stringPayload(body?.logicalPath) ?? stringPayload(body?.logical_path);

  return {
    id,
    at,
    title: humanizeTimelineTitle(title),
    summary,
    status,
    relatedIds: [
      ...relatedIds.filter((item): item is string => typeof item === "string"),
      ...bodyRelatedIds.filter((item): item is string => typeof item === "string"),
      objectId,
      spaceId,
      layerId,
      bodyArtifactId,
      bodyPath
    ].filter(
      (item): item is string => typeof item === "string"
    )
  };
}

function objectPayload(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function arrayPayload(value: unknown, key: string): unknown[] | undefined {
  const record = objectPayload(value);
  const field = record?.[key];
  return Array.isArray(field) ? field : undefined;
}

function stringPayload(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0 ? value : undefined;
}

function gateStatusPayload(value: unknown): GateStatus | undefined {
  return value === "passing" || value === "blocked" || value === "needs-proof" || value === "pending" ? value : undefined;
}

function humanizeTimelineTitle(value: string): string {
  return value
    .replace(/[_-]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function favoriteSpaceStorageKey(workspaceId: string) {
  return `layrs:studio:favorites:${workspaceId}`;
}

function loadFavoriteSpaceIds(workspaceId: string): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(favoriteSpaceStorageKey(workspaceId));
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

function saveFavoriteSpaceIds(workspaceId: string, spaceIds: string[]) {
  try {
    globalThis.localStorage?.setItem(favoriteSpaceStorageKey(workspaceId), JSON.stringify(spaceIds));
  } catch {
    // Favorites are a local convenience; failing to persist them should not block Studio.
  }
}

function errorMessageFromPayload(payload: unknown): string | undefined {
  const record = objectPayload(payload);
  const error = objectPayload(record?.error);
  return stringPayload(record?.message) ?? stringPayload(error?.message) ?? stringPayload(record?.code) ?? stringPayload(error?.code);
}

function optionalStatus(error: unknown): string {
  if (error instanceof LayrsApiError) {
    return `(${error.status})`;
  }
  return "";
}

function errorMessage(error: unknown): string {
  if (error instanceof LayrsApiError) {
    return `${error.message} (${error.status})`;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return "Unable to reach Layrs server.";
}
