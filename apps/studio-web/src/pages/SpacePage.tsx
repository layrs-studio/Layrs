import { useEffect, useState } from "react";
import type {
  Account,
  Layer,
  LayerAccessPolicy,
  Space,
  StudioSnapshot,
  Team
} from "@layrs/client-sdk";
import { ConfirmModal, DangerZone, StatusPill, Tabs } from "@layrs/ui";
import { layerHref } from "../routes";
import { AccessPolicyEditor } from "../components/AccessPolicyEditor";
import { EmptyState, PanelTitle } from "../components/common";
import type { LensRegistryState } from "../components/LensFileViewer";
import { LayerSelector } from "../components/LayerSelector";
import { LayerStepsPanel } from "../components/LayerStepsPanel";
import { SpaceFilesPanel } from "../components/SpaceFilesPanel";

type SpaceTab = "files" | "steps" | "access" | "gates" | "weaves" | "spaceSettings" | "layerSettings";
type DeleteTarget = "space" | "layer" | null;

interface WeaveRequest {
  weaveId: string;
  sourceLayerId: string;
  targetLayerId: string;
  title: string;
  body: string;
  status: string;
  plannedSteps: string[];
  appliedSteps: string[];
  conflicts?: WeaveConflict[];
  createdAt: string;
  updatedAt: string;
}

interface WeaveConflict {
  conflictId: string;
  path: string;
  lensId: string;
  status: string;
  message: string;
}

