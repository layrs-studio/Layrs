import { LensDiffHost } from "@layrs/lenses";
import { DangerZone, StatusPill, Tabs } from "@layrs/ui";
import type {
  AvailableSpaceView,
  BootstrapData,
  LayerAccessKind,
  LensDiffEntry,
  LocalLayerSummary,
  LocalSpaceSummary,
  WorkingTreeScan
} from "./tauri";
import { FolderChoice, FolderField, PathText } from "./DesktopSettingsView";
import {
  activeLayerCaption,
  compactPath,
  defaultCreateDraft,
  diffWindowState,
  displayPath,
  formatUnixTime,
  layerDisplayName,
  layersByLatestStep,
  syncStatusLabel
} from "./desktopModel";
import type {
  CommandKey,
  CreateDraft,
  LayerFile,
  LocalChange,
  LocalSpaceTab,
  PulseTarget,
  TimelineItem
} from "./desktopTypes";

interface AvailableSpacesViewProps {
  spaces: AvailableSpaceView[];
  createDrafts: Record<string, CreateDraft>;
  defaultLocalRoot: string;
  connected: boolean;
  busyAction: string | null;
  onRefresh: () => void;
  onOpenLocal: (localSpaceId: string) => void;
  onCreate: (space: AvailableSpaceView) => void;
  onDraftChange: (spaceId: string, draft: CreateDraft) => void;
  onChooseTargetFolder: (space: AvailableSpaceView) => void;
}

export function AvailableSpacesView({
  spaces,
  createDrafts,
  defaultLocalRoot,
  connected,
  busyAction,
  onRefresh,
  onOpenLocal,
  onCreate,
  onDraftChange,
  onChooseTargetFolder
}: AvailableSpacesViewProps) {
  const freshness = spaces[0]?.freshness;
  const freshnessMessage = spaces[0]?.message;
  return (
    <div className="desktop-view" id="desktop-available">
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Distant</span>
            <h1>Server Spaces</h1>
          </div>
          <button type="button" className="desktop-secondary-button" onClick={onRefresh} disabled={!connected}>
            Refresh
          </button>
        </div>
        <div className="desktop-table desktop-table--spaces" role="table" aria-label="Available spaces">
          <div className="desktop-table__head" role="row">
            <span role="columnheader">Space</span>
            <span role="columnheader">Workspace</span>
            <span role="columnheader">Layers</span>
            <span role="columnheader">Local</span>
            <span role="columnheader">Target folder</span>
            <span role="columnheader">Action</span>
          </div>
          {spaces.map((space) => {
            const draft = createDrafts[space.spaceId] ?? defaultCreateDraft(space, defaultLocalRoot);
            const localSpace = space.localSpaces[0];
            return (
              <div className="desktop-table__row" role="row" key={space.spaceId}>
                <strong role="cell">{space.name}</strong>
                <span role="cell">{space.workspaceId}</span>
                <span role="cell">{space.layers.length}</span>
                <span role="cell">{localSpace ? <PathText value={localSpace.rootPath} /> : "Not on this machine"}</span>
                <span className="desktop-create-fields" role="cell">
                  {localSpace ? (
                    <em>Already local</em>
                  ) : (
                    <>
                      <FolderChoice
                        value={draft.targetFolder}
                        placeholder="Choose target folder"
                        onChoose={() => onChooseTargetFolder(space)}
                      />
                      <select
                        aria-label={`Initial Layer for ${space.name}`}
                        value={draft.layerId}
                        onChange={(event) => onDraftChange(space.spaceId, { ...draft, layerId: event.currentTarget.value })}
                      >
                        <option value="">Current Layer</option>
                        {space.layers.map((layer) => (
                          <option value={layer.layerId} key={layer.layerId}>
                            {layer.displayName}
                          </option>
                        ))}
                      </select>
                    </>
                  )}
                </span>
                <span className="desktop-button-row" role="cell">
                  {localSpace ? (
                    <button type="button" className="desktop-primary-button" onClick={() => onOpenLocal(localSpace.localSpaceId)}>
                      Open
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="desktop-primary-button"
                      onClick={() => onCreate(space)}
                      disabled={!connected || busyAction === `create:${space.spaceId}`}
                    >
                      Pull to this machine
                    </button>
                  )}
                </span>
              </div>
            );
          })}
        </div>
        {spaces.length === 0 ? (
          <p className="desktop-empty">
            {connected
              ? "No distant Spaces are visible to the connected account. Create one in Studio Web, then Refresh."
              : "Connect a device in Settings to load distant Spaces."}
          </p>
        ) : null}
        {freshness ? <p className="desktop-footnote">Distant list: {freshness}{freshnessMessage ? ` - ${freshnessMessage}` : ""}</p> : null}
      </section>
    </div>
  );
}

