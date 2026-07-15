import type { LensDiffEntry, LocalSpaceSummary, WeaveSessionSummary, WorkingTreeScan } from "./tauri";

export function isLayerRestricted(access: string | undefined): boolean {
  return access === "redacted" || access === "blocked";
}

export function sanitizeWorkingTreeForAccess(
  scan: WorkingTreeScan,
  space: LocalSpaceSummary
): WorkingTreeScan | undefined {
  const restrictedLayerIds = new Set(
    space.layers.filter((layer) => isLayerRestricted(layer.access)).map((layer) => layer.layerId)
  );
  if (restrictedLayerIds.has(scan.activeLayerId)) {
    return undefined;
  }
  return {
    ...scan,
    steps: scan.steps.filter((step) => !restrictedLayerIds.has(step.layerId)),
    layerActivities: scan.layerActivities.filter((activity) => !restrictedLayerIds.has(activity.layerId))
  };
}

export function purgeRestrictedWorkingTrees(
  spaces: LocalSpaceSummary[],
  workingTrees: Record<string, WorkingTreeScan>
): Record<string, WorkingTreeScan> {
  const next: Record<string, WorkingTreeScan> = {};
  for (const space of spaces) {
    const scan = workingTrees[space.localSpaceId];
    if (!scan) {
      continue;
    }
    const sanitized = sanitizeWorkingTreeForAccess(scan, space);
    if (sanitized) {
      next[space.localSpaceId] = sanitized;
    }
  }
  return next;
}

export function purgeRestrictedDiffWindows(
  spaces: LocalSpaceSummary[],
  windows: Record<string, LensDiffEntry>
): Record<string, LensDiffEntry> {
  const openPrefixes = new Set(
    spaces.flatMap((space) =>
      space.layers
        .filter((layer) => !isLayerRestricted(layer.access))
        .map((layer) => `${space.workspaceId}::${space.spaceId}::${layer.layerId}::${space.localSpaceId}::`)
    )
  );
  return Object.fromEntries(Object.entries(windows).filter(([key]) => [...openPrefixes].some((prefix) => key.startsWith(prefix))));
}

export function purgeRestrictedWeaves(
  spaces: LocalSpaceSummary[],
  sessions: Record<string, WeaveSessionSummary | null>
): Record<string, WeaveSessionSummary | null> {
  const next: Record<string, WeaveSessionSummary | null> = {};
  for (const space of spaces) {
    const session = sessions[space.localSpaceId];
    if (!session) {
      continue;
    }
    const restricted = new Set(
      space.layers.filter((layer) => isLayerRestricted(layer.access)).map((layer) => layer.layerId)
    );
    if (restricted.has(session.sourceLayerId) || restricted.has(session.targetLayerId)) {
      continue;
    }
    next[space.localSpaceId] = session;
  }
  return next;
}
