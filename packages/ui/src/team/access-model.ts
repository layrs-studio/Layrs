import type { MemberRole, PolicyRulesRead, Team } from "./types";

/*
 * Explanation of the server's access rules (layrs-sync native_policy). This
 * mirrors the evaluation to *explain* a right; it never grants one. Every
 * change is re-evaluated and enforced by the server.
 *
 * - Actions (v1 rules, also carried by v2/v3): a deny anywhere wins; else
 *   allowed when some policy allows it; else not granted.
 * - Content (v2 contentPolicies, v3 pathPolicies): every policy naming the
 *   person must agree. Inside one policy, a matching deny wins, a matching
 *   allow grants, otherwise the policy default applies.
 * - Writing a path also requires reading it (v3) and no v2 restriction.
 */

export type Effect = "allow" | "deny";
export type Selector = { kind: "exact" | "subtree"; path: string };
export type PathRule = { effect: Effect; selector: Selector };
export type Permission = { default: Effect; rules: PathRule[] };
export type ActionRule = { effect: Effect; principalIds: string[]; actions: string[] };

type Parsed =
  | { version: 1; actions: ActionRule[] }
  | { version: 2; actions: ActionRule[]; content: { principalIds: string[]; default: Effect; rules: PathRule[] }[] }
  | { version: 3; actions: ActionRule[]; paths: { principalIds: string[]; read: Permission; write: Permission }[] };

export type ReadPolicy = { policyId: string; policyProductObjectId: string; label: string; kind: "team" | "custom" | "individual"; parsed: Parsed | null };

const object = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);
const effect = (value: unknown): value is Effect => value === "allow" || value === "deny";
const strings = (value: unknown): value is string[] => Array.isArray(value) && value.every((item) => typeof item === "string");

function pathRules(value: unknown): PathRule[] | null {
  if (!Array.isArray(value)) return null;
  const rules: PathRule[] = [];
  for (const rule of value) {
    if (!object(rule) || !effect(rule.effect) || !object(rule.selector)) return null;
    const { kind, path } = rule.selector;
    if ((kind !== "exact" && kind !== "subtree") || typeof path !== "string") return null;
    rules.push({ effect: rule.effect, selector: { kind, path } });
  }
  return rules;
}

function actionRules(value: unknown): ActionRule[] | null {
  if (!Array.isArray(value)) return null;
  const rules: ActionRule[] = [];
  for (const rule of value) {
    if (!object(rule) || !effect(rule.effect) || !strings(rule.principalIds) || !strings(rule.actions)) return null;
    rules.push({ effect: rule.effect, principalIds: rule.principalIds, actions: rule.actions });
  }
  return rules;
}

function permission(value: unknown): Permission | null {
  if (!object(value) || !effect(value.default)) return null;
  const rules = pathRules(value.rules);
  return rules ? { default: value.default, rules } : null;
}

/** Parses a closed rules document; unknown shapes return null (explained as "non lisible"). */
export function parseRules(value: unknown): Parsed | null {
  if (!object(value)) return null;
  const actions = actionRules(value.rules);
  if (!actions) return null;
  if (value.schemaVersion === "layrs.space-policy-rules.v1") return { version: 1, actions };
  if (value.schemaVersion === "layrs.space-policy-rules.v2" && Array.isArray(value.contentPolicies)) {
    const content = [];
    for (const item of value.contentPolicies) {
      const rules = object(item) ? pathRules(item.rules) : null;
      if (!object(item) || !strings(item.principalIds) || !effect(item.default) || !rules) return null;
      content.push({ principalIds: item.principalIds, default: item.default, rules });
    }
    return { version: 2, actions, content };
  }
  if (value.schemaVersion === "layrs.space-policy-rules.v3" && Array.isArray(value.pathPolicies)) {
    const paths = [];
    for (const item of value.pathPolicies) {
      const read = object(item) ? permission(item.read) : null;
      const write = object(item) ? permission(item.write) : null;
      if (!object(item) || !strings(item.principalIds) || !read || !write) return null;
      paths.push({ principalIds: item.principalIds, read, write });
    }
    return { version: 3, actions, paths };
  }
  return null;
}

