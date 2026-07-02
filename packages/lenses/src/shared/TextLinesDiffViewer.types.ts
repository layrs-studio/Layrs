import type { DiffColumnWindow, DiffLine, DiffLineWindow, DiffModel } from "@layrs/lens-sdk";

export type FieldRecord = Record<string, unknown>;

export type DiffViewMode = "changesOnly" | "wholeFile";
export type DiffViewerMode = "default" | "columns";

export interface DiffRow {
  hunk: DiffModel["hunks"][number];
  hunkIndex: number;
  line: DiffLine;
  lineIndex: number;
}

export interface DiffColumnRow {
  hunk: DiffModel["hunks"][number];
  hunkIndex: number;
  fileLine?: DiffLine;
  insertLine?: DiffLine;
  deleteLine?: DiffLine;
  lineIndex: number;
}

export type DiffEntry =
  | {
      kind: "hunk";
      key: string;
      hunk: DiffModel["hunks"][number];
      hunkIndex: number;
    }
  | {
      kind: "hidden";
      key: string;
      count: number;
      oldStart?: number;
      oldEnd?: number;
      newStart?: number;
      newEnd?: number;
    }
  | {
      kind: "line";
      key: string;
      row: DiffRow;
    }
  | {
      kind: "columnLine";
      key: string;
      row: DiffColumnRow;
    };

export interface DiffRenderMetadata {
  totalLineCount: number;
  totalDiffLineCount?: number;
  renderedLineCount: number;
  lineWindow?: DiffLineWindow;
  columnWindow?: DiffColumnWindow;
  hasMoreBefore: boolean;
  hasMoreAfter: boolean;
  hasMoreColumns: boolean;
  truncated: boolean;
}

export interface DiffVirtualizationState {
  maxRenderedLineCount: number;
  overscanLineCount: number;
  rowHeightPx: number;
}

export interface DiffScrollFrame {
  scrollTop: number;
  viewportHeight: number;
}

export interface DiffVirtualRange {
  startIndex: number;
  endIndex: number;
  maxDomRows: number;
  offsetY: number;
  totalHeight: number;
}

