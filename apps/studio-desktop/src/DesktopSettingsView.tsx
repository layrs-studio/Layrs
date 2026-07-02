import { StatusPill } from "@layrs/ui";
import type { BootstrapData, DesktopShortcutSettings, DesktopStatus, DeviceLoginStartResponse } from "./tauri";
import { compactPath, displayPath, normalizeShortcut, shortcutFromKeyboardEvent } from "./desktopModel";
import { defaultShortcuts, statusLabels, type CommandKey, type LoadState } from "./desktopTypes";

interface SettingsViewProps {
  status: DesktopStatus | null;
  bootstrap: BootstrapData | null;
  endpointDraft: string;
  defaultLocalRoot: string;
  login: DeviceLoginStartResponse | null;
  pollStatus: string | null;
  pollInFlight: boolean;
  autoReceive: boolean;
  autoPublish: boolean;
  autoLocalSteps: boolean;
  syncIntervalMinutes: number;
  shortcuts: DesktopShortcutSettings;
  loadState: LoadState;
  saving: boolean;
  onEndpointChange: (value: string) => void;
  onChooseDefaultRoot: () => void;
  onSaveSettings: () => void;
  onBeginLogin: () => void;
  onPollNow: () => void;
  onAutoReceiveChange: (value: boolean) => void;
  onAutoPublishChange: (value: boolean) => void;
  onAutoLocalStepsChange: (value: boolean) => void;
  onSyncIntervalChange: (value: number) => void;
  onShortcutsChange: (value: DesktopShortcutSettings) => void;
}

export function SettingsView({
  status,
  bootstrap,
  endpointDraft,
  defaultLocalRoot,
  login,
  pollStatus,
  pollInFlight,
  autoReceive,
  autoPublish,
  autoLocalSteps,
  syncIntervalMinutes,
  shortcuts,
  loadState,
  saving,
  onEndpointChange,
  onChooseDefaultRoot,
  onSaveSettings,
  onBeginLogin,
  onPollNow,
  onAutoReceiveChange,
  onAutoPublishChange,
  onAutoLocalStepsChange,
  onSyncIntervalChange,
  onShortcutsChange
}: SettingsViewProps) {
  return (
    <div className="desktop-view" id="desktop-settings">
      <section className="desktop-panel desktop-panel--wide">
        <div className="desktop-heading-line">
          <div className="layrs-section-heading">
            <span>Settings</span>
            <h1>Desktop sync and device</h1>
          </div>
          <StatusPill status={status?.secretStore.available ? "passing" : "blocked"} label={status?.secretStore.available ? "Secret store ready" : "Secret store required"} />
        </div>

        <div className="desktop-settings-grid desktop-settings-grid--cards">
          <section className="desktop-setting-card">
            <span>Account</span>
            <strong>{bootstrap?.account?.displayName ?? (status?.connected ? "Connected device" : "Not connected")}</strong>
            <em>{bootstrap?.account?.email ?? (status?.connected ? "Refresh Distant to load account details" : "Device login required")}</em>
            <button type="button" className="desktop-primary-button" onClick={onBeginLogin} disabled={!status?.secretStore.available || pollInFlight}>
              {status?.connected ? "Reconnect device" : "Connect device"}
            </button>
            {login ? (
              <div className="desktop-code">
                <div>
                  <span>User code</span>
                  <strong aria-label="Device user code">{login.userCode}</strong>
                </div>
                <a href={login.verificationUriComplete ?? login.verificationUri} target="_blank" rel="noreferrer">
                  Open verification page
                </a>
                <button type="button" onClick={onPollNow} disabled={pollInFlight}>
                  {pollInFlight ? "Checking..." : "Check now"}
                </button>
                <p>{statusLabels[pollStatus ?? "pending"] ?? pollStatus}</p>
              </div>
            ) : null}
          </section>

          <section className="desktop-setting-card">
            <span>Server</span>
            <label className="desktop-field">
              <span>Endpoint</span>
              <input value={endpointDraft} onChange={(event) => onEndpointChange(event.currentTarget.value)} />
            </label>
            <button type="button" className="desktop-primary-button desktop-save-button" onClick={onSaveSettings} disabled={loadState !== "ready" || saving}>
              Save settings
            </button>
          </section>

          <section className="desktop-setting-card">
            <span>Sync</span>
            <ToggleRow label="Auto receive" checked={autoReceive} onChange={onAutoReceiveChange} />
            <ToggleRow label="Auto publish" checked={autoPublish} onChange={onAutoPublishChange} />
            <ToggleRow label="Auto local steps" checked={autoLocalSteps} onChange={onAutoLocalStepsChange} />
            <label className="desktop-field">
              <span>Sync interval</span>
              <input
                type="number"
                min={1}
                max={1440}
                value={syncIntervalMinutes}
                onChange={(event) => onSyncIntervalChange(Number(event.currentTarget.value))}
              />
            </label>
          </section>

          <section className="desktop-setting-card">
            <span>Shortcuts</span>
            <ToggleRow
              label="Enable keyboard shortcuts"
              checked={shortcuts.enabled}
              onChange={(enabled) => onShortcutsChange({ ...shortcuts, enabled })}
            />
            <ShortcutCaptureField
              label="Save Step"
              value={shortcuts.saveStep}
              onChange={(saveStep) => onShortcutsChange({ ...shortcuts, saveStep })}
            />
            <ShortcutCaptureField
              label="Publish"
              value={shortcuts.publish}
              onChange={(publish) => onShortcutsChange({ ...shortcuts, publish })}
            />
            <ToggleRow
              label="Use Save Step again to publish pending step"
              checked={shortcuts.smartSavePublishesPendingStep}
              onChange={(smartSavePublishesPendingStep) => onShortcutsChange({ ...shortcuts, smartSavePublishesPendingStep })}
            />
            <button type="button" className="desktop-secondary-button" onClick={() => onShortcutsChange(defaultShortcuts)}>
              Reset defaults
            </button>
          </section>

          <section className="desktop-setting-card">
            <span>Storage</span>
            <FolderField
              label="Default Local Spaces folder"
              value={defaultLocalRoot}
              placeholder="Choose the default folder for new Local Spaces"
              onChoose={onChooseDefaultRoot}
              wide
            />
            <div className="desktop-device-grid">
              <div>
                <span>Device id</span>
                <strong>{status?.deviceId ?? "Not initialized"}</strong>
                <em>{status?.secretStore.provider ?? "Unknown provider"}</em>
              </div>
              <p>{status?.secretStore.message ?? "Desktop status is not loaded yet."}</p>
            </div>
          </section>
        </div>
      </section>
    </div>
  );
}

