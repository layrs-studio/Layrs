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

type SpaceTab = "files" | "steps" | "weaves" | "gates" | "access" | "settings";
type DeleteTarget = "space" | "layer" | null;

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
            { id: "weaves", label: "Weaves", disabled: true, note: "Coming later" },
            { id: "gates", label: "Gates", disabled: true, note: "Coming later" },
            { id: "access", label: "Access", count: currentPolicy?.rules.length ?? 0 },
            { id: "settings", label: "Settings" }
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

      {activeTab === "settings" ? (
        <section className="studio-panel studio-panel--wide" id="space-settings">
          <PanelTitle eyebrow="Settings" title="Space administration" />
          <div className="studio-settings-grid">
            <div className="studio-setting-card">
              <span>Space</span>
              <strong>{space.name}</strong>
              <p>{space.description || "No description yet."}</p>
            </div>
            <div className="studio-setting-card">
              <span>Layer</span>
              <strong>{selectedLayer?.name ?? "No Layer selected"}</strong>
              <p>{selectedLayer ? `${selectedLayer.kind} Layer` : "Choose a Layer to manage Layer-level settings."}</p>
            </div>
          </div>
          <div className="studio-danger-stack">
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

async function fetchJson(url: string, signal: AbortSignal): Promise<unknown> {
  const response = await fetch(url, { credentials: "include", signal });
  if (!response.ok) {
    throw new Error(response.statusText);
  }
  return response.json() as Promise<unknown>;
}
