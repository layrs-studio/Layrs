import {
  layerAccessPolicyFromWire,
  layerAccessPolicyPath,
  layerAccessPolicyToLegacyRegistry,
  layerAccessPolicyToWire,
  legacyRegistryToLayerAccessPolicy,
  type LayerAccessPolicyWire
} from "./access";
import { getStudioFixture } from "./fixture";
import { normalizeStudioSnapshot, type StudioSnapshotWire } from "./normalizers";
import { getErrorCode, getErrorMessage, readJson, slugify, unwrapItems } from "./utils";
import type {
  AuditEvent,
  AuthSessionResponse,
  Device,
  DeviceFlow,
  Invitation,
  Layer,
  LayerAccessPolicy,
  LayerAccessRegistry,
  LayerAccessRule,
  LayrsId,
  Space,
  StudioSnapshot,
  Team,
  TeamMember,
  TeamMemberRole,
  Workspace,
  WorkspaceMember,
  WorkspaceMemberRole
} from "./types";

export interface LayrsClientOptions {
  baseUrl?: string;
  fetchImpl?: typeof fetch;
}

export interface LoginRequest {
  email: string;
  password: string;
}

export interface SignupRequest {
  name: string;
  email: string;
  password: string;
}

export interface CreateWorkspaceInput {
  name: string;
  slug: string;
  description?: string;
}

export interface CreateTeamInput {
  workspaceId: LayrsId;
  name: string;
  purpose?: string;
  memberAccountIds?: LayrsId[];
}

export interface AddTeamMemberInput {
  workspaceId: LayrsId;
  teamId: LayrsId;
  email?: string;
  accountId?: LayrsId;
  role: TeamMemberRole;
}

export interface CreateInvitationInput {
  workspaceId: LayrsId;
  email: string;
  role: WorkspaceMemberRole;
  teamIds?: LayrsId[];
}

export interface CreateLayerAccessRuleInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  rule: LayerAccessRule;
}

export interface UpdateLayerAccessRuleInput extends CreateLayerAccessRuleInput {}

export interface DeleteLayerAccessRuleInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  layerId: LayrsId;
  ruleId: LayrsId;
}

export interface CreateSpaceInput {
  workspaceId: LayrsId;
  name: string;
  key: string;
  teamId?: LayrsId;
  description?: string;
}

export interface CreateLayerInput {
  workspaceId: LayrsId;
  spaceId: LayrsId;
  name: string;
  parentLayerId?: LayrsId;
  baseLayerIds?: LayrsId[];
  description?: string;
}

export interface DeleteSpaceResult {
  id: LayrsId;
  workspaceId: LayrsId;
  deleted: boolean;
  deletedLayers: number;
  deletedArtifacts: number;
}

export interface DeleteLayerResult {
  id: LayrsId;
  spaceId: LayrsId;
  deleted: boolean;
}

export interface StartDeviceFlowInput {
  label: string;
  kind: Device["kind"];
}

