import { useMemo, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Eye, File, Folder, FolderOpen, Lock, Pencil } from "lucide-react";
import type { AccessLevel, AccessReason } from "./access-model";

export type TreeEntry = { path: string; directory: boolean };
export type EntryAccess = { level: AccessLevel; reason: AccessReason | null; source: "server" | "rules" } | null;

type Node = { path: string; name: string; directory: boolean; children: Node[] };

/** Builds a tree from flat projection paths; missing parents become folders. */
export function buildTree(entries: TreeEntry[]): Node[] {
  const root: Node = { path: "", name: "", directory: true, children: [] };
  const index = new Map<string, Node>([["", root]]);
  const ensure = (path: string, directory: boolean): Node => {
    const known = index.get(path);
    if (known) {
      if (directory) known.directory = true;
      return known;
    }
    const cut = path.lastIndexOf("/");
    const parent = ensure(cut < 0 ? "" : path.slice(0, cut), true);
    const node: Node = { path, name: cut < 0 ? path : path.slice(cut + 1), directory, children: [] };
    parent.children.push(node);
    index.set(path, node);
    return node;
  };
  for (const entry of entries) ensure(entry.path, entry.directory);
  const sort = (nodes: Node[]) => {
    nodes.sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name, "fr"));
    nodes.forEach((node) => sort(node.children));
  };
  sort(root.children);
  return root.children;
}

export const levelIcons: Record<AccessLevel, ReactNode> = {
  none: <Lock aria-hidden size={14} />,
  read: <Eye aria-hidden size={14} />,
  write: <Pencil aria-hidden size={14} />,
};
const levelText: Record<AccessLevel, string> = { none: "Interdit", read: "Lecture", write: "Écriture" };

export function reasonTag(reason: AccessReason | null): string | null {
  if (!reason) return null;
  switch (reason.kind) {
    case "deny":
      return "Refus prioritaire";
    case "explicit":
      return "Règle explicite";
    case "inherited":
      return "Hérité";
    case "default":
      return "Par défaut";
    case "unrestricted":
      return "Selon le rôle";
    case "no-project-access":
      return "Rôle";
    default:
      return null;
  }
}

/** Path tree with the selected person's right on each entry, in text and icon. */
export function AccessTree({
  entries,
  access,
  expanded,
  onToggle,
  selected,
  onSelect,
  label,
}: {
  entries: TreeEntry[];
  access: (path: string) => EntryAccess;
  expanded: Set<string>;
  onToggle: (path: string) => void;
  selected: string;
  onSelect: (path: string) => void;
  label: string;
}) {
  const tree = useMemo(() => buildTree(entries), [entries]);
  const render = (nodes: Node[], depth: number): ReactNode =>
    nodes.map((node) => {
      const open = expanded.has(node.path);
      const right = access(node.path);
      const tag = reasonTag(right?.reason ?? null);
      return (
        <li key={node.path}>
          <div className={["access-tree__row", selected === node.path && "access-tree__row--selected"].filter(Boolean).join(" ")} style={{ paddingInlineStart: 8 + depth * 18 }}>
            {node.directory ? (
              <button type="button" className="access-tree__toggle" aria-expanded={open} aria-label={`${open ? "Replier" : "Déplier"} ${node.name}`} onClick={() => onToggle(node.path)}>
                {open ? <ChevronDown aria-hidden size={14} /> : <ChevronRight aria-hidden size={14} />}
              </button>
            ) : (
              <span className="access-tree__toggle" aria-hidden />
            )}
            <button type="button" className="access-tree__name" aria-current={selected === node.path ? "true" : undefined} onClick={() => onSelect(node.path)} title={node.path}>
              {node.directory ? open ? <FolderOpen aria-hidden size={16} /> : <Folder aria-hidden size={16} /> : <File aria-hidden size={16} />}
              <span>{node.name}</span>
            </button>
            {right ? (
              <span className={`access-level access-level--${right.level}`} title={right.source === "server" ? "Droit confirmé par le serveur" : "Calculé à partir des règles lues"}>
                {levelIcons[right.level]}
                {levelText[right.level]}
                {tag && <small>· {tag}</small>}
              </span>
            ) : (
              <span className="access-level access-level--unknown">Non évalué</span>
            )}
          </div>
          {node.directory && open && node.children.length > 0 && <ul>{render(node.children, depth + 1)}</ul>}
        </li>
      );
    });
  return (
    <ul className="access-tree" aria-label={label}>
      {render(tree, 0)}
    </ul>
  );
}
