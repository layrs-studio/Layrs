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
  type TimelineItem
} from "@layrs/client-sdk";
import { parseLensManifestList, type LensManifest } from "@layrs/lens-sdk";
import type { StudioRoute } from "./routes";
import {
  defaultLayerForSpace,
  type RequiredAuth,
  type StudioDispatch,
  type StudioSessionLoader,
  type StudioState
} from "./state/studioReducer";
import { fallbackLensManifests, type LensRegistryState } from "./components/LensFileViewer";
import { sanitizeArtifacts, sanitizeStudioSnapshot } from "./privateData";
import type { PrivateJsonFetcher } from "./studioPrivateApi";

export type TimelineFeedState =
  | { status: "idle" }
  | { status: "loading" | "fallback"; key: string; message?: string }
  | { status: "available"; key: string; items: TimelineItem[]; message?: string };

export type ArtifactFeedState =
  | { status: "idle" }
  | { status: "loading" | "fallback"; key: string; message?: string }
  | { status: "available"; key: string; items: Artifact[]; message?: string };

export type AccessPolicyState =
  | { status: "idle" }
  | { status: "loading"; key: string }
  | { status: "available"; key: string; policies: LayerAccessPolicy[] }
  | { status: "error"; key: string; message: string };

export type TimelineFeedTarget = {
  workspaceId: string;
  spaceId: string;
  layerId: string;
};

export async function bootstrap(client: LayrsClientLike, dispatch: StudioDispatch) {
  dispatch({ type: "checking" });
  await resolveAuthenticatedSession(() => client.getSession(), client, dispatch);
}

export async function resolveAuthenticatedSession(
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

export async function loadWorkspace(
  client: LayrsClientLike,
  dispatch: StudioDispatch,
  auth: RequiredAuth,
  workspaceId: string,
  notice?: string
) {
  const snapshot = sanitizeStudioSnapshot(await client.getStudioSnapshot(workspaceId));
  dispatch({ type: "ready", auth, snapshot, activeWorkspaceId: workspaceId, notice });
}

export async function reloadReadyWorkspace(
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

export function resolveSelectedLayer(layers: Layer[], selectedSpace: Extract<StudioState, { status: "ready" }>["snapshot"]["spaces"][number] | undefined, routeLayerId?: string) {
  if (!selectedSpace) {
    return undefined;
  }

  return (
    layers.find((layer) => layer.id === routeLayerId) ??
    defaultLayerForSpace(layers, selectedSpace)
  );
}

export function resolveTimelineFeedTarget(state: StudioState, route: StudioRoute): TimelineFeedTarget | undefined {
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

export function createClientForRuntime(): LayrsClientLike {
  const env = runtimeEnv();
  if (isMockMode()) {
    return createMockLayrsClient();
  }

  return createLayrsClient({ baseUrl: env.VITE_LAYRS_API_URL ?? "" });
}

export function isMockMode(): boolean {
  const env = runtimeEnv();
  return env.VITE_LAYRS_STUDIO_MODE === "mock" || env.VITE_LAYRS_API_MOCK === "true";
}

function runtimeEnv(): Record<string, string | undefined> {
  return (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {};
}

export function runtimeApiBaseUrl(): string {
  return (runtimeEnv().VITE_LAYRS_API_URL ?? "").replace(/\/$/, "");
}

export async function loadLensRegistry(baseUrl: string, fetchJson: PrivateJsonFetcher): Promise<LensRegistryState> {
  try {
    const payload = await fetchJson(`${baseUrl}/v1/lenses`);
    const manifests = normalizeLensManifests(payload);
    if (manifests.length === 0) {
      return {
        status: "fallback",
        manifests: fallbackLensManifests
      };
    }

    return { status: "available", manifests };
  } catch (error) {
    if (statusFromError(error) === 401) {
      throw error;
    }
    return {
      status: "fallback",
      manifests: fallbackLensManifests,
      message: `Lens registry unavailable. Studio is using canonical local lenses. ${optionalStatus(error)}`
    };
  }
}

export async function loadTimelineFeed(
  baseUrl: string,
  target: TimelineFeedTarget,
  fetchJson: PrivateJsonFetcher,
  key: string
): Promise<TimelineFeedState> {
  const path = `/v1/workspaces/${encodeURIComponent(target.workspaceId)}/spaces/${encodeURIComponent(target.spaceId)}/layers/${encodeURIComponent(target.layerId)}/timeline?limit=50`;

  try {
    const payload = await fetchJson(`${baseUrl}${path}`);
    return { status: "available", key, items: normalizeTimelineItems(payload) };
  } catch (error) {
    if (statusFromError(error) === 401) {
      throw error;
    }
    return {
      status: "fallback",
      key,
      message: `Timeline endpoint unavailable. Studio is using snapshot activity. ${optionalStatus(error)}`
    };
  }
}

export async function loadArtifactFeed(
  baseUrl: string,
  target: TimelineFeedTarget,
  fetchJson: PrivateJsonFetcher,
  key: string
): Promise<ArtifactFeedState> {
  const path = `/v1/workspaces/${encodeURIComponent(target.workspaceId)}/spaces/${encodeURIComponent(target.spaceId)}/layers/${encodeURIComponent(target.layerId)}/artifacts`;

  try {
    const payload = await fetchJson(`${baseUrl}${path}`);
    return { status: "available", key, items: sanitizeArtifacts(normalizeArtifactCollection(payload)) };
  } catch (error) {
    if (statusFromError(error) === 401) {
      throw error;
    }
    return {
      status: "fallback",
      key,
      message: `Artifact endpoint unavailable. Studio is using snapshot files. ${optionalStatus(error)}`
    };
  }
}

function normalizeLensManifests(payload: unknown): LensManifest[] {
  const record = objectPayload(payload);
  if (!record || !Array.isArray(record.items)) {
    throw new Error("Lens registry response must contain a canonical items array");
  }
  if (Object.keys(record).some((key) => key !== "items" && key !== "errors")) {
    throw new Error("Lens registry response contains unknown fields");
  }
  if (record.errors !== undefined && !Array.isArray(record.errors)) {
    throw new Error("Lens registry response errors must be an array");
  }
  return parseLensManifestList(record.items);
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

export function loadFavoriteSpaceIds(workspaceId: string): string[] {
  try {
    const raw = globalThis.localStorage?.getItem(favoriteSpaceStorageKey(workspaceId));
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
}

export function saveFavoriteSpaceIds(workspaceId: string, spaceIds: string[]) {
  try {
    globalThis.localStorage?.setItem(favoriteSpaceStorageKey(workspaceId), JSON.stringify(spaceIds));
  } catch {
    // Favorites are a local convenience; failing to persist them should not block Studio.
  }
}

function optionalStatus(error: unknown): string {
  if (error instanceof LayrsApiError) {
    return `(${error.status})`;
  }
  const status = statusFromError(error);
  if (status !== undefined) {
    return `(${status})`;
  }
  return "";
}

function statusFromError(error: unknown): number | undefined {
  if (!error || typeof error !== "object") {
    return undefined;
  }
  const status = (error as { status?: unknown }).status;
  return typeof status === "number" ? status : undefined;
}

export function errorMessage(error: unknown): string {
  if (error instanceof LayrsApiError) {
    return `${error.message} (${error.status})`;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return "Unable to reach Layrs server.";
}
