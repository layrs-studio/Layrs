import { StrictMode, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ToastProvider } from "../../../packages/ui/src/components/Toast";
import { AdvancedPanel } from "../../../packages/ui/src/team/AdvancedPanel";
import { AccessPanel } from "../../../packages/ui/src/team/AccessPanel";
import { readPolicies } from "../../../packages/ui/src/team/access-model";
import type { Directory } from "../../../packages/ui/src/team/useDirectory";
import type { PolicyPreview, PolicyTransport } from "../../../packages/ui/src/team/types";

const id = (letter: string) => `blake3:${letter.repeat(64)}`;
const preview: PolicyPreview = {
  origin: "https://layrs.example.test",
  principalId: "principal:alice",
  worktreeId: "worktree:demo",
  spaceId: "space:demo",
  expectedPolicySetId: id("a"), expectedAuthorizationEpoch: 3, proposedPolicySetId: id("b"),
  request: { clientId: "client:demo", idempotencyKey: "policy-replay-component", requestDigest: id("c") },
} as PolicyPreview;

export function mountPolicyReplayHarness(host: HTMLElement) {
  let root: Root = createRoot(host);
  let committed = false;
  const applied: string[] = [];
  let previews = 0;
  const policy: PolicyTransport = async <T,>(command: { action: string; [key: string]: unknown }): Promise<T> => {
    if (command.action === "previewPolicy") {
      previews++;
      if (command.policySetId !== preview.proposedPolicySetId) throw new Error("wrong stored policy");
      return { preview } as T;
    }
    if (command.action === "applyPolicy") {
      applied.push(JSON.stringify(command.preview));
      if (!committed) {
        committed = true; // The server committed B, but its answer was lost.
        throw new Error("remote_transport_failed: answer lost after commit");
      }
      return { receipt: { status: "applied" } } as T;
    }
    throw new Error(`unexpected policy command: ${command.action}`);
  };
  const directory = (): Directory => ({
    members: [], accounts: [], teams: [], management: { mode: "custom", currentPolicySetId: committed ? id("b") : id("a"), managedPolicySetId: id("a") },
    rules: { principalId: "principal:alice", spaceId: "space:demo", policySetId: committed ? id("b") : id("a"), authorizationEpoch: committed ? 4 : 3, policies: [] },
    policies: [], canAdminister: true, canManage: false, loading: false, error: "", rulesError: "", people: new Map(), reload: async () => {},
  });
  const render = () => root.render(createElement(ToastProvider, null,
    createElement(AdvancedPanel, { scope: "test-policy-replay", spaceId: "space:demo", directory: directory(), policy, canManagePolicy: true, projectName: "Démo" })));
  render();
  return {
    remount: () => { root.unmount(); root = createRoot(host); render(); },
    state: () => ({ committed, applied: [...applied], previews }),
    dispose: () => root.unmount(),
    targetPolicy: preview.proposedPolicySetId,
  };
}

export function mountAccessReplayHarness(host: HTMLElement, replayReceipt: "applied" | "rejected" = "applied") {
  let root: Root = createRoot(host);
  let committed = false;
  let acknowledged = false;
  const nextPreview = { ...preview, expectedPolicySetId: id("b"), expectedAuthorizationEpoch: 4, proposedPolicySetId: id("e"),
    request: { clientId: "client:demo", idempotencyKey: "policy-after-rejection", requestDigest: id("f") } };
  const actions: string[] = [];
  const applied: string[] = [];
  const policy: PolicyTransport = async <T,>(command: { action: string; [key: string]: unknown }): Promise<T> => {
    actions.push(command.action);
    if (command.action === "readPendingPolicy") return { preview: committed && !acknowledged ? preview : null } as T;
    if (command.action === "previewPolicyRules") return { preview } as T;
    if (command.action === "previewReplacePolicyRules") {
      if (!acknowledged) throw new Error("new policy preview before settlement");
      return { preview: nextPreview } as T;
    }
    if (command.action === "applyPolicy") {
      applied.push(JSON.stringify(command.preview));
      if (!committed) {
        committed = true;
        throw new Error("remote_transport_failed: answer lost after commit");
      }
      acknowledged = true;
      return { receipt: { status: replayReceipt } } as T;
    }
    throw new Error(`unexpected policy command: ${command.action}`);
  };
  const rules = () => ({
    principalId: "principal:alice", spaceId: "space:demo", policySetId: committed ? id("b") : id("a"), authorizationEpoch: committed ? 4 : 3,
    policies: committed ? [{ policyId: "policy:access", policyVersionId: "policy-version:access", policyProductObjectId: id("e"), rules: {
      schemaVersion: "layrs.space-policy-rules.v3", rules: [], pathPolicies: [{ principalIds: ["principal:alice"],
        read: { default: "allow", rules: [{ effect: "allow", selector: { kind: "exact", path: "readme.txt" } }] },
        write: { default: "deny", rules: [{ effect: "deny", selector: { kind: "exact", path: "readme.txt" } }] },
      }],
    } }] : [],
  });
  const directory = (): Directory => {
    const current = rules();
    return { members: [], accounts: [], teams: [], management: { mode: "custom", currentPolicySetId: current.policySetId, managedPolicySetId: id("a") },
      rules: current, policies: readPolicies(current, []), canAdminister: true, canManage: false, loading: false, error: "", rulesError: "", people: new Map(), reload: async () => {} };
  };
  const loadContext = async () => ({ principalId: "principal:alice", spaceId: "space:demo", currentPolicySetId: committed ? id("b") : id("a"), authorizationEpoch: committed ? 4 : 3,
    layers: [{ record: { layerId: "layer:main", name: "main" }, headStepIds: [id("d")] }] });
  const loadPage = async (request: { stepId: string; expectedPolicySetId: string; expectedAuthorizationEpoch: number }) => ({
    schemaVersion: "layrs.projection-page.v1", principalId: "principal:alice", spaceId: "space:demo", stepId: request.stepId, rootTreeId: id("f"),
    policySetId: request.expectedPolicySetId, authorizationEpoch: request.expectedAuthorizationEpoch, offset: 0,
    entries: [{ path: "readme.txt", target: { kind: "file", fileId: id("1") }, executable: false, contentReadable: true, writable: false }], nextCursor: null,
  });
  const render = () => root.render(createElement(StrictMode, null, createElement(ToastProvider, null,
    createElement(AccessPanel, { scope: "test-access-replay", directory: directory(), policy, loadContext, loadPage,
      self: { principalId: "principal:alice", displayName: "Alice Démo" }, projectName: "Démo", canChange: true,
      recoverPendingPolicy: async () => (await policy<{ preview: PolicyPreview | null }>({ action: "readPendingPolicy" })).preview }))));
  render();
  return {
    remount: () => { root.unmount(); root = createRoot(host); render(); },
    state: () => ({ committed, acknowledged, actions: [...actions], applied: [...applied] }),
    dispose: () => root.unmount(),
  };
}