export interface LayrsClientLike {
  getSession(): Promise<AuthSessionResponse>;
  login(request: LoginRequest): Promise<AuthSessionResponse>;
  signup(request: SignupRequest): Promise<AuthSessionResponse>;
  logout(): Promise<void>;
  listWorkspaces(): Promise<Workspace[]>;
  createWorkspace(request: CreateWorkspaceInput): Promise<Workspace>;
  getStudioSnapshot(workspaceId?: LayrsId): Promise<StudioSnapshot>;
  listTeams(workspaceId: LayrsId): Promise<Team[]>;
  getTeam(workspaceId: LayrsId, teamId: LayrsId): Promise<Team>;
  createTeam(request: CreateTeamInput): Promise<Team>;
  listTeamMembers(workspaceId: LayrsId, teamId: LayrsId): Promise<TeamMember[]>;
  addTeamMember(request: AddTeamMemberInput): Promise<TeamMember>;
  removeTeamMember(workspaceId: LayrsId, teamId: LayrsId, accountId: LayrsId): Promise<void>;
  listWorkspaceMembers(workspaceId: LayrsId): Promise<WorkspaceMember[]>;
  listInvitations(workspaceId: LayrsId): Promise<Invitation[]>;
  createInvitation(request: CreateInvitationInput): Promise<Invitation>;
  listMyInvitations(): Promise<Invitation[]>;
  acceptInvitation(invitationId: LayrsId): Promise<Invitation>;
  declineInvitation(invitationId: LayrsId): Promise<Invitation>;
  createSpace(request: CreateSpaceInput): Promise<Space>;
  deleteSpace(workspaceId: LayrsId, spaceId: LayrsId): Promise<DeleteSpaceResult>;
  createLayer(request: CreateLayerInput): Promise<Layer>;
  deleteLayer(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<DeleteLayerResult>;
  getLayerAccessPolicy(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<LayerAccessPolicy>;
  replaceLayerAccessPolicy(policy: LayerAccessPolicy): Promise<LayerAccessPolicy>;
  createLayerAccessRule(input: CreateLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  updateLayerAccessRule(input: UpdateLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  deleteLayerAccessRule(input: DeleteLayerAccessRuleInput): Promise<LayerAccessPolicy>;
  updateLayerAccessRegistry(registry: LayerAccessRegistry): Promise<LayerAccessRegistry>;
  listDevices(): Promise<Device[]>;
  startDeviceFlow(request: StartDeviceFlowInput): Promise<DeviceFlow>;
  listAuditEvents(workspaceId: LayrsId): Promise<AuditEvent[]>;
}

export class LayrsApiError extends Error {
  readonly status: number;
  readonly code?: string;

  constructor(message: string, status: number, code?: string) {
    super(message);
    this.name = "LayrsApiError";
    this.status = status;
    this.code = code;
  }
}

export class LayrsClient implements LayrsClientLike {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;

  constructor(options: LayrsClientOptions = {}) {
    this.baseUrl = (options.baseUrl ?? "").replace(/\/$/, "");
    this.fetchImpl = (options.fetchImpl ?? globalThis.fetch).bind(globalThis) as typeof fetch;
  }

  getSession(): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/session");
  }

  login(request: LoginRequest): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/login", {
      method: "POST",
      body: request
    });
  }

  signup(request: SignupRequest): Promise<AuthSessionResponse> {
    return this.request<AuthSessionResponse>("/v1/auth/signup", {
      method: "POST",
      body: request
    });
  }

  async logout(): Promise<void> {
    await this.request<unknown>("/v1/auth/logout", { method: "POST" });
  }

  async listWorkspaces(): Promise<Workspace[]> {
    return unwrapItems(await this.request<Workspace[] | { items: Workspace[] }>("/v1/workspaces"));
  }

  createWorkspace(request: CreateWorkspaceInput): Promise<Workspace> {
    return this.request<Workspace>("/v1/workspaces", {
      method: "POST",
      body: request
    });
  }

  async getStudioSnapshot(workspaceId?: LayrsId): Promise<StudioSnapshot> {
    const query = workspaceId ? `?workspace_id=${encodeURIComponent(workspaceId)}` : "";
    return normalizeStudioSnapshot(await this.request<StudioSnapshotWire>(`/v1/studio/snapshot${query}`));
  }

  async listTeams(workspaceId: LayrsId): Promise<Team[]> {
    return unwrapItems(
      await this.request<Team[] | { items: Team[] }>(`/v1/workspaces/${encodeURIComponent(workspaceId)}/teams`)
    );
  }

