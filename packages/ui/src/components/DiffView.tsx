import { Fragment, useMemo, useState } from "react";
import { ChevronsUpDown } from "lucide-react";
import { diffStats, inlineRows, sideBySideRows, type DiffLine, type SideCell } from "./diff-model";

export type DiffMode = "split" | "inline";

/**
 * Text comparison. Long unchanged regions are folded and can be expanded.
 * The page never scrolls horizontally: split lines wrap, inline scrolls locally.
 */
export function DiffView({ lines, mode = "split", beforeLabel = "Avant", afterLabel = "Après" }: {
  lines: DiffLine[];
  mode?: DiffMode;
  beforeLabel?: string;
  afterLabel?: string;
}) {
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set());
  const stats = useMemo(() => diffStats(lines), [lines]);
  const expand = (id: number) => setExpanded((current) => new Set(current).add(id));

  if (mode === "inline") {
    const rows = inlineRows(lines, 3, expanded);
    return (
      <div className="layrs-diff layrs-diff--inline">
        <div className="layrs-diff__summary">
          <span className="layrs-diff__removed">−{stats.removed} ligne{stats.removed === 1 ? "" : "s"} supprimée{stats.removed === 1 ? "" : "s"}</span>
          <span className="layrs-diff__added">+{stats.added} ligne{stats.added === 1 ? "" : "s"} ajoutée{stats.added === 1 ? "" : "s"}</span>
        </div>
        <table className="layrs-diff__table">
          <tbody>
            {rows.map((row, index) =>
              row.kind === "fold" ? (
                <FoldRow key={`fold-${row.id}`} hidden={row.hidden} columns={3} onExpand={() => expand(row.id)} />
              ) : (
                <tr key={index} className={`layrs-diff__row layrs-diff__row--${row.op}`}>
                  <td className="layrs-diff__number">{row.before ?? ""}</td>
                  <td className="layrs-diff__number">{row.after ?? ""}</td>
                  <td className="layrs-diff__code">
                    <span className="layrs-diff__marker" aria-hidden>
                      {row.op === "insert" ? "+" : row.op === "delete" ? "−" : " "}
                    </span>
                    <span className="layrs-visually-hidden">{row.op === "insert" ? "Ajouté : " : row.op === "delete" ? "Supprimé : " : ""}</span>
                    {row.text}
                    {row.noNewline && <span className="layrs-diff__eof"> Sans retour final</span>}
                  </td>
                </tr>
              ),
            )}
          </tbody>
        </table>
      </div>
    );
  }

  const rows = sideBySideRows(lines, 3, expanded);
  return (
    <div className="layrs-diff layrs-diff--split">
      <div className="layrs-diff__heads">
        <div className="layrs-diff__head layrs-diff__head--before">
          <span>{beforeLabel}</span>
          <span className="layrs-diff__removed">−{stats.removed} ligne{stats.removed === 1 ? "" : "s"} supprimée{stats.removed === 1 ? "" : "s"}</span>
        </div>
        <div className="layrs-diff__head layrs-diff__head--after">
          <span>{afterLabel}</span>
          <span className="layrs-diff__added">+{stats.added} ligne{stats.added === 1 ? "" : "s"} ajoutée{stats.added === 1 ? "" : "s"}</span>
        </div>
      </div>
      <table className="layrs-diff__table">
        <colgroup>
          <col className="layrs-diff__col-number" />
          <col />
          <col className="layrs-diff__col-number" />
          <col />
        </colgroup>
        <tbody>
          {rows.map((row, index) =>
            row.kind === "fold" ? (
              <FoldRow key={`fold-${row.id}`} hidden={row.hidden} columns={4} onExpand={() => expand(row.id)} />
            ) : (
              <tr key={index} className={`layrs-diff__row layrs-diff__row--${row.op}`}>
                <SideCellView cell={row.before} side="before" changed={row.op === "change"} />
                <SideCellView cell={row.after} side="after" changed={row.op === "change"} />
              </tr>
            ),
          )}
        </tbody>
      </table>
    </div>
  );
}

function SideCellView({ cell, side, changed }: { cell: SideCell | null; side: "before" | "after"; changed: boolean }) {
  if (!cell) {
    return (
      <Fragment>
        <td className="layrs-diff__number layrs-diff__empty" />
        <td className="layrs-diff__code layrs-diff__empty" />
      </Fragment>
    );
  }
  const tone = changed ? (side === "before" ? "delete" : "insert") : "equal";
  return (
    <Fragment>
      <td className={`layrs-diff__number layrs-diff__cell--${tone}`}>{cell.number}</td>
      <td className={`layrs-diff__code layrs-diff__cell--${tone}`}>
        {changed && (
          <span className="layrs-diff__marker" aria-hidden>
            {side === "before" ? "−" : "+"}
          </span>
        )}
        {changed && <span className="layrs-visually-hidden">{side === "before" ? "Supprimé : " : "Ajouté : "}</span>}
        {cell.text}
        {cell.noNewline && <span className="layrs-diff__eof"> Sans retour final</span>}
      </td>
    </Fragment>
  );
}

function FoldRow({ hidden, columns, onExpand }: { hidden: number; columns: number; onExpand: () => void }) {
  return (
    <tr className="layrs-diff__fold">
      <td colSpan={columns}>
        <button type="button" onClick={onExpand}>
          <ChevronsUpDown aria-hidden size={14} />
          Afficher {hidden} ligne{hidden === 1 ? "" : "s"} identique{hidden === 1 ? "" : "s"}
        </button>
      </td>
    </tr>
  );
}
