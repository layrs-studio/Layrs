import { useEffect, useMemo, useState } from "react";
import type {
  Account,
  Layer,
  LayerAccessPolicy,
  LayerAccessPrincipalSet,
  LayerAccessRule,
  LayerAccessRuleMode,
  LayerAccessVisibility,
  Team,
  WorkspaceMember
} from "@layrs/client-sdk";
import { EmptyState } from "./common";

const ruleModes: LayerAccessRuleMode[] = ["restricted", "reserved_redacted"];
const visibilityOptions: LayerAccessVisibility[] = ["full", "stub"];
const emptyPrincipals = (): LayerAccessPrincipalSet => ({ accounts: [], teams: [] });

export function AccessPolicyEditor({
  account,
  currentLayer,
  layers,
  policies,
  teams,
  workspaceMembers,
  onSave
}: {
  account: Account;
  currentLayer?: Layer;
  layers: Layer[];
  policies: LayerAccessPolicy[];
  teams: Team[];
  workspaceMembers: WorkspaceMember[];
  onSave: (policies: LayerAccessPolicy[]) => Promise<void>;
}) {
  const currentPolicy = currentLayer ? policies.find((policy) => policy.layerId === currentLayer.id) : undefined;
  const [rules, setRules] = useState<LayerAccessRule[]>(currentPolicy?.rules ?? []);
  const [selectedLayerIds, setSelectedLayerIds] = useState<string[]>(currentLayer ? [currentLayer.id] : []);
  const accountOptions = useMemo(() => accountOptionsFor(account, workspaceMembers), [account, workspaceMembers]);

  useEffect(() => {
    setRules(currentPolicy?.rules ?? []);
    setSelectedLayerIds(currentLayer ? [currentLayer.id] : []);
  }, [currentLayer?.id, currentPolicy]);

  if (!currentLayer) {
    return <EmptyState title="No Layer selected" detail="Select a Layer before editing access rules." />;
  }
  const activeLayer = currentLayer;

  function updateRule(ruleId: string, patch: Partial<LayerAccessRule>) {
    setRules((items) => items.map((item) => (item.id === ruleId ? { ...item, ...patch } : item)));
  }

  function updatePermission(ruleId: string, permission: "read" | "write" | "admin", kind: "accounts" | "teams", value: string) {
    setRules((items) =>
      items.map((item) =>
        item.id === ruleId
          ? {
              ...item,
              permissions: {
                ...item.permissions,
                [permission]: {
                  ...item.permissions[permission],
                  [kind]: value ? [value] : []
                }
              }
            }
          : item
      )
    );
  }

  function addRule() {
    setRules((items) => [
      ...items,
      {
        id: `access-rule-${Date.now()}`,
        path: "*",
        mode: "restricted",
        visibility: "full",
        permissions: {
          read: { accounts: [], teams: teams[0] ? [teams[0].id] : [] },
          write: emptyPrincipals(),
          admin: emptyPrincipals()
        }
      }
    ]);
  }

  async function save() {
    const targetIds = selectedLayerIds.length > 0 ? selectedLayerIds : [activeLayer.id];
    const nextPolicies = targetIds.map((layerId) => {
      const layer = layers.find((item) => item.id === layerId) ?? activeLayer;
      const existing = policies.find((policy) => policy.layerId === layerId);
      return {
        schema: existing?.schema ?? "layrs.layer_access.v1",
        workspaceId: existing?.workspaceId ?? currentPolicy?.workspaceId ?? "",
        spaceId: existing?.spaceId ?? layer.spaceId,
        layerId,
        policyEpoch: existing?.policyEpoch ?? 1,
        generatedAt: existing?.generatedAt ?? new Date().toISOString(),
        rules,
        signature: existing?.signature ?? { keyId: "studio-web-local", value: "unsigned" }
      };
    });

    await onSave(nextPolicies);
  }

  return (
    <div className="studio-access-editor">
      <fieldset className="studio-layer-multiselect">
        <legend>Apply to Layers</legend>
        {layers.map((layer) => (
          <label key={layer.id}>
            <input
              checked={selectedLayerIds.includes(layer.id)}
              onChange={(event) => {
                setSelectedLayerIds((items) =>
                  event.currentTarget.checked ? [...items, layer.id] : items.filter((item) => item !== layer.id)
                );
              }}
              type="checkbox"
            />
            <span>{layer.name}</span>
          </label>
        ))}
      </fieldset>

      <div className="studio-access-rule-list">
        {rules.length === 0 ? (
          <EmptyState title="No rules" detail="Add a path rule for the selected Layer." />
        ) : (
          rules.map((rule) => (
            <article className="studio-access-rule" key={rule.id}>
              <div className="studio-access-rule__top">
                <label className="studio-field">
                  <span>Path</span>
                  <input
                    onChange={(event) => updateRule(rule.id, { path: event.currentTarget.value })}
                    value={rule.path}
                  />
                </label>
                <label className="studio-field">
                  <span>Mode</span>
                  <select
                    onChange={(event) => updateRule(rule.id, { mode: event.currentTarget.value as LayerAccessRuleMode })}
                    value={rule.mode}
                  >
                    {ruleModes.map((mode) => (
                      <option key={mode} value={mode}>
                        {mode}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="studio-field">
                  <span>Visibility</span>
                  <select
                    onChange={(event) => updateRule(rule.id, { visibility: event.currentTarget.value as LayerAccessVisibility })}
                    value={rule.visibility}
                  >
                    {visibilityOptions.map((visibility) => (
                      <option key={visibility} value={visibility}>
                        {visibility}
                      </option>
                    ))}
                  </select>
                </label>
                <button type="button" onClick={() => setRules((items) => items.filter((item) => item.id !== rule.id))}>
                  Delete
                </button>
              </div>

              <div className="studio-access-principals">
                <PrincipalSelect
                  label="Read Team"
                  options={teams.map((team) => ({ id: team.id, name: team.name }))}
                  value={rule.permissions.read.teams[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "read", "teams", value)}
                />
                <PrincipalSelect
                  label="Write Team"
                  options={teams.map((team) => ({ id: team.id, name: team.name }))}
                  value={rule.permissions.write.teams[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "write", "teams", value)}
                />
                <PrincipalSelect
                  label="Admin Team"
                  options={teams.map((team) => ({ id: team.id, name: team.name }))}
                  value={rule.permissions.admin.teams[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "admin", "teams", value)}
                />
                <PrincipalSelect
                  label="Read Account"
                  options={accountOptions}
                  value={rule.permissions.read.accounts[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "read", "accounts", value)}
                />
                <PrincipalSelect
                  label="Write Account"
                  options={accountOptions}
                  value={rule.permissions.write.accounts[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "write", "accounts", value)}
                />
                <PrincipalSelect
                  label="Admin Account"
                  options={accountOptions}
                  value={rule.permissions.admin.accounts[0] ?? ""}
                  onChange={(value) => updatePermission(rule.id, "admin", "accounts", value)}
                />
              </div>
            </article>
          ))
        )}
      </div>

      <div className="studio-button-row">
        <button type="button" onClick={addRule}>
          Add path rule
        </button>
        <button className="studio-primary-button" type="button" onClick={() => void save()}>
          Save access rules
        </button>
      </div>
    </div>
  );
}

function PrincipalSelect({
  label,
  onChange,
  options,
  value
}: {
  label: string;
  onChange: (value: string) => void;
  options: Array<{ id: string; name: string }>;
  value: string;
}) {
  return (
    <label className="studio-field">
      <span>{label}</span>
      <select onChange={(event) => onChange(event.currentTarget.value)} value={value}>
        <option value="">None</option>
        {options.map((option) => (
          <option key={option.id} value={option.id}>
            {option.name}
          </option>
        ))}
      </select>
    </label>
  );
}

function accountOptionsFor(account: Account, workspaceMembers: WorkspaceMember[]) {
  const accounts = new Map<string, { id: string; name: string }>();
  accounts.set(account.id, { id: account.id, name: account.name });

  for (const member of workspaceMembers) {
    accounts.set(member.accountId, { id: member.accountId, name: member.name || member.email });
  }

  return [...accounts.values()];
}