function ToggleRow({ label, checked, onChange }: { label: string; checked: boolean; onChange: (value: boolean) => void }) {
  return (
    <label className="desktop-toggle">
      <span>{label}</span>
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.currentTarget.checked)} />
    </label>
  );
}

function ShortcutCaptureField({
  label,
  onChange,
  value
}: {
  label: string;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label className="desktop-field">
      <span>{label}</span>
      <input
        value={value}
        onChange={(event) => onChange(normalizeShortcut(event.currentTarget.value))}
        onKeyDown={(event) => {
          if (event.key === "Tab") {
            return;
          }
          event.preventDefault();
          const shortcut = shortcutFromKeyboardEvent(event);
          if (shortcut) {
            onChange(shortcut);
          }
        }}
        placeholder="Press shortcut"
      />
    </label>
  );
}

export function ShortcutFooter({ hasLocalSpace, shortcuts }: { hasLocalSpace: boolean; shortcuts: DesktopShortcutSettings }) {
  if (!shortcuts.enabled) {
    return <span className="desktop-shortcut-footer is-muted">Keyboard shortcuts disabled</span>;
  }

  return (
    <div className={hasLocalSpace ? "desktop-shortcut-footer" : "desktop-shortcut-footer is-muted"}>
      <span>
        <kbd>{shortcuts.saveStep}</kbd> Step
      </span>
      {shortcuts.smartSavePublishesPendingStep ? (
        <span>
          <kbd>{shortcuts.saveStep}</kbd> again Publish
        </span>
      ) : null}
      <span>
        <kbd>{shortcuts.publish}</kbd> Publish
      </span>
    </div>
  );
}

export function CommandErrors({ errors }: { errors: Partial<Record<CommandKey, string>> }) {
  const entries = Object.entries(errors).filter(([, value]) => value);
  if (entries.length === 0) {
    return null;
  }

  return (
    <div className="desktop-command-errors">
      {entries.map(([key, value]) => (
        <p className="desktop-alert desktop-alert--error" key={key}>
          {key}: {value}
        </p>
      ))}
    </div>
  );
}

export function FolderField({
  label,
  value,
  placeholder,
  wide = false,
  onChoose
}: {
  label: string;
  value: string;
  placeholder: string;
  wide?: boolean;
  onChoose: () => void;
}) {
  return (
    <div className={wide ? "desktop-field desktop-field--wide" : "desktop-field"}>
      <span>{label}</span>
      <FolderChoice value={value} placeholder={placeholder} onChoose={onChoose} />
    </div>
  );
}

export function FolderChoice({ value, placeholder, onChoose }: { value: string; placeholder: string; onChoose: () => void }) {
  const normalized = value ? displayPath(value) : "";
  return (
    <div className="desktop-folder-choice">
      <span className={normalized ? "desktop-folder-choice__path" : "desktop-folder-choice__path is-empty"} title={normalized || placeholder}>
        {normalized ? compactPath(normalized, 58) : placeholder}
      </span>
      <button type="button" className="desktop-secondary-button" onClick={onChoose}>
        Choose folder
      </button>
    </div>
  );
}

export function PathText({ value }: { value: string }) {
  const normalized = displayPath(value);
  return (
    <span className="desktop-path-text" title={normalized}>
      {compactPath(normalized, 48)}
    </span>
  );
}

