import { layers, teams, workspaceMembers } from "./fixture-data";
import type {
  AccessMode,
  AccessSubjectKind,
  LayerAccessPolicy,
  LayerAccessPrincipalSet,
  LayerAccessRegistry,
  LayerAccessRegistryRule,
  LayerAccessRule,
  LayerAccessRuleMode,
  LayerAccessSignature,
  LayerAccessVisibility,
  LayrsId
} from "./types";

interface LayerAccessPrincipalSetWire {
  accounts?: LayrsId[];
  teams?: LayrsId[];
}

interface LayerAccessRulePermissionsWire {
  read?: LayerAccessPrincipalSetWire;
  write?: LayerAccessPrincipalSetWire;
  admin?: LayerAccessPrincipalSetWire;
}

interface LayerAccessSignatureWire {
  key_id?: string;
  keyId?: string;
  value?: string;
}

interface LayerAccessRuleWire {
  id?: LayrsId;
  path?: string;
  artifact_id?: LayrsId;
  artifactId?: LayrsId;
  mode?: LayerAccessRuleMode;
  visibility?: LayerAccessVisibility;
  permissions?: LayerAccessRulePermissionsWire;
}

export interface LayerAccessPolicyWire {
  schema?: string;
  workspace_id?: LayrsId;
  workspaceId?: LayrsId;
  space_id?: LayrsId;
  spaceId?: LayrsId;
  layer_id?: LayrsId;
  layerId?: LayrsId;
  policy_epoch?: number;
  policyEpoch?: number;
  generated_at?: string;
  generatedAt?: string;
  rules?: LayerAccessRuleWire[];
  signature?: LayerAccessSignatureWire;
}

export function layerAccessPolicyPath(workspaceId: LayrsId, spaceId: LayrsId, layerId: LayrsId): string {
  return `/v1/workspaces/${encodeURIComponent(workspaceId)}/spaces/${encodeURIComponent(spaceId)}/layers/${encodeURIComponent(layerId)}/access`;
}

export function layerAccessPolicyFromWire(policy: LayerAccessPolicy | LayerAccessPolicyWire): LayerAccessPolicy {
  const wire = policy as LayerAccessPolicyWire;
  return {
    schema: wire.schema ?? "layrs.layer_access.v1",
    workspaceId: wire.workspaceId ?? wire.workspace_id ?? "",
    spaceId: wire.spaceId ?? wire.space_id ?? "",
    layerId: wire.layerId ?? wire.layer_id ?? "",
    policyEpoch: wire.policyEpoch ?? wire.policy_epoch ?? 0,
    generatedAt: wire.generatedAt ?? wire.generated_at ?? new Date(0).toISOString(),
    rules: (wire.rules ?? []).map(layerAccessRuleFromWire),
    signature: layerAccessSignatureFromWire(wire.signature)
  };
}

export function layerAccessPolicyToWire(policy: LayerAccessPolicy): LayerAccessPolicyWire {
  return {
    schema: policy.schema,
    workspace_id: policy.workspaceId,
    space_id: policy.spaceId,
    layer_id: policy.layerId,
    policy_epoch: policy.policyEpoch,
    generated_at: policy.generatedAt,
    rules: policy.rules.map((rule) => ({
      id: rule.id,
      path: rule.path,
      artifact_id: rule.artifactId,
      mode: rule.mode,
      visibility: rule.visibility,
      permissions: {
        read: principalSetToWire(rule.permissions.read),
        write: principalSetToWire(rule.permissions.write),
        admin: principalSetToWire(rule.permissions.admin)
      }
    })),
    signature: {
      key_id: policy.signature.keyId,
      value: policy.signature.value
    }
  };
}

function layerAccessRuleFromWire(rule: LayerAccessRule | LayerAccessRuleWire): LayerAccessRule {
  const wire = rule as LayerAccessRuleWire;
  return {
    id: wire.id ?? `rule-${cryptoSafeRandomId()}`,
    path: wire.path ?? ".",
    artifactId: wire.artifactId ?? wire.artifact_id,
    mode: wire.mode ?? "restricted",
    visibility: wire.visibility ?? "stub",
    permissions: {
      read: principalSetFromWire(wire.permissions?.read),
      write: principalSetFromWire(wire.permissions?.write),
      admin: principalSetFromWire(wire.permissions?.admin)
    }
  };
}

