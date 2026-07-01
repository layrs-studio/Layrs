import type {
  Account,
  AuthSessionResponse,
  Layer,
  Session,
  Space,
  StudioSnapshot,
  Workspace
} from "@layrs/client-sdk";

export type AuthScreenMode = "login" | "signup";

export interface RequiredAuth {
  account: Account;
  session: Session;
  workspaces: Workspace[];
}

export type StudioState =
  | { status: "checking" }
  | { status: "signed-out"; mode: AuthScreenMode; error?: string }
  | { status: "server-error"; message: string }
  | { status: "onboarding"; account: Account; session: Session; workspaces: Workspace[]; error?: string }
  | {
      status: "ready";
      account: Account;
      session: Session;
      workspaces: Workspace[];
      snapshot: StudioSnapshot;
      activeWorkspaceId: string;
      selectedSpaceId: string;
      selectedLayerId: string;
      notice?: string;
      error?: string;
    };

export type StudioEvent =
  | { type: "checking" }
  | { type: "signed-out"; mode?: AuthScreenMode; error?: string }
  | { type: "server-error"; message: string }
  | { type: "onboarding"; account: Account; session: Session; workspaces: Workspace[]; error?: string }
  | { type: "ready"; auth: RequiredAuth; snapshot: StudioSnapshot; activeWorkspaceId: string; notice?: string }
  | { type: "select-route"; spaceId?: string; layerId?: string }
  | { type: "select-space"; spaceId: string }
  | { type: "select-layer"; layerId: string }
  | { type: "notice"; notice?: string; error?: string };

export function studioReducer(state: StudioState, event: StudioEvent): StudioState {
  switch (event.type) {
    case "checking":
      return { status: "checking" };
    case "signed-out":
      return { status: "signed-out", mode: event.mode ?? "login", error: event.error };
    case "server-error":
      return { status: "server-error", message: event.message };
    case "onboarding":
      return {
        status: "onboarding",
        account: event.account,
        session: event.session,
        workspaces: event.workspaces,
        error: event.error
      };
    case "ready": {
      const selectedSpace = event.snapshot.spaces[0];
      const selectedLayer = selectedSpace ? defaultLayerForSpace(event.snapshot.layers, selectedSpace) : event.snapshot.layers[0];
      return {
        status: "ready",
        account: event.auth.account,
        session: event.auth.session,
        workspaces: event.auth.workspaces,
        snapshot: event.snapshot,
        activeWorkspaceId: event.activeWorkspaceId,
        selectedSpaceId: selectedSpace?.id ?? "",
        selectedLayerId: selectedLayer?.id ?? "",
        notice: event.notice
      };
    }
    case "select-route": {
      if (state.status !== "ready") {
        return state;
      }

      const selectedSpace =
        state.snapshot.spaces.find((space) => space.id === event.spaceId) ??
        state.snapshot.spaces.find((space) => space.id === state.selectedSpaceId) ??
        state.snapshot.spaces[0];
      const selectedLayer =
        state.snapshot.layers.find((layer) => layer.id === event.layerId && layer.spaceId === selectedSpace?.id) ??
        (selectedSpace ? defaultLayerForSpace(state.snapshot.layers, selectedSpace) : undefined);

      return {
        ...state,
        selectedSpaceId: selectedSpace?.id ?? "",
        selectedLayerId: selectedLayer?.id ?? "",
        error: undefined
      };
    }
    case "select-space": {
      if (state.status !== "ready") {
        return state;
      }
      const selectedSpace = state.snapshot.spaces.find((space) => space.id === event.spaceId);
      const selectedLayer = selectedSpace ? defaultLayerForSpace(state.snapshot.layers, selectedSpace) : undefined;
      return { ...state, selectedSpaceId: event.spaceId, selectedLayerId: selectedLayer?.id ?? "" };
    }
    case "select-layer":
      return state.status === "ready" ? { ...state, selectedLayerId: event.layerId } : state;
    case "notice":
      return state.status === "ready" ? { ...state, notice: event.notice, error: event.error } : state;
    default:
      return state;
  }
}

export function defaultLayerForSpace(layers: Layer[], space: Space): Layer | undefined {
  const spaceLayers = layers.filter((layer) => layer.spaceId === space.id);
  return (
    spaceLayers.find((layer) => layer.name.trim().toLowerCase() === "main") ??
    spaceLayers.find((layer) => layer.id === space.currentLayerId) ??
    spaceLayers[0]
  );
}

export type StudioDispatch = (event: StudioEvent) => void;
export type StudioSessionLoader = () => Promise<AuthSessionResponse>;
