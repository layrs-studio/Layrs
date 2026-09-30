/*
 * Stubbed native side for Desktop renderer tests. Everything here is
 * fictional; the stub records each invocation for assertions.
 */
import type { Page } from "@playwright/test";

export type Invocation = { command: string; args?: Record<string, unknown> };

export type Scenario = {
  kind?: "complete" | "projection";
  conflicted?: boolean;
  /** Remembered server of the project. */
  origin?: string;
  /** Token already in the OS store for that server. */
  signedIn?: boolean;
  publish?: "hold" | "unconfirmed" | "ok";
  stale?: boolean;
};

declare global {
  interface Window {
    __layrsInvocations?: Invocation[];
    __layrsRelease?: (outcome: "cancelled" | "applied") => void;
  }
}

export const root = "C:/projects/demo";
export const projectPath = (view: string) => `/#/projects/${encodeURIComponent(root)}/${view}`;
export const hash = (seed: string) => `blake3:${seed.repeat(64).slice(0, 64)}`;

export async function installWorkStub(page: Page, scenario: Scenario) {
  await page.addInitScript(
    ({ scenario, root }) => {
      const hash = (seed: string) => `blake3:${seed.repeat(64).slice(0, 64)}`;
      const invocations: Invocation[] = [];
      window.__layrsInvocations = invocations;
      let signedIn = scenario.signedIn === true;
      let polls = 0;
      let conflicted = scenario.conflicted === true;
      const operation = { operationId: "op:1", summary: "Initialisation", kind: "initialize", createdAt: "2026-09-01T10:00:00Z" };
      const status = () => ({
        state: {
          space: { name: "Démo", spaceId: "space:demo" },
          worktree: { worktreeId: "worktree:demo", checkoutRoot: root },
          operation,
          layers: [
            { layerId: "layer:main", name: "main", settings: { automaticCapture: false, automaticCaptureIdleMs: 600000 } },
            { layerId: "layer:topic", name: "topic", settings: { automaticCapture: false, automaticCaptureIdleMs: 600000 } },
          ],
          view: { worktrees: [{ worktreeId: "worktree:demo", activeLayerId: "layer:main", status: conflicted ? "conflicted" : "clean" }] },
        },
        needsRepair: false,
        changes: [],
        operations: [operation],
        markers: [],
        releases: [],
      });
      const conflicts = () =>
        conflicted
          ? [
              {
                conflictId: "conflict:code",
                document: {
                  placements: [{ path: "code.ts" }],
                  bases: [{ termId: "term:base", provenance: { stepId: hash("b") }, values: [] }],
                  sides: [
                    { termId: "term:main", provenance: { stepId: hash("c") }, values: [] },
                    { termId: "term:topic", provenance: { stepId: hash("d") }, values: [] },
                  ],
                },
              },
              {
                conflictId: "conflict:rename",
                document: {
                  placements: [{ path: "old.txt" }, { path: "new.txt" }],
                  sides: [
                    { termId: "term:r1", provenance: { stepId: hash("c") }, values: [] },
                    { termId: "term:r2", provenance: { stepId: hash("d") }, values: [] },
                  ],
                },
              },
            ]
          : [];
      const identity = { storeId: "store:demo", worktreeId: "worktree:limited", spaceId: "space:demo", origin: "https://layrs.example.test", principalId: "principal:alice" };
      const projection = {
        generation: 4,
        digest: "a".repeat(64),
        snapshot: {
          schemaVersion: "layrs.projection-snapshot.v1",
          baseStepId: hash("1"),
          baseRootTreeId: hash("2"),
          observedPolicySetId: hash("e"),
          observedAuthorizationEpoch: 3,
          pathSemantics: "portable_windows",
          entries: [
            { path: "docs", target: { kind: "tree", treeId: hash("f") }, executable: false, writable: true, state: "materialized" },
            { path: "docs/allowed.txt", target: { kind: "file", fileId: hash("7") }, executable: false, writable: true, state: "materialized" },
            { path: "docs/readme.txt", target: { kind: "file", fileId: hash("8") }, executable: false, writable: false, state: "reserved" },
          ],
        },
      };
      const capture = {
        schemaVersion: "layrs.projection-capture.v1",
        sequence: 3,
        projectionGeneration: 4,
        projectionDigest: "a".repeat(64),
        identity,
        changes: [{ path: "docs/allowed.txt", expected: { fileId: hash("7"), executable: false }, replacement: { fileId: hash("6"), executable: false, bytes: 12 } }],
        digest: hash("c"),
      };
      const projectionStatus = { identity, projection, needsRepair: false, capabilities: { capture: true, publish: true, namespace: true, receive: true, restore: true }, latestCapture: null, pendingPublication: null };
      let release: ((outcome: "cancelled" | "applied") => void) | null = null;
      window.__layrsRelease = (outcome) => release?.(outcome);
      window.__TAURI_INTERNALS__ = {
        invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
          invocations.push({ command, args });
          const reply = (value: unknown) => value as T;
          const request = (args?.request ?? {}) as { action?: string | { action: string; [key: string]: unknown }; [key: string]: unknown };
          if (command === "native_checkout_kind") return reply(scenario.kind ?? "complete");
          if (command === "native_projection") {
            if (request.action === "status") return reply(projectionStatus);
            if (request.action === "capture") return reply(capture);
            if (request.action === "history") return reply({ captures: [] });
            throw new Error(`renderer stub: projection ${String(request.action)}`);
          }
          if (command === "native_workspace") {
            switch (request.action) {
              case "status":
                return reply(status());
              case "conflicts":
                return reply({ conflicts: conflicts() });
              case "compareConflictVariants":
                return reply({ available: true, beforeTerm: request.before, afterTerm: request.after, changes: [{ path: "code.ts", executableChanged: false, textOmitted: false, textDiff: [{ op: "delete", text: 'const value = "base";\n' }, { op: "insert", text: 'const value = "topic";\n' }] }] });
              case "previewResolution":
                return reply({ expectedOperation: "op:1", expectedStep: hash("9"), conflicts: [{}] });
              case "applyResolution":
                if (scenario.stale) throw new Error("invalid_worktree_state: invalid Worktree state: preview is stale");
                conflicted = false;
                return reply(status());
              case "localIdentity":
                return reply({ identity: null });
              default:
                throw new Error(`renderer stub: workspace ${String(request.action)}`);
            }
          }
          if (command === "native_remote") {
            const action = (request.action ?? { action: "" }) as { action: string; [key: string]: unknown };
            switch (action.action) {
              case "whoami":
                if (!signedIn) throw new Error("remote_unauthorized: Sign in to this server first with remote login");
                return reply({ principal_id: "principal:alice", actor_id: "actor:alice", display_name: "Alice Démo", kind: "account" });
              case "startLogin":
                return reply({ status: "pending", userCode: "WDJBMJHT", verificationUri: "https://layrs.example.test/auth/device", expiresIn: 600, interval: 5 });
              case "pollLogin":
                polls++;
                if (polls < 2) return reply({ status: "pending", interval: 5 });
                signedIn = true;
                return reply({ status: "authorized", principalId: "principal:alice" });
              case "listSpaces":
                return reply({
                  schemaVersion: "layrs.space-catalog.v1",
                  spaces: [
                    { spaceId: "space:site", slug: "site", name: "Site vitrine", workspace: { workspaceId: "workspace:a", slug: "atelier", name: "Atelier" }, workspaceRole: "member", access: "full", lastActivityAt: "2026-09-29T16:42:05Z" },
                    { spaceId: "space:brand", slug: "brand", name: "Identité", workspace: { workspaceId: "workspace:a", slug: "atelier", name: "Atelier" }, workspaceRole: "member", access: "limited", lastActivityAt: null },
                  ],
                  nextCursor: null,
                  hasMore: false,
                });
              case "publishCancellable":
                if (scenario.publish === "unconfirmed") throw new Error("remote_transport_failed: remote transport failed; pending publication remains retryable");
                if (scenario.publish === "hold")
                  return new Promise<T>((resolve, reject) => {
                    release = (outcome) =>
                      outcome === "cancelled"
                        ? reject(new Error("remote_transfer_cancelled: remote transfer cancelled before commit"))
                        : resolve(reply({ transferId: action.transferId, receipt: { status: "applied" } }));
                  });
                return reply({ transferId: action.transferId, receipt: { status: "applied" } });
              case "cancelTransfer":
                return reply({ transferId: action.transferId, status: "cancelRequested" });
              case "cloneCancellable":
                return reply({ status: "cloned", transferId: action.transferId, state: { worktree: { checkoutRoot: root } } });
              default:
                throw new Error(`renderer stub: remote ${action.action}`);
            }
          }
          throw new Error(`renderer stub: ${command}`);
        },
      };
      if (scenario.origin) {
        localStorage.setItem(
          "layrs.desktop.servers.v1",
          JSON.stringify({ projects: { [root]: scenario.origin }, authorities: {}, targets: {}, activity: {}, last: scenario.origin }),
        );
      }
    },
    { scenario, root },
  );
}

export async function remoteActions(page: Page) {
  return page.evaluate(() =>
    (window.__layrsInvocations ?? [])
      .filter((call) => call.command === "native_remote")
      .map((call) => (call.args?.request as { action: Record<string, unknown> }).action),
  );
}

export async function workspaceActions(page: Page) {
  return page.evaluate(() =>
    (window.__layrsInvocations ?? [])
      .filter((call) => call.command === "native_workspace" || call.command === "native_projection")
      .map((call) => call.args?.request as Record<string, unknown>)
      .filter((request) => !["status", "conflicts", "localIdentity", "compareConflictVariants"].includes(String(request.action))),
  );
}