interface DraftLocalSpaceViewProps {
  busyAction: string | null;
  draftSpaceFolder: string;
  draftSpaceName: string;
  initSpaceFolder: string;
  initSpaceName: string;
  onChooseDraftFolder: () => void;
  onChooseInitFolder: () => void;
  onCreateDraftSpace: () => void;
  onInitLocalSpace: () => void;
  onDraftSpaceNameChange: (value: string) => void;
  onInitSpaceNameChange: (value: string) => void;
}

export function DraftLocalSpaceView({
  busyAction,
  draftSpaceFolder,
  draftSpaceName,
  initSpaceFolder,
  initSpaceName,
  onChooseDraftFolder,
  onChooseInitFolder,
  onCreateDraftSpace,
  onInitLocalSpace,
  onDraftSpaceNameChange,
  onInitSpaceNameChange
}: DraftLocalSpaceViewProps) {
  return (
    <div className="desktop-view" id="desktop-draft">
      <section className="desktop-panel desktop-panel--wide desktop-draft-page">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Offline Local Spaces</span>
            <h1>Local setup</h1>
          </div>
        </div>
        <div className="desktop-draft-grid">
          <div className="desktop-setting-card desktop-draft-form desktop-local-init-card">
            <span>Initialize existing folder</span>
            <strong>Use files that are already on this machine</strong>
            <label className="desktop-field">
              <span>Space name</span>
              <input value={initSpaceName} onChange={(event) => onInitSpaceNameChange(event.currentTarget.value)} placeholder="My existing project" />
            </label>
            <FolderField
              label="Existing folder"
              value={initSpaceFolder}
              placeholder="Choose the folder to initialize"
              onChoose={onChooseInitFolder}
            />
            <button
              type="button"
              className="desktop-primary-button"
              onClick={onInitLocalSpace}
              disabled={!initSpaceName.trim() || !initSpaceFolder.trim() || busyAction === "init-local"}
            >
              Initialize existing folder
            </button>
            <em>The folder becomes a Local Space, then opens on Changes so you can review and save Steps.</em>
          </div>
          <div className="desktop-setting-card desktop-draft-form desktop-local-init-card">
            <span>Create empty local Space</span>
            <strong>Start offline with an empty Main Layer</strong>
            <label className="desktop-field">
              <span>Space name</span>
              <input value={draftSpaceName} onChange={(event) => onDraftSpaceNameChange(event.currentTarget.value)} placeholder="New game space" />
            </label>
            <FolderField
              label="New folder"
              value={draftSpaceFolder}
              placeholder="Choose where to create this Local Space"
              onChoose={onChooseDraftFolder}
            />
            <button
              type="button"
              className="desktop-primary-button"
              onClick={onCreateDraftSpace}
              disabled={!draftSpaceName.trim() || !draftSpaceFolder.trim() || busyAction === "create-draft"}
            >
              Create empty local Space
            </button>
            <em>Stays local until you choose a Workspace and send it to Studio from the Local header.</em>
          </div>
          <div className="desktop-setting-card">
            <span>Publish path</span>
            <strong>Draft publishing stays in Local</strong>
            <em>Create empty local Space still opens as a draft. Select it under Local, choose a Workspace, then Send to Studio before regular Publish is enabled.</em>
          </div>
        </div>
      </section>
    </div>
  );
}

