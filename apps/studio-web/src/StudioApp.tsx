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
import { studioReducer, type AuthScreenMode, type RequiredAuth, type StudioState } from "./state/studioReducer";
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
  resolveAuthenticatedSession,
  resolveSelectedLayer,
  resolveTimelineFeedTarget,
  runtimeApiBaseUrl,
  saveFavoriteSpaceIds,
  type AccessPolicyState,
  type ArtifactFeedState,
} from "./studioRuntime";
import { sanitizeStudioSnapshot } from "./privateData";
import {
  isPrivateRequestCancelled,
  privateRequestKey,
  StudioPrivateApiController,
  StudioPrivateApiProvider
} from "./studioPrivateApi";

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
  const [artifactFeed, setArtifactFeed] = useState<ArtifactFeedState>({ status: "idle" });
  const [accessPolicies, setAccessPolicies] = useState<AccessPolicyState>({ status: "idle" });
  const [feedRefreshToken, setFeedRefreshToken] = useState(0);
  const timelineFeedTarget = useMemo(() => resolveTimelineFeedTarget(state, route), [route, state]);
  const feedRequestKey = timelineFeedTarget
    ? privateRequestKey(
        "feeds",
        timelineFeedTarget.workspaceId,
        timelineFeedTarget.spaceId,
        timelineFeedTarget.layerId,
        feedRefreshToken
      )
    : "";
  const accessLayerIds =
    state.status === "ready" && timelineFeedTarget
      ? state.snapshot.layers
          .filter((layer) => layer.spaceId === timelineFeedTarget.spaceId)
          .map((layer) => layer.id)
          .sort()
      : [];
  const accessRequestKey = timelineFeedTarget
    ? privateRequestKey(
        "access",
        timelineFeedTarget.workspaceId,
        timelineFeedTarget.spaceId,
        accessLayerIds.join(","),
        feedRefreshToken
      )
    : "";
  const privateApi = useMemo(
    () =>
      new StudioPrivateApiController((message) => {
        setLensRegistry({ status: "fallback", manifests: fallbackLensManifests });
        setArtifactFeed({ status: "idle" });
        setAccessPolicies({ status: "idle" });
        setFeedRefreshToken(0);
        lastToastRef.current = {};
        globalThis.history?.replaceState({}, "", homeHref());
        setRoute(parseStudioRoute(globalThis.location?.pathname ?? homeHref()));
        dispatch({ type: "signed-out", mode: "login", error: message });
      }),
    []
  );

  const navigate = useCallback((href: string) => {
    globalThis.history?.pushState({}, "", href);
    setRoute(parseStudioRoute(globalThis.location?.pathname ?? href));
  }, []);

  useEffect(() => {
    void bootstrap(client, dispatch);
  }, [client]);

  useEffect(() => {
    if (!timelineFeedTarget) {
      setArtifactFeed({ status: "idle" });
      return;
    }

    if (isMock) {
      setLensRegistry({ status: "available", manifests: fallbackLensManifests });
      setArtifactFeed({ status: "fallback", key: feedRequestKey });
      return;
    }

    const baseUrl = runtimeApiBaseUrl();

    setLensRegistry({ status: "loading", manifests: fallbackLensManifests });
    setArtifactFeed({ status: "loading", key: feedRequestKey });

    void privateApi
      .request("workspace-feeds", feedRequestKey, async ({ fetchJson }) =>
        Promise.all([
          loadLensRegistry(baseUrl, fetchJson),
          loadArtifactFeed(baseUrl, timelineFeedTarget, fetchJson, feedRequestKey)
        ])
      )
      .then(([nextRegistry, nextArtifacts]) => {
        setLensRegistry(nextRegistry);
        setArtifactFeed(nextArtifacts);
      })
      .catch((requestError) => {
        if (!isPrivateRequestCancelled(requestError)) {
          setArtifactFeed({ status: "fallback", key: feedRequestKey, message: errorMessage(requestError) });
        }
      });

    return () => privateApi.cancel("workspace-feeds", feedRequestKey);
  }, [feedRequestKey, isMock, privateApi, timelineFeedTarget?.workspaceId, timelineFeedTarget?.spaceId, timelineFeedTarget?.layerId]);

  useEffect(() => {
    if (state.status !== "ready" || !timelineFeedTarget) {
      setAccessPolicies({ status: "idle" });
      return;
    }

    const spaceLayers = state.snapshot.layers.filter((layer) => layer.spaceId === timelineFeedTarget.spaceId);
    if (isMock) {
      setAccessPolicies({
        status: "available",
        key: accessRequestKey,
        policies: state.snapshot.layerAccessPolicies.filter((policy) => policy.spaceId === timelineFeedTarget.spaceId)
      });
      return;
    }

    setAccessPolicies({ status: "loading", key: accessRequestKey });
    void privateApi
      .request("access-policies", accessRequestKey, async () =>
        Promise.all(
          spaceLayers.map((layer) =>
            client.getLayerAccessPolicy(state.activeWorkspaceId, timelineFeedTarget.spaceId, layer.id)
          )
        )
      )
      .then((policies) => setAccessPolicies({ status: "available", key: accessRequestKey, policies }))
      .catch((requestError) => {
        if (!isPrivateRequestCancelled(requestError)) {
          setAccessPolicies({
            status: "error",
            key: accessRequestKey,
            message: `Access policies could not be loaded from Layrs. Editing is blocked. ${errorMessage(requestError)}`
          });
        }
      });

    return () => privateApi.cancel("access-policies", accessRequestKey);
  }, [accessRequestKey, client, isMock, privateApi, state.status, timelineFeedTarget?.spaceId, timelineFeedTarget?.workspaceId]);

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

  async function loadPrivateWorkspace(auth: RequiredAuth, workspaceId: string, notice?: string) {
    const snapshot = await privateApi.request(
      "workspace-load",
      privateRequestKey("workspace", workspaceId),
      async () => sanitizeStudioSnapshot(await client.getStudioSnapshot(workspaceId))
    );
    dispatch({ type: "ready", auth, snapshot, activeWorkspaceId: workspaceId, notice });
    setFeedRefreshToken((value) => value + 1);
  }

  async function reloadPrivateWorkspace(readyState: Extract<StudioState, { status: "ready" }>, notice: string) {
    await loadPrivateWorkspace(
      {
        account: readyState.account,
        session: readyState.session,
        workspaces: readyState.workspaces
      },
      readyState.activeWorkspaceId,
      notice
    );
  }

  function reportReadyActionError(actionError: unknown) {
    if (!isPrivateRequestCancelled(actionError)) {
      dispatch({ type: "notice", error: errorMessage(actionError) });
    }
  }

  async function handleLogin(email: string, password: string) {
    privateApi.reset();
    dispatch({ type: "checking" });
    await resolveAuthenticatedSession(() => client.login({ email, password }), client, dispatch);
  }

  async function handleSignup(name: string, email: string, password: string) {
    privateApi.reset();
    dispatch({ type: "checking" });
    await resolveAuthenticatedSession(() => client.signup({ name, email, password }), client, dispatch);
  }

  async function handleCreateWorkspace(name: string, slug: string) {
    if (state.status !== "onboarding") {
      return;
    }

    try {
      const workspace = await privateApi.request("create-workspace", slug, async () =>
        client.createWorkspace({ name, slug, description: "Server-backed Layrs workspace." })
      );
      const auth = {
        account: state.account,
        session: { ...state.session, activeWorkspaceId: workspace.id },
        workspaces: [...state.workspaces, workspace]
      };
      await loadPrivateWorkspace(auth, workspace.id, "Workspace created.");
      navigate(homeHref());
    } catch (actionError) {
      if (!isPrivateRequestCancelled(actionError)) {
        dispatch({
          type: "onboarding",
          account: state.account,
          session: state.session,
          workspaces: state.workspaces,
          error: errorMessage(actionError)
        });
      }
    }
  }

  async function handleWorkspaceChange(workspaceId: string) {
    if (state.status !== "ready") {
      return;
    }

    dispatch({ type: "checking" });
    navigate(homeHref());
    try {
      await loadPrivateWorkspace(
        { account: state.account, session: state.session, workspaces: state.workspaces },
        workspaceId
      );
    } catch (actionError) {
      if (!isPrivateRequestCancelled(actionError)) {
        dispatch({ type: "server-error", message: errorMessage(actionError) });
      }
    }
  }

  async function handleCreateTeam(input: { name: string }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const team = await privateApi.request("create-team", input.name, async () =>
        client.createTeam({ workspaceId: state.activeWorkspaceId, name: input.name })
      );
      navigate(teamHref(team.id));
      await reloadPrivateWorkspace(state, "Team created.");
    } catch (actionError) {
      reportReadyActionError(actionError);
    }
  }

  async function handleCreateSpace(input: { name: string; key: string; teamId?: string; description?: string }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const space = await privateApi.request("create-space", input.key, async () =>
        client.createSpace({ workspaceId: state.activeWorkspaceId, ...input })
      );
      navigate(spaceHref(space.id));
      await reloadPrivateWorkspace(state, "Space created.");
    } catch (actionError) {
      reportReadyActionError(actionError);
    }
  }

  async function handleDeleteSpace(spaceId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      const result = await privateApi.request("delete-space", spaceId, async () =>
        client.deleteSpace(state.activeWorkspaceId, spaceId)
      );
      navigate(homeHref());
      await reloadPrivateWorkspace(
        state,
        `Space deleted. ${result.deletedLayers} Layer(s) and ${result.deletedArtifacts} artifact(s) were removed.`
      );
    } catch (actionError) {
      reportReadyActionError(actionError);
      if (!isPrivateRequestCancelled(actionError)) {
        throw actionError;
      }
    }
  }

  async function handleDeleteLayer(spaceId: string, layerId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await privateApi.request("delete-layer", privateRequestKey(spaceId, layerId), async () =>
        client.deleteLayer(state.activeWorkspaceId, spaceId, layerId)
      );
      navigate(spaceHref(spaceId));
      await reloadPrivateWorkspace(state, "Layer deleted.");
    } catch (actionError) {
      reportReadyActionError(actionError);
      if (!isPrivateRequestCancelled(actionError)) {
        throw actionError;
      }
    }
  }

  async function handleAddTeamMember(teamId: string, input: { email: string; role: TeamMemberRole }) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await privateApi.request("add-team-member", privateRequestKey(teamId, input.email), async () =>
        client.addTeamMember({ workspaceId: state.activeWorkspaceId, teamId, ...input })
      );
      await reloadPrivateWorkspace(state, "Team member added.");
    } catch (actionError) {
      reportReadyActionError(actionError);
    }
  }

  async function handleRemoveTeamMember(teamId: string, accountId: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await privateApi.request("remove-team-member", privateRequestKey(teamId, accountId), async () =>
        client.removeTeamMember(state.activeWorkspaceId, teamId, accountId)
      );
      await reloadPrivateWorkspace(state, "Team member removed.");
    } catch (actionError) {
      reportReadyActionError(actionError);
    }
  }

  async function handleCreateInvitation(teamId: string, email: string) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await privateApi.request("create-invitation", privateRequestKey(teamId, email), async () =>
        client.createInvitation({ workspaceId: state.activeWorkspaceId, email, role: "member", teamIds: [teamId] })
      );
      await reloadPrivateWorkspace(state, "Invitation created.");
    } catch (actionError) {
      reportReadyActionError(actionError);
    }
  }

  async function handleSaveAccessPolicies(policies: LayerAccessPolicy[]) {
    if (state.status !== "ready") {
      return;
    }

    try {
      await privateApi.request(
        "save-access-policies",
        policies.map((policy) => `${policy.layerId}:${policy.policyEpoch}`).sort().join(","),
        async () => Promise.all(policies.map((policy) => client.replaceLayerAccessPolicy(policy)))
      );
      await reloadPrivateWorkspace(state, "Layer access rules saved.");
    } catch (actionError) {
      reportReadyActionError(actionError);
      if (!isPrivateRequestCancelled(actionError)) {
        throw actionError;
      }
    }
  }

  async function handleRefreshWorkspace() {
    if (state.status !== "ready") {
      return;
    }

    try {
      await loadPrivateWorkspace(
        { account: state.account, session: state.session, workspaces: state.workspaces },
        state.activeWorkspaceId
      );
    } catch (actionError) {
      reportReadyActionError(actionError);
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
        onAction={() => {
          privateApi.reset();
          void bootstrap(client, dispatch);
        }}
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
    <StudioPrivateApiProvider api={privateApi}>
      <StudioWorkspace
      accessPolicies={accessPolicies}
      accessRequestKey={accessRequestKey}
      artifactRequestKey={feedRequestKey}
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
      onRefreshWorkspace={handleRefreshWorkspace}
      onSaveAccessPolicies={handleSaveAccessPolicies}
      onWorkspaceChange={handleWorkspaceChange}
      route={route}
      state={state}
      artifactFeed={artifactFeed}
      feedRefreshToken={feedRefreshToken}
      />
    </StudioPrivateApiProvider>
  );
}
function StudioWorkspace({
  accessPolicies,
  accessRequestKey,
  artifactRequestKey,
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
  onRefreshWorkspace,
  onSaveAccessPolicies,
  onWorkspaceChange,
  route,
  state,
  artifactFeed,
  feedRefreshToken
}: {
  accessPolicies: AccessPolicyState;
  accessRequestKey: string;
  artifactRequestKey: string;
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
  onRefreshWorkspace: () => Promise<void>;
  onSaveAccessPolicies: (policies: LayerAccessPolicy[]) => Promise<void>;
  onWorkspaceChange: (workspaceId: string) => void;
  route: StudioRoute;
  state: Extract<StudioState, { status: "ready" }>;
  artifactFeed: ArtifactFeedState;
  feedRefreshToken: number;
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
          accessPolicies={accessPolicies.status !== "idle" && accessPolicies.key === accessRequestKey ? accessPolicies : { status: "idle" }}
          lensRegistry={lensRegistry}
          layers={selectedLayers}
          onNavigate={onNavigate}
          onDeleteLayer={onDeleteLayer}
          onDeleteSpace={onDeleteSpace}
          onSaveAccessPolicies={onSaveAccessPolicies}
          onRefreshWorkspace={onRefreshWorkspace}
          selectedLayer={selectedLayer}
          snapshot={snapshot}
          serverArtifacts={artifactFeed.status === "available" && artifactFeed.key === artifactRequestKey ? artifactFeed.items : undefined}
          refreshKey={feedRefreshToken}
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
          {["Gates", "Audit"].map((label) => (
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
