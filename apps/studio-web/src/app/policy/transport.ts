import type { Api } from "../session";
import { objectId } from "./blake3";
import { canonicalBytes, canonicalJson } from "./jcs";

/*
 * Web implementation of the policy commands Desktop runs natively
 * (readPolicyRules, previewPolicyRules, previewReplacePolicyRules,
 * previewPolicy, applyPolicy). Previews are pure reads kept in memory; only
 * `applyPolicy` uploads the rules document and posts one PolicyRoot
 * transaction. The server recomputes every identity and re-authorizes the
 * change; this client never decides a right.
 */

type Json = Record<string, unknown>;
type PolicyVersionEntry = { policyId: string; policyVersionId: string; policyProductObjectId: string; dependencyObjectIds: string[] };
type PolicySet = { schemaVersion: "layrs.policy-set.v1"; policySetId: string; spaceId: string; policyVersions: PolicyVersionEntry[] };
type Bootstrap = {
  principalId: string;
  spaceId: string;
  authorizationEpoch: number;
  currentPolicySetId: string;
  currentPolicySet: { policySetId: string; record: PolicySet };
};
type Descriptor = { kind: string; objectId: string; canonicalByteLen: number; encodedByteLen: number; compression: string };

export type WebPolicyRulesRead = {
  principalId: string;
  spaceId: string;
  policySetId: string;
  authorizationEpoch: number;
  policies: { policyId: string; policyVersionId: string; policyProductObjectId: string; rules: unknown }[];
};

export type WebPolicyPreview = {
  schemaVersion: "layrs.web-policy-preview.v1";
  principalId: string;
  spaceId: string;
  expectedPolicySetId: string;
  expectedAuthorizationEpoch: number;
  proposedPolicySetId: string;
  request: Json;
  inlineRules: { objectId: string; rules: unknown; replacement?: { policyId: string; previousProductObjectId: string } } | null;
};

const MAX_RULES = 64 * 1024;
const MAX_OBJECT = 256 * 1024;
const HASH = /^blake3:[0-9a-f]{64}$/;
const OPAQUE = (prefix: string) => new RegExp(`^${prefix}[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$`);
const POLICY_ID = OPAQUE("policy:");

function fail(message = "La politique d’accès lue est incohérente. Rechargez la page avant de réessayer."): never {
  throw new Error(message);
}

function object(value: unknown): Json {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail();
  return value as Json;
}

function uuid() {
  return crypto.randomUUID();
}

export function policySetId(set: PolicySet) {
  return objectId("layrs.sync.v3/policy-set", canonicalBytes({ schemaVersion: set.schemaVersion, spaceId: set.spaceId, policyVersions: set.policyVersions }));
}

export function productObjectId(product: Json) {
  const identity = { ...product };
  delete identity.productObjectId;
  return objectId("layrs-product-v1", canonicalBytes(identity));
}

export function evidenceObjectId(bytes: Uint8Array) {
  return objectId("layrs-evidence-v1", bytes);
}

export function requestDigest(request: Json) {
  const semantic = { ...request };
  delete semantic.requestDigest;
  return objectId("layrs.sync.v3/request-digest", canonicalBytes(semantic));
}

function checkedBootstrap(value: unknown, spaceId: string): Bootstrap {
  const base = object(value) as Bootstrap;
  const set = object(object(base.currentPolicySet).record) as PolicySet;
  if (
    base.spaceId !== spaceId ||
    typeof base.principalId !== "string" ||
    !Number.isSafeInteger(base.authorizationEpoch) ||
    base.authorizationEpoch < 1 ||
    !HASH.test(base.currentPolicySetId) ||
    set.schemaVersion !== "layrs.policy-set.v1" ||
    set.spaceId !== spaceId ||
    set.policySetId !== base.currentPolicySetId ||
    !Array.isArray(set.policyVersions) ||
    set.policyVersions.length > 32 ||
    policySetId(set) !== base.currentPolicySetId
  )
    fail();
  return base;
}

/** Closed rules documents only: the same bounds the server enforces. */
export function checkedRules(value: unknown) {
  const bytes = canonicalBytes(value);
  if (!bytes.length || bytes.length > MAX_RULES) fail("Le document de règles dépasse 64 Kio ou est vide.");
  const rules = object(value);
  if (!["layrs.space-policy-rules.v1", "layrs.space-policy-rules.v2", "layrs.space-policy-rules.v3"].includes(String(rules.schemaVersion)))
    fail("Version de règles non prise en charge.");
  return { rules, bytes };
}

