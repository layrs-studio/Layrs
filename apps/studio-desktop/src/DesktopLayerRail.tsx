import { StatusPill } from "@layrs/ui";
import { formatUnixTime, layerDisplayName, layersByLatestStep, syncStatusLabel } from "./desktopModel";
import type { CommandKey } from "./desktopTypes";
import type { LocalLayerSummary, LocalSpaceSummary, WorkingTreeScan } from "./tauri";

interface LayerRailPanelProps {
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  query: string;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
  workingTree?: WorkingTreeScan;
  onCreateLayer: () => void;
  onQueryChange: (value: string) => void;
  onSelectLayer: (layerId: string) => void;
}

export function LayerRailPanel({
  busyAction,
  commandErrors,
  query,
  selectedLayer,
  selectedSpace,
  workingTree,
  onCreateLayer,
  onQueryChange,
  onSelectLayer
}: LayerRailPanelProps) {
  const layers = layersByLatestStep(selectedSpace, workingTree, query);
  const proposedName = query.trim();
  const exactNameExists = proposedName
    ? selectedSpace.layers.some((layer) => layer.displayName.toLowerCase() === proposedName.toLowerCase())
    : false;
  const canCreateFromCurrent =
    Boolean(proposedName) &&
    !exactNameExists &&
    busyAction !== "create-layer" &&
    !commandErrors["create-layer"];

  return (
    <div className="desktop-layer-card desktop-layer-card--rail">
      <div className="layrs-section-heading">
        <span>Layers</span>
        <h3>{selectedLayer?.displayName ?? "Switch Layer"}</h3>
      </div>
      <label className="desktop-field">
        <span>Search or create Layer</span>
        <input value={query} onChange={(event) => onQueryChange(event.currentTarget.value)} placeholder="Search Layers or type a new name" />
      </label>
      <div className="desktop-layer-list desktop-layer-list--rail">
        {layers.map(({ layer, latestStepAt, stepCount }) => {
          const isActive = layer.layerId === selectedSpace.activeLayerId;
          return (
            <article className={isActive ? "desktop-layer-row is-active" : "desktop-layer-row"} key={layer.layerId}>
              <button
                type="button"
                className="desktop-layer-row__main"
                onClick={() => onSelectLayer(layer.layerId)}
                disabled={isActive || !layer.canOpen || busyAction === `switch:${layer.layerId}`}
              >
                <strong>{layer.displayName}</strong>
                <span>{layer.parentLayerId ? `Parent: ${layerDisplayName(selectedSpace, layer.parentLayerId)}` : "Base Layer"}</span>
                <span className="desktop-layer-row__latest">
                  {stepCount > 0 ? `Latest step ${formatUnixTime(latestStepAt)}` : "No local steps yet"}
                </span>
              </button>
              <div className="desktop-layer-row__rules">
                <StatusPill status={layer.access === "blocked" ? "blocked" : layer.access === "redacted" ? "needs-proof" : "passing"} label={layer.access} />
                <StatusPill status={layer.syncStatus === "local-only" ? "needs-proof" : "passing"} label={syncStatusLabel(layer.syncStatus)} />
              </div>
            </article>
          );
        })}
        {layers.length === 0 ? <p className="desktop-empty">No Layers match this search.</p> : null}
      </div>
      {canCreateFromCurrent ? (
        <button type="button" className="desktop-secondary-button" onClick={onCreateLayer}>
          Create "{proposedName}" from current
        </button>
      ) : null}
    </div>
  );
}
