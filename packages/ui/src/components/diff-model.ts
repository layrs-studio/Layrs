/*
 * Pure diff layout shared by the side-by-side and inline views. Input is the
 * engine's line diff (`equal` / `delete` / `insert`, text keeps its newline).
 */

export type DiffOp = "equal" | "delete" | "insert";
export type DiffLine = { op: DiffOp | string; text: string };

export type DiffStats = { added: number; removed: number };

export type SideCell = { number: number; text: string; noNewline: boolean };

export type SideRow =
  | { kind: "line"; op: "equal" | "change"; before: SideCell | null; after: SideCell | null }
  | { kind: "fold"; hidden: number; id: number };

export type InlineRow =
  | { kind: "line"; op: DiffOp; before: number | null; after: number | null; text: string; noNewline: boolean }
  | { kind: "fold"; hidden: number; id: number };

export function diffStats(lines: DiffLine[]): DiffStats {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.op === "insert") added++;
    else if (line.op === "delete") removed++;
  }
  return { added, removed };
}

function strip(text: string) {
  const noNewline = !text.endsWith("\n");
  return { text: noNewline ? text : text.slice(0, -1).replace(/\r$/, ""), noNewline };
}

function op(line: DiffLine): DiffOp {
  return line.op === "insert" || line.op === "delete" ? line.op : "equal";
}

/** Keeps `context` equal lines around changes; longer equal runs become folds. */
function foldable(ops: DiffOp[], context: number, expanded: ReadonlySet<number>) {
  const visible = new Array<boolean>(ops.length).fill(false);
  ops.forEach((value, index) => {
    if (value === "equal") return;
    for (let offset = Math.max(0, index - context); offset <= Math.min(ops.length - 1, index + context); offset++) visible[offset] = true;
  });
  const hasChange = ops.some((value) => value !== "equal");
  const groups: { start: number; end: number; fold: boolean; id: number }[] = [];
  let start = 0;
  while (start < ops.length) {
    const shown = visible[start] || !hasChange;
    let end = start;
    while (end + 1 < ops.length && (visible[end + 1] || !hasChange) === shown) end++;
    // Folding a run shorter than 4 lines saves nothing.
    const fold = !shown && end - start + 1 >= 4 && !expanded.has(start);
    groups.push({ start, end, fold, id: start });
    start = end + 1;
  }
  return groups;
}

export function inlineRows(lines: DiffLine[], context = 3, expanded: ReadonlySet<number> = new Set()): InlineRow[] {
  const ops = lines.map(op);
  const rows: InlineRow[] = [];
  let before = 0;
  let after = 0;
  for (const group of foldable(ops, context, expanded)) {
    if (group.fold) {
      const hidden = group.end - group.start + 1;
      before += hidden;
      after += hidden;
      rows.push({ kind: "fold", hidden, id: group.id });
      continue;
    }
    for (let index = group.start; index <= group.end; index++) {
      const value = ops[index];
      const { text, noNewline } = strip(lines[index].text);
      if (value === "equal") rows.push({ kind: "line", op: value, before: ++before, after: ++after, text, noNewline });
      else if (value === "delete") rows.push({ kind: "line", op: value, before: ++before, after: null, text, noNewline });
      else rows.push({ kind: "line", op: value, before: null, after: ++after, text, noNewline });
    }
  }
  return rows;
}

/** Pairs each run of deletions with the following insertions, row by row. */
export function sideBySideRows(lines: DiffLine[], context = 3, expanded: ReadonlySet<number> = new Set()): SideRow[] {
  const ops = lines.map(op);
  const rows: SideRow[] = [];
  let before = 0;
  let after = 0;
  for (const group of foldable(ops, context, expanded)) {
    if (group.fold) {
      const hidden = group.end - group.start + 1;
      before += hidden;
      after += hidden;
      rows.push({ kind: "fold", hidden, id: group.id });
      continue;
    }
    let index = group.start;
    while (index <= group.end) {
      if (ops[index] === "equal") {
        const { text, noNewline } = strip(lines[index].text);
        rows.push({
          kind: "line",
          op: "equal",
          before: { number: ++before, text, noNewline },
          after: { number: ++after, text, noNewline },
        });
        index++;
        continue;
      }
      const removed: SideCell[] = [];
      const added: SideCell[] = [];
      while (index <= group.end && ops[index] === "delete") removed.push({ number: ++before, ...strip(lines[index++].text) });
      while (index <= group.end && ops[index] === "insert") added.push({ number: ++after, ...strip(lines[index++].text) });
      for (let row = 0; row < Math.max(removed.length, added.length); row++) {
        rows.push({ kind: "line", op: "change", before: removed[row] ?? null, after: added[row] ?? null });
      }
    }
  }
  return rows;
}