export function webPolicyTransport(api: Api, spaceId: string) {
  const base = `/v3/spaces/${encodeURIComponent(spaceId)}`;
  const clientId = `client:web-${uuid()}`;

  const bootstrap = async () => checkedBootstrap(await api(`${base}/bootstrap`), spaceId);

  async function transfer(url: string, token: string, method: "GET" | "PUT", body?: Uint8Array) {
    // Capabilities are only ever sent to this instance's fixed transfer routes.
    const parsed = new URL(url, location.origin);
    const path = method === "GET" ? "/v3/transfers/downloads" : "/v3/transfers/uploads";
    if (parsed.protocol !== "https:" || parsed.pathname !== path || parsed.search || parsed.hash || parsed.username || parsed.password)
      fail("Adresse de transfert refusée.");
    const response = await fetch(path, {
      method,
      headers: { Authorization: `Bearer ${token}`, ...(body ? { "Content-Type": "application/octet-stream" } : {}) },
      body: body as BodyInit | undefined,
      redirect: "error",
      credentials: "omit",
      cache: "no-store",
      signal: AbortSignal.timeout(60_000),
    });
    if (!response.ok) fail(`Transfert refusé (${response.status}).`);
    if (method === "PUT") return new Uint8Array();
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > MAX_OBJECT) fail();
    return bytes;
  }

  /** Downloads policy objects authorized by the current PolicySet root. */
  async function download(state: Bootstrap, kind: "product" | "evidence", requested: string[]) {
    // Identical rules documents share one object: claim each object once.
    const ids = [...new Set(requested)];
    if (!ids.length) return new Map<string, Uint8Array>();
    const plan = object(
      await api(`${base}/fetch-plans`, "POST", {
        protocol: "layrs.sync.v3",
        type: "fetchPlanRequest",
        spaceId,
        clientId,
        requestId: `request:${uuid()}`,
        claims: ids.map((id) => ({ kind: "object", object: { kind, objectId: id }, root: { kind: "policySet", policySetId: state.currentPolicySetId }, repoPath: null })),
        pageCursor: null,
        maxObjects: ids.length,
      }),
    );
    const capabilities = plan.downloadCapabilities as Json[];
    if (
      plan.principalId !== state.principalId ||
      plan.authorizationEpoch !== state.authorizationEpoch ||
      plan.hasMore !== false ||
      !Array.isArray(capabilities) ||
      (Array.isArray(plan.deniedClaims) && plan.deniedClaims.length)
    )
      fail("Lecture des règles refusée par le serveur. Seuls les administrateurs du projet peuvent les consulter.");
    const result = new Map<string, Uint8Array>();
    for (const capability of capabilities) {
      const descriptor = object(capability.descriptor) as Descriptor;
      if (
        descriptor.kind !== kind ||
        !ids.includes(descriptor.objectId) ||
        result.has(descriptor.objectId) ||
        descriptor.compression !== "identity" ||
        descriptor.canonicalByteLen !== descriptor.encodedByteLen ||
        descriptor.canonicalByteLen > MAX_OBJECT
      )
        fail();
      const bytes = await transfer(String(capability.downloadUrl), String(capability.token), "GET");
      if (bytes.length !== descriptor.canonicalByteLen) fail();
      result.set(descriptor.objectId, bytes);
    }
    if (result.size !== ids.length) fail();
    return result;
  }

  async function readPolicyRules(): Promise<WebPolicyRulesRead> {
    const state = await bootstrap();
    const entries = state.currentPolicySet.record.policyVersions;
    const products = await download(state, "product", entries.map((entry) => entry.policyProductObjectId));
    const records = entries.map((entry) => {
      const bytes = products.get(entry.policyProductObjectId)!;
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      const product = object(JSON.parse(text));
      const record = object(product.record);
      if (
        canonicalJson(product) !== text ||
        product.kind !== "policy" ||
        product.productObjectId !== entry.policyProductObjectId ||
        productObjectId(product) !== entry.policyProductObjectId ||
        product.productId !== entry.policyId ||
        record.policyId !== entry.policyId ||
        record.versionId !== entry.policyVersionId ||
        record.spaceId !== spaceId ||
        typeof record.rulesObjectId !== "string" ||
        !entry.dependencyObjectIds.includes(record.rulesObjectId)
      )
        fail();
      return { entry, rulesObjectId: record.rulesObjectId };
    });
    const evidence = await download(state, "evidence", records.map((item) => item.rulesObjectId));
    const policies = records.map(({ entry, rulesObjectId }) => {
      const bytes = evidence.get(rulesObjectId)!;
      const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      const rules = JSON.parse(text);
      if (bytes.length > MAX_RULES || evidenceObjectId(bytes) !== rulesObjectId || canonicalJson(rules) !== text) fail();
      return { policyId: entry.policyId, policyVersionId: entry.policyVersionId, policyProductObjectId: entry.policyProductObjectId, rules };
    });
    const after = await bootstrap();
    if (after.principalId !== state.principalId || after.currentPolicySetId !== state.currentPolicySetId || after.authorizationEpoch !== state.authorizationEpoch)
      fail("La politique a changé pendant la lecture. Rechargez avant de continuer.");
    return { principalId: state.principalId, spaceId, policySetId: state.currentPolicySetId, authorizationEpoch: state.authorizationEpoch, policies };
  }

  function preview(state: Bootstrap, proposedPolicySet: Json, proposedId: string, products: Json[], inlineRules: WebPolicyPreview["inlineRules"]): WebPolicyPreview {
    const nonce = uuid();
    const request: Json = {
      protocol: "layrs.sync.v3",
      type: "syncTransactionRequest",
      spaceId,
      clientId,
      idempotencyKey: `policy-${nonce}`,
      requestDigest: "",
      knownEventCursor: null,
      steps: [],
      observations: [],
      products,
      viewMutation: null,
      policyRootMutation: {
        mutationId: `mutation:policy-${nonce}`,
        expectedPolicySetId: state.currentPolicySetId,
        expectedAuthorizationEpoch: state.authorizationEpoch,
        proposedPolicySet,
      },
    };
    request.requestDigest = requestDigest(request);
    return {
      schemaVersion: "layrs.web-policy-preview.v1",
      principalId: state.principalId,
      spaceId,
      expectedPolicySetId: state.currentPolicySetId,
      expectedAuthorizationEpoch: state.authorizationEpoch,
      proposedPolicySetId: proposedId,
      request,
      inlineRules,
    };
  }

  function policyProduct(policyId: string, rulesObjectId: string, previous: string | null) {
    const product: Json = {
      kind: "policy",
      productId: policyId,
      productObjectId: "",
      dependsOnStepIds: [],
      dependsOnProductIds: previous ? [policyId] : [],
      previousProductObjectId: previous,
      record: {
        schemaVersion: "layrs.policy.v1",
        policyId,
        versionId: `policy-version:${uuid()}`,
        spaceId,
        rulesObjectId,
        createdAt: new Date().toISOString(),
      },
    };
    product.productObjectId = productObjectId(product);
    return product;
  }

  async function previewRules(value: unknown, replaced: string | null) {
    const { rules, bytes } = checkedRules(value);
    if (replaced !== null && !POLICY_ID.test(replaced)) fail("Politique à remplacer invalide.");
    const state = await bootstrap();
    const current = state.currentPolicySet.record;
    const evidence = evidenceObjectId(bytes);
    let policyId: string;
    let previous: string | null = null;
    const versions = current.policyVersions.map((entry) => ({ ...entry, dependencyObjectIds: [...entry.dependencyObjectIds] }));
    if (replaced) {
      const entry = versions.find((item) => item.policyId === replaced);
      if (!entry) fail("Cette politique n’existe plus. Relisez les règles actuelles.");
      policyId = replaced;
      previous = entry.policyProductObjectId;
    } else {
      if (versions.length >= 32) fail("Le projet a atteint 32 politiques. Remplacez une politique existante.");
      policyId = `policy:${uuid()}`;
    }
    const product = policyProduct(policyId, evidence, previous);
    const record = product.record as Json;
    const next: PolicyVersionEntry = {
      policyId,
      policyVersionId: String(record.versionId),
      policyProductObjectId: String(product.productObjectId),
      dependencyObjectIds: [...new Set([evidence, ...(previous ? [previous] : [])])].sort(),
    };
    const policyVersions = [...versions.filter((entry) => entry.policyId !== policyId), next].sort((a, b) => (a.policyId < b.policyId ? -1 : a.policyId > b.policyId ? 1 : 0));
    const set: PolicySet = { schemaVersion: "layrs.policy-set.v1", policySetId: "", spaceId, policyVersions };
    set.policySetId = policySetId(set);
    return {
      preview: preview(state, { kind: "inline", policySet: set }, set.policySetId, [product], {
        objectId: evidence,
        rules,
        ...(previous ? { replacement: { policyId, previousProductObjectId: previous } } : {}),
      }),
    };
  }

  async function previewStored(id: unknown) {
    if (typeof id !== "string" || !HASH.test(id)) fail("Référence de politique invalide.");
    const state = await bootstrap();
    return { preview: preview(state, { kind: "stored", policySetId: id }, id, [], null) };
  }

  async function upload(state: WebPolicyPreview) {
    const inline = state.inlineRules;
    if (!inline) return;
    const bytes = canonicalBytes(inline.rules);
    if (evidenceObjectId(bytes) !== inline.objectId) fail();
    const descriptor = { kind: "evidence", objectId: inline.objectId, canonicalByteLen: bytes.length, encodedByteLen: bytes.length, compression: "identity" };
    const plan = object(
      await api(`${base}/object-plans`, "POST", {
        protocol: "layrs.sync.v3",
        type: "objectPlanRequest",
        spaceId,
        clientId,
        requestId: `request:${uuid()}`,
        objects: [descriptor],
      }),
    );
    const present = plan.presentObjects as Json[];
    const missing = plan.missingObjects as Json[];
    const capabilities = plan.uploadCapabilities as Json[];
    if (plan.principalId !== state.principalId || !Array.isArray(present) || !Array.isArray(missing) || !Array.isArray(capabilities)) fail();
    if (present.length === 1 && !missing.length) return;
    const capability = capabilities[0];
    if (missing.length !== 1 || capabilities.length !== 1 || canonicalJson(object(capability).descriptor) !== canonicalJson(descriptor)) fail();
    await transfer(String(capability.uploadUrl), String(capability.token), "PUT", bytes);
  }

  async function applyPolicy(value: unknown) {
    const state = object(value) as WebPolicyPreview;
    const request = object(state.request);
    const mutation = object(request.policyRootMutation);
    if (
      state.schemaVersion !== "layrs.web-policy-preview.v1" ||
      state.spaceId !== spaceId ||
      request.spaceId !== spaceId ||
      request.clientId !== clientId ||
      requestDigest(request) !== request.requestDigest ||
      mutation.expectedPolicySetId !== state.expectedPolicySetId ||
      mutation.expectedAuthorizationEpoch !== state.expectedAuthorizationEpoch
    )
      fail("Cet aperçu n’appartient pas à ce projet ou à cette session. Préparez-en un nouveau.");
    await upload(state);
    const receipt = object(await api(`${base}/transactions`, "POST", request));
    if (
      receipt.principalId !== state.principalId ||
      receipt.spaceId !== spaceId ||
      receipt.clientId !== clientId ||
      receipt.idempotencyKey !== request.idempotencyKey ||
      receipt.requestDigest !== request.requestDigest ||
      !["applied", "rejected"].includes(String(receipt.status))
    )
      fail("Reçu du serveur incohérent. Rechargez la politique avant de conclure.");
    return { receipt };
  }

  return async function policy<T>(command: { action: string; [key: string]: unknown }): Promise<T> {
    switch (command.action) {
      case "readPolicyRules":
        return (await readPolicyRules()) as T;
      case "previewPolicyRules":
        return (await previewRules(command.rules, null)) as T;
      case "previewReplacePolicyRules":
        return (await previewRules(command.rules, String(command.policyId))) as T;
      case "previewPolicy":
        return (await previewStored(command.policySetId)) as T;
      case "applyPolicy":
        return (await applyPolicy(command.preview)) as T;
      case "collaboration": {
        const inner = object(command.command);
        if (inner.action === "projectionContext") return (await api(`${base}/bootstrap`)) as T;
        if (inner.action === "projectionPage") {
          const { action: _action, ...request } = inner;
          return (await api(`${base}/projection`, "POST", request)) as T;
        }
        return fail("Commande de lecture inconnue.");
      }
      default:
        return fail("Commande de politique inconnue.");
    }
  };
}