export function SpacePage({
  account,
  layers,
  onDeleteLayer,
  onDeleteSpace,
  onNavigate,
  onSaveAccessPolicies,
  selectedLayer,
  space,
  snapshot,
  serverArtifacts,
  workspaceId,
  lensRegistry,
  teams
}: {
  account: Account;
  layers: Layer[];
  lensRegistry: LensRegistryState;
  onDeleteLayer: (spaceId: string, layerId: string) => Promise<void>;
  onDeleteSpace: (spaceId: string) => Promise<void>;
  onNavigate: (href: string) => void;
  onSaveAccessPolicies: (policies: LayerAccessPolicy[]) => Promise<void>;
  selectedLayer?: Layer;
  serverArtifacts?: StudioSnapshot["artifacts"];
  space?: Space;
  snapshot: StudioSnapshot;
  workspaceId: string;
  teams: Team[];
}) {
  const [activeTab, setActiveTab] = useState<SpaceTab>("files");
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget>(null);
  const [confirmationValue, setConfirmationValue] = useState("");
  const [settingsMessage, setSettingsMessage] = useState<string | null>(null);
  const snapshotStepCount = snapshot.steps.filter((step) => step.layerId === selectedLayer?.id).length;
  const liveStepCount = useLayerStepCount({ layer: selectedLayer, spaceId: space?.id, snapshotStepCount, workspaceId });

  if (!space) {
    return <EmptyState title="Space not found" detail="Choose an existing Space from the Workspace page." />;
  }

  const currentSpace = space;
  const owningTeam = teams.find((team) => team.id === space.teamId);
  const snapshotArtifacts = selectedLayer
    ? snapshot.artifacts.filter((artifact) => artifact.layerId === selectedLayer.id)
    : snapshot.artifacts.filter((artifact) => artifact.spaceId === space.id);
  const layerArtifacts = serverArtifacts ?? snapshotArtifacts;
  const currentPolicy = selectedLayer
    ? snapshot.layerAccessPolicies.find((policy) => policy.layerId === selectedLayer.id)
    : undefined;
  const targetName = deleteTarget === "space" ? space.name : selectedLayer?.name ?? "";
  const confirmDisabled = confirmationValue !== targetName || (deleteTarget === "layer" && (!selectedLayer || layers.length <= 1));

  function openDeleteDialog(target: DeleteTarget) {
    setDeleteTarget(target);
    setConfirmationValue("");
  }

  async function confirmDelete() {
    if (deleteTarget === "space") {
      await onDeleteSpace(currentSpace.id);
    }
    if (deleteTarget === "layer" && selectedLayer) {
      await onDeleteLayer(currentSpace.id, selectedLayer.id);
    }
    setDeleteTarget(null);
    setConfirmationValue("");
  }

  async function runLayerSettingsAction(action: "disconnect-parent" | "clear-steps") {
    if (!selectedLayer) {
      return;
    }
    const payload = await fetchJson(layerSettingsActionPath(workspaceId, currentSpace.id, selectedLayer.id, action), new AbortController().signal, {
      method: "POST"
    });
    const record = payload && typeof payload === "object" ? (payload as { message?: unknown }) : undefined;
    const message =
      typeof record?.message === "string"
        ? record.message
        : action === "disconnect-parent"
          ? "Layer disconnected from parent."
          : "Layer Steps cleared from active history.";
    setSettingsMessage(message);
  }

  return (
    <section className="studio-grid" aria-label="Space">
      <section className="studio-panel studio-panel--wide">
        <div className="studio-space-heading">
          <div>
            <PanelTitle eyebrow="Space" title={space.name} />
            <p>{space.description}</p>
          </div>
          <div className="studio-space-heading__meta">
            <StatusPill status={space.status} />
            <span>{owningTeam?.name ?? "Unassigned Team"}</span>
          </div>
        </div>
        <div className="studio-layer-toolbar">
          <LayerSelector
            layers={layers}
            selectedLayerId={selectedLayer?.id}
            onSelect={(layerId) => onNavigate(layerHref(space.id, layerId))}
          />
          <StatusPill status={currentPolicy && currentPolicy.rules.length > 0 ? "reviewing" : "passing"} label={`policy ${currentPolicy?.policyEpoch ?? 0}`} />
          <button type="button" className="studio-secondary-button" onClick={() => setActiveTab("weaves")}>
            Weaves
          </button>
          <button type="button" className="studio-secondary-button" onClick={() => setActiveTab("spaceSettings")}>
            Space settings
          </button>
        </div>
      </section>

      <section className="studio-panel studio-panel--wide studio-space-tabs-panel" aria-label="Space tabs">
        <Tabs
          activeId={activeTab}
          ariaLabel="Space sections"
          onChange={(nextTab) => setActiveTab(nextTab as SpaceTab)}
          tabs={[
            { id: "files", label: "Files", count: layerArtifacts.length },
            { id: "steps", label: "Steps", count: liveStepCount },
            { id: "access", label: "Access", count: currentPolicy?.rules.length ?? 0 },
            { id: "layerSettings", label: "Layer settings", disabled: !selectedLayer },
            { id: "gates", label: "Gates", disabled: true, note: "Coming later" }
          ]}
        />
      </section>

      {activeTab === "files" ? (
        <SpaceFilesPanel artifacts={layerArtifacts} layer={selectedLayer} lensRegistry={lensRegistry} workspaceId={workspaceId} />
      ) : null}

      {activeTab === "steps" ? (
        <LayerStepsPanel
          artifacts={layerArtifacts}
          layer={selectedLayer}
          lensRegistry={lensRegistry}
          snapshotSteps={snapshot.steps}
          spaceId={space.id}
          workspaceId={workspaceId}
        />
      ) : null}

      {activeTab === "weaves" ? (
        <SpaceWeavesPanel layers={layers} selectedLayer={selectedLayer} space={space} workspaceId={workspaceId} />
      ) : null}

      {activeTab === "access" ? (
        <section className="studio-panel studio-panel--wide" id="access">
          <PanelTitle eyebrow="Access Rules" title={selectedLayer?.name ?? "No Layer selected"} />
          <AccessPolicyEditor
            account={account}
            currentLayer={selectedLayer}
            layers={layers}
            policies={snapshot.layerAccessPolicies}
            teams={teams}
            workspaceMembers={snapshot.workspaceMembers}
            onSave={onSaveAccessPolicies}
          />
        </section>
      ) : null}

      {activeTab === "spaceSettings" ? (
        <section className="studio-panel studio-panel--wide" id="space-settings">
          <PanelTitle eyebrow="Space Settings" title="Space administration" />
          <div className="studio-settings-grid">
            <div className="studio-setting-card">
              <span>Space</span>
              <strong>{space.name}</strong>
              <p>{space.description || "No description yet."}</p>
            </div>
            <div className="studio-setting-card">
              <span>Team</span>
              <strong>{owningTeam?.name ?? "Unassigned Team"}</strong>
              <p>Space ownership and memberships are managed at Space level.</p>
            </div>
          </div>
          <div className="studio-danger-stack">
            <DangerZone
              title="Delete Space"
              description="Removes this Space, every Layer, access policy and published artifact associated with it."
            >
              <button type="button" className="studio-danger-button" onClick={() => openDeleteDialog("space")}>
                Delete Space
              </button>
            </DangerZone>
          </div>
        </section>
      ) : null}

      {activeTab === "layerSettings" ? (
        <section className="studio-panel studio-panel--wide" id="layer-settings">
          <PanelTitle eyebrow="Layer Settings" title={selectedLayer?.name ?? "No Layer selected"} />
          {settingsMessage ? <p className="studio-alert">{settingsMessage}</p> : null}
          <div className="studio-settings-grid">
            <div className="studio-setting-card">
              <span>Layer</span>
              <strong>{selectedLayer?.name ?? "No Layer selected"}</strong>
              <p>{selectedLayer ? `${selectedLayer.kind} Layer` : "Choose a Layer to manage Layer-level settings."}</p>
            </div>
            <div className="studio-setting-card">
              <span>Parent</span>
              <strong>{selectedLayer?.parentLayerId ? layers.find((layer) => layer.id === selectedLayer.parentLayerId)?.name ?? selectedLayer.parentLayerId : "None"}</strong>
              <p>{selectedLayer?.parentLayerId ? `Lineage ${selectedLayer.lineageStatus ?? "linked"}` : "Base Layer"}</p>
            </div>
          </div>
          <div className="studio-danger-stack">
            <DangerZone
              title="Disconnect selected Layer from parent"
              description="Stops future parent Step propagation for this Layer. Existing files and Steps remain available."
            >
              <button
                type="button"
                className="studio-danger-button"
                disabled={!selectedLayer?.parentLayerId || selectedLayer.lineageStatus === "unlinked"}
                onClick={() => selectedLayer && void runLayerSettingsAction("disconnect-parent")}
              >
                Disconnect from parent
              </button>
            </DangerZone>
            <DangerZone
              title="Clear selected Layer Steps"
              description="Removes this Layer history from active review while keeping published files. Step rows are retained for audit."
            >
              <button
                type="button"
                className="studio-danger-button"
                disabled={!selectedLayer}
                onClick={() => selectedLayer && void runLayerSettingsAction("clear-steps")}
              >
                Clear Steps
              </button>
            </DangerZone>
            <DangerZone
              title="Delete selected Layer"
              description="Removes published files associated with this Layer. The main Layer cannot be deleted while it is the only Layer."
            >
              <button
                type="button"
                className="studio-danger-button"
                disabled={!selectedLayer || layers.length <= 1}
                onClick={() => openDeleteDialog("layer")}
              >
                Delete Layer
              </button>
            </DangerZone>
          </div>
        </section>
      ) : null}

      <ConfirmModal
        danger
        confirmLabel={deleteTarget === "space" ? "Delete Space" : "Delete Layer"}
        confirmationLabel={`Type ${targetName} to confirm`}
        confirmationValue={confirmationValue}
        description={
          <p>
            This action is destructive and cannot be undone from Studio. Routine review actions are intentionally kept
            outside this Settings tab.
          </p>
        }
        disabled={confirmDisabled}
        onCancel={() => {
          setDeleteTarget(null);
          setConfirmationValue("");
        }}
        onConfirm={() => void confirmDelete()}
        onConfirmationValueChange={setConfirmationValue}
        open={deleteTarget !== null}
        title={deleteTarget === "space" ? `Delete ${space.name}` : `Delete ${selectedLayer?.name ?? "Layer"}`}
      />
    </section>
  );
}