interface LocalSpacesViewProps {
  localSpaces: LocalSpaceSummary[];
  selectedSpace: LocalSpaceSummary | null;
  selectedLayer: LocalLayerSummary | null;
  files: LayerFile[];
  changes: LocalChange[];
  selectedDiff: LensDiffEntry | null;
  selectedDiffPath: string | null;
  selectedDiffContextKey: string;
  selectedScanRevision: number;
  timeline: TimelineItem[];
  workingTree?: WorkingTreeScan;
  workingTreeChangeCount: number;
  workspaces: BootstrapData["workspaces"];
  sendWorkspaceId: string;
  newLayerName: string;
  layerSearchQuery: string;
  pulseTargets: Set<PulseTarget>;
  activeTab: LocalSpaceTab;
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onSelectSpace: (localSpaceId: string) => void;
  onOpenSpace: (localSpaceId: string) => void;
  onForgetSpace: (localSpaceId: string) => void;
  onScan: (localSpaceId: string) => void;
  onSelectDiff: (path: string) => void;
  onLoadDiffWindow: (path: string, start: number, limit: number) => void;
  onSelectTimeline: (item: TimelineItem) => void;
  onSendWorkspaceChange: (value: string) => void;
  onSendDraft: () => void;
  onSelectLayer: (layerId: string) => void;
  onNewLayerNameChange: (value: string) => void;
  onLayerSearchChange: (value: string) => void;
  onTabChange: (tab: LocalSpaceTab) => void;
  onCreateLayer: () => void;
  onDeleteLayer: (layerId: string) => void;
  onReceive: () => void;
  onPublish: () => void;
}

