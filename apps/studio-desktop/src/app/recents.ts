/*
 * Local folders opened on this computer. A recent entry is a convenience,
 * never a proof of server authorization; it holds no secret.
 */

export type RecentProject = {
  root: string;
  name: string;
  kind: "complete" | "projection";
  openedAt: string;
  favorite?: boolean;
};

const STORAGE_KEY = "layrs.desktop.recents.v1";
const LEGACY_CHECKOUT_KEY = "layrs.native.checkout";
const LIMIT = 12;

function folderName(root: string) {
  return root.split(/[\\/]/).filter(Boolean).pop() || root;
}

export function readRecents(): RecentProject[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as RecentProject[];
      return Array.isArray(parsed) ? parsed.filter((entry) => typeof entry?.root === "string" && entry.root) : [];
    }
    // The previous workbench remembered a single checkout.
    const legacy = localStorage.getItem(LEGACY_CHECKOUT_KEY);
    return legacy ? [{ root: legacy, name: folderName(legacy), kind: "complete", openedAt: new Date(0).toISOString() }] : [];
  } catch {
    return [];
  }
}

function write(entries: RecentProject[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
  } catch {
    // Recents are optional; the project still opens.
  }
}

export function rememberProject(entry: Omit<RecentProject, "openedAt" | "favorite">) {
  const current = readRecents();
  const previous = current.find((item) => item.root === entry.root);
  const next: RecentProject = { ...entry, name: entry.name || previous?.name || folderName(entry.root), openedAt: new Date().toISOString(), favorite: previous?.favorite };
  write([next, ...current.filter((item) => item.root !== entry.root)].slice(0, LIMIT));
}

export function forgetProject(root: string) {
  write(readRecents().filter((item) => item.root !== root));
}

export function toggleFavorite(root: string) {
  write(readRecents().map((item) => (item.root === root ? { ...item, favorite: !item.favorite } : item)));
}

export { folderName };