function SpaceWeavesPanel({
  layers,
  selectedLayer,
  space,
  workspaceId
}: {
  layers: Layer[];
  selectedLayer?: Layer;
  space: Space;
  workspaceId: string;
}) {
  const defaultSource = layers.find((layer) => layer.id !== selectedLayer?.id)?.id ?? layers[0]?.id ?? "";
  const defaultTarget = selectedLayer?.id ?? layers[0]?.id ?? "";
  const [requests, setRequests] = useState<WeaveRequest[]>([]);
  const [sourceLayerId, setSourceLayerId] = useState(defaultSource);
  const [targetLayerId, setTargetLayerId] = useState(defaultTarget);
  const [error, setError] = useState("");
  const [isBusy, setIsBusy] = useState(false);

  useEffect(() => {
    setSourceLayerId((current) => current || defaultSource);
    setTargetLayerId((current) => current || defaultTarget);
  }, [defaultSource, defaultTarget]);

  useEffect(() => {
    if (isMockMode()) {
      return;
    }
    const controller = new AbortController();
    void loadWeaves(controller.signal);
    return () => controller.abort();

    async function loadWeaves(signal: AbortSignal) {
      try {
        const payload = await fetchJson(weavesPath(workspaceId, space.id), signal);
        if (!signal.aborted) {
          setRequests(weaveArrayFromPayload(payload));
        }
      } catch (loadError) {
        if (!signal.aborted) {
          setError(loadError instanceof Error ? loadError.message : "Could not load Weaves.");
        }
      }
    }
  }, [space.id, workspaceId]);

  async function mutateWeave(action: () => Promise<unknown>) {
    setIsBusy(true);
    setError("");
    try {
      await action();
      const payload = await fetchJson(weavesPath(workspaceId, space.id), new AbortController().signal);
      setRequests(weaveArrayFromPayload(payload));
    } catch (mutationError) {
      setError(mutationError instanceof Error ? mutationError.message : "Weave action failed.");
    } finally {
      setIsBusy(false);
    }
  }

  function layerName(layerId: string) {
    return layers.find((layer) => layer.id === layerId)?.name ?? layerId;
  }

  const canCreate = sourceLayerId && targetLayerId && sourceLayerId !== targetLayerId && !isBusy;

  return (
    <section className="studio-panel studio-panel--wide" id="weaves">
      <PanelTitle eyebrow="Weaves" title="Layer reconciliation" />
      {error ? <p className="studio-inline-error">{error}</p> : null}
      <div className="studio-settings-grid">
        <label className="studio-field">
          <span>Source Layer</span>
          <select value={sourceLayerId} onChange={(event) => setSourceLayerId(event.target.value)}>
            {layers.map((layer) => (
              <option key={layer.id} value={layer.id}>
                {layer.name}
              </option>
            ))}
          </select>
        </label>
        <label className="studio-field">
          <span>Target Layer</span>
          <select value={targetLayerId} onChange={(event) => setTargetLayerId(event.target.value)}>
            {layers.map((layer) => (
              <option key={layer.id} value={layer.id}>
                {layer.name}
              </option>
            ))}
          </select>
        </label>
        <div className="studio-setting-card">
          <span>Rule</span>
          <strong>Durable request</strong>
          <p>Apply stays disabled server-side while unresolved conflicts exist.</p>
        </div>
      </div>
      <div className="studio-action-row">
        <button
          type="button"
          className="studio-primary-button"
          disabled={!canCreate}
          onClick={() =>
            void mutateWeave(() =>
              fetchJson(weavesPath(workspaceId, space.id), new AbortController().signal, {
                method: "POST",
                body: JSON.stringify({
                  sourceLayerId,
                  targetLayerId,
                  title: `${layerName(sourceLayerId)} into ${layerName(targetLayerId)}`
                })
              })
            )
          }
        >
          Create Weave Request
        </button>
      </div>
      <div className="studio-step-list">
        {requests.length === 0 ? (
          <EmptyState title="No Weaves yet" detail="Create a request to reconcile one Layer into another." />
        ) : (
          requests.map((request) => (
            <article className="studio-step-row" key={request.weaveId}>
              <span>{request.status}</span>
              <div>
                <strong>{request.title}</strong>
                <p>
                  {layerName(request.sourceLayerId)} {"->"} {layerName(request.targetLayerId)}
                </p>
                <small>
                  {request.plannedSteps.length} planned Steps, {request.appliedSteps.length} applied,{" "}
                  {request.conflicts?.length ?? 0} conflicts
                </small>
              </div>
              <div className="studio-action-row">
                <button
                  type="button"
                  className="studio-secondary-button"
                  disabled={isBusy || request.status === "applied" || request.status === "aborted"}
                  onClick={() =>
                    void mutateWeave(() =>
                      fetchJson(
                        `${weavesPath(workspaceId, space.id)}/${encodeURIComponent(request.weaveId)}/apply`,
                        new AbortController().signal,
                        { method: "POST" }
                      )
                    )
                  }
                >
                  Apply
                </button>
                <button
                  type="button"
                  className="studio-danger-button"
                  disabled={isBusy || request.status === "applied" || request.status === "aborted"}
                  onClick={() =>
                    void mutateWeave(() =>
                      fetchJson(
                        `${weavesPath(workspaceId, space.id)}/${encodeURIComponent(request.weaveId)}/abort`,
                        new AbortController().signal,
                        { method: "POST" }
                      )
                    )
                  }
                >
                  Abort
                </button>
              </div>
            </article>
          ))
        )}
      </div>
    </section>
  );
}