export function LocalSpacesView({
  localSpaces,
  selectedSpace,
  selectedLayer,
  files,
  changes,
  selectedDiff,
  selectedDiffPath,
  selectedDiffContextKey,
  selectedScanRevision,
  timeline,
  workingTree,
  workingTreeChangeCount,
  workspaces,
  sendWorkspaceId,
  newLayerName,
  layerSearchQuery,
  pulseTargets,
  activeTab,
  busyAction,
  commandErrors,
  onSelectSpace,
  onOpenSpace,
  onForgetSpace,
  onScan,
  onSelectDiff,
  onLoadDiffWindow,
  onSelectTimeline,
  onSendWorkspaceChange,
  onSendDraft,
  onSelectLayer,
  onNewLayerNameChange,
  onLayerSearchChange,
  onTabChange,
  onCreateLayer,
  onDeleteLayer,
  onReceive,
  onPublish
}: LocalSpacesViewProps) {
  const activeAccess = selectedLayer?.access ?? "open";
  const pulseClassName = [
    pulseTargets.has("changes") ? "is-pulsing-changes" : "",
    pulseTargets.has("layer") ? "is-pulsing-layer" : "",
    pulseTargets.has("steps") ? "is-pulsing-steps" : "",
    pulseTargets.has("sync") ? "is-pulsing-sync" : ""
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={`desktop-view desktop-view--local ${pulseClassName}`} id="desktop-local">
      <section className="desktop-panel desktop-local-list">
        <div className="layrs-section-heading">
          <span>Local Spaces</span>
          <h2>This machine</h2>
        </div>
        <div className="desktop-stack">
          {localSpaces.map((space) => (
            <button
              type="button"
              className={selectedSpace?.localSpaceId === space.localSpaceId ? "desktop-space-card is-active" : "desktop-space-card"}
              key={space.localSpaceId}
              onClick={() => onSelectSpace(space.localSpaceId)}
            >
              <strong>
                {space.name}
                {space.state === "draft" ? <span className="desktop-inline-badge">Draft</span> : null}
              </strong>
              <PathText value={space.rootPath} />
              <em>{activeLayerCaption(space)}</em>
            </button>
          ))}
        </div>
        {localSpaces.length === 0 ? <p className="desktop-empty">No Local Spaces detected. Pull one from Distant or create one offline.</p> : null}
        {selectedSpace ? (
          <LayerRailPanel
            busyAction={busyAction}
            query={layerSearchQuery}
            selectedLayer={selectedLayer}
            selectedSpace={selectedSpace}
            workingTree={workingTree}
            onQueryChange={onLayerSearchChange}
            onSelectLayer={onSelectLayer}
          />
        ) : null}
      </section>

      <section className="desktop-panel desktop-space-detail">
        {selectedSpace ? (
          <>
            <div className="desktop-heading-line">
              <div className="layrs-section-heading">
                <span title={displayPath(selectedSpace.rootPath)}>{compactPath(selectedSpace.rootPath, 72)}</span>
                <h1>
                  {selectedSpace.name}
                  {selectedSpace.state === "draft" ? <span className="desktop-inline-badge">Local draft</span> : null}
                </h1>
              </div>
              <div className="desktop-actions">
                {selectedSpace.state === "draft" ? (
                  <>
                    <select
                      aria-label="Workspace target for Draft Local Space"
                      value={sendWorkspaceId}
                      onChange={(event) => onSendWorkspaceChange(event.currentTarget.value)}
                    >
                      <option value="">Choose Workspace</option>
                      {workspaces.map((workspace) => (
                        <option value={workspace.id} key={workspace.id}>
                          {workspace.name}
                        </option>
                      ))}
                    </select>
                    <button
                      type="button"
                      className="desktop-primary-button"
                      onClick={onSendDraft}
                      disabled={!sendWorkspaceId || busyAction === "send-draft" || Boolean(commandErrors["send-draft"])}
                    >
                      Send to Studio
                    </button>
                  </>
                ) : null}
                <button
                  type="button"
                  className="desktop-secondary-button"
                  onClick={onReceive}
                  disabled={selectedSpace.state === "draft" || busyAction === "receive" || Boolean(commandErrors.receive)}
                >
                  Receive
                </button>
                <button
                  type="button"
                  className="desktop-primary-button"
                  onClick={onPublish}
                  disabled={
                    (selectedSpace.state === "draft" && !sendWorkspaceId) ||
                    busyAction === "publish" ||
                    busyAction === "send-draft" ||
                    Boolean(commandErrors.publish) ||
                    Boolean(commandErrors["send-draft"])
                  }
                >
                  Publish
                </button>
                <button type="button" className="desktop-secondary-button" onClick={() => onOpenSpace(selectedSpace.localSpaceId)} disabled={Boolean(commandErrors.open)}>
                  Open folder
                </button>
                <button
                  type="button"
                  className="desktop-secondary-button"
                  onClick={() => onScan(selectedSpace.localSpaceId)}
                  disabled={busyAction === `scan:${selectedSpace.localSpaceId}` || Boolean(commandErrors.scan)}
                >
                  Scan
                </button>
                <details className="desktop-more-menu">
                  <summary>More</summary>
                  <button
                    type="button"
                    className="desktop-danger-button"
                    onClick={() => onForgetSpace(selectedSpace.localSpaceId)}
                    disabled={busyAction === "forget-local"}
                  >
                    Forget local
                  </button>
                </details>
              </div>
            </div>

            <Tabs
              activeId={activeTab}
              ariaLabel="Local Space sections"
              onChange={(nextTab) => onTabChange(nextTab as LocalSpaceTab)}
              tabs={[
                { id: "changes", label: "Changes", count: workingTreeChangeCount },
                { id: "files", label: "Files", count: files.length },
                { id: "steps", label: "Steps", count: timeline.filter((item) => item.kind === "step").length },
                { id: "layers", label: "Layers", count: selectedSpace.layers.length },
                { id: "sync", label: "Sync" },
                { id: "settings", label: "Settings" }
              ]}
            />

            {activeTab === "changes" ? (
              <div className="desktop-review-grid">
                <ChangesPanel
                  changes={changes}
                  selectedPath={selectedDiff?.path ?? selectedDiffPath}
                  onSelectDiff={onSelectDiff}
                />
              <LensDiffPanel
                isLoading={selectedDiff ? busyAction === `diff-window:${selectedDiff.path}` : false}
                onLoadWindow={onLoadDiffWindow}
                renderContextKey={selectedDiffContextKey}
                renderRevision={selectedScanRevision}
                selectedDiff={selectedDiff}
              />
              </div>
            ) : null}

            {activeTab === "files" ? <FilesPanel files={files} selectedLayerAccess={activeAccess} /> : null}

            {activeTab === "steps" ? (
              <div className="desktop-steps-grid">
                <TimelinePanel timeline={timeline} onSelectTimeline={onSelectTimeline} />
                <ChangesPanel changes={changes} selectedPath={selectedDiff?.path ?? selectedDiffPath} onSelectDiff={onSelectDiff} />
                <LensDiffPanel
                  isLoading={selectedDiff ? busyAction === `diff-window:${selectedDiff.path}` : false}
                  onLoadWindow={onLoadDiffWindow}
                  renderContextKey={selectedDiffContextKey}
                  renderRevision={selectedScanRevision}
                  selectedDiff={selectedDiff}
                />
              </div>
            ) : null}

            {activeTab === "layers" ? (
              <LayerManagementPanel
                selectedSpace={selectedSpace}
                selectedLayer={selectedLayer}
                workingTree={workingTree}
                query={layerSearchQuery}
                newLayerName={newLayerName}
                busyAction={busyAction}
                commandErrors={commandErrors}
                onQueryChange={onLayerSearchChange}
                onSelectLayer={onSelectLayer}
                onNewLayerNameChange={onNewLayerNameChange}
                onCreateLayer={onCreateLayer}
                onDeleteLayer={onDeleteLayer}
              />
            ) : null}

            {activeTab === "sync" ? (
              <SyncPanel selectedSpace={selectedSpace} selectedLayer={selectedLayer} changes={changes} commandErrors={commandErrors} />
            ) : null}

            {activeTab === "settings" ? (
              <LocalSpaceSettingsPanel selectedSpace={selectedSpace} selectedLayer={selectedLayer} onForgetSpace={onForgetSpace} busyAction={busyAction} />
            ) : null}
          </>
        ) : (
          <p className="desktop-empty">Select a Local Space to inspect Layers, files, local changes and timeline.</p>
        )}
      </section>
    </div>
  );
}