/** Names each policy after the team that owns it, when there is one. */
export function readPolicies(read: PolicyRulesRead, teams: Team[]): ReadPolicy[] {
  return read.policies.map((policy) => {
    const team = teams.find((item) => item.policyProductObjectId === policy.policyProductObjectId);
    const parsed = parseRules(policy.rules);
    const individual = parsed?.version === 1 && parsed.actions.length === 1 && parsed.actions[0].principalIds.length === 1;
    return {
      policyId: policy.policyId,
      policyProductObjectId: policy.policyProductObjectId,
      label: team ? `Équipe ${team.name}` : individual ? "Accès individuel" : parsed?.version === 1 ? "Règles du projet" : "Règles de fichiers",
      kind: team ? "team" : individual ? "individual" : "custom",
      parsed,
    };
  });
}

export function selectorMatches(selector: Selector, path: string) {
  return path === selector.path || (selector.kind === "subtree" && path.startsWith(`${selector.path}/`));
}

export function actionEffect(policies: ReadPolicy[], principal: string, action: string): Effect | null {
  let allowed = false;
  for (const policy of policies) {
    for (const rule of policy.parsed?.actions ?? []) {
      if (!rule.principalIds.includes(principal) || !rule.actions.includes(action)) continue;
      if (rule.effect === "deny") return "deny";
      allowed = true;
    }
  }
  return allowed ? "allow" : null;
}

const memberActions: Record<Exclude<MemberRole, "revoked">, string[]> = {
  member: ["space.bootstrap", "object.plan_upload", "object.complete_upload", "object.plan_fetch", "object.fetch", "event.read", "transaction.apply", "review.read", "review.write", "review.approve", "release.read"],
  auditor: ["release.read", "space.bootstrap", "object.plan_fetch", "object.fetch", "event.read", "review.read"],
  automation: ["release.read", "space.bootstrap", "object.plan_fetch", "object.fetch", "event.read", "proof.run"],
  release_manager: ["space.bootstrap", "object.plan_fetch", "object.fetch", "event.read", "release.read", "release.publish", "release.revoke"],
};

export const capabilityLabels: [string, string][] = [
  ["space.bootstrap", "Ouvrir le projet"],
  ["transaction.apply", "Publier"],
  ["review.write", "Proposer et commenter"],
  ["review.approve", "Approuver"],
  ["release.publish", "Publier des versions"],
  ["proof.run", "Déposer des preuves CI"],
  ["policy.administer", "Administrer les accès"],
];

export type RoleSummary = {
  /** Closest named role, or null when the grants match no role. */
  role: MemberRole | null;
  label: string;
  capabilities: string[];
  denied: boolean;
  /** Policies that name this person, readable. */
  sources: string[];
};

export function roleSummary(policies: ReadPolicy[], principal: string): RoleSummary {
  const sources = policies
    .filter((policy) => {
      const parsed = policy.parsed;
      if (!parsed) return false;
      if (parsed.actions.some((rule) => rule.principalIds.includes(principal))) return true;
      if (parsed.version === 2) return parsed.content.some((item) => item.principalIds.includes(principal));
      if (parsed.version === 3) return parsed.paths.some((item) => item.principalIds.includes(principal));
      return false;
    })
    .map((policy) => policy.label);
  if (actionEffect(policies, principal, "space.bootstrap") === "deny")
    return { role: "revoked", label: "Accès retiré", capabilities: [], denied: true, sources };
  const granted = new Set(
    Object.values(memberActions)
      .flat()
      .concat("policy.administer", "audit.read_evidence")
      .filter((action) => actionEffect(policies, principal, action) === "allow"),
  );
  const capabilities = capabilityLabels.filter(([action]) => granted.has(action)).map(([, label]) => label);
  if (!granted.has("space.bootstrap")) return { role: null, label: "Aucun accès au projet", capabilities: [], denied: false, sources };
  if (granted.has("policy.administer")) return { role: null, label: "Administration du projet", capabilities, denied: false, sources };
  const order: Exclude<MemberRole, "revoked">[] = ["member", "release_manager", "automation", "auditor"];
  const role = order.find((name) => memberActions[name].every((action) => granted.has(action))) ?? null;
  const labels: Record<string, string> = { member: "Membre", release_manager: "Gestion des versions", automation: "Automatisation", auditor: "Lecture seule" };
  return { role, label: role ? labels[role] : "Droits particuliers", capabilities, denied: false, sources };
}

