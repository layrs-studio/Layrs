import { useEffect, useMemo, useRef, useState, type CSSProperties, type UIEvent } from "react";
import type { DiffLine, DiffModel } from "@layrs/lens-sdk";
import { DEFAULT_DIFF_VIEWPORT_HEIGHT_PX } from "./TextLinesDiffViewer.constants";
import {
  columnRowFromLine,
  flattenColumnDiffEntries,
  flattenDiffEntries,
  formatColumnCellLineNumber,
  formatDiffWindowLabel,
  formatHiddenLineRange,
  formatHunkTitle,
  formatLineNumber,
  getDiffRenderMetadata,
  getDiffVirtualization,
  getHiddenUnchangedLineCount,
  getRenderableLineSegment,
  getRenderedEntryLineCount,
  getVirtualRange,
  joinClassNames
} from "./TextLinesDiffViewer.model";
import type { DiffColumnRow, DiffEntry, DiffScrollFrame, DiffViewerMode, DiffViewMode } from "./TextLinesDiffViewer.types";

export interface TextLinesDiffViewerProps {
  diff: DiffModel;
  ariaLabel?: string;
  className?: string;
}

export function TextLinesDiffViewer({
  diff,
  ariaLabel = "Text diff",
  className
}: TextLinesDiffViewerProps) {
  const [showWholeFile, setShowWholeFile] = useState(false);
  const [viewerMode, setViewerMode] = useState<DiffViewerMode>("default");
  const viewMode: DiffViewMode = showWholeFile ? "wholeFile" : "changesOnly";
  const showColumnFileContext = viewerMode === "columns" && showWholeFile;
  const allEntries = useMemo(() => flattenDiffEntries(diff, "wholeFile"), [diff]);
  const entries = useMemo(
    () => viewerMode === "columns" ? flattenColumnDiffEntries(diff, viewMode) : flattenDiffEntries(diff, viewMode),
    [diff, viewMode, viewerMode]
  );
  const fullLineCount = useMemo(() => allEntries.filter((entry) => entry.kind === "line").length, [allEntries]);
  const lineCount = useMemo(() => getRenderedEntryLineCount(entries), [entries]);
  const hiddenUnchangedLineCount = useMemo(() => getHiddenUnchangedLineCount(entries), [entries]);
  const renderMetadata = getDiffRenderMetadata(diff, fullLineCount);
  const virtualization = getDiffVirtualization(diff);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [scrollFrame, setScrollFrame] = useState<DiffScrollFrame>({
    scrollTop: 0,
    viewportHeight: DEFAULT_DIFF_VIEWPORT_HEIGHT_PX
  });

  useEffect(() => {
    setShowWholeFile(false);
  }, [diff]);

  useEffect(() => {
    setScrollFrame({
      scrollTop: 0,
      viewportHeight: viewportRef.current?.clientHeight || DEFAULT_DIFF_VIEWPORT_HEIGHT_PX
    });
    if (viewportRef.current) {
      viewportRef.current.scrollTop = 0;
      viewportRef.current.scrollLeft = 0;
    }
  }, [diff, showWholeFile, viewerMode]);

  const virtualRange = getVirtualRange(entries.length, scrollFrame, virtualization);
  const visibleEntries = entries.slice(virtualRange.startIndex, virtualRange.endIndex);
  const canPageBefore = virtualRange.startIndex > 0;
  const canPageAfter = virtualRange.endIndex < entries.length;
  const showPagingControls =
    entries.length > virtualRange.maxDomRows ||
    renderMetadata.hasMoreBefore ||
    renderMetadata.hasMoreAfter ||
    renderMetadata.hasMoreColumns ||
    renderMetadata.truncated ||
    renderMetadata.totalLineCount > fullLineCount ||
    (renderMetadata.totalDiffLineCount ?? 0) > fullLineCount;
  const spacerStyle = {
    height: `${virtualRange.totalHeight}px`,
    "--layrs-text-diff-row-height": `${virtualization.rowHeightPx}px`
  } as CSSProperties;
  const windowStyle = {
    transform: `translateY(${virtualRange.offsetY}px)`
  } as CSSProperties;
  const scrollToEntry = (entryIndex: number) => {
    const scrollTop = Math.max(0, Math.min(entryIndex, Math.max(0, entries.length - 1))) * virtualization.rowHeightPx;
    if (viewportRef.current) {
      viewportRef.current.scrollTop = scrollTop;
    }
    setScrollFrame((frame) => ({ ...frame, scrollTop }));
  };
  const handleScroll = (event: UIEvent<HTMLDivElement>) => {
    const viewport = event.currentTarget;
    setScrollFrame({
      scrollTop: viewport.scrollTop,
      viewportHeight: viewport.clientHeight || DEFAULT_DIFF_VIEWPORT_HEIGHT_PX
    });
  };

  return (
    <div className={joinClassNames("layrs-text-diff", `layrs-text-diff--${viewerMode}`, className)}>
      <DiffWindowControls
        canPageAfter={canPageAfter}
        canPageBefore={canPageBefore}
        hiddenUnchangedLineCount={hiddenUnchangedLineCount}
        label={formatDiffWindowLabel(renderMetadata, visibleEntries, lineCount, viewMode)}
        onFirst={() => scrollToEntry(0)}
        onLast={() => scrollToEntry(Math.max(0, entries.length - virtualRange.maxDomRows))}
        onNext={() => scrollToEntry(Math.min(entries.length - 1, virtualRange.startIndex + virtualRange.maxDomRows))}
        onPrevious={() => scrollToEntry(Math.max(0, virtualRange.startIndex - virtualRange.maxDomRows))}
        onToggleWholeFile={(checked) => setShowWholeFile(checked)}
        onViewerModeChange={setViewerMode}
        showPagingControls={showPagingControls}
        showWholeFile={showWholeFile}
        viewerMode={viewerMode}
      />
      <div className="layrs-text-diff__viewport" ref={viewportRef} role="table" aria-label={ariaLabel} onScroll={handleScroll}>
        {viewerMode === "columns" ? (
          <ColumnDiffHeader includeFileColumn={showColumnFileContext} />
        ) : (
          <div className="layrs-text-diff__head" role="row">
            <span role="columnheader">Old</span>
            <span role="columnheader">New</span>
            <span role="columnheader">Line</span>
            <span role="columnheader">Content</span>
          </div>
        )}
        {renderMetadata.hasMoreBefore ? (
          viewerMode === "columns" ? (
            <ColumnDiffMarker includeFileColumn={showColumnFileContext} label="Earlier lines are outside this diff window" />
          ) : (
            <DiffWindowMarker label="Earlier lines are outside this diff window" />
          )
        ) : null}
        {entries.length > 0 ? (
          <div className="layrs-text-diff__spacer" style={spacerStyle}>
            <div className="layrs-text-diff__window" style={windowStyle}>
              {viewerMode === "columns"
                ? renderVisibleColumnDiffEntries(visibleEntries, showColumnFileContext)
                : renderVisibleDiffEntries(visibleEntries)}
            </div>
          </div>
        ) : (
          viewerMode === "columns" ? (
            <ColumnDiffMarker includeFileColumn={showColumnFileContext} label="No line changes in this diff window" />
          ) : (
            <DiffWindowMarker label="No line changes in this diff window" />
          )
        )}
        {renderMetadata.hasMoreAfter ? (
          viewerMode === "columns" ? (
            <ColumnDiffMarker includeFileColumn={showColumnFileContext} label="Later lines are outside this diff window" />
          ) : (
            <DiffWindowMarker label="Later lines are outside this diff window" />
          )
        ) : null}
      </div>
    </div>
  );
}
function DiffWindowControls({
  canPageAfter,
  canPageBefore,
  hiddenUnchangedLineCount,
  label,
  onFirst,
  onLast,
  onNext,
  onPrevious,
  onToggleWholeFile,
  onViewerModeChange,
  showPagingControls,
  showWholeFile,
  viewerMode
}: {
  canPageAfter: boolean;
  canPageBefore: boolean;
  hiddenUnchangedLineCount: number;
  label: string;
  onFirst: () => void;
  onLast: () => void;
  onNext: () => void;
  onPrevious: () => void;
  onToggleWholeFile: (checked: boolean) => void;
  onViewerModeChange: (mode: DiffViewerMode) => void;
  showPagingControls: boolean;
  showWholeFile: boolean;
  viewerMode: DiffViewerMode;
}) {
  return (
    <div className="layrs-text-diff__controls">
      <div className="layrs-text-diff__controls-main">
        <span>{label}</span>
        {!showWholeFile && hiddenUnchangedLineCount > 0 ? (
          <em>{hiddenUnchangedLineCount} unchanged lines hidden</em>
        ) : null}
      </div>
      <label className="layrs-text-diff__toggle">
        <input
          checked={showWholeFile}
          onChange={(event) => onToggleWholeFile(event.currentTarget.checked)}
          type="checkbox"
        />
        <span>View whole file</span>
      </label>
      <div className="layrs-text-diff__viewer-mode" aria-label="Diff viewer">
        <span>Diff viewer</span>
        <button
          aria-pressed={viewerMode === "default"}
          className={viewerMode === "default" ? "is-active" : undefined}
          onClick={() => onViewerModeChange("default")}
          type="button"
        >
          Default
        </button>
        <button
          aria-pressed={viewerMode === "columns"}
          className={viewerMode === "columns" ? "is-active" : undefined}
          onClick={() => onViewerModeChange("columns")}
          type="button"
        >
          Columns
        </button>
      </div>
      {showPagingControls ? (
        <div className="layrs-text-diff__pager">
          <button type="button" onClick={onFirst} disabled={!canPageBefore}>
            First
          </button>
          <button type="button" onClick={onPrevious} disabled={!canPageBefore}>
            Previous
          </button>
          <button type="button" onClick={onNext} disabled={!canPageAfter}>
            Next
          </button>
          <button type="button" onClick={onLast} disabled={!canPageAfter}>
            Last
          </button>
        </div>
      ) : null}
    </div>
  );
}