interface LayerManagementPanelProps {
  selectedSpace: LocalSpaceSummary;
  selectedLayer: LocalLayerSummary | null;
  workingTree?: WorkingTreeScan;
  query: string;
  newLayerName: string;
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onQueryChange: (value: string) => void;
  onSelectLayer: (layerId: string) => void;
  onNewLayerNameChange: (value: string) => void;
  onCreateLayer: () => void;
  onDeleteLayer: (layerId: string) => void;
}

interface LayerRailPanelProps {
  selectedSpace: LocalSpaceSummary;
  selectedLayer: LocalLayerSummary | null;
  workingTree?: WorkingTreeScan;
  query: string;
  busyAction: string | null;
  onQueryChange: (value: string) => void;
  onSelectLayer: (layerId: string) => void;
}

function LayerRailPanel({
  selectedSpace,
  selectedLayer,
  workingTree,
  query,
  busyAction,
  onQueryChange,
  onSelectLayer
}: LayerRailPanelProps) {
  const layers = layersByLatestStep(selectedSpace, workingTree, query);

  return (
    <div className="desktop-layer-card desktop-layer-card--rail">
      <div className="layrs-section-heading">
        <span>Layers</span>
        <h3>{selectedLayer?.displayName ?? "Switch Layer"}</h3>
      </div>
      <label className="desktop-field">
        <span>Search Layers</span>
        <input value={query} onChange={(event) => onQueryChange(event.currentTarget.value)} placeholder="Search by name, access, sync" />
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
    </div>
  );
}

function SyncPanel({
  changes,
  commandErrors,
  selectedLayer,
  selectedSpace
}: {
  changes: LocalChange[];
  commandErrors: Partial<Record<CommandKey, string>>;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Sync status</strong>
        <span>{selectedSpace.state}</span>
      </div>
      <div className="desktop-settings-grid desktop-settings-grid--cards">
        <div className="desktop-setting-card">
          <span>Active Layer</span>
          <strong>{selectedLayer?.displayName ?? "No active Layer"}</strong>
          <em>{selectedLayer ? syncStatusLabel(selectedLayer.syncStatus) : "No sync state"}</em>
        </div>
        <div className="desktop-setting-card">
          <span>Pending changes</span>
          <strong>{changes.length}</strong>
          <em>Review in Changes before publishing.</em>
        </div>
        <div className="desktop-setting-card">
          <span>Receive</span>
          <strong>{commandErrors.receive ? "Blocked" : "Ready"}</strong>
          <em>{commandErrors.receive ?? "Manual receive keeps server data explicit."}</em>
        </div>
        <div className="desktop-setting-card">
          <span>Publish</span>
          <strong>{commandErrors.publish ? "Blocked" : "Ready"}</strong>
          <em>{commandErrors.publish ?? "Manual publish sends the active Layer state."}</em>
        </div>
      </div>
    </section>
  );
}