/** Why a right is what it is, for one person, one path and one mode. */
export type AccessReason =
  | { kind: "no-project-access" }
  | { kind: "unrestricted" }
  | { kind: "deny"; selector: Selector; policy: string }
  | { kind: "explicit"; selector: Selector; policy: string }
  | { kind: "inherited"; selector: Selector; policy: string }
  | { kind: "default"; effect: Effect; policy: string }
  | { kind: "read-first" }
  | { kind: "v2-read-only"; policy: string };

export type PathAccess = { read: boolean; write: boolean; readReason: AccessReason; writeReason: AccessReason };

function decide(permission: { default: Effect; rules: PathRule[] }, path: string, policy: string): { allowed: boolean; reason: AccessReason } {
  let grant: PathRule | null = null;
  for (const rule of permission.rules) {
    if (!selectorMatches(rule.selector, path)) continue;
    if (rule.effect === "deny") return { allowed: false, reason: { kind: "deny", selector: rule.selector, policy } };
    // Prefer the most specific allow for the explanation; the result is the same.
    if (!grant || rule.selector.path.length > grant.selector.path.length || rule.selector.kind === "exact") grant = rule;
  }
  if (grant) return { allowed: true, reason: { kind: grant.selector.path === path ? "explicit" : "inherited", selector: grant.selector, policy } };
  return { allowed: permission.default === "allow", reason: { kind: "default", effect: permission.default, policy } };
}

/** Combines per-policy decisions: the first refusal explains a denial. */
function combine(decisions: { allowed: boolean; reason: AccessReason }[]) {
  const refusal = decisions.find((item) => !item.allowed);
  if (refusal) return refusal;
  const explained = decisions.find((item) => item.reason.kind === "explicit") ?? decisions.find((item) => item.reason.kind === "inherited") ?? decisions[0];
  return explained ?? { allowed: true, reason: { kind: "unrestricted" } as AccessReason };
}

export function pathAccess(policies: ReadPolicy[], principal: string, path: string): PathAccess {
  const canOpen = ["space.bootstrap", "object.fetch"].every((action) => actionEffect(policies, principal, action) === "allow");
  const canPublish = actionEffect(policies, principal, "transaction.apply") === "allow";
  if (!canOpen) return { read: false, write: false, readReason: { kind: "no-project-access" }, writeReason: { kind: "no-project-access" } };
  const reads: { allowed: boolean; reason: AccessReason }[] = [];
  const writes: { allowed: boolean; reason: AccessReason }[] = [];
  for (const policy of policies) {
    const parsed = policy.parsed;
    if (parsed?.version === 2)
      for (const item of parsed.content.filter((entry) => entry.principalIds.includes(principal))) {
        reads.push(decide(item, path, policy.label));
        writes.push({ allowed: false, reason: { kind: "v2-read-only", policy: policy.label } });
      }
    if (parsed?.version === 3)
      for (const item of parsed.paths.filter((entry) => entry.principalIds.includes(principal))) {
        reads.push(decide(item.read, path, policy.label));
        writes.push(decide(item.write, path, policy.label));
      }
  }
  const read = combine(reads);
  let write = combine(writes);
  if (!read.allowed) write = { allowed: false, reason: { kind: "read-first" } };
  if (!canPublish) write = { allowed: false, reason: { kind: "no-project-access" } };
  return { read: read.allowed, write: write.allowed, readReason: read.reason, writeReason: write.reason };
}

export function describeReason(reason: AccessReason, mode: "read" | "write") {
  const where = (selector: Selector) => (selector.kind === "subtree" ? `le dossier « ${selector.path} » et son contenu` : `« ${selector.path} »`);
  switch (reason.kind) {
    case "no-project-access":
      return mode === "read" ? "Son rôle ne permet pas d’ouvrir le contenu du projet." : "Son rôle ne permet pas de publier dans ce projet.";
    case "unrestricted":
      return "Aucune règle de fichier : le rôle dans le projet s’applique.";
    case "deny":
      return `Refus explicite sur ${where(reason.selector)} (${reason.policy}) ; un refus est toujours prioritaire.`;
    case "explicit":
      return `Autorisé explicitement sur ce chemin (${reason.policy}).`;
    case "inherited":
      return `Hérité de la règle sur ${where(reason.selector)} (${reason.policy}).`;
    case "default":
      return `Règle par défaut de « ${reason.policy} » : ${reason.effect === "allow" ? "autorisé" : "interdit"} sauf exception.`;
    case "read-first":
      return "L’écriture exige d’abord la lecture, qui est interdite.";
    case "v2-read-only":
      return `Les règles de contenu anciennes (${reason.policy}) n’autorisent que la lecture.`;
  }
}