function DiffWindowMarker({ label }: { label: string }) {
  return (
    <div className="layrs-text-diff__window-marker" role="row">
      <span role="cell">...</span>
      <span role="cell">...</span>
      <span role="cell">...</span>
      <span role="cell">{label}</span>
    </div>
  );
}

function ColumnDiffHeader({ includeFileColumn }: { includeFileColumn: boolean }) {
  return (
    <div
      className={includeFileColumn ? "layrs-text-diff__columns-head has-file-column" : "layrs-text-diff__columns-head"}
      role="row"
    >
      {includeFileColumn ? <span role="columnheader">File</span> : null}
      <span role="columnheader">Additions</span>
      <span role="columnheader">Deletions</span>
    </div>
  );
}

function ColumnDiffMarker({
  includeFileColumn,
  label
}: {
  includeFileColumn: boolean;
  label: string;
}) {
  return (
    <div
      className={includeFileColumn ? "layrs-text-diff__columns-marker has-file-column" : "layrs-text-diff__columns-marker"}
      role="row"
    >
      <span role="cell">{label}</span>
    </div>
  );
}

function renderVisibleDiffEntries(entries: DiffEntry[]) {
  return entries.map((entry) => {
    if (entry.kind === "hunk") {
      return (
        <div className="layrs-text-diff__hunk-title" key={entry.key} role="row">
          {formatHunkTitle(entry.hunk)}
        </div>
      );
    }

    if (entry.kind === "hidden") {
      return <HiddenUnchangedLinesMarker entry={entry} key={entry.key} />;
    }

    if (entry.kind === "columnLine") {
      return (
        <ColumnDiffLineRow
          includeFileColumn={false}
          key={entry.key}
          row={entry.row}
        />
      );
    }

    return (
      <DiffLineRow
        key={entry.key}
        line={entry.row.line}
      />
    );
  });
}