function LocalSpaceSettingsPanel({
  busyAction,
  onForgetSpace,
  selectedLayer,
  selectedSpace
}: {
  busyAction: string | null;
  onForgetSpace: (localSpaceId: string) => void;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Local Space settings</strong>
        <span>{selectedSpace.state}</span>
      </div>
      <div className="desktop-settings-grid desktop-settings-grid--cards">
        <div className="desktop-setting-card">
          <span>Folder</span>
          <strong title={displayPath(selectedSpace.rootPath)}>{compactPath(selectedSpace.rootPath, 88)}</strong>
          <em>Project files stay on disk when a Local Space is forgotten.</em>
        </div>
        <div className="desktop-setting-card">
          <span>Layer</span>
          <strong>{selectedLayer?.displayName ?? "No active Layer"}</strong>
          <em>{selectedLayer?.access ?? "No access state"}</em>
        </div>
      </div>
      <div className="desktop-danger-stack">
        <DangerZone
          title="Forget this Local Space"
          description="Disconnects this folder from Layrs Desktop and archives .layrs metadata. Your project files remain in place."
        >
          <button
            type="button"
            className="desktop-danger-button"
            onClick={() => onForgetSpace(selectedSpace.localSpaceId)}
            disabled={busyAction === "forget-local"}
          >
            Forget local
          </button>
        </DangerZone>
      </div>
    </section>
  );
}

function LayerManagementPanel({
  selectedSpace,
  selectedLayer,
  workingTree,
  query,
  newLayerName,
  busyAction,
  commandErrors,
  onQueryChange,
  onSelectLayer,
  onNewLayerNameChange,
  onCreateLayer,
  onDeleteLayer
}: LayerManagementPanelProps) {
  const layers = layersByLatestStep(selectedSpace, workingTree, query);
  const activeAccess = selectedLayer?.access ?? "open";
  const activeSyncStatus = selectedLayer?.syncStatus ?? (selectedSpace.state === "draft" ? "local" : "linked");
  const parentLabel = selectedLayer?.parentLayerId ? layerDisplayName(selectedSpace, selectedLayer.parentLayerId) : "None";

  return (
    <section className="desktop-layer-card">
      <div className="layrs-section-heading">
        <span>Layers</span>
        <h3>{selectedLayer?.displayName ?? "No active Layer"}</h3>
      </div>
      <label className="desktop-field">
        <span>Search Layers</span>
        <input value={query} onChange={(event) => onQueryChange(event.currentTarget.value)} placeholder="Search by name, access, sync" />
      </label>
      <div className="desktop-layer-list">
        {layers.map(({ layer, latestStepAt, stepCount }) => {
          const isActive = layer.layerId === selectedSpace.activeLayerId;
          const canDelete = !isActive && selectedSpace.layers.length > 1;
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
              <button
                type="button"
                className="desktop-layer-delete"
                onClick={() => onDeleteLayer(layer.layerId)}
                disabled={!canDelete || busyAction === `delete-layer:${layer.layerId}`}
              >
                Delete
              </button>
            </article>
          );
        })}
        {layers.length === 0 ? <p className="desktop-empty">No Layers match this search.</p> : null}
      </div>
      <div className="desktop-layer-rules">
        <div>
          <span>Access</span>
          <strong>{activeAccess}</strong>
        </div>
        <div>
          <span>Sync</span>
          <strong>{syncStatusLabel(activeSyncStatus)}</strong>
        </div>
        <div>
          <span>Parent</span>
          <strong>{parentLabel}</strong>
        </div>
      </div>
      <div className="desktop-layer-create">
        <label className="desktop-field">
          <span>New Layer</span>
          <input value={newLayerName} onChange={(event) => onNewLayerNameChange(event.currentTarget.value)} placeholder="Layer name" />
        </label>
        <button
          type="button"
          className="desktop-secondary-button"
          onClick={onCreateLayer}
          disabled={!newLayerName.trim() || busyAction === "create-layer" || Boolean(commandErrors["create-layer"])}
        >
          Create from current
        </button>
      </div>
    </section>
  );
}

function FilesPanel({ files, selectedLayerAccess }: { files: LayerFile[]; selectedLayerAccess: LayerAccessKind }) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Files in active Layer</strong>
        <span>{files.length}</span>
      </div>
      <div className="desktop-file-list">
        {files.map((file) => (
          <article className={file.redacted ? "desktop-file-row is-redacted" : "desktop-file-row"} key={file.path}>
            <div>
              <strong>{file.path}</strong>
              <span>
                {file.kind} - {file.sizeLabel}
              </span>
            </div>
            <em>{file.redacted ? "Restricted by Layer access policy" : file.lensId ?? file.state}</em>
            <button type="button" disabled={file.redacted || selectedLayerAccess === "blocked"} title={file.redacted ? "Restricted by Layer access policy" : undefined}>
              View
            </button>
          </article>
        ))}
        {files.length === 0 ? <p className="desktop-empty">Scan the Local Space to load files.</p> : null}
      </div>
    </section>
  );
}

