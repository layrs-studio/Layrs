/** Non-secret association between a local checkout and the server context it was cloned from. */
type Binding = { contextId: string; spaceId: string };
const key = (origin: string, principalId: string) => `layrs.desktop.recent-context.v1:${origin}:${principalId}`;

function read(origin: string, principalId: string): Record<string, Binding> {
  if (!origin || !principalId) return {};
  try {
    const value = JSON.parse(localStorage.getItem(key(origin, principalId)) ?? "{}");
    return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, Binding> : {};
  } catch { return {}; }
}

export function recentContext(root: string, origin: string, principalId: string): Binding | null {
  const value = read(origin, principalId)[root];
  return value && typeof value.contextId === "string" && typeof value.spaceId === "string" ? value : null;
}

export function rememberRecentContext(root: string, origin: string, principalId: string, contextId: string, spaceId: string) {
  if (!root || !origin || !principalId || !contextId || !spaceId) return;
  try { localStorage.setItem(key(origin, principalId), JSON.stringify({ ...read(origin, principalId), [root]: { contextId, spaceId } })); } catch { /* optional */ }
}