function renderVisibleColumnDiffEntries(
  entries: DiffEntry[],
  includeFileColumn: boolean
) {
  return entries.map((entry) => {
    if (entry.kind === "hunk") {
      return (
        <div
          className={includeFileColumn ? "layrs-text-diff__columns-hunk-title has-file-column" : "layrs-text-diff__columns-hunk-title"}
          key={entry.key}
          role="row"
        >
          {formatHunkTitle(entry.hunk)}
        </div>
      );
    }

    if (entry.kind === "hidden") {
      return <ColumnHiddenUnchangedLinesMarker entry={entry} includeFileColumn={includeFileColumn} key={entry.key} />;
    }

    if (entry.kind === "line") {
      return (
        <ColumnDiffLineRow
          includeFileColumn={includeFileColumn}
          key={entry.key}
          row={columnRowFromLine(entry.row.line)}
        />
      );
    }

    return (
      <ColumnDiffLineRow
        includeFileColumn={includeFileColumn}
        key={entry.key}
        row={entry.row}
      />
    );
  });
}

function HiddenUnchangedLinesMarker({ entry }: { entry: Extract<DiffEntry, { kind: "hidden" }> }) {
  return (
    <div className="layrs-text-diff__window-marker layrs-text-diff__hidden-marker" role="row">
      <span role="cell">{formatHiddenLineRange(entry.oldStart, entry.oldEnd)}</span>
      <span role="cell">{formatHiddenLineRange(entry.newStart, entry.newEnd)}</span>
      <span role="cell">...</span>
      <span role="cell">{entry.count} unchanged {entry.count === 1 ? "line" : "lines"} hidden</span>
    </div>
  );
}