function useLayerStepCount({
  layer,
  snapshotStepCount,
  spaceId,
  workspaceId
}: {
  layer?: Layer;
  snapshotStepCount: number;
  spaceId?: string;
  workspaceId: string;
}) {
  const [count, setCount] = useState(snapshotStepCount);

  useEffect(() => {
    setCount(snapshotStepCount);
  }, [snapshotStepCount]);

  useEffect(() => {
    if (!layer || !spaceId || isMockMode()) {
      return;
    }

    const controller = new AbortController();
    const path = `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layer.id)}/steps`;
    void fetchJson(`${runtimeApiBaseUrl()}${path}`, controller.signal)
      .then((payload) => {
        if (!controller.signal.aborted) {
          setCount(stepArrayFromPayload(payload).length);
        }
      })
      .catch(() => {
        if (!controller.signal.aborted) {
          setCount(snapshotStepCount);
        }
      });

    return () => controller.abort();
  }, [layer, snapshotStepCount, spaceId, workspaceId]);

  return count;
}

function stepArrayFromPayload(payload: unknown): unknown[] {
  if (Array.isArray(payload)) {
    return payload;
  }
  const record = objectPayload(payload);
  const items = record?.items ?? record?.steps;
  return Array.isArray(items) ? items : [];
}

function weaveArrayFromPayload(payload: unknown): WeaveRequest[] {
  if (Array.isArray(payload)) {
    return payload as WeaveRequest[];
  }
  const record = objectPayload(payload);
  const items = record?.items ?? record?.weaves ?? record?.weaveRequests;
  return Array.isArray(items) ? (items as WeaveRequest[]) : [];
}