  getTeam(workspaceId: LayrsId, teamId: LayrsId): Promise<Team> {
    return this.request<Team>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}`
    );
  }

  createTeam(request: CreateTeamInput): Promise<Team> {
    return this.request<Team>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/teams`, {
      method: "POST",
      body: {
        name: request.name,
        purpose: request.purpose,
        memberAccountIds: request.memberAccountIds
      }
    });
  }

  async listTeamMembers(workspaceId: LayrsId, teamId: LayrsId): Promise<TeamMember[]> {
    return unwrapItems(
      await this.request<TeamMember[] | { items: TeamMember[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}/members`
      )
    );
  }

  addTeamMember(request: AddTeamMemberInput): Promise<TeamMember> {
    return this.request<TeamMember>(
      `/v1/workspaces/${encodeURIComponent(request.workspaceId)}/teams/${encodeURIComponent(request.teamId)}/members`,
      {
        method: "POST",
        body: {
          email: request.email,
          accountId: request.accountId,
          role: request.role
        }
      }
    );
  }

  async removeTeamMember(workspaceId: LayrsId, teamId: LayrsId, accountId: LayrsId): Promise<void> {
    await this.request<unknown>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/teams/${encodeURIComponent(teamId)}/members/${encodeURIComponent(accountId)}`,
      { method: "DELETE" }
    );
  }

  async listWorkspaceMembers(workspaceId: LayrsId): Promise<WorkspaceMember[]> {
    return unwrapItems(
      await this.request<WorkspaceMember[] | { items: WorkspaceMember[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/members`
      )
    );
  }

  async listInvitations(workspaceId: LayrsId): Promise<Invitation[]> {
    return unwrapItems(
      await this.request<Invitation[] | { items: Invitation[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/invitations`
      )
    );
  }

  createInvitation(request: CreateInvitationInput): Promise<Invitation> {
    return this.request<Invitation>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/invitations`, {
      method: "POST",
      body: {
        email: request.email,
        role: request.role,
        teamIds: request.teamIds ?? []
      }
    });
  }

  async listMyInvitations(): Promise<Invitation[]> {
    return unwrapItems(await this.request<Invitation[] | { items: Invitation[] }>("/v1/me/invitations"));
  }

  acceptInvitation(invitationId: LayrsId): Promise<Invitation> {
    return this.request<Invitation>(`/v1/invitations/${encodeURIComponent(invitationId)}/accept`, { method: "POST" });
  }

  declineInvitation(invitationId: LayrsId): Promise<Invitation> {
    return this.request<Invitation>(`/v1/invitations/${encodeURIComponent(invitationId)}/decline`, { method: "POST" });
  }

  createSpace(request: CreateSpaceInput): Promise<Space> {
    return this.request<Space>(`/v1/workspaces/${encodeURIComponent(request.workspaceId)}/spaces`, {
      method: "POST",
      body: request
    });
  }

  deleteSpace(workspaceId: LayrsId, spaceId: LayrsId): Promise<DeleteSpaceResult> {
    return this.request<DeleteSpaceResult>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}`,
      { method: "DELETE" }
    );
  }

  createLayer(request: CreateLayerInput): Promise<Layer> {
    return this.request<Layer>(
      `/v1/workspaces/${encodeURIComponent(request.workspaceId)}/spaces/${encodeURIComponent(request.spaceId)}/layers`,
      {
        method: "POST",
        body: request
      }
    );
  }

  deleteLayer(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): Promise<DeleteLayerResult> {
    return this.request<DeleteLayerResult>(
      `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}`,
      { method: "DELETE" }
    );
  }

  async getLayerAccessPolicy(
    workspaceId: LayrsId,
    spaceId: LayrsId,
    layerId: LayrsId
  ): Promise<LayerAccessPolicy> {
    const policy = await this.request<LayerAccessPolicyWire>(
      layerAccessPolicyPath(workspaceId, spaceId, layerId)
    );
    return layerAccessPolicyFromWire(policy);
  }

  async replaceLayerAccessPolicy(policy: LayerAccessPolicy): Promise<LayerAccessPolicy> {
    const saved = await this.request<LayerAccessPolicyWire>(
      layerAccessPolicyPath(policy.workspaceId, policy.spaceId, policy.layerId),
      {
        method: "PUT",
        body: layerAccessPolicyToWire(policy)
      }
    );
    return layerAccessPolicyFromWire(saved);
  }

  async createLayerAccessRule(input: CreateLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: [...policy.rules.filter((rule) => rule.id !== input.rule.id), input.rule]
    });
  }

  async updateLayerAccessRule(input: UpdateLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: policy.rules.map((rule) => (rule.id === input.rule.id ? input.rule : rule))
    });
  }

  async deleteLayerAccessRule(input: DeleteLayerAccessRuleInput): Promise<LayerAccessPolicy> {
    const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
    return this.replaceLayerAccessPolicy({
      ...policy,
      rules: policy.rules.filter((rule) => rule.id !== input.ruleId)
    });
  }

  async updateLayerAccessRegistry(registry: LayerAccessRegistry): Promise<LayerAccessRegistry> {
    // Legacy compat only: use replaceLayerAccessPolicy for server writes.
    return { ...registry, updatedAt: new Date().toISOString() };
  }

  async listDevices(): Promise<Device[]> {
    return unwrapItems(await this.request<Device[] | { items: Device[] }>("/v1/devices"));
  }

  startDeviceFlow(request: StartDeviceFlowInput): Promise<DeviceFlow> {
    return this.request<DeviceFlow>("/v1/desktop/device/start", {
      method: "POST",
      body: request
    });
  }

  async listAuditEvents(workspaceId: LayrsId): Promise<AuditEvent[]> {
    return unwrapItems(
      await this.request<AuditEvent[] | { items: AuditEvent[] }>(
        `/v1/workspaces/${encodeURIComponent(workspaceId)}/audit-events`
      )
    );
  }

  private async request<T>(path: string, options: { method?: string; body?: unknown } = {}): Promise<T> {
    const fetchImpl = this.fetchImpl;
    const response = await fetchImpl(`${this.baseUrl}${path}`, {
      method: options.method ?? "GET",
      credentials: "include",
      headers: options.body === undefined ? undefined : { "Content-Type": "application/json" },
      body: options.body === undefined ? undefined : JSON.stringify(options.body)
    });

    if (response.status === 204) {
      return undefined as T;
    }

    const payload = await readJson(response);

    if (!response.ok) {
      const message = getErrorMessage(payload, response.statusText || "Layrs API request failed");
      const code = getErrorCode(payload);
      throw new LayrsApiError(message, response.status, code);
    }

    return payload as T;
  }
}

