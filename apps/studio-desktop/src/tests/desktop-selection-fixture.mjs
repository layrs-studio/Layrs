/** Test-only native boundary. No fixture is imported by the shipped application. */
export const first = "C:/projects/first";
export const second = "C:/projects/second";
export const hash = char => `blake3:${char.repeat(64)}`;
export const route = (root, view) => `/#/projects/${encodeURIComponent(root)}/${view}`;

export async function installDesktop(page, scenario = {}) {
  await page.addInitScript(({ first, second, scenario }) => {
    const hash = char => `blake3:${char.repeat(64)}`;
    const origin = "https://example.test";
    localStorage.setItem("layrs.desktop.servers.v1", JSON.stringify({ projects: { [first]: origin }, authorities: {}, targets: {}, activity: {}, last: origin }));
    const calls = [];
    window.__desktopCalls = calls;
    let created = false, repaired = false, resumed = false, cloned = false;
    const state = root => {
      const isSecond = root === second;
      const worktreeId = isSecond ? "worktree:second" : "worktree:first";
      const operation = { operationId: isSecond ? "operation:second" : "operation:first", summary: "Capture locale", kind: "capture", createdAt: "2026-10-24T10:00:00Z" };
      return {
        space: { name: "Projet de test", spaceId: "space:test" },
        worktree: { worktreeId, checkoutRoot: root }, operation,
        layers: ["main", "task"].map(name => ({ layerId: `layer:${name}`, name, settings: { automaticCapture: false, automaticCaptureIdleMs: 60000 } })),
        view: { worktrees: [
          // A foreign historical observation comes first to catch accidental global selection.
          { worktreeId: isSecond ? "worktree:first" : "worktree:second", activeLayerId: "layer:main", baseStepId: hash("e"), status: "ready" },
          { worktreeId, activeLayerId: isSecond ? "layer:task" : "layer:main", baseStepId: hash(isSecond ? "b" : "a"), status: isSecond && scenario.interrupted && !resumed ? "stale" : "ready" },
        ] },
      };
    };
    const status = root => ({ state: state(root), needsRepair: root === second && scenario.interrupted && !repaired, changes: [], operations: [state(root).operation], markers: [], releases: [] });
    const describe = root => ({ path: root, worktreeId: state(root).worktree.worktreeId, state: state(root) });
    const projection = {
      generation: 4, digest: "a".repeat(64), snapshot: {
        schemaVersion: "layrs.projection-snapshot.v1", baseStepId: hash("a"), baseRootTreeId: hash("b"), observedPolicySetId: hash("c"), observedAuthorizationEpoch: 1, pathSemantics: "portable_windows",
        entries: [
          { path: "chosen.txt", target: { kind: "file", fileId: hash("1") }, executable: false, writable: true, state: "materialized" },
          { path: "excluded.txt", target: { kind: "file", fileId: hash("2") }, executable: false, writable: true, state: "unavailable" },
          { path: "secret.txt", target: { kind: "file", fileId: hash("3") }, executable: false, writable: false, state: "reserved" },
          { path: "unsupported", target: { kind: "symlink", symlinkId: hash("4") }, executable: false, writable: true, state: "unavailable" },
        ],
      },
    };
    let selection = { kind: "selected", paths: ["chosen.txt", "unsupported"], subtrees: [] };
    const identity = { storeId: "store:test", worktreeId: "worktree:partial", spaceId: "space:test", origin, principalId: "principal:test" };
    let imported = Boolean(scenario.offline?.imported), converted = false, captured = false, conversionInterrupted = false, databasesRecovered = false, initializationResumed = false, rootReplaced = false;
    const offlinePreparations = [];
    if (scenario.offline?.mixedPreparations) {
      for (let i = 0; i < 1088; i++) offlinePreparations.push({ id: `preparation:${i}`, status: i >= 1024 ? "released" : i % 2 ? "acquiring" : "ready", request: { version: { kind: "generation", worktreeId: identity.worktreeId, generation: projection.generation, digest: projection.digest }, other: null, paths: ["chosen.txt"], operation: "read" } });
    }
    const usage = { contentBytes: 128, reservedBytes: 0, limitBytes: 1024, protectionRoots: 1, windowsPowerLossQualified: false };
    const currentIdentity = root => root === second && converted ? { ...identity, storeId: "store:family", worktreeId: "worktree:copy" } : identity;
    const inspection = root => {
      const shared = scenario.offline?.shared !== false || root === second && converted;
      const recovery = [...(databasesRecovered ? scenario.offline?.remainingRecovery ?? (scenario.offline?.recovery ?? []).filter(r => !["familyRollback", "worktreeRollback"].includes(r)) : scenario.offline?.recovery ?? [])].filter(r => !initializationResumed || r !== "initializing");
      if (scenario.offline?.pendingJournal && !resumed && !recovery.includes("filesystemOperation")) recovery.push("filesystemOperation");
      const binding = { identity: currentIdentity(root), checkoutIdentity: rootReplaced ? "native-checkout:replacement" : "native-checkout:fixture", markerDigest: hash(rootReplaced ? "9" : "8"), databaseIdentity: rootReplaced ? "native-database:replacement" : "native-database:fixture", familyMarkerDigest: shared ? hash(rootReplaced ? "6" : "7") : null };
      if (scenario.offline?.invalidBinding) binding.markerDigest = "8".repeat(64);
      return { identity: scenario.offline?.invalidInspection ? null : currentIdentity(root), binding, shared, recovery, conversion: { allowed: !shared && !recovery.length, reason: recovery.length ? `recovery_required: ${recovery.join(", ")}` : shared ? "conversion source is already shared" : null } };
    };
    const recoveryReply = root => {
      const result = inspection(root), field = scenario.offline?.wrongRecoveryBinding;
      if (field) result.binding[field] = field.endsWith("Digest") ? hash("5") : "native:unexpected-reply";
      return result;
    };
    const patch = root => ({ schemaVersion: "layrs.projection-capture.v1", sequence: 3, projectionGeneration: projection.generation, projectionDigest: projection.digest, identity: currentIdentity(root), changes: [], digest: hash("e") });
    const projectionStatus = (root = first) => ({ identity: currentIdentity(root), projection, selection, needsRepair: false, pendingPublication: null, latestCapture: captured || imported ? patch(root) : null, capabilities: { capture: true, publish: !imported, namespace: true, receive: true, restore: true }, ...(scenario.offline ? { offline: { shared: scenario.offline.shared !== false || root === second && converted }, latestCaptureImported: imported } : {}) });
    window.__advanceProjection = () => { projection.generation++; };
    window.__replaceProjectionRoot = () => { rootReplaced = true; };
    window.__currentProjectionInspection = () => structuredClone(inspection(first));
    window.__recoveryMutationCount = 0;
    window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
      calls.push(structuredClone({ command, args }));
      const request = args?.request;
      const copy = value => structuredClone(value);
      if (command === "select_folder") return second;
      if (command === "native_git_bridge") {
        if (scenario.gitErrorAction === request.action) throw new Error(scenario.gitError ?? "git_bridge_error: target changed since review");
        const target = { path: args.root, storeId: "store:test", spaceId: "space:test", worktreeId: state(args.root).worktree.worktreeId, operationId: state(args.root).operation.operationId, viewId: hash("c") };
        const report = { importId: "import:fixture", objectFormat: "sha1", commits: 2, sourceBytes: 128, warnings: [], references: [{ gitRef: "refs/heads/main", gitId: "d".repeat(40), stepId: hash("e") }] };
        if (request.action === "prepareImport") return { checkpoint: { path: request.checkpoint, digest: `sha256:${"a".repeat(64)}`, commits: 2, objects: 4, references: 1 } };
        if (request.action === "reviewImport") return { reviewId: "review:import", review: { target, checkpoint: request.checkpoint, digest: request.digest, report } };
        if (request.action === "applyImport") return { import: { importId: report.importId, alreadyImported: false }, report, target };
        if (request.action === "reviewExport") return { reviewId: "review:export", review: { target, destination: request.destination, mode: request.mode.kind === "selected_history" ? { ...request.mode, references: [...request.mode.references].sort() } : request.mode, contentPreview: "unavailable" } };
        if (["export", "resumeExport"].includes(request.action)) return { target, destination: request.destination, export: { commits: { [hash("a")]: "f".repeat(40) }, references: { "refs/heads/main": "f".repeat(40) }, warnings: [] } };
        throw new Error(`Unsupported bridge action ${request.action}`);
      }
      if (command === "native_checkout_kind") {
        if (scenario.offline?.sourceUnavailable === "error") throw new Error("source_unavailable: original checkout cannot open");
        if (scenario.offline?.sourceUnavailable === "absent") return "absent";
        if (scenario.unregistered && args.root === second && !resumed) throw new Error("invalid_worktree_locator: pending unregistered Worktree intent");
        return scenario.projection || cloned ? "projection" : "complete";
      }
      if (command === "native_workspace") {
        if (request.action === "status") return copy(status(args.root));
        if (request.action === "listWorktrees") return { currentWorktreeId: state(args.root).worktree.worktreeId, worktrees: [first, ...(created || scenario.existing ? [second] : [])].map(root => ({ ...state(root).worktree, activeOperationId: state(root).operation.operationId })) };
        if (request.action === "createWorktree") {
          created = true;
          if (scenario.interrupted || scenario.unregistered) throw new Error("injected: creation interrupted after durable intent");
          return copy(describe(request.path));
        }
        if (request.action === "openWorktree") {
          if (scenario.unregistered && !resumed) throw new Error("invalid_worktree_locator: pending unregistered Worktree intent");
          return copy(describe(request.path));
        }
        if (request.action === "recover") { repaired = true; return copy(status(args.root)); }
        if (request.action === "resumeWorktreeCreation") { resumed = true; repaired = true; return copy(describe(args.root)); }
        throw new Error(`Unsupported workspace action ${request.action}`);
      }
      if (command === "native_projection" && request.action === "status") {
        if (scenario.offline?.sourceUnavailable) throw new Error("source_unavailable: original checkout cannot open");
        if (scenario.offline?.forbidOrdinaryStatus) throw new Error("ordinary_status_must_not_open_conversion_source");
        if (scenario.offline?.pendingJournal && !resumed) throw new Error("projection_store_error: pending filesystem journal requires explicit recovery");
        return copy(projectionStatus(args.root));
      }
      if (command === "native_projection" && scenario.offline) {
        const recoveryRequest = request.action === "offline" ? request.request : request;
        if (["recoverDatabases", "resumeInitialization", "cancelSharedReservation", "resumeInstallation", "resumeNamespace", "resumeRefresh", "resumeRestore", "resumeRebase"].includes(recoveryRequest.action) && JSON.stringify(recoveryRequest.expected) !== JSON.stringify(inspection(args.root).binding)) throw new Error("recovery binding changed before mutation");
        if (request.action === scenario.offline.pendingJournal) { resumed = true; window.__recoveryMutationCount++; return { projection }; }
        if (request.action === "history") return { captures: [{ ...patch(args.root), projectionGeneration: 2, projectionDigest: "b".repeat(64) }] };
        if (request.action === "capture") { captured = true; imported = false; return patch(args.root); }
        if (request.action !== "offline") throw new Error(`Unexpected projection action ${request.action}`);
        const action = request.request;
        if (action.action === "previewRestore") {
          if (scenario.offline.restoreRefusal === action.action) throw new Error("restore requires currently writable regular-file leaves; no local content available");
          if (action.expectedGeneration !== projection.generation) throw new Error("restore projection changed");
          const preview = { operationId: "1234-56", destination: { identity: currentIdentity(args.root), checkoutIdentity: "native-checkout:fixture" }, source: { kind: "version", version: action.version, paths: action.paths }, projectionGeneration: projection.generation, projectionDigest: projection.digest, checkoutDigest: hash("b"), afterCheckoutDigest: hash("c"), before: Object.fromEntries(action.paths.map(path => [path, { fileId: hash("1"), executable: false, bytes: 7 }])), after: Object.fromEntries(action.paths.map(path => [path, scenario.offline.restoreAbsence ? null : { fileId: hash("2"), executable: true, bytes: 5 }])), digest: hash("d") };
          if (scenario.offline.foreignRestoreDestination) preview.destination = { ...preview.destination, identity: { ...preview.destination.identity, [scenario.offline.foreignRestoreDestination]: "foreign" } };
          if (scenario.offline.missingRestoreDestination) delete preview.destination;
          if (scenario.offline.invalidRestorePreview) preview.source.paths = ["foreign.txt"];
          window.__restorePreview = copy(preview);
          return copy({ preview });
        }
        if (action.action === "applyRestore") {
          if (scenario.offline.restoreRefusal === action.action || action.preview.projectionGeneration !== projection.generation) throw new Error("restore preview is stale: checkout or generation changed");
          captured = true; imported = false;
          return { restoration: { operationId: scenario.offline.wrongRestoreReceipt ? "9999" : action.preview.operationId, capture: patch(args.root) } };
        }
        if (["inspectConversionSource", "inspectRecovery"].includes(action.action)) {
          if (scenario.offline.inspectionError) throw new Error(scenario.offline.inspectionError);
          window.__lastInspection = copy(inspection(args.root));
          return copy(window.__lastInspection);
        }
        if (action.action === "recoverDatabases") {
          if (scenario.offline.recoveryRefusal === action.action) throw new Error("database rollback refused: filesystem identity changed");
          if (!inspection(args.root).recovery.some(r => ["familyRollback", "worktreeRollback"].includes(r))) throw new Error("no database rollback requested");
          databasesRecovered = true;
          window.__recoveryMutationCount++;
          const result = recoveryReply(args.root);
          if (scenario.offline.wrongDatabaseIdentity) { result.identity = { ...result.identity, worktreeId: "worktree:foreign" }; result.binding.identity = result.identity; }
          return copy(result);
        }
        if (action.action === "resumeInitialization") {
          const { recovery } = inspection(args.root);
          if (recovery.length !== 1 || recovery[0] !== "initializing") throw new Error("initialization refuses other recovery needs");
          initializationResumed = true;
          window.__recoveryMutationCount++;
          return copy(recoveryReply(args.root));
        }
        if (action.action === "abandonConversion") {
          if (scenario.offline.recoveryRefusal === action.action || converted) throw new Error("accepted destination cannot be abandoned");
          return { destination: scenario.offline.wrongRecoveryReply ? "C:/wrong" : action.destination, abandoned: true, filesRetained: true };
        }
        if (scenario.offline.sourceUnavailable) throw new Error("source_unavailable: original checkout cannot open");
        if (action.action === "cancelSharedReservation") {
          if (scenario.offline.recoveryRefusal === action.action) throw new Error("operation may own checkout staging; resume the exact filesystem operation");
          window.__recoveryMutationCount++;
          usage.reservedBytes = 0;
          return { reservationCancelled: true };
        }
        if (action.action === "releaseGenerationRetention") {
          if (scenario.offline.recoveryRefusal === action.action || action.generation === projection.generation) throw new Error("generation is required by retained capture history");
          return { generation: action.generation, releasedClaims: 2 };
        }
        if (action.action === "currentVersion") return { shared: true, version: { kind: "generation", worktreeId: currentIdentity(args.root).worktreeId, generation: projection.generation, digest: projection.digest } };
        if (action.action === "availability") {
          const paths = action.request.paths.flatMap(path => [action.request.version, ...(action.request.other ? [action.request.other] : [])].map((_, operand) => ({ path, operand, absent: false, selected: path !== "excluded.txt", observedWritable: path !== "secret.txt", materialized: path === "chosen.txt", retained: path === "chosen.txt", fileId: hash("1"), manifestId: path === "chosen.txt" ? hash("2") : null, blockers: path === "secret.txt" ? ["protectedPath"] : path === "excluded.txt" ? ["missingContent"] : [] })));
          return { request: action.request, paths, effectivePaths: action.request.paths, blockers: [], available: paths.every(p => !p.blockers.length), prepared: false };
        }
        if (action.action === "readFile") return { version: action.version, path: action.path, encoding: "base64", bytes: scenario.offline.binary ? 2 : 5, content: scenario.offline.binary ? "/wA=" : "c2FsdXQ=" };
        if (action.action === "compare") return { differences: [{ path: action.request.paths[0], before: hash("1"), after: hash("2"), beforeExecutable: false, afterExecutable: true }] };
        if (action.action === "preparations") return copy({ preparations: offlinePreparations });
        if (action.action === "releasePreparation") { const p = offlinePreparations.find(p => p.id === action.id); if (!p) throw new Error("unknown_preparation"); p.status = "released"; return { released: action.id }; }
        if (action.action === "familyUsage") return copy({ identity, path: "C:/family", usage });
        if (action.action === "setFamilyQuota") { usage.limitBytes = action.bytes; return copy({ usage }); }
        if (action.action === "evictUnretained") { usage.contentBytes = 64; return copy({ usage, evictedObjects: 1 }); }
        if (action.action === "createFamily") return copy({ identity: { ...identity, storeId: "store:family" }, path: action.path, usage: { ...usage, limitBytes: action.quotaBytes } });
        if (["convertIntoFamily", "resumeConversion"].includes(action.action)) {
          if (scenario.offline.interrupted && !conversionInterrupted && action.action === "convertIntoFamily") { conversionInterrupted = true; throw new Error("conversion_interrupted: resume destination"); }
          converted = true; imported = true;
          const destinationIdentity = currentIdentity(second);
          return copy({ path: action.destination, identity: destinationIdentity, projection, selection, receipt: { sourceCheckout: args.root, sourceIdentity: identity, destinationIdentity, sourceRetained: true, importedCaptureCount: 3, sourceGeneration: projection.generation, sourceProjectionDigest: projection.digest, sourceDatabaseDigest: "d".repeat(64), checkoutDigest: "e".repeat(64), publicationHistoryDigest: "f".repeat(64) } });
        }
        throw new Error(`Unexpected offline action ${action.action}`);
      }
      if (command === "native_remote") {
        const action = request.action;
        if (action.action === "whoami") return { principal_id: "principal:test", display_name: "Test" };
        if (scenario.offline && ["observeProjectionVersion", "prepareProjectionOffline", "resumeProjectionOffline"].includes(action.action)) {
          if (action.expectedGeneration !== projection.generation) throw new Error("offline_generation_changed: refresh before retry");
          if (action.action === "observeProjectionVersion") return { status: "projectionVersionObserved", version: { kind: "remote", stepId: action.step, rootTreeId: hash("a"), policySetId: hash("b"), authorizationEpoch: 2, observationDigest: "c".repeat(64) } };
          if (action.action === "prepareProjectionOffline") {
            const preparation = { id: "preparation:test", request: action.request, status: scenario.offline.interruptedPreparation ? "acquiring" : "ready" };
            offlinePreparations.push(preparation);
            if (scenario.offline.interruptedPreparation) throw new Error("offline_download_interrupted: preparation persisted");
            return copy({ status: "projectionOfflinePrepared", preparation });
          }
          const preparation = offlinePreparations.find(p => p.id === action.preparationId);
          if (!preparation || preparation.status === "released") throw new Error("unknown_or_released_preparation");
          preparation.status = "ready";
          return copy({ status: "projectionOfflinePrepared", preparation });
        }
        if (action.action === "listSpaces") return { schemaVersion: "layrs.space-catalog.v1", spaces: [], nextCursor: null, hasMore: false };
        if (action.action === "cloneProjectionSelected") { cloned = true; selection = action.selection; projection.snapshot.entries = []; return copy({ status: "projectionCloned", identity, projection, selection }); }
        if (action.action === "expandProjectionSelection") {
          if (scenario.collision) throw new Error("reserved_projection_path: collision with local file excluded.txt");
          if (action.expectedGeneration !== projection.generation) throw new Error("selection expansion generation changed");
          selection = { kind: "selected", paths: [...new Set([...selection.paths, ...action.selection.paths])], subtrees: [...new Set([...selection.subtrees, ...action.selection.subtrees])] };
          projection.generation++;
          projection.snapshot.entries[1].state = "materialized";
          return copy({ status: "projectionSelectionExpanded", identity, projection, selection });
        }
        throw new Error(`Unsupported remote action ${action.action}`);
      }
      throw new Error(`Unsupported command ${command}`);
    } };
  }, { first, second, scenario });
}

export const calls = page => page.evaluate(() => window.__desktopCalls);