function objectPayload(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function runtimeEnv(): Record<string, string | undefined> {
  return (import.meta as unknown as { env?: Record<string, string | undefined> }).env ?? {};
}

function runtimeApiBaseUrl(): string {
  return (runtimeEnv().VITE_LAYRS_API_URL ?? "").replace(/\/$/, "");
}

function isMockMode(): boolean {
  const env = runtimeEnv();
  return env.VITE_LAYRS_STUDIO_MODE === "mock" || env.VITE_LAYRS_API_MOCK === "true";
}

async function fetchJson(url: string, signal: AbortSignal, init: RequestInit = {}): Promise<unknown> {
  const response = await fetch(url, {
    credentials: "include",
    headers: init.body === undefined ? undefined : { "Content-Type": "application/json" },
    signal,
    ...init
  });
  if (!response.ok) {
    throw new Error(response.statusText);
  }
  return response.json() as Promise<unknown>;
}

function weavesPath(workspaceId: string, spaceId: string): string {
  return `${runtimeApiBaseUrl()}/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/weave-requests`;
}

function layerSettingsActionPath(
  workspaceId: string,
  spaceId: string,
  layerId: string,
  action: "disconnect-parent" | "clear-steps"
): string {
  return `${runtimeApiBaseUrl()}/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}/${action}`;
}