export function createMockLayrsClient(): LayrsClientLike {
  let snapshot: StudioSnapshot = getStudioFixture();

  return {
    async getSession() {
      return {
        state: "authenticated",
        account: snapshot.account,
        session: snapshot.session,
        workspaces: snapshot.workspaces,
        activeWorkspaceId: snapshot.session.activeWorkspaceId
      };
    },
    async login() {
      return this.getSession();
    },
    async signup() {
      return this.getSession();
    },
    async logout() {
      return undefined;
    },
    async listWorkspaces() {
      return snapshot.workspaces;
    },
    async createWorkspace(request) {
      const workspace: Workspace = {
        id: `workspace-${request.slug}`,
        name: request.name,
        slug: request.slug,
        description: request.description ?? "New Layrs workspace.",
        health: "pending",
        updatedAt: new Date().toISOString()
      };
      snapshot = {
        ...snapshot,
        workspace,
        workspaces: [...snapshot.workspaces, workspace],
        session: { ...snapshot.session, activeWorkspaceId: workspace.id }
      };
      return workspace;
    },
    async getStudioSnapshot(workspaceId) {
      const workspace = snapshot.workspaces.find((item) => item.id === workspaceId) ?? snapshot.workspace;
      return {
        ...snapshot,
        workspace,
        session: { ...snapshot.session, activeWorkspaceId: workspace.id }
      };
    },
    async listTeams(workspaceId) {
      return snapshot.teams.filter((team) => team.workspaceId === workspaceId);
    },
    async getTeam(workspaceId, teamId) {
      const team = snapshot.teams.find((item) => item.workspaceId === workspaceId && item.id === teamId);
      if (!team) {
        throw new LayrsApiError("Team not found", 404, "not_found");
      }
      return team;
    },
    async createTeam(request) {
      const team: Team = {
        id: `team-${slugify(request.name)}`,
        workspaceId: request.workspaceId,
        name: request.name,
        purpose: request.purpose ?? "New team created from Studio Web.",
        members: request.memberAccountIds?.length ?? 0,
        gateResponsibility: "Workspace operations"
      };
      snapshot = { ...snapshot, teams: [...snapshot.teams, team] };
      return team;
    },
    async listTeamMembers(workspaceId, teamId) {
      return snapshot.teamMembers.filter((member) => member.workspaceId === workspaceId && member.teamId === teamId);
    },
    async addTeamMember(request) {
      const accountId = request.accountId ?? `account-${slugify(request.email ?? "invited-member")}`;
      const member: TeamMember = {
        id: `team-membership-${request.teamId}-${accountId}`,
        workspaceId: request.workspaceId,
        teamId: request.teamId,
        accountId,
        email: request.email ?? `${accountId}@layrs.local`,
        name: request.email?.split("@")[0] ?? accountId,
        role: request.role,
        createdAt: new Date().toISOString()
      };
      snapshot = {
        ...snapshot,
        teams: snapshot.teams.map((team) =>
          team.workspaceId === request.workspaceId && team.id === request.teamId
            ? { ...team, members: team.members + 1 }
            : team
        ),
        teamMembers: [...snapshot.teamMembers.filter((item) => item.id !== member.id), member]
      };
      return member;
    },
    async removeTeamMember(workspaceId, teamId, accountId) {
      const nextMembers = snapshot.teamMembers.filter(
        (member) => !(member.workspaceId === workspaceId && member.teamId === teamId && member.accountId === accountId)
      );
      snapshot = {
        ...snapshot,
        teams: snapshot.teams.map((team) =>
          team.workspaceId === workspaceId && team.id === teamId
            ? { ...team, members: Math.max(0, team.members - (nextMembers.length === snapshot.teamMembers.length ? 0 : 1)) }
            : team
        ),
        teamMembers: nextMembers
      };
    },
    async listWorkspaceMembers(workspaceId) {
      return snapshot.workspaceMembers.filter((member) => member.workspaceId === workspaceId);
    },
    async listInvitations(workspaceId) {
      return snapshot.invitations.filter((invitation) => invitation.workspaceId === workspaceId);
    },
    async createInvitation(request) {
      const invitation: Invitation = {
        id: `invitation-${slugify(request.email)}`,
        workspaceId: request.workspaceId,
        email: request.email,
        role: request.role,
        teamIds: request.teamIds ?? [],
        status: "pending",
        createdAt: new Date().toISOString(),
        expiresAt: new Date(Date.now() + 7 * 24 * 60 * 60 * 1000).toISOString()
      };
      snapshot = {
        ...snapshot,
        invitations: [...snapshot.invitations.filter((item) => item.id !== invitation.id), invitation]
      };
      return invitation;
    },
    async listMyInvitations() {
      return snapshot.invitations.filter((invitation) => invitation.status === "pending");
    },
    async acceptInvitation(invitationId) {
      const invitation = snapshot.invitations.find((item) => item.id === invitationId);
      if (!invitation) {
        throw new LayrsApiError("Invitation not found", 404, "not_found");
      }
      const accepted = { ...invitation, status: "accepted" as const };
      snapshot = {
        ...snapshot,
        invitations: snapshot.invitations.map((item) => (item.id === invitationId ? accepted : item))
      };
      return accepted;
    },
    async declineInvitation(invitationId) {
      const invitation = snapshot.invitations.find((item) => item.id === invitationId);
      if (!invitation) {
        throw new LayrsApiError("Invitation not found", 404, "not_found");
      }
      const declined = { ...invitation, status: "declined" as const };
      snapshot = {
        ...snapshot,
        invitations: snapshot.invitations.map((item) => (item.id === invitationId ? declined : item))
      };
      return declined;
    },
    async createSpace(request) {
      const space: Space = {
        id: `space-${request.key}`,
        workspaceId: request.workspaceId,
        teamId: request.teamId ?? snapshot.teams[0]?.id ?? "team-unassigned",
        name: request.name,
        description: request.description ?? "New Space created from Studio Web.",
        status: "pending",
        currentLayerId: "",
        updatedAt: new Date().toISOString()
      };
      snapshot = { ...snapshot, spaces: [...snapshot.spaces, space] };
      return space;
    },
    async deleteSpace(workspaceId, spaceId) {
      const deletedLayers = snapshot.layers.filter((layer) => layer.spaceId === spaceId).length;
      const deletedArtifacts = snapshot.artifacts.filter((artifact) => artifact.spaceId === spaceId).length;
      snapshot = {
        ...snapshot,
        spaces: snapshot.spaces.filter((space) => !(space.workspaceId === workspaceId && space.id === spaceId)),
        layers: snapshot.layers.filter((layer) => layer.spaceId !== spaceId),
        artifacts: snapshot.artifacts.filter((artifact) => artifact.spaceId !== spaceId),
        layerAccessPolicies: snapshot.layerAccessPolicies.filter((policy) => policy.spaceId !== spaceId),
        accessRegistries: snapshot.accessRegistries.filter((registry) =>
          snapshot.layers.some((layer) => layer.id === registry.layerId && layer.spaceId !== spaceId)
        ),
        timeline: snapshot.timeline.filter((event) => !event.relatedIds.includes(spaceId))
      };
      return { id: spaceId, workspaceId, deleted: true, deletedLayers, deletedArtifacts };
    },
    async createLayer(request) {
      const layer: Layer = {
        id: `layer-${slugify(request.name)}`,
        spaceId: request.spaceId,
        parentId: request.parentLayerId,
        name: request.name,
        kind: "proposal",
        status: "review",
        summary: request.description ?? "New Layer created from Studio Web.",
        artifactIds: [],
        stepIds: [],
        gateIds: []
      };
      snapshot = { ...snapshot, layers: [...snapshot.layers, layer] };
      return layer;
    },
    async deleteLayer(_workspaceId, spaceId, layerId) {
      snapshot = {
        ...snapshot,
        layers: snapshot.layers.filter((layer) => !(layer.spaceId === spaceId && layer.id === layerId)),
        artifacts: snapshot.artifacts.filter((artifact) => artifact.layerId !== layerId),
        layerAccessPolicies: snapshot.layerAccessPolicies.filter((policy) => policy.layerId !== layerId),
        accessRegistries: snapshot.accessRegistries.filter((registry) => registry.layerId !== layerId),
        timeline: snapshot.timeline.filter((event) => !event.relatedIds.includes(layerId))
      };
      return { id: layerId, spaceId, deleted: true };
    },
    async getLayerAccessPolicy(workspaceId, spaceId, layerId) {
      const policy = snapshot.layerAccessPolicies.find(
        (item) => item.workspaceId === workspaceId && item.spaceId === spaceId && item.layerId === layerId
      );
      if (!policy) {
        throw new LayrsApiError("Layer access policy not found", 404, "not_found");
      }
      return policy;
    },
    async replaceLayerAccessPolicy(policy) {
      const saved = {
        ...policy,
        policyEpoch: policy.policyEpoch + 1,
        generatedAt: new Date().toISOString()
      };
      const policies = [...snapshot.layerAccessPolicies.filter((item) => item.layerId !== policy.layerId), saved];
      snapshot = {
        ...snapshot,
        layerAccessPolicies: policies,
        accessRegistries: policies.map(layerAccessPolicyToLegacyRegistry)
      };
      return saved;
    },
    async createLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: [...policy.rules.filter((rule) => rule.id !== input.rule.id), input.rule]
      });
    },
    async updateLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: policy.rules.map((rule) => (rule.id === input.rule.id ? input.rule : rule))
      });
    },
    async deleteLayerAccessRule(input) {
      const policy = await this.getLayerAccessPolicy(input.workspaceId, input.spaceId, input.layerId);
      return this.replaceLayerAccessPolicy({
        ...policy,
        rules: policy.rules.filter((rule) => rule.id !== input.ruleId)
      });
    },
    async updateLayerAccessRegistry(registry) {
      const policy = legacyRegistryToLayerAccessPolicy(registry);
      snapshot = {
        ...snapshot,
        accessRegistries: snapshot.accessRegistries.map((item) =>
          item.id === registry.id ? { ...registry, updatedAt: new Date().toISOString() } : item
        ),
        layerAccessPolicies: [
          ...snapshot.layerAccessPolicies.filter((item) => item.layerId !== registry.layerId),
          { ...policy, generatedAt: new Date().toISOString() }
        ]
      };
      return registry;
    },
    async listDevices() {
      return snapshot.devices;
    },
    async startDeviceFlow(request) {
      return {
        id: `device-flow-${slugify(request.label)}`,
        userCode: "LAYRS-2026",
        verificationUri: "https://studio.layrs.local/device",
        status: "pending",
        expiresAt: new Date(Date.now() + 15 * 60 * 1000).toISOString()
      };
    },
    async listAuditEvents() {
      return snapshot.auditEvents;
    }
  };
}

export function createLayrsClient(options: LayrsClientOptions = {}): LayrsClient {
  return new LayrsClient(options);
}

