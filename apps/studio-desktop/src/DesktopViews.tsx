import { LensDiffHost } from "@layrs/lenses";
import { DangerZone, Tabs } from "@layrs/ui";
import type { AvailableSpaceView, BootstrapData, LayerAccessKind, LensDiffEntry, LocalLayerSummary, LocalSpaceSummary, WeaveSessionSummary, WorkingTreeScan } from "./tauri";
import { LayerRailPanel } from "./DesktopLayerRail";
import { FolderChoice, FolderField, PathText } from "./DesktopSettingsView";
import {
  compactPath,
  defaultCreateDraft,
  diffWindowState,
  displayPath,
  layerDisplayName,
  syncStatusLabel
} from "./desktopModel";
import type { CommandKey, CreateDraft, LayerFile, LocalChange, LocalSpaceTab, PulseTarget, TimelineItem } from "./desktopTypes";
import { WeavesPanel } from "./DesktopWeavesView";

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
            <em>Stays local until you link it from the Local Space Sync tab.</em>
          </div>
          <div className="desktop-setting-card">
            <span>Publish path</span>
            <strong>Draft publishing stays in Local</strong>
            <em>Create empty local Space still opens as a draft. Select it under Local, then use Sync to create it in a Workspace.</em>
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
  layerSearchQuery: string;
  pulseTargets: Set<PulseTarget>;
  activeTab: LocalSpaceTab;
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onScan: (localSpaceId: string) => void;
  onSelectDiff: (path: string) => void;
  onLoadDiffWindow: (path: string, start: number, limit: number) => void;
  onSelectTimeline: (item: TimelineItem) => void;
  onSelectLayer: (layerId: string) => void;
  onLayerSearchChange: (value: string) => void;
  onTabChange: (tab: LocalSpaceTab) => void;
  onCreateLayer: () => void;
  onWeaveToParent: () => void;
  onSync: () => void;
  onClearLayerSteps: (layerId: string) => void;
  onDeleteLayer: (layerId: string) => void;
  onDisconnectLayer: (layerId: string) => void;
  onOpenSpaceSettings: () => void;
  onOpenSpaceWeaves: () => void;
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
  layerSearchQuery,
  pulseTargets,
  activeTab,
  busyAction,
  commandErrors,
  onScan,
  onSelectDiff,
  onLoadDiffWindow,
  onSelectTimeline,
  onSelectLayer,
  onLayerSearchChange,
  onTabChange,
  onCreateLayer,
  onWeaveToParent,
  onSync,
  onClearLayerSteps,
  onDeleteLayer,
  onDisconnectLayer,
  onOpenSpaceSettings,
  onOpenSpaceWeaves
}: LocalSpacesViewProps) {
  const activeAccess = selectedLayer?.access ?? "open";
  const parentLayer = selectedLayer?.parentLayerId
    ? selectedSpace?.layers.find((layer) => layer.layerId === selectedLayer.parentLayerId) ?? null
    : null;
  const canWeaveToParent = Boolean(selectedLayer?.parentLayerId && selectedLayer?.lineageStatus !== "unlinked");
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
                <div className="desktop-space-nav">
                  <button type="button" className="desktop-ghost-button" onClick={onOpenSpaceSettings}>
                    Space settings
                  </button>
                  <button type="button" className="desktop-ghost-button" onClick={onOpenSpaceWeaves}>
                    Weaves
                  </button>
                </div>
              </div>
              <div className="desktop-actions">
                <button
                  type="button"
                  className="desktop-primary-button"
                  onClick={onSync}
                  disabled={
                    busyAction === "sync" ||
                    busyAction === "send-draft" ||
                    Boolean(commandErrors.sync) ||
                    Boolean(commandErrors["send-draft"])
                  }
                >
                  Sync
                </button>
                {parentLayer && canWeaveToParent ? (
                  <button
                    type="button"
                    className="desktop-secondary-button"
                    onClick={onWeaveToParent}
                    disabled={busyAction === "weave-parent" || Boolean(commandErrors["weave-parent"])}
                  >
                    Weave to {parentLayer.displayName}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="desktop-secondary-button"
                  onClick={() => onScan(selectedSpace.localSpaceId)}
                  disabled={busyAction === `scan:${selectedSpace.localSpaceId}` || Boolean(commandErrors.scan)}
                >
                  Scan
                </button>
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
                { id: "settings", label: "Layer settings" }
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

            {activeTab === "settings" ? (
              <LayerSettingsView
                busyAction={busyAction}
                commandErrors={commandErrors}
                onClearSteps={onClearLayerSteps}
                onDeleteLayer={onDeleteLayer}
                onDisconnectLayer={onDisconnectLayer}
                selectedLayer={selectedLayer}
                selectedSpace={selectedSpace}
              />
            ) : null}

          </>
        ) : (
          <p className="desktop-empty">
            {localSpaces.length === 0
              ? "No Local Spaces detected. Pull one from Distant or create one offline."
              : "Select a Local Space from the sidebar to inspect Layers, files, local changes and timeline."}
          </p>
        )}
      </section>
      {selectedSpace ? (
        <section className="desktop-panel desktop-layer-rail">
          <LayerRailPanel
            busyAction={busyAction}
            commandErrors={commandErrors}
            query={layerSearchQuery}
            selectedLayer={selectedLayer}
            selectedSpace={selectedSpace}
            workingTree={workingTree}
            onCreateLayer={onCreateLayer}
            onQueryChange={onLayerSearchChange}
            onSelectLayer={onSelectLayer}
          />
        </section>
      ) : null}
    </div>
  );
}