function ColumnHiddenUnchangedLinesMarker({
  entry,
  includeFileColumn
}: {
  entry: Extract<DiffEntry, { kind: "hidden" }>;
  includeFileColumn: boolean;
}) {
  return (
    <div
      className={includeFileColumn ? "layrs-text-diff__columns-marker layrs-text-diff__hidden-marker has-file-column" : "layrs-text-diff__columns-marker layrs-text-diff__hidden-marker"}
      role="row"
    >
      <span role="cell">
        {entry.count} unchanged {entry.count === 1 ? "line" : "lines"} hidden
      </span>
    </div>
  );
}

function ColumnDiffLineRow({
  includeFileColumn,
  row
}: {
  includeFileColumn: boolean;
  row: DiffColumnRow;
}) {
  const rowState = row.insertLine && row.deleteLine ? "modify" : row.insertLine ? "insert" : row.deleteLine ? "delete" : "equal";

  return (
    <div
      className={[
        "layrs-text-diff__columns-row",
        `layrs-text-diff__columns-row--${rowState}`,
        includeFileColumn ? "has-file-column" : ""
      ]
        .filter(Boolean)
        .join(" ")}
      role="row"
    >
      {includeFileColumn ? (
        <DiffColumnCell line={row.fileLine} variant="file" />
      ) : null}
      <DiffColumnCell line={row.insertLine} variant="insert" />
      <DiffColumnCell line={row.deleteLine} variant="delete" />
    </div>
  );
}

function DiffColumnCell({
  line,
  variant
}: {
  line?: DiffLine;
  variant: "file" | "insert" | "delete";
}) {
  if (!line) {
    return <span className={`layrs-text-diff__column-cell layrs-text-diff__column-cell--${variant} is-empty`} role="cell" />;
  }

  const segment = getRenderableLineSegment(line);
  return (
    <span className={`layrs-text-diff__column-cell layrs-text-diff__column-cell--${variant}`} role="cell">
      <span className="layrs-text-diff__column-line-number">{formatColumnCellLineNumber(line)}</span>
      <code>
        {segment.hasMoreBefore ? (
          <span className="layrs-text-diff__column-more" aria-label="Earlier columns hidden">
            ...
          </span>
        ) : null}
        {segment.text || " "}
        {segment.hasMoreAfter ? (
          <span className="layrs-text-diff__column-more" aria-label="Later columns hidden">
            ...
          </span>
        ) : null}
      </code>
    </span>
  );
}

function DiffLineRow({ line }: { line: DiffLine }) {
  const marker = line.op === "insert" ? "+" : line.op === "delete" ? "-" : " ";
  const segment = getRenderableLineSegment(line);

  return (
    <div className={`layrs-text-diff__row layrs-text-diff__row--${line.op}`} role="row">
      <span className="layrs-text-diff__line-number" role="cell">
        {formatLineNumber(line.oldLine)}
      </span>
      <span className="layrs-text-diff__line-number" role="cell">
        {formatLineNumber(line.newLine)}
      </span>
      <span className="layrs-text-diff__marker" role="cell">
        {marker}
      </span>
      <code className="layrs-text-diff__content" role="cell">
        {segment.hasMoreBefore ? (
          <span className="layrs-text-diff__column-more" aria-label="Earlier columns hidden">
            ...
          </span>
        ) : null}
        {segment.text || " "}
        {segment.hasMoreAfter ? (
          <span className="layrs-text-diff__column-more" aria-label="Later columns hidden">
            ...
          </span>
        ) : null}
      </code>
    </div>
  );
}