export type AccessLevel = "none" | "read" | "write";

export function accessLevel(access: Pick<PathAccess, "read" | "write">): AccessLevel {
  return access.write ? "write" : access.read ? "read" : "none";
}

export const accessLabels: Record<AccessLevel, string> = {
  none: "Interdit",
  read: "Lecture seule",
  write: "Lecture et écriture",
};

/*
 * Change of one person's right on one selector, in a v3 path policy. Every
 * other subject, default and rule of that policy is kept; the person is split
 * out of shared entries so nobody else is affected.
 */
export type PathChange = {
  principalId: string;
  path: string;
  kind: "exact" | "subtree";
  access: AccessLevel;
  /** Only for a new policy: what the other paths keep. */
  others: Effect;
};

const REPO_PATH = (value: string) =>
  !!value && new TextEncoder().encode(value).length <= 4096 && !/[\\\x00-\x1f\x7f]/.test(value) && value.split("/").every((part) => part !== "" && part !== "." && part !== "..");

/** Policies the form may rewrite: v3 documents only, older ones need the JSON editor. */
export function editablePathPolicy(policies: ReadPolicy[], principal: string) {
  return policies.find((policy) => policy.parsed?.version === 3 && policy.parsed.paths.some((entry) => entry.principalIds.includes(principal))) ?? null;
}

export function applyPathChange(original: unknown | null, change: PathChange) {
  if (!change.principalId.startsWith("principal:") || !REPO_PATH(change.path)) throw new Error("Choisissez une personne et un chemin valides.");
  const document =
    original === null
      ? { schemaVersion: "layrs.space-policy-rules.v3", rules: [] as unknown[], pathPolicies: [] as { principalIds: string[]; read: Permission; write: Permission }[] }
      : (JSON.parse(JSON.stringify(original)) as { schemaVersion: string; rules: unknown[]; pathPolicies: { principalIds: string[]; read: Permission; write: Permission }[] });
  if (document.schemaVersion !== "layrs.space-policy-rules.v3" || !Array.isArray(document.pathPolicies)) throw new Error("Cette politique demande l’éditeur de règles JSON.");
  const selector: Selector = { kind: change.kind, path: change.path };
  const read: PathRule = { effect: change.access === "none" ? "deny" : "allow", selector };
  const write: PathRule = { effect: change.access === "write" ? "allow" : "deny", selector };
  const update = (current: Permission, rule: PathRule): Permission => ({
    default: current.default,
    rules: [...current.rules.filter((item) => item.selector.kind !== selector.kind || item.selector.path !== selector.path), rule],
  });
  let matched = false;
  document.pathPolicies = document.pathPolicies.flatMap((entry) => {
    if (!entry.principalIds.includes(change.principalId)) return [entry];
    matched = true;
    const others = entry.principalIds.filter((id) => id !== change.principalId);
    return [...(others.length ? [{ ...entry, principalIds: others }] : []), { principalIds: [change.principalId], read: update(entry.read, read), write: update(entry.write, write) }];
  });
  if (!matched)
    document.pathPolicies.push({
      principalIds: [change.principalId],
      read: { default: change.others, rules: [read] },
      write: { default: change.others, rules: [write] },
    });
  if (document.pathPolicies.length > 64) throw new Error("Cette politique compte déjà 64 groupes de personnes.");
  if (new TextEncoder().encode(JSON.stringify(document)).length > 64 * 1024) throw new Error("Les règles dépasseraient 64 Kio.");
  return document;
}

/** Evaluates a person's rights after replacing (or adding) one policy's rules. */
export function withReplacedRules(policies: ReadPolicy[], policyId: string | null, rules: unknown): ReadPolicy[] {
  const parsed = parseRules(rules);
  if (policyId) return policies.map((policy) => (policy.policyId === policyId ? { ...policy, parsed } : policy));
  return [...policies, { policyId: "policy:preview", policyProductObjectId: "", label: "Nouvelle règle de fichiers", kind: "custom", parsed }];
}