function ChangesPanel({
  changes,
  selectedPath,
  onSelectDiff
}: {
  changes: LocalChange[];
  selectedPath: string | null;
  onSelectDiff: (path: string) => void;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Changes</strong>
        <span>{changes.length}</span>
      </div>
      <div className="desktop-change-list">
        {changes.map((change) => (
          <button
            type="button"
            className={
              selectedPath === change.path
                ? `desktop-change desktop-change--${change.state} is-active`
                : `desktop-change desktop-change--${change.state}`
            }
            key={`${change.state}-${change.path}`}
            onClick={() => onSelectDiff(change.path)}
          >
            <strong>{change.path}</strong>
            <span>{change.summary}</span>
            <em>{change.lensId}</em>
          </button>
        ))}
        {changes.length === 0 ? <p className="desktop-empty">No changes to show.</p> : null}
      </div>
    </section>
  );
}

function LensDiffPanel({
  isLoading,
  onLoadWindow,
  renderContextKey,
  renderRevision,
  selectedDiff
}: {
  isLoading: boolean;
  onLoadWindow: (path: string, start: number, limit: number) => void;
  renderContextKey: string;
  renderRevision: number;
  selectedDiff: LensDiffEntry | null;
}) {
  const windowState = selectedDiff ? diffWindowState(selectedDiff) : null;
  const lensSurfaceKey = selectedDiff
    ? `${renderContextKey}:${renderRevision}:${selectedDiff.path}:${selectedDiff.diff.kind}:${selectedDiff.diff.summary}:${selectedDiff.diff.hunks.length}:${windowState?.label ?? "full"}`
    : `empty:${renderContextKey}:${renderRevision}`;
  return (
    <section className="desktop-subpanel desktop-diff-panel">
      <div className="desktop-subheading">
        <strong>Lens diff</strong>
        <span>{selectedDiff?.lensId ?? "Lens"}</span>
      </div>
      <div className="desktop-diff-panel__meta">
        <strong>{selectedDiff?.path ?? "Select a local change"}</strong>
        <span>{selectedDiff ? selectedDiff.title : "No Lens diff selected"}</span>
      </div>
      <LensDiffHost
        key={lensSurfaceKey}
        className="desktop-shared-lens-diff"
        diff={selectedDiff?.diff ?? null}
        emptyMessage="Select a local change to inspect its Lens diff."
        title={selectedDiff ? selectedDiff.path : "Lens diff"}
      />
      {selectedDiff && windowState?.isWindowed ? (
        <div className="desktop-diff-window-controls">
          <span>{windowState.label}</span>
          <div>
            <button
              type="button"
              className="desktop-secondary-button"
              disabled={isLoading || !windowState.canLoadPrevious}
              onClick={() => onLoadWindow(selectedDiff.path, windowState.previousStart, windowState.limit)}
            >
              Load previous
            </button>
            <button
              type="button"
              className="desktop-secondary-button"
              disabled={isLoading || !windowState.canLoadNext}
              onClick={() => onLoadWindow(selectedDiff.path, windowState.nextStart, windowState.limit)}
            >
              Load next
            </button>
          </div>
        </div>
      ) : null}
      {selectedDiff?.message ? <p className="desktop-footnote">{selectedDiff.message}</p> : null}
    </section>
  );
}

function TimelinePanel({
  timeline,
  onSelectTimeline
}: {
  timeline: TimelineItem[];
  onSelectTimeline: (item: TimelineItem) => void;
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Timeline</strong>
        <span>{timeline.length}</span>
      </div>
      <div className="desktop-timeline">
        {timeline.map((item) => (
          <button
            type="button"
            className={item.isActive ? "desktop-timeline-row is-active" : "desktop-timeline-row"}
            key={item.id}
            onClick={() => onSelectTimeline(item)}
          >
            <time>{item.at}</time>
            <div>
              <strong>{item.title}</strong>
              <span>{item.actor}</span>
              <p>{item.summary}</p>
              {item.diffStats ? (
                <em>
                  {item.diffStats.files} files, +{item.diffStats.additions}, -{item.diffStats.deletions}
                </em>
              ) : null}
            </div>
          </button>
        ))}
      </div>
    </section>
  );
}