function layerAccessSignatureFromWire(signature: LayerAccessSignature | LayerAccessSignatureWire | undefined): LayerAccessSignature {
  const wire = signature as LayerAccessSignatureWire | undefined;
  return {
    keyId: wire?.keyId ?? wire?.key_id ?? "server_key_local",
    value: signature?.value ?? "unsigned-dev"
  };
}

function principalSetFromWire(set: LayerAccessPrincipalSetWire | undefined): LayerAccessPrincipalSet {
  return {
    accounts: set?.accounts ?? [],
    teams: set?.teams ?? []
  };
}

function principalSetToWire(set: LayerAccessPrincipalSet): LayerAccessPrincipalSetWire {
  return {
    accounts: set.accounts,
    teams: set.teams
  };
}

export function legacyRegistryToLayerAccessPolicy(registry: LayerAccessRegistry): LayerAccessPolicy {
  const layer = layers.find((item) => item.id === registry.layerId);
  return {
    schema: "layrs.layer_access.v1",
    workspaceId: registry.workspaceId,
    spaceId: layer?.spaceId ?? "",
    layerId: registry.layerId,
    policyEpoch: 1,
    generatedAt: registry.updatedAt,
    rules: registry.rules.map((rule) => legacyRegistryRuleToLayerAccessRule(registry.layerId, rule)),
    signature: {
      keyId: "legacy_registry_adapter",
      value: registry.updatedAt
    }
  };
}

function legacyRegistryRuleToLayerAccessRule(layerId: LayrsId, rule: LayerAccessRegistryRule): LayerAccessRule {
  return {
    id: rule.id,
    path: `.layrs/layers/${layerId}/access.json`,
    mode: rule.mode === "none" ? "reserved_redacted" : "restricted",
    visibility: rule.mode === "none" ? "stub" : "full",
    permissions: {
      read: legacyPrincipalSet(rule, "read"),
      write: legacyPrincipalSet(rule, "write"),
      admin: legacyPrincipalSet(rule, "admin")
    }
  };
}

function legacyPrincipalSet(rule: LayerAccessRegistryRule, mode: AccessMode): LayerAccessPrincipalSet {
  if (rule.mode !== mode) {
    return { accounts: [], teams: [] };
  }
  return rule.subjectKind === "account"
    ? { accounts: [rule.subjectId], teams: [] }
    : { accounts: [], teams: [rule.subjectId] };
}

export function layerAccessPolicyToLegacyRegistry(policy: LayerAccessPolicy): LayerAccessRegistry {
  const rules: LayerAccessRegistryRule[] = [];
  addLegacyRules(rules, policy, "read", policy.rules.flatMap((rule) => rule.permissions.read.accounts), "account");
  addLegacyRules(rules, policy, "read", policy.rules.flatMap((rule) => rule.permissions.read.teams), "team");
  addLegacyRules(rules, policy, "write", policy.rules.flatMap((rule) => rule.permissions.write.accounts), "account");
  addLegacyRules(rules, policy, "write", policy.rules.flatMap((rule) => rule.permissions.write.teams), "team");
  addLegacyRules(rules, policy, "admin", policy.rules.flatMap((rule) => rule.permissions.admin.accounts), "account");
  addLegacyRules(rules, policy, "admin", policy.rules.flatMap((rule) => rule.permissions.admin.teams), "team");

  return {
    id: `access-${policy.layerId}`,
    workspaceId: policy.workspaceId,
    layerId: policy.layerId,
    rules,
    updatedAt: policy.generatedAt
  };
}

function addLegacyRules(
  rules: LayerAccessRegistryRule[],
  policy: LayerAccessPolicy,
  mode: AccessMode,
  subjectIds: LayrsId[],
  subjectKind: AccessSubjectKind
): void {
  for (const subjectId of unique(subjectIds)) {
    rules.push({
      id: `access-${policy.layerId}-${mode}-${subjectKind}-${subjectId}`,
      subjectKind,
      subjectId,
      subjectName: subjectDisplayName(subjectKind, subjectId),
      mode
    });
  }
}

function subjectDisplayName(subjectKind: AccessSubjectKind, subjectId: LayrsId): string {
  if (subjectKind === "team") {
    return teams.find((team) => team.id === subjectId)?.name ?? subjectId;
  }
  return workspaceMembers.find((member) => member.accountId === subjectId)?.name ?? subjectId;
}

function unique<T>(items: T[]): T[] {
  return [...new Set(items)];
}

function cryptoSafeRandomId(): string {
  const value = globalThis.crypto?.randomUUID?.();
  return value ?? Math.random().toString(36).slice(2);
}