function SyncPanel({
  busyAction,
  changes,
  commandErrors,
  onSendDraft,
  onSendWorkspaceChange,
  sendWorkspaceId,
  selectedLayer,
  selectedSpace,
  workspaces
}: {
  busyAction: string | null;
  changes: LocalChange[];
  commandErrors: Partial<Record<CommandKey, string>>;
  onSendDraft: () => void;
  onSendWorkspaceChange: (value: string) => void;
  sendWorkspaceId: string;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary;
  workspaces: BootstrapData["workspaces"];
}) {
  return (
    <section className="desktop-subpanel">
      <div className="desktop-subheading">
        <strong>Sync status</strong>
        <span>{selectedSpace.state}</span>
      </div>
      {selectedSpace.state === "draft" ? (
        <div className="desktop-setting-card desktop-setting-card--wide">
          <span>Draft Local Space</span>
          <strong>Create this Space in Studio before normal Sync</strong>
          <em>Choose the target Workspace here; Local headers stay focused on copied Spaces.</em>
          <div className="desktop-draft-sync-row">
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
              Create in Studio and Sync
            </button>
          </div>
          {commandErrors["send-draft"] ? <p className="desktop-alert desktop-alert--error">{commandErrors["send-draft"]}</p> : null}
        </div>
      ) : null}
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
  commandErrors,
  onForgetSpace,
  onOpenSpace,
  selectedLayer,
  selectedSpace
}: {
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onForgetSpace: (localSpaceId: string) => void;
  onOpenSpace: (localSpaceId: string) => void;
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
        <div className="desktop-setting-card">
          <span>Folder action</span>
          <strong>Open local folder</strong>
          <em>Opens the copied Space on this machine.</em>
          <button
            type="button"
            className="desktop-secondary-button"
            onClick={() => onOpenSpace(selectedSpace.localSpaceId)}
            disabled={busyAction === `open:${selectedSpace.localSpaceId}` || Boolean(commandErrors.open)}
          >
            Open folder
          </button>
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

export function SpaceSettingsView({
  busyAction,
  changes,
  commandErrors,
  onBack,
  onForgetSpace,
  onOpenSpace,
  onSendDraft,
  onSendWorkspaceChange,
  selectedLayer,
  selectedSpace,
  sendWorkspaceId,
  workspaces
}: {
  busyAction: string | null;
  changes: LocalChange[];
  commandErrors: Partial<Record<CommandKey, string>>;
  onBack: () => void;
  onForgetSpace: (localSpaceId: string) => void;
  onOpenSpace: (localSpaceId: string) => void;
  onSendDraft: () => void;
  onSendWorkspaceChange: (value: string) => void;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary | null;
  sendWorkspaceId: string;
  workspaces: BootstrapData["workspaces"];
}) {
  if (!selectedSpace) {
    return <p className="desktop-empty">Select a Local Space before opening Space settings.</p>;
  }

  return (
    <div className="desktop-settings-stack">
      <div className="desktop-page-return">
        <button type="button" className="desktop-ghost-button" onClick={onBack}>
          Back to Space
        </button>
        <span>{selectedSpace.name}</span>
      </div>
      <LocalSpaceSettingsPanel
        busyAction={busyAction}
        commandErrors={commandErrors}
        onForgetSpace={onForgetSpace}
        onOpenSpace={onOpenSpace}
        selectedLayer={selectedLayer}
        selectedSpace={selectedSpace}
      />
      <SyncPanel
        busyAction={busyAction}
        changes={changes}
        commandErrors={commandErrors}
        onSendDraft={onSendDraft}
        onSendWorkspaceChange={onSendWorkspaceChange}
        selectedLayer={selectedLayer}
        selectedSpace={selectedSpace}
        sendWorkspaceId={sendWorkspaceId}
        workspaces={workspaces}
      />
    </div>
  );
}

export function LayerSettingsView({
  busyAction,
  commandErrors,
  onClearSteps,
  onDeleteLayer,
  onDisconnectLayer,
  selectedLayer,
  selectedSpace
}: {
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onClearSteps: (layerId: string) => void;
  onDeleteLayer: (layerId: string) => void;
  onDisconnectLayer: (layerId: string) => void;
  selectedLayer: LocalLayerSummary | null;
  selectedSpace: LocalSpaceSummary | null;
}) {
  if (!selectedSpace || !selectedLayer) {
    return <p className="desktop-empty">Select a Layer before opening Layer settings.</p>;
  }
  const parentName = selectedLayer.parentLayerId ? layerDisplayName(selectedSpace, selectedLayer.parentLayerId) : "None";
  const canDelete = selectedSpace.layers.length > 1 && selectedSpace.activeLayerId !== selectedLayer.layerId;
  const canDisconnect = Boolean(selectedLayer.parentLayerId && selectedLayer.lineageStatus !== "unlinked");

  return (
    <div className="desktop-view">
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-subheading">
          <strong>Layer settings</strong>
          <span>{selectedLayer.displayName}</span>
        </div>
        <div className="desktop-settings-grid desktop-settings-grid--cards">
          <div className="desktop-setting-card">
            <span>Parent</span>
            <strong>{parentName}</strong>
            <em>{selectedLayer.parentLayerId ? `Lineage ${selectedLayer.lineageStatus ?? "linked"}` : "Base Layer"}</em>
          </div>
          <div className="desktop-setting-card">
            <span>Access</span>
            <strong>{selectedLayer.access}</strong>
            <em>{selectedLayer.canOpen ? "Open locally" : "Restricted locally"}</em>
          </div>
          <div className="desktop-setting-card">
            <span>Sync</span>
            <strong>{syncStatusLabel(selectedLayer.syncStatus)}</strong>
            <em>Layer-specific sync state.</em>
          </div>
        </div>
      </section>
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-subheading">
          <strong>Danger zone</strong>
          <span>{selectedLayer.displayName}</span>
        </div>
        <div className="desktop-danger-stack">
          <DangerZone
            title="Disconnect from parent"
            description="Stops automatic propagation of future parent Steps into this Layer. Existing files and Steps stay in place."
          >
            <button
              type="button"
              className="desktop-danger-button"
              disabled={!canDisconnect || busyAction === "disconnect-layer" || Boolean(commandErrors["disconnect-layer"])}
              onClick={() => onDisconnectLayer(selectedLayer.layerId)}
            >
              Disconnect from parent
            </button>
          </DangerZone>
          <DangerZone
            title="Clear Layer Steps"
            description="Removes this Layer history from the active timeline while keeping files and object data. Step metadata is archived for diagnostics."
          >
            <button
              type="button"
              className="desktop-danger-button"
              disabled={busyAction === "clear-steps" || Boolean(commandErrors["clear-steps"])}
              onClick={() => onClearSteps(selectedLayer.layerId)}
            >
              Clear steps
            </button>
          </DangerZone>
          <DangerZone
            title="Delete Layer"
            description="Deletes this Layer state. Switch away from the active Layer before deleting it."
          >
            <button
              type="button"
              className="desktop-danger-button"
              disabled={!canDelete || busyAction === `delete-layer:${selectedLayer.layerId}`}
              onClick={() => onDeleteLayer(selectedLayer.layerId)}
            >
              Delete layer
            </button>
          </DangerZone>
        </div>
      </section>
    </div>
  );
}

export function SpaceWeavesView({
  busyAction,
  commandErrors,
  onBack,
  onAbortWeave,
  onApplyWeave,
  onContinueWeave,
  onPreviewWeave,
  onResolveWeaveConflict,
  onWeaveSourceLayerChange,
  onWeaveTargetLayerChange,
  selectedSpace,
  weaveSession,
  weaveSourceLayerId,
  weaveTargetLayerId
}: {
  busyAction: string | null;
  commandErrors: Partial<Record<CommandKey, string>>;
  onBack: () => void;
  selectedSpace: LocalSpaceSummary | null;
  weaveSession: WeaveSessionSummary | null;
  weaveSourceLayerId: string;
  weaveTargetLayerId: string;
  onWeaveSourceLayerChange: (value: string) => void;
  onWeaveTargetLayerChange: (value: string) => void;
  onPreviewWeave: () => void;
  onApplyWeave: () => void;
  onResolveWeaveConflict: (path: string, resolution: string, manualText?: string) => void;
  onContinueWeave: () => void;
  onAbortWeave: () => void;
}) {
  if (!selectedSpace) {
    return <p className="desktop-empty">Select a Local Space before opening Weaves.</p>;
  }
  return (
    <div className="desktop-view">
      <div className="desktop-page-return">
        <button type="button" className="desktop-ghost-button" onClick={onBack}>
          Back to Space
        </button>
        <span>{selectedSpace.name}</span>
      </div>
      <WeavesPanel
        busyAction={busyAction}
        commandErrors={commandErrors}
        selectedSpace={selectedSpace}
        session={weaveSession}
        sourceLayerId={weaveSourceLayerId}
        targetLayerId={weaveTargetLayerId}
        onSourceLayerChange={onWeaveSourceLayerChange}
        onTargetLayerChange={onWeaveTargetLayerChange}
        onPreview={onPreviewWeave}
        onApply={onApplyWeave}
        onResolveConflict={onResolveWeaveConflict}
        onContinue={onContinueWeave}
        onAbort={onAbortWeave}
      />
    </div>
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

